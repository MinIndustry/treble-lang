//! Offline rendering of a piece (§8.9).
//!
//! A piece has a definite length, so it does not need the loop engine's
//! stay-one-boundary-ahead scheduling: the whole timeline is compiled up front,
//! every note is scheduled at an absolute frame, and the graph is pulled a
//! block at a time until the arrangement and its tail have gone by.
//!
//! ## Occurrence slots and render islands
//!
//! Every pattern of every *occurrence* gets its own instrument slot, rather
//! than one slot per pattern name. Two things fall out of that, and both are
//! why it is worth the extra slots:
//!
//! - **Sweeps do not collide.** A parameter automation is a window of absolute
//!   frames, and [`System::apply_automations`] applies every automation each
//!   block with the last write winning. Two occurrences of one section sharing
//!   a filter node would therefore fight over it — the second occurrence's
//!   ramp, still holding its start value, would overwrite the first's while
//!   the first was mid-sweep. Distinct nodes make each occurrence's window its
//!   own business.
//! - **Filter state does not bleed.** An occurrence gets a fresh delay line and
//!   reverb rather than one carrying the previous playing's buffer, which is
//!   what "the section sounds the same wherever it is played" (§8.5, §8.6)
//!   means once there are stateful filters in the chain.
//!
//! Slots are partitioned into independent render islands. Ungrouped lines from
//! one occurrence run only for that occurrence plus its declared tail; lines
//! sharing a named bus remain in one piece-wide island so the shared filter
//! state is preserved. Offline islands use a fixed Rayon worker pool, then mix
//! in declaration order through one master limiter for deterministic output.

use std::collections::BTreeMap;
use std::sync::{Arc, mpsc};

use crate::ast::PatternDef;
use crate::piece::{Piece, Section};
use treble::app::prelude::{AudioGraph, AutomationSpec, AutomationTarget, BusSpec, ParameterRamp};
use treble::audio::{EventScheduler, InstrumentAudioMessage, render_block};
use treble::core::graph::{AudioOutputSink, Entry, Sink, SinkTelemetry, System};
use treble::core::{Block, Note};
use treble::instruments::prelude::InstrumentRegistry;

use super::compile::{
    CompiledPattern, LineTravel, NOTE_GATE, PatternGate, RampWindow, Travel, compile_pattern,
    core_curve, cycle_seconds, cycle_strikes_at, fx_ramp_window, pattern_fx, transform_travels,
};

/// A rendered piece: interleaved stereo samples plus what they came from.
#[derive(Debug, Clone)]
pub struct RenderedPiece {
    pub sample_rate: u32,
    /// Stereo-interleaved samples.
    pub samples: Vec<f32>,
    /// How long the arrangement itself lasts, excluding the tail.
    pub seconds: f64,
    /// How long was rendered in total, arrangement plus tail.
    pub rendered_seconds: f64,
    /// How many section occurrences were played.
    pub occurrences: usize,
    /// How many notes were scheduled — a cheap sanity check that a render that
    /// came out silent was meant to be.
    pub notes: usize,
    /// Sections the arrangement never played (§8.4).
    pub unused: Vec<String>,
    /// Runtime and master-bus measurements from this render.
    pub telemetry: RenderTelemetry,
}

/// Evidence about the work and headroom of an offline render.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderTelemetry {
    pub islands: usize,
    pub slots: usize,
    pub worker_threads: usize,
    pub pre_limiter_peak: f32,
    pub post_limiter_peak: f32,
    pub max_gain_reduction_db: f32,
    pub limited_samples: usize,
}

/// One line of one occurrence, compiled and placed on the piece's timeline.
struct Placed {
    pattern: CompiledPattern,
    /// The graph slot this line's notes go to.
    slot: usize,
    /// The absolute frame the occurrence starts at.
    start_frame: u64,
    /// How long one of this section's cycles is, in frames.
    cycle_frames: u64,
    /// Which cycles of the section the line sounds on, 1-based and inclusive.
    from_cycle: u32,
    to_cycle: u32,
    /// The section's own length, for resolving an open-ended span.
    section_cycles: u32,
}

#[derive(Default)]
struct IslandBuilder {
    graph: AudioGraph,
    placed: Vec<Placed>,
    automations: Vec<AutomationSpec>,
    bus_members: Vec<usize>,
    bus: Option<(String, Vec<treble::instruments::spec::FxSpec>)>,
    start_frame: u64,
    end_frame: u64,
}

struct RenderIsland {
    system: System,
    scheduler: EventScheduler,
    start_frame: u64,
    end_frame: u64,
}

