//! Offline rendering of a piece (§8.9).
//!
//! A piece has a definite length, so it does not need the loop engine's
//! stay-one-boundary-ahead scheduling: the whole timeline is compiled up front,
//! every note is scheduled at an absolute frame, and the graph is pulled a
//! block at a time until the arrangement and its tail have gone by.
//!
//! ## One slot per occurrence
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
//! The cost is a slot per (occurrence, line), which an offline render can
//! afford in a way the live engine could not.

use std::collections::BTreeMap;

use crate::ast::PatternDef;
use crate::piece::{Piece, Section};
use treble::app::prelude::{AudioGraph, AutomationSpec, AutomationTarget, BusSpec, ParameterRamp};
use treble::audio::{EventScheduler, InstrumentAudioMessage, render_block};
use treble::core::Note;
use treble::instruments::prelude::InstrumentRegistry;

use super::compile::{
    CompiledPattern, LineTravel, NOTE_GATE, PatternGate, RampWindow, Travel, compile_pattern,
    core_curve, cycle_seconds, cycle_strikes, fx_ramp_window, pattern_fx, transform_travels,
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

    let mut graph = AudioGraph::new();
    let mut placed: Vec<Placed> = Vec::new();
    let mut automations: Vec<AutomationSpec> = Vec::new();
    // Bus name -> the slots feeding it, across every occurrence. A bus is one
    // bus for the whole piece (§8.2), so its members accumulate.
    let mut bus_members: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut bus_chains: BTreeMap<String, Vec<treble::instruments::spec::FxSpec>> = BTreeMap::new();

    let mut at_frame = 0u64;
    for (index, occurrence) in piece.timeline.iter().enumerate() {
        progress(Progress::Compiling {
            done: index,
            total: piece.timeline.len(),
        });
        let section = &piece.sections[occurrence.section];
        let cycle_frames = section_cycle_frames(section, sample_rate);

        // A section is self-contained (§8.6): its lines' travels are measured
        // from the start of *this* occurrence, so each playing sweeps alike.
        let origin_cycle = occurrence.start_cycle;

        let lines = section
            .patterns
            .iter()
            .chain(&piece.throughout)
            .filter(|line| !line.muted && !section.muted);

        for line in lines {
            let placed_line = mount_line(
                &mut graph,
                registry,
                section,
                line,
                origin_cycle,
                at_frame,
                cycle_frames,
                &mut automations,
                piece.seed,
            )?;
            if let Some(group) = &line.group {
                bus_members
                    .entry(group.clone())
                    .or_default()
                    .push(placed_line.slot);
            }
            placed.push(placed_line);
        }

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
                        cycle: origin_cycle as f64,
                        origin: origin_cycle,
                        divisions: section.sig.0,
                    },
                )
                .map_err(|error| format!("group '{}': {error}", group.name))?;
                bus_chains.insert(group.name.clone(), fx);
            }
        }

        at_frame += cycle_frames * u64::from(section.cycles);
    }

    let buses: Vec<BusSpec> = bus_members
        .into_iter()
        .map(|(name, members)| BusSpec {
            fx: bus_chains.remove(&name).unwrap_or_default(),
            name,
            members,
        })
        .collect();
    progress(Progress::Compiling {
        done: piece.timeline.len(),
        total: piece.timeline.len(),
    });

    graph.set_buses(buses);
    graph.set_automations(automations);

    progress(Progress::Building {
        slots: placed.len(),
    });
    let mut system = graph
        .compile(sample_rate as f32)
        .map_err(|error| format!("could not build the piece's audio graph: {error:?}"))?;

    let mut scheduler = EventScheduler::new();
    let notes = schedule(&mut scheduler, &placed, &graph);

    let arrangement_frames = at_frame;
    let total_frames = arrangement_frames + (piece.tail * f64::from(sample_rate)).round() as u64;

    let mut samples: Vec<f32> = Vec::with_capacity(total_frames as usize * 2);
    let mut frame = 0u64;
    // Report about a hundred times over the render rather than per block: a
    // block is a millisecond of audio and the reports would cost more than the
    // rendering.
    let report_every = (total_frames / 100).max(1);
    let mut next_report = report_every;
    while frame < total_frames {
        frame = render_block(&mut system, &mut scheduler, frame, &mut samples);
        if frame >= next_report {
            progress(Progress::Rendering {
                frames: frame.min(total_frames),
                total_frames,
            });
            next_report = frame + report_every;
        }
    }
    progress(Progress::Done {
        frames: total_frames,
    });
    // The last block overshoots whenever the total is not a multiple of the
    // block size; trim rather than leaving a fraction of a block of tail.
    samples.truncate(total_frames as usize * 2);

    Ok(RenderedPiece {
        sample_rate,
        samples,
        seconds: piece.seconds(),
        rendered_seconds: total_frames as f64 / f64::from(sample_rate),
        occurrences: piece.timeline.len(),
        notes,
        unused: piece.unused.clone(),
    })
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
            // `cycle_strikes` counts cycles from the piece's start, since that
            // is what the line's travels and its per-cycle choices are indexed
            // by; the span is 1-based within the section, so it converts here.
            let absolute = line.pattern.ramp_origin + u64::from(cycle - 1);
            let cycle_start = line.start_frame + u64::from(cycle - 1) * line.cycle_frames;
            for strike in cycle_strikes(&line.pattern, absolute) {
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
                let cycle_start = at_frame + u64::from(cycle - 1) * cycle_frames;
                for strike in cycle_strikes(&compiled, absolute) {
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