struct IslandAudio {
    start_frame: u64,
    samples: Vec<f32>,
}

enum WorkerMessage {
    Advanced(u64),
    Done {
        index: usize,
        result: Result<IslandAudio, String>,
    },
}

/// Render a piece to interleaved stereo samples.
///
/// Where a render has got to.
///
/// Reported rather than printed, so the caller decides what a progress display
/// looks like — a CLI bar, a GUI meter, or nothing at all.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    /// Compiling one occurrence's lines. `done` of `total` occurrences.
    Compiling { done: usize, total: usize },
    /// Building the audio graph: every slot compiles its instrument here, so
    /// on a large piece this is a visible pause rather than an instant.
    Building { slots: usize },
    /// Rendering audio. `frames` of `total_frames` written.
    Rendering { frames: u64, total_frames: u64 },
    /// Everything is rendered; the samples are in hand.
    Done { frames: u64 },
}

/// Render a piece, reporting progress as it goes.
///
/// The plain [`render`] is this with the reports thrown away.
pub fn render_with_progress(
    piece: &Piece,
    registry: &InstrumentRegistry,
    sample_rate: u32,
    progress: &mut dyn FnMut(Progress),
) -> Result<RenderedPiece, String> {
    render_inner(piece, registry, sample_rate, progress)
}

/// `registry` supplies the instruments — the built-ins plus whatever the
/// buffer's `def` and `load` lines put there. Nothing else is borrowed: the
/// render builds its own graph, so a performance already sounding is left
/// alone and a headless caller needs no `App` at all.
pub fn render(
    piece: &Piece,
    registry: &InstrumentRegistry,
    sample_rate: u32,
) -> Result<RenderedPiece, String> {
    render_inner(piece, registry, sample_rate, &mut |_| {})
}

fn render_inner(
    piece: &Piece,
    registry: &InstrumentRegistry,
    sample_rate: u32,
    progress: &mut dyn FnMut(Progress),
) -> Result<RenderedPiece, String> {
    if piece.timeline.is_empty() {
        return Err("the arrangement plays no section, so there is nothing to render".into());
    }

    let tail_frames = (piece.tail * f64::from(sample_rate)).round() as u64;
    let mut occurrence_islands = Vec::new();
    // Lines sharing a named bus stay in one piece-wide island. This preserves
    // the bus's filter state while independent occurrence branches can sleep
    // before their start and disappear after their tail.
    let mut grouped_islands: BTreeMap<String, IslandBuilder> = BTreeMap::new();
    let mut bus_chains: BTreeMap<String, Vec<treble::instruments::spec::FxSpec>> = BTreeMap::new();

    let mut at_frame = 0u64;
    for (index, occurrence) in piece.timeline.iter().enumerate() {
        progress(Progress::Compiling {
            done: index,
            total: piece.timeline.len(),
        });
        let section = &piece.sections[occurrence.section];
        let cycle_frames = section_cycle_frames(section, sample_rate);
        let occurrence_frames = cycle_frames * u64::from(section.cycles);

        for group in &section.groups {
            if group.transforms.iter().any(transform_travels) {
                return Err(format!(
                    "group '{}': a bus serves the whole piece, so its filters cannot travel — \
                     move the range onto the member lines",
                    group.name
                ));
            }
            if !bus_chains.contains_key(&group.name) {
                let (fx, _) = pattern_fx(
                    &group.transforms,
                    section.cycle_seconds(),
                    LineTravel {
                        line: Travel::START,
                        cycle: occurrence.start_cycle as f64,
                        origin: occurrence.start_cycle,
                        divisions: section.sig.0,
                    },
                )
                .map_err(|error| format!("group '{}': {error}", group.name))?;
                bus_chains.insert(group.name.clone(), fx);
            }
        }

        // A section is self-contained (§8.6): its lines' travels are measured
        // from the start of *this* occurrence, so each playing sweeps alike.
        let origin_cycle = occurrence.start_cycle;

        let lines = section
            .patterns
            .iter()
            .chain(&piece.throughout)
            .filter(|line| !line.muted && !section.muted);

        let mut occurrence = IslandBuilder {
            start_frame: at_frame,
            end_frame: at_frame + occurrence_frames + tail_frames,
            ..IslandBuilder::default()
        };
        for line in lines {
            let island = if let Some(group) = &line.group {
                grouped_islands
                    .entry(group.clone())
                    .or_insert_with(|| IslandBuilder {
                        start_frame: 0,
                        ..IslandBuilder::default()
                    })
            } else {
                &mut occurrence
            };
            let placed_line = mount_line(
                &mut island.graph,
                registry,
                section,
                line,
                origin_cycle,
                at_frame,
                cycle_frames,
                &mut island.automations,
                piece.seed,
            )?;
            if line.group.is_some() {
                island.bus_members.push(placed_line.slot);
            }
            island.placed.push(placed_line);
        }

        if !occurrence.placed.is_empty() {
            occurrence_islands.push(occurrence);
        }
        at_frame += occurrence_frames;
    }

    let arrangement_frames = at_frame;
    let total_frames = arrangement_frames + tail_frames;
    for (name, island) in &mut grouped_islands {
        island.end_frame = total_frames;
        island.bus = Some((name.clone(), bus_chains.remove(name).unwrap_or_default()));
    }
    progress(Progress::Compiling {
        done: piece.timeline.len(),
        total: piece.timeline.len(),
    });

    let builders = occurrence_islands
        .into_iter()
        .chain(grouped_islands.into_values())
        .collect::<Vec<_>>();
    let slots = builders.iter().map(|island| island.placed.len()).sum();
    progress(Progress::Building { slots });
    let mut notes = 0;
    let mut islands = Vec::with_capacity(builders.len());
    for builder in builders {
        let (island, island_notes) = build_island(builder, sample_rate)?;
        notes += island_notes;
        islands.push(island);
    }

    let island_count = islands.len();
    let worker_threads = if island_count == 0 {
        0
    } else {
        rayon::current_num_threads().min(island_count)
    };
    let rendered = render_islands(islands, total_frames, progress)?;
    let (samples, master) = mix_and_limit(rendered, total_frames, sample_rate);
    progress(Progress::Done {
        frames: total_frames,
    });

    Ok(RenderedPiece {
        sample_rate,
        samples,
        seconds: piece.seconds(),
        rendered_seconds: total_frames as f64 / f64::from(sample_rate),
        occurrences: piece.timeline.len(),
        notes,
        unused: piece.unused.clone(),
        telemetry: RenderTelemetry {
            islands: island_count,
            slots,
            worker_threads,
            pre_limiter_peak: master.pre_limiter_peak,
            post_limiter_peak: master.post_limiter_peak,
            max_gain_reduction_db: master.max_gain_reduction_db,
            limited_samples: master.limited_samples,
        },
    })
}

fn build_island(
    mut builder: IslandBuilder,
    sample_rate: u32,
) -> Result<(RenderIsland, usize), String> {
    if let Some((name, fx)) = builder.bus.take() {
        builder.graph.set_buses(vec![BusSpec {
            name,
            fx,
            members: builder.bus_members,
        }]);
    }
    builder.graph.set_automations(builder.automations);
    let mut system = builder
        .graph
        .compile(sample_rate as f32)
        .map_err(|error| format!("could not build a render island: {error:?}"))?;
    // Island outputs are summed before one final piece-wide limiter. Leaving
    // each island's limiter active would compress branches independently and
    // change both their balance and the historical single-master semantics.
    system
        .set_sink_parameter(0, "limiter_threshold", f32::MAX)
        .map_err(|error| format!("could not configure a render island: {error}"))?;
    let mut scheduler = EventScheduler::new();
    let notes = schedule(&mut scheduler, &builder.placed, &builder.graph);
    Ok((
        RenderIsland {
            system,
            scheduler,
            start_frame: builder.start_frame,
            end_frame: builder.end_frame,
        },
        notes,
    ))
}

fn render_islands(
    islands: Vec<RenderIsland>,
    piece_frames: u64,
    progress: &mut dyn FnMut(Progress),
) -> Result<Vec<IslandAudio>, String> {
    let total_work = islands
        .iter()
        .map(|island| island.end_frame - island.start_frame)
        .sum::<u64>()
        .max(1);
    let island_count = islands.len();
    let mut completed_work = 0u64;
    let mut completed = 0usize;
    let mut results = (0..island_count).map(|_| None).collect::<Vec<_>>();
    let (sender, receiver) = mpsc::channel();
    let report_every = (total_work / 100).max(1);

    std::thread::scope(|thread_scope| {
        let worker_sender = sender.clone();
        thread_scope.spawn(move || {
            rayon::scope(|scope| {
                for (index, island) in islands.into_iter().enumerate() {
                    let sender = worker_sender.clone();
                    scope.spawn(move |_| {
                        let result = render_island(island, report_every, &sender);
                        let _ = sender.send(WorkerMessage::Done { index, result });
                    });
                }
            });
        });
        drop(sender);
        while completed < island_count {
            let Ok(message) = receiver.recv() else {
                break;
            };
            match message {
                WorkerMessage::Advanced(frames) => {
                    completed_work = (completed_work + frames).min(total_work);
                    let piece_progress = ((completed_work as u128 * piece_frames as u128)
                        / total_work as u128) as u64;
                    progress(Progress::Rendering {
                        frames: piece_progress.min(piece_frames),
                        total_frames: piece_frames,
                    });
                }
                WorkerMessage::Done { index, result } => {
                    results[index] = Some(result);
                    completed += 1;
                }
            }
        }
    });

    results
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            result.ok_or_else(|| format!("render island {index} stopped without a result"))?
        })
        .collect()
}

fn render_island(
    mut island: RenderIsland,
    report_every: u64,
    progress: &mpsc::Sender<WorkerMessage>,
) -> Result<IslandAudio, String> {
    let frames = island.end_frame - island.start_frame;
    let mut samples = Vec::with_capacity(frames as usize * 2);
    let mut frame = island.start_frame;
    let mut reported = frame;
    let mut next_report = frame + report_every;
    while frame < island.end_frame {
        frame = render_block(
            &mut island.system,
            &mut island.scheduler,
            frame,
            &mut samples,
        );
        if frame >= next_report {
            let bounded = frame.min(island.end_frame);
            let _ = progress.send(WorkerMessage::Advanced(bounded - reported));
            reported = bounded;
            next_report = frame + report_every;
        }
    }
    if reported < island.end_frame {
        let _ = progress.send(WorkerMessage::Advanced(island.end_frame - reported));
    }
    samples.truncate(frames as usize * 2);
    Ok(IslandAudio {
        start_frame: island.start_frame,
        samples,
    })
}

fn mix_and_limit(
    islands: Vec<IslandAudio>,
    total_frames: u64,
    sample_rate: u32,
) -> (Vec<f32>, SinkTelemetry) {
    let mut mixed = vec![0.0f32; total_frames as usize * 2];
    // `islands` is in declaration order even though workers finish in another
    // order. Fixed-order accumulation keeps floating-point renders bit-exact.
    for island in islands {
        let start = island.start_frame as usize;
        for (offset, frame) in island.samples.chunks_exact(2).enumerate() {
            let target = (start + offset) * 2;
            if target + 1 >= mixed.len() {
                break;
            }
            mixed[target] += frame[0];
            mixed[target + 1] += frame[1];
        }
    }

    let mut sink = AudioOutputSink::new(sample_rate as f32);
    let mut telemetry = SinkTelemetry::default();
    for chunk in mixed.chunks_mut(512 * 2) {
        let block: Block = chunk
            .chunks_exact(2)
            .map(|frame| [frame[0], frame[1]])
            .collect();
        sink.push(Arc::new(block), 0);
        for (target, frame) in chunk.chunks_exact_mut(2).zip(sink.consume()) {
            target.copy_from_slice(&frame);
        }
        if let Some(block) = sink.telemetry() {
            telemetry.pre_limiter_peak = telemetry.pre_limiter_peak.max(block.pre_limiter_peak);
            telemetry.post_limiter_peak = telemetry.post_limiter_peak.max(block.post_limiter_peak);
            telemetry.max_gain_reduction_db = telemetry
                .max_gain_reduction_db
                .max(block.max_gain_reduction_db);
            telemetry.limited_samples += block.limited_samples;
        }
    }
    (mixed, telemetry)
}

/// How many frames one of a section's cycles lasts.
fn section_cycle_frames(section: &Section, sample_rate: u32) -> u64 {
    (cycle_seconds(section.bpm, section.sig) * f64::from(sample_rate)).round() as u64
}

/// Compile one line of one occurrence, give it a slot, and declare its sweeps.
#[allow(clippy::too_many_arguments)]
fn mount_line(
    graph: &mut AudioGraph,
    registry: &InstrumentRegistry,
    section: &Section,
    line: &PatternDef,
    origin_cycle: u64,
    start_frame: u64,
    cycle_frames: u64,
    automations: &mut Vec<AutomationSpec>,
    seed: u64,
) -> Result<Placed, String> {
    let travel = LineTravel {
        line: Travel::START,
        cycle: origin_cycle as f64,
        origin: origin_cycle,
        divisions: section.sig.0,
    };
    let (audio_fx, fx_ramps) = pattern_fx(&line.transforms, section.cycle_seconds(), travel)
        .map_err(|error| format!("'{}': {error}", line.name))?;

    let mut spec = registry
        .get(&line.instrument)
        .ok_or_else(|| format!("'{}': unknown instrument '{}'", line.name, line.instrument))?
        .clone();
    // The instrument's own filters keep the front of the chain, so the line's
    // Nth filter lands at `instrument_fx + N` — which is the index a sweep has
    // to name.
    let instrument_fx = spec.fx.len();
    spec.fx.extend(audio_fx.clone());

    let slot = graph
        .add_spec(spec.clone())
        .map_err(|error| format!("'{}': {error}", line.name))?;

    let compiled = compile_pattern(
        line,
        section.scale,
        audio_fx,
        fx_ramps,
        instrument_fx,
        spec,
        PatternGate {
            name: line.name.clone(),
            ..PatternGate::default()
        },
        origin_cycle,
        section.sig.0,
        seed,
    );

    // Sweeps are declared against this occurrence's own frame window, which is
    // what keeps two playings of one section from fighting over a filter.
    for ramp in &compiled.fx_ramps {
        let Some(window) =
            fx_ramp_window(ramp, compiled.window, compiled.ramp_origin, section.sig.0)
        else {
            continue;
        };
        let (start, end) = window_frames(window, origin_cycle, start_frame, cycle_frames);
        automations.push(AutomationSpec {
            target: AutomationTarget::InstrumentFx {
                slot,
                fx_index: compiled.instrument_fx + ramp.chain_index,
            },
            ramp: ParameterRamp {
                param: ramp.param.clone(),
                from: ramp.from,
                to: ramp.to,
                start_frame: start,
                end_frame: end,
                curve: core_curve(window.curve),
            },
        });
    }

    let span = line.span;
    Ok(Placed {
        pattern: compiled,
        slot,
        start_frame,
        cycle_frames,
        from_cycle: span.map_or(1, |span| span.start()),
        to_cycle: span.map_or(section.cycles, |span| span.end(section.cycles)),
        section_cycles: section.cycles,
    })
}

/// A ramp window's absolute frames, measured from the occurrence's own start.
fn window_frames(
    window: RampWindow,
    origin_cycle: u64,
    start_frame: u64,
    cycle_frames: u64,
) -> (u64, u64) {
    let into = window.origin.saturating_sub(origin_cycle);
    let start = start_frame + into * cycle_frames;
    let span = (window.span * cycle_frames as f64).round().max(1.0) as u64;
    (start, start + span)
}

/// Put every note of every placed line onto the scheduler's timeline.
fn schedule(scheduler: &mut EventScheduler, placed: &[Placed], graph: &AudioGraph) -> usize {
    let mut notes = 0;
    for line in placed {
        let Some(&source_index) = graph.source_map.get(&line.slot) else {
            continue;
        };
        for cycle in line.from_cycle..=line.to_cycle.min(line.section_cycles) {
            // Travels and generative choices count from the piece's start, so a
            // sweep keeps moving and a seeded phrase keeps evolving; the span is
            // 1-based within the section, so it converts here. Alternations use
            // the cycle within the section instead: `<a b c>` is per-bar
            // harmony, so alternative *k* belongs to bar *k* and must not rotate
            // because the arrangement moved the section (§8.5).
            let absolute = line.pattern.ramp_origin + u64::from(cycle - 1);
            let within_section = u64::from(cycle - 1);
            let cycle_start = line.start_frame + u64::from(cycle - 1) * line.cycle_frames;
            for strike in cycle_strikes_at(&line.pattern, absolute, within_section) {
                let start = cycle_start + (strike.start * line.cycle_frames as f64).round() as u64;
                let end = start
                    + ((strike.end - strike.start) * NOTE_GATE * line.cycle_frames as f64).round()
                        as u64;
                for midi in strike.notes {
                    let note = Note::from_midi(midi);
                    scheduler.schedule(
                        start,
                        InstrumentAudioMessage::NoteStart {
                            source_index,
                            note,
                            velocity: strike.velocity,
                        },
                    );
                    scheduler.schedule(
                        end.max(start + 1),
                        InstrumentAudioMessage::NoteStop { source_index, note },
                    );
                    notes += 1;
                }
            }
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use treble::instruments::prelude::InstrumentRegistry;

    /// Resolve a source and render it, asserting both steps succeed.
    fn rendered(source: &str) -> RenderedPiece {
        let (program, errors) = crate::parser::parse_program(source);
        assert!(errors.is_empty(), "parse: {errors:?}");
        let (piece, errors) = crate::piece::resolve(&program, (120, (4, 4), None));
        assert!(errors.is_empty(), "resolve: {errors:?}");
        render(&piece, &InstrumentRegistry::built_in(), 44_100).expect("render")
    }

    fn peak(rendered: &RenderedPiece, from: u64, to: u64) -> f32 {
        rendered.samples[(from as usize * 2)..(to as usize * 2).min(rendered.samples.len())]
            .iter()
            .fold(0.0f32, |loudest, sample| loudest.max(sample.abs()))
    }

    #[test]
    fn a_piece_renders_audible_samples() {
        let piece = rendered("bpm 120\ntail 1.0\nsection a 2 {\n  beat kick \"x ~ x ~\"\n}\n");
        assert_eq!(piece.sample_rate, 44_100);
        assert!((piece.seconds - 4.0).abs() < 1e-6, "{}", piece.seconds);
        assert_eq!(piece.samples.len(), 5 * 44_100 * 2);
        assert_eq!(piece.notes, 4);
        assert!(
            peak(&piece, 0, 44_100) > 0.001,
            "the first second is silent"
        );
    }

    /// The pitches a source schedules, in order, for one named line.
    fn midi_of(source: &str, line: &str) -> Vec<u8> {
        let (program, errors) = crate::parser::parse_program(source);
        assert!(errors.is_empty(), "parse: {errors:?}");
        let (piece, errors) = crate::piece::resolve(&program, (120, (4, 4), None));
        assert!(errors.is_empty(), "resolve: {errors:?}");
        let mut notes = scheduled_notes(&piece, &InstrumentRegistry::built_in(), 44_100)
            .expect("scheduled notes");
        notes.sort_by_key(|note| note.start);
        notes
            .iter()
            .filter(|note| note.line == line)
            .map(|note| note.midi)
            .collect()
    }

    /// `<a b c>` is per-bar harmony: alternative *k* belongs to bar *k* of the
    /// section, so a section sounds the same wherever the arrangement puts it
    /// (§8.5) and a line's span does not rotate the cycle under it.
    #[test]
    fn alternation_counts_from_the_section_not_the_piece() {
        // Two identical sections at different points in one arrangement.
        let source = "bpm 120\ntail 0\n\
             section pad 4 {\n  rest sine \"~\"\n}\n\
             section one 3 {\n  a sine \"<c3 e4 c5>\"\n}\n\
             section gap 1 {\n  quiet sine \"~\"\n}\n\
             section two 3 {\n  b sine \"<c3 e4 c5>\"\n}\n\
             arrange pad one gap two\n";
        let written = vec![48, 64, 72];
        assert_eq!(midi_of(source, "a"), written, "the first playing");
        assert_eq!(
            midi_of(source, "b"),
            written,
            "an identical section must not rotate because it starts elsewhere"
        );

        // A span that opens later still reads the alternation by bar number, so
        // it stays under whatever the full-span lines are playing.
        let spanned = "bpm 120\ntail 0\n\
             section s 4 {\n  chords sine \"<c3 e4 c5 g4>\"\n  late sine \"<c3 e4 c5 g4>\" @ 3..\n}\n";
        assert_eq!(midi_of(spanned, "chords"), vec![48, 64, 72, 67]);
        assert_eq!(
            midi_of(spanned, "late"),
            vec![72, 67],
            "a line entering at bar 3 takes the third alternative, not the first"
        );
    }

    #[test]
    fn the_arrangement_orders_what_is_heard() {
        let piece = rendered(
            "bpm 120\ntail 0\n\
             section a 1 {\n  beat kick \"x\"\n}\n\
             section b 1 {\n  rest kick \"~\"\n}\n\
             arrange b a\n",
        );
        let cycle = 2 * 44_100;
        assert!(
            peak(&piece, 0, cycle) < 1e-6,
            "the silent section must be silent"
        );
        assert!(
            peak(&piece, cycle, 2 * cycle) > 0.001,
            "the sounding one must follow"
        );
    }

    #[test]
    fn ungrouped_occurrences_become_independent_render_islands() {
        let piece = rendered("bpm 120\ntail 0\nsection a 1 {\n  k kick \"x\"\n}\narrange a a a\n");
        assert_eq!(piece.occurrences, 3);
        assert_eq!(piece.telemetry.islands, 3);
        assert_eq!(piece.telemetry.slots, 3);
        assert!(piece.telemetry.worker_threads >= 1);
    }

    #[test]
    fn a_shared_bus_remains_one_piece_wide_island() {
        let piece = rendered(
            "bpm 120\ntail 0\nsection a 1 {\n  group drums {\n    k kick \"x\"\n  } | reverb 0.25\n}\narrange a a a\n",
        );
        assert_eq!(piece.occurrences, 3);
        assert_eq!(piece.telemetry.islands, 1);
        assert_eq!(piece.telemetry.slots, 3);
    }

    #[test]
    fn a_span_places_a_line_inside_its_section() {
        let piece = rendered("bpm 120\ntail 0\nsection a 4 {\n  fill kick \"x\" @ 4\n}\n");
        assert_eq!(piece.notes, 1);
        let cycle = 2 * 44_100;
        assert!(peak(&piece, 0, 3 * cycle) < 1e-6, "cycles 1-3 are silent");
        assert!(peak(&piece, 3 * cycle, 4 * cycle) > 0.001, "cycle 4 sounds");
    }

    #[test]
    fn nothing_leaves_above_the_master_ceiling() {
        // Eight voices at once: the engine's ceiling has to hold, and a render
        // that clipped would be the artifact this whole path was fixed for.
        let piece = rendered(
            "bpm 120\ntail 0\nsection dense 2 {\n\
             \x20 k kick \"x*4\"\n  s snare \"x*4\"\n  h hihat \"x*8\"\n  c clap \"x*4\"\n\
             \x20 t tom \"x*4\"\n  r rim \"x*4\"\n  p pad \"[c3,e3,g3]\"\n  b bass \"c2*4\"\n}\n",
        );
        let worst = piece.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(worst <= 0.95 + 1e-6, "peaked at {worst}");
        assert!(piece.telemetry.pre_limiter_peak >= piece.telemetry.post_limiter_peak);
        assert!(piece.telemetry.max_gain_reduction_db > 0.0);
        assert!(piece.telemetry.limited_samples > 0);
    }

    #[test]
    fn progress_is_reported_in_order_and_reaches_the_end() {
        let (program, _) = crate::parser::parse_program(
            "bpm 120\ntail 0\nsection a 2 {\n  beat kick \"x ~ x ~\"\n}\n",
        );
        let (piece, _) = crate::piece::resolve(&program, (120, (4, 4), None));
        let mut seen: Vec<String> = Vec::new();
        let mut last_fraction = 0.0f64;
        let out = render_with_progress(
            &piece,
            &InstrumentRegistry::built_in(),
            44_100,
            &mut |progress| {
                match &progress {
                    Progress::Compiling { .. } => seen.push("compiling".into()),
                    Progress::Building { .. } => seen.push("building".into()),
                    Progress::Rendering {
                        frames,
                        total_frames,
                    } => {
                        let fraction = *frames as f64 / *total_frames as f64;
                        assert!(fraction >= last_fraction, "progress went backwards");
                        assert!(fraction <= 1.0, "progress exceeded 100%");
                        last_fraction = fraction;
                        seen.push("rendering".into());
                    }
                    Progress::Done { .. } => seen.push("done".into()),
                };
            },
        )
        .expect("render");

        assert_eq!(seen.first().map(String::as_str), Some("compiling"));
        assert_eq!(seen.last().map(String::as_str), Some("done"));
        assert!(seen.contains(&"building".to_string()));
        assert!(
            seen.iter().filter(|s| *s == "rendering").count() > 10,
            "too few progress reports to drive a bar"
        );
        assert!(!out.samples.is_empty());
    }

    /// `seed` (§8.8) reaches the generative constructs, and the same seed
    /// reproduces.
    ///
    /// It did not: the walk was hashed on the step index alone, so the
    /// directive documented as salting the generative constructs changed
    /// nothing at all.
    #[test]
    fn the_seed_rerolls_a_generated_walk_and_repeats_exactly() {
        let render_with = |seed: u64| {
            let source = format!(
                "seed {seed}\nbpm 120\ntail 0\nsection a 4 {{\n  walk pluck \"solo(0..7, 6)\"\n}}\n"
            );
            let (program, errors) = crate::parser::parse_program(&source);
            assert!(errors.is_empty(), "{errors:?}");
            let (piece, errors) = crate::piece::resolve(&program, (120, (4, 4), None));
            assert!(errors.is_empty(), "{errors:?}");
            let registry = InstrumentRegistry::built_in();
            let notes = scheduled_notes(&piece, &registry, 44_100).expect("notes");
            notes.into_iter().map(|note| note.midi).collect::<Vec<u8>>()
        };

        let zero = render_with(0);
        let seven = render_with(7);
        assert!(!zero.is_empty(), "the walk generated nothing");
        assert_ne!(zero, seven, "the seed changed nothing");
        assert_eq!(zero, render_with(0), "the same seed did not reproduce");
        assert_eq!(seven, render_with(7), "the same seed did not reproduce");
    }

    /// Rendering twice gives the same samples, which is what makes a render
    /// usable in a build.
    #[test]
    fn two_renders_of_one_piece_are_identical() {
        let source = "bpm 120\ntail 0\nsection a 2 {\n  k kick \"x*4\"\n  h hihat \"x*8\"\n                        p pad \"[c3,e3,g3]\"\n}\n";
        let (program, _) = crate::parser::parse_program(source);
        let (piece, _) = crate::piece::resolve(&program, (120, (4, 4), None));
        let registry = InstrumentRegistry::built_in();
        let first = render(&piece, &registry, 44_100).expect("render");
        let second = render(&piece, &registry, 44_100).expect("render");
        assert_eq!(
            first.samples, second.samples,
            "two renders of one piece differed"
        );
    }

    #[test]
    fn an_empty_arrangement_is_refused_rather_than_rendered_silent() {
        let (program, _) = crate::parser::parse_program("bpm 120\n");
        let (piece, _) = crate::piece::resolve(&program, (120, (4, 4), None));
        let error =
            render(&piece, &InstrumentRegistry::built_in(), 44_100).expect_err("nothing to render");
        assert!(error.contains("nothing to render"), "{error}");
    }
}

/// Every note the piece schedules, as `(absolute frame, midi)`.
///
/// The same walk [`render`] makes, stopping before any audio. Exposed because
/// a piece's note content is worth checking on its own — a harmonic premise
/// ("every note comes from one collection") is a property of the notes, not of
/// the samples, and asserting it against the audio would be guesswork.
pub fn scheduled_notes(
    piece: &Piece,
    registry: &InstrumentRegistry,
    sample_rate: u32,
) -> Result<Vec<ScheduledNote>, String> {
    let mut notes = Vec::new();
    let mut at_frame = 0u64;
    for occurrence in &piece.timeline {
        let section = &piece.sections[occurrence.section];
        let cycle_frames = section_cycle_frames(section, sample_rate);
        let origin_cycle = occurrence.start_cycle;
        for line in section
            .patterns
            .iter()
            .chain(&piece.throughout)
            .filter(|line| !line.muted && !section.muted)
        {
            let travel = LineTravel {
                line: Travel::START,
                cycle: origin_cycle as f64,
                origin: origin_cycle,
                divisions: section.sig.0,
            };
            let (audio_fx, fx_ramps) =
                pattern_fx(&line.transforms, section.cycle_seconds(), travel)
                    .map_err(|error| format!("'{}': {error}", line.name))?;
            let spec = registry
                .get(&line.instrument)
                .ok_or_else(|| format!("'{}': unknown instrument", line.name))?
                .clone();
            let instrument_fx = spec.fx.len();
            let compiled = compile_pattern(
                line,
                section.scale,
                audio_fx,
                fx_ramps,
                instrument_fx,
                spec,
                PatternGate::default(),
                origin_cycle,
                section.sig.0,
                piece.seed,
            );
            let from = line.span.map_or(1, |s| s.start());
            let to = line.span.map_or(section.cycles, |s| s.end(section.cycles));
            for cycle in from..=to.min(section.cycles) {
                let absolute = origin_cycle + u64::from(cycle - 1);
                let within_section = u64::from(cycle - 1);
                let cycle_start = at_frame + u64::from(cycle - 1) * cycle_frames;
                for strike in cycle_strikes_at(&compiled, absolute, within_section) {
                    let start = cycle_start + (strike.start * cycle_frames as f64).round() as u64;
                    let end = start
                        + ((strike.end - strike.start) * NOTE_GATE * cycle_frames as f64).round()
                            as u64;
                    for midi in strike.notes {
                        notes.push(ScheduledNote {
                            start,
                            end: end.max(start + 1),
                            midi,
                            line: line.name.clone(),
                            section: section.name.clone(),
                        });
                    }
                }
            }
        }
        at_frame += cycle_frames * u64::from(section.cycles);
    }
    notes.sort_by_key(|note| (note.start, note.midi));
    Ok(notes)
}

/// One scheduled note, with enough context to reason about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledNote {
    /// Absolute frame the note starts on.
    pub start: u64,
    /// Absolute frame its gate closes.
    pub end: u64,
    pub midi: u8,
    /// The pattern line that played it.
    pub line: String,
    /// The section it was played in.
    pub section: String,
}
