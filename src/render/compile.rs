//! Lowering the language onto the engine.
//!
//! This is the middleware half of Treble: everything that turns a parsed
//! buffer into something [`treble`](treble) can play. Mini-notation expands to
//! events, transforms reshape them, `def` blocks lower to `InstrumentSpec`s,
//! and `fx` names resolve against the engine's own filter registry.
//!
//! It lives behind the `render` feature because the language proper knows
//! nothing about the engine — a consumer that only wants to parse should not
//! pay for a DSP dependency. Turning the feature on is what makes
//! [`crate::render`] available, and with it the headless renderer.
//!
//! Extracted from the live front end rather than written afresh, so a piece
//! rendered offline and a performance played live compile through exactly the
//! same code and cannot drift into two dialects of one language.

#![allow(clippy::too_many_arguments)]

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use treble::instruments::spec::FxSpec;

use crate::ast::{
    Accidental, Alternation, ArpMode, Atom, EnvelopeDef, FxArg, FxCall, FxValue, Group, GroupMode,
    InstrumentDef, LfoRate, LfoWave, Lifecycle, MiniNotation, MixMode, Modifier, MonoAllocation,
    NoteLetter, PanSweep, PatternDef, PitchRoot, PolyAllocation, Ramp, RampCurve, Relation,
    ScaleMode, SegmentDef, Sequence, Solo, Step, ToneDef, Transform, VoiceDef, Waveform,
};

/// Mixer mute/solo for one pattern, carried through compilation.
///
/// A plain data holder rather than a front-end type: the compiler needs to know
/// whether a line is gated, and nothing more.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatternGate {
    pub name: String,
    pub muted: bool,
    pub solo: bool,
}

pub const NOTE_GATE: f64 = 0.82;

/// Probability used by a bare `?`, when no explicit value follows it.
pub const DEFAULT_DROP: f64 = 0.5;

/// Wet mix used by `delay` when no third argument is given.
pub const DEFAULT_DELAY_MIX: f64 = 0.35;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Rest,
    Trigger,
    Notes(Vec<u8>),
    /// `<a b c>` — one option per cycle, in order.
    Alternatives(Vec<Vec<TimedEvent>>),
    /// `[a|b|c]` — one option per cycle, chosen deterministically by hash.
    Choice(Vec<Vec<TimedEvent>>),
    /// `solo(low..high, steps)` — a generated melody. The notes are the
    /// pattern's scale degrees for the range, precomputed at compile time;
    /// the walk over them happens per cycle at resolve time.
    Solo {
        notes: Vec<u8>,
        steps: u32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimedEvent {
    pub start: f64,
    pub end: f64,
    pub event: Event,
    /// `Some(p)` when a `?` applies, carrying its drop probability.
    pub drop: Option<f64>,
    /// An inner step's own `:v` / `X`, which beats the enclosing step's.
    pub velocity: Option<f32>,
}

#[derive(Debug, PartialEq)]
pub struct ResolvedEvent {
    pub start: f64,
    pub end: f64,
    pub notes: Vec<u8>,
    /// The innermost written velocity for this event, if any.
    pub velocity: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub event: Event,
    /// `Some(p)` when a `?` applies, carrying its drop probability.
    pub drop: Option<f64>,
    pub source_step: usize,
    /// The step's own velocity (`:v`, or `X`'s implied 1.0). Absolute: it
    /// replaces the line's `| vel` for this step rather than scaling it.
    pub velocity: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct CompiledPattern {
    pub name: String,
    pub instrument: String,
    pub segments: Vec<Segment>,
    pub cycle_factor: f64,
    pub velocity: f32,
    pub audio_fx: Vec<FxSpec>,
    pub spec: treble::instruments::spec::InstrumentSpec,
    pub gate: PatternGate,
    /// The group this pattern belongs to, if any.
    pub group: Option<String>,
    /// The group's `vel` multiplier, folded in when the cycle is scheduled so
    /// it also applies to per-cycle rebuilt (ramping) velocities.
    pub velocity_scale: f32,
    /// The pattern's audio transforms as written, for the read-only mixer view.
    pub chain: Vec<String>,
    pub conditional: Vec<(u32, Transform)>,
    /// Set only when an *event* value on the line travels, in which case the
    /// segments are rebuilt each cycle rather than reused. An audio-only ramp
    /// leaves this `None` and rides on `automations` instead, so opening a
    /// filter does not cost a per-cycle re-expansion.
    pub ramp: Option<RampState>,
    /// The line's `| ramp` window, present whenever the line has a span and
    /// something on it travels — including when only an audio value does.
    pub window: Option<RampWindow>,
    /// Travelling audio parameters, addressed by position in this pattern's own
    /// `audio_fx` chain. `instrument_fx` shifts them into the compiled spec.
    pub fx_ramps: Vec<FxRamp>,
    /// How many filters the instrument itself contributes before this pattern's
    /// own chain. `evaluate` builds the mounted spec as the instrument's `fx`
    /// followed by `audio_fx`, so the pattern's Nth filter sits at
    /// `instrument_fx + N` — which is the index an automation has to name.
    pub instrument_fx: usize,
    /// A travelling group `| vel`, resolved per cycle against the group's own
    /// window rather than this pattern's.
    pub velocity_scale_ramp: Option<(Ramp<f64>, RampWindow)>,
    /// The cycle this line's travels are anchored to — also the origin for
    /// any `r(...)` window its values carry, so those exist without `| ramp`.
    pub ramp_origin: u64,
    /// The signature's numerator when this snapshot compiled — how `r(...)`
    /// spans in time divisions convert to cycles.
    pub divisions: u8,
    /// The buffer's `seed` (§8.8), mixed into every generative choice this
    /// line makes. Without it the walk was hashed on the step index alone and
    /// the directive documented in §8.8 changed nothing.
    pub seed: u64,
}

impl CompiledPattern {
    /// The per-value travel context for this line at a (fractional) cycle.
    pub fn line_travel(&self, cycle: f64) -> LineTravel {
        LineTravel {
            line: self
                .window
                .map_or(Travel::START, |window| window.travel_at(cycle)),
            cycle,
            origin: self.ramp_origin,
            divisions: self.divisions,
        }
    }
}

/// Where a travelling value started, how long it takes to arrive, and enough of
/// the source to rebuild the pattern at each point along the way.
#[derive(Debug, Clone)]
pub struct RampState {
    pub notation: MiniNotation,
    pub scale: Option<(PitchRoot, ScaleMode)>,
    pub transforms: Vec<Transform>,
}

/// A line's `| ramp <cycles> [lin|exp]`, anchored to the cycle its travel
/// started from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RampWindow {
    pub origin: u64,
    /// Cycles. Fractional spans come from `r(...)` values, whose spans are
    /// written in time divisions and divided by the signature's numerator.
    pub span: f64,
    pub curve: RampCurve,
}

impl RampWindow {
    /// How far the travel has got at `cycle`, and how it gets there.
    pub fn travel(&self, cycle: u64) -> Travel {
        self.travel_at(cycle as f64)
    }

    /// [`Self::travel`] at a fractional cycle — the readout moves smoothly
    /// between boundaries, like the per-block audio automations do.
    ///
    /// For `osc` the progress is the triangle value: `0 → 1 → 0` over one
    /// span, wrapping forever, so interpolating between the ends oscillates.
    pub fn travel_at(&self, cycle: f64) -> Travel {
        let progress = match self.curve {
            RampCurve::Oscillate => {
                let phase = self.phase_at(cycle);
                1.0 - (2.0 * phase - 1.0).abs()
            }
            _ => ramp_progress_at(cycle, self.origin, self.span),
        };
        Travel {
            progress,
            curve: self.curve,
        }
    }

    /// Position within the window for the readout: clamped travel for curves
    /// that arrive, the raw phase within the current period for `osc`.
    pub fn phase_at(&self, cycle: f64) -> f64 {
        if self.curve != RampCurve::Oscillate || self.span <= 0.0 {
            return ramp_progress_at(cycle, self.origin, self.span);
        }
        let elapsed = (cycle - self.origin as f64).max(0.0);
        (elapsed / self.span).fract()
    }
}

/// How far along a line's travel a value should be read, and along which curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Travel {
    pub progress: f64,
    pub curve: RampCurve,
}

impl Travel {
    /// The state of a line that does not travel: every value reads as written.
    pub const START: Self = Self {
        progress: 0.0,
        curve: RampCurve::Linear,
    };
}

/// How each value on a line reads its travel at one cycle: a value without a
/// window of its own follows the line's `| ramp`, a `r(...)` value follows
/// the window it carries — span in time divisions, anchored to the same
/// origin as the line's.
#[derive(Debug, Clone, Copy)]
pub struct LineTravel {
    pub line: Travel,
    pub cycle: f64,
    pub origin: u64,
    pub divisions: u8,
}

impl LineTravel {
    /// A context where nothing has travelled yet: every value, `r(...)` ones
    /// included, reads at its starting end.
    pub const STILL: Self = Self {
        line: Travel::START,
        cycle: 0.0,
        origin: 0,
        divisions: 4,
    };

    pub fn of<T: Copy>(&self, ramp: &Ramp<T>) -> Travel {
        match ramp.own_window() {
            Some((span_divisions, curve)) => RampWindow {
                origin: self.origin,
                span: span_divisions / self.divisions.max(1) as f64,
                curve,
            }
            .travel_at(self.cycle),
            None => self.line,
        }
    }
}

/// One travelling audio parameter, ready to become an [`AutomationSpec`] once
/// the frame timeline is known.
///
/// A filter parameter can only take one value at a graph build, so the compiled
/// [`FxSpec`] carries the value at the pattern's current progress and the travel
/// between the ends becomes an automation the render thread evaluates per block.
#[derive(Debug, Clone, PartialEq)]
pub struct FxRamp {
    /// Index into the owning chain: the pattern's `audio_fx`, or a bus's `fx`.
    pub chain_index: usize,
    pub param: String,
    pub from: f32,
    pub to: f32,
    /// A window of the value's own, from `r(from,to,span,style)` — span in
    /// time divisions plus the curve. `None` rides the line's `| ramp`.
    pub window: Option<(f64, RampCurve)>,
}

/// The window a travelling fx parameter reads against: its own `r(...)` span
/// (in time divisions, converted with the signature) or the line's `| ramp`.
pub fn fx_ramp_window(
    ramp: &FxRamp,
    line: Option<RampWindow>,
    origin: u64,
    divisions: u8,
) -> Option<RampWindow> {
    match ramp.window {
        Some((span_divisions, curve)) => Some(RampWindow {
            origin,
            span: span_divisions / divisions.max(1) as f64,
            curve,
        }),
        None => line,
    }
}

pub fn core_curve(curve: RampCurve) -> treble::app::prelude::RampCurve {
    match curve {
        RampCurve::Linear => treble::app::prelude::RampCurve::Linear,
        RampCurve::Exponential => treble::app::prelude::RampCurve::Exponential,
        RampCurve::Oscillate => treble::app::prelude::RampCurve::Oscillate,
    }
}

/// One note-on a cycle produces: where in the cycle it lands, as a fraction,
/// which notes it strikes, and how hard.
#[derive(Debug, Clone, PartialEq)]
pub struct Strike {
    pub start: f64,
    pub end: f64,
    pub notes: Vec<u8>,
    pub velocity: f32,
}

/// Everything one pattern sounds in one cycle, before frames get involved.
///
/// Split out of [`schedule_cycle`] so the velocity rules can be read off
/// directly rather than inferred from what the audio backend received.
pub fn cycle_strikes(pattern: &CompiledPattern, cycle: u64) -> Vec<Strike> {
    cycle_strikes_at(pattern, cycle, cycle)
}

/// [`cycle_strikes`], with the alternation clock stated separately.
///
/// Everything indexed by `cycle` — travels, `?p` drops, `[a|b]` choices, a
/// `solo`'s walk — counts from wherever the caller's clock starts, so a sweep
/// keeps travelling and a seeded phrase keeps evolving. `<a b c>` is the
/// exception: it is written as per-bar harmony, so alternative *k* belongs to
/// bar *k* of the section it sits in and must not move when the arrangement
/// puts that section somewhere else (§8.5). A piece passes the cycle within the
/// section for `alternation_cycle`; a live buffer, which has no sections, passes
/// the same value for both and is unaffected.
pub fn cycle_strikes_at(
    pattern: &CompiledPattern,
    cycle: u64,
    alternation_cycle: u64,
) -> Vec<Strike> {
    let (segments, cycle_factor, line_velocity) = pattern_cycle(pattern, cycle);
    // A group's shared `| vel` is a bus level rather than a strike, so it is
    // the one velocity that multiplies — including over a step's own `:v`.
    let scale = match &pattern.velocity_scale_ramp {
        Some((ramp, window)) => travel_f64(
            ramp,
            LineTravel {
                line: window.travel(cycle),
                cycle: cycle as f64,
                origin: window.origin,
                divisions: pattern.divisions,
            },
        )
        .start()
        .clamp(0.0, 1.0) as f32,
        None => pattern.velocity_scale,
    };
    let mut strikes = Vec::new();
    for (segment_index, segment) in segments.iter().enumerate() {
        if segment
            .drop
            .is_some_and(|p| should_drop(cycle, &pattern.name, segment_index, p))
        {
            continue;
        }
        let outer_start = segment.start * cycle_factor;
        let outer_duration = (segment.end - segment.start) * cycle_factor;
        // The step index keeps sibling steps independent; the buffer's seed is
        // what makes the whole passage rerollable.
        let seed = mix_seed(pattern.seed, segment.source_step);
        for resolved in resolve_events_at(
            &segment.event,
            cycle,
            alternation_cycle,
            &pattern.name,
            seed,
        ) {
            let start = outer_start + resolved.start * outer_duration;
            // `:v` and `X` are absolute: they replace the line's `| vel` for the
            // step rather than scaling it, so a performer nudging `vel` to
            // balance a line does not have to recompute their accents. The
            // innermost written velocity wins.
            let velocity = resolved
                .velocity
                .or(segment.velocity)
                .unwrap_or(line_velocity)
                * scale;
            strikes.push(Strike {
                start,
                end: start + (resolved.end - resolved.start) * outer_duration,
                notes: resolved.notes,
                velocity,
            });
        }
    }
    strikes
}

#[allow(clippy::too_many_arguments)]
pub fn compile_pattern(
    pattern: &PatternDef,
    scale: Option<(PitchRoot, ScaleMode)>,
    audio_fx: Vec<FxSpec>,
    fx_ramps: Vec<FxRamp>,
    instrument_fx: usize,
    spec: treble::instruments::spec::InstrumentSpec,
    gate: PatternGate,
    ramp_origin: u64,
    divisions: u8,
    seed: u64,
) -> CompiledPattern {
    let window = ramp_window(pattern, ramp_origin);
    let mut slots = Vec::new();
    let mut cycle_factor = 1.0;
    // The precompiled segments stand for a line whose events do not travel; one
    // whose events do is rebuilt per cycle by `pattern_cycle` instead.
    expand_sequence(
        &pattern.notation,
        &pattern.notation.sequence,
        1.0,
        scale,
        &mut slots,
        &mut cycle_factor,
        None,
    );
    let mut segments = normalize(slots);
    let mut velocity = 1.0;
    let mut conditional = Vec::new();
    for transform in &pattern.transforms {
        match transform {
            Transform::Every(cycles, inner) => {
                conditional.push(((*cycles).max(1), (**inner).clone()));
            }
            _ => apply_transform(&mut segments, &mut cycle_factor, &mut velocity, transform),
        }
    }
    CompiledPattern {
        name: pattern.name.clone(),
        instrument: pattern.instrument.clone(),
        segments,
        cycle_factor,
        velocity,
        audio_fx,
        spec,
        gate,
        group: pattern.group.clone(),
        velocity_scale: 1.0,
        chain: describe_audio_chain(&pattern.transforms),
        conditional,
        ramp: ramp_state(pattern, scale, window),
        window,
        fx_ramps,
        instrument_fx,
        velocity_scale_ramp: None,
        ramp_origin,
        divisions,
        seed,
    }
}

pub fn pattern_cycle(pattern: &CompiledPattern, cycle: u64) -> (Vec<Segment>, f64, f32) {
    // Without travelling event values the compiled segments stand for every
    // cycle; with them (a line `| ramp`, or `r(...)` windows of their own)
    // the pattern is rebuilt at the values the line holds now.
    let travel = pattern.line_travel(cycle as f64);
    let (mut segments, mut cycle_factor, mut velocity) = match &pattern.ramp {
        Some(ramp) => rebuild_for_progress(ramp, travel),
        None => (
            pattern.segments.clone(),
            pattern.cycle_factor,
            pattern.velocity,
        ),
    };
    for (interval, transform) in &pattern.conditional {
        if (cycle + 1).is_multiple_of(*interval as u64) {
            let resolved = resolve_transform(transform, travel);
            apply_transform(&mut segments, &mut cycle_factor, &mut velocity, &resolved);
        }
    }
    (segments, cycle_factor, velocity)
}

/// Re-expand a ramped pattern at one point along its travel.
pub fn rebuild_for_progress(ramp: &RampState, travel: LineTravel) -> (Vec<Segment>, f64, f32) {
    let notation = resolve_notation(&ramp.notation, travel);
    let mut slots = Vec::new();
    let mut cycle_factor = 1.0;
    expand_sequence(
        &notation,
        &notation.sequence,
        1.0,
        ramp.scale,
        &mut slots,
        &mut cycle_factor,
        None,
    );
    let mut segments = normalize(slots);
    let mut velocity = 1.0f32;
    for transform in ramp.transforms.iter() {
        if matches!(transform, Transform::Every(_, _)) {
            continue;
        }
        apply_transform(
            &mut segments,
            &mut cycle_factor,
            &mut velocity,
            &resolve_transform(transform, travel),
        );
    }
    (segments, cycle_factor, velocity)
}

/// The line's `| ramp` span and curve, if it has one.
pub fn ramp_span(transforms: &[Transform]) -> Option<(u32, RampCurve)> {
    transforms.iter().find_map(|transform| match transform {
        Transform::RampSpan { cycles, curve } => Some((*cycles, *curve)),
        _ => None,
    })
}

/// The line's ramp window, or `None` when it has no span or nothing to move.
pub fn ramp_window(pattern: &PatternDef, origin: u64) -> Option<RampWindow> {
    let (span, curve) = ramp_span(&pattern.transforms)?;
    let span = span as f64;
    let travels =
        notation_travels(&pattern.notation) || pattern.transforms.iter().any(transform_travels);
    travels.then_some(RampWindow {
        origin,
        span,
        curve,
    })
}

/// The per-cycle rebuild state for a pattern, or `None` when nothing about its
/// *events* moves.
///
/// An audio-only ramp deliberately produces no state: its travel is declared as
/// an automation instead, so the line keeps the precompiled fast path.
pub fn ramp_state(
    pattern: &PatternDef,
    scale: Option<(PitchRoot, ScaleMode)>,
    window: Option<RampWindow>,
) -> Option<RampState> {
    // Event values travel across the line's window, or across `r(...)`
    // windows of their own — either way the pattern rebuilds each cycle.
    let timed =
        notation_timed(&pattern.notation) || pattern.transforms.iter().any(transform_timed_event);
    let travels = window.is_some()
        && (notation_travels(&pattern.notation)
            || pattern.transforms.iter().any(event_transform_travels));
    (travels || timed).then(|| RampState {
        notation: pattern.notation.clone(),
        scale,
        transforms: pattern.transforms.clone(),
    })
}

/// How long one cycle lasts, in seconds, for the given tempo and metre.
///
/// Cycle-relative LFO rates are converted to hertz against this, so a `bpm` or
/// `sig` edit re-tunes every sweep on the next evaluation.
pub fn cycle_seconds(bpm: u32, signature: (u8, u8)) -> f64 {
    let quarters = signature.0 as f64 * 4.0 / signature.1.max(1) as f64;
    quarters * 60.0 / bpm.max(1) as f64
}

/// Compile a transform list into a filter chain plus the travel each of its
/// parameters is on.
///
/// A filter parameter can only take one value at a graph build, so the returned
/// [`FxSpec`]s carry each ramp's value at `travel` — the point the line's build
/// has reached — and the travel itself comes back separately, to be declared as
/// an automation the render thread evaluates per block. A `Ramp::Fixed` yields
/// no travel at all, so an existing buffer compiles to exactly what it did
/// before ramps existed.
pub fn pattern_fx(
    transforms: &[Transform],
    cycle_seconds: f64,
    travel: LineTravel,
) -> Result<(Vec<FxSpec>, Vec<FxRamp>), String> {
    let mut effects = Vec::new();
    let mut ramps = Vec::new();
    for transform in transforms {
        append_pattern_fx(transform, cycle_seconds, travel, &mut effects, &mut ramps)?;
    }
    Ok((effects, ramps))
}

/// One audio parameter: the value the graph is built with, and the endpoints it
/// travels between when it travels at all.
pub struct AudioParam {
    /// The value's own `r(...)` window, if it carries one.
    pub window: Option<(f64, RampCurve)>,
    pub value: f32,
    pub sweep: Option<(f32, f32)>,
}

/// Resolve one audio parameter against the engine's own limits.
///
/// Every value the ramp passes through is checked, not only the one it starts
/// on: the whole travel is played. treble-lang already rejects the intervals its
/// §4.3 documents at parse time, so what is left here are the ceilings only the
/// engine knows — a 20 kHz cutoff, a 20 second delay line — and the defence
/// against a value arriving from anywhere other than the parser.
pub fn audio_param(
    name: &str,
    ramp: &Ramp<f64>,
    minimum: f64,
    maximum: f64,
    travel: LineTravel,
) -> Result<AudioParam, String> {
    for value in ramp.values() {
        if !value.is_finite() || !(minimum..=maximum).contains(&value) {
            return Err(format!(
                "{name} value {value} is outside the supported range {minimum}–{maximum}"
            ));
        }
    }
    let (inner, window) = match ramp {
        Ramp::Timed {
            ramp,
            span_divisions,
            curve,
        } => (ramp.as_ref(), Some((*span_divisions, *curve))),
        other => (other, None),
    };
    Ok(AudioParam {
        // The full ramp, not the unwrapped inner: a `r(...)` value reads its
        // held value against its own window, not the line's.
        value: travel_f64(ramp, travel).start() as f32,
        // A step chain holds each stage rather than sweeping, which one
        // from→to automation cannot express; `audio_step_chains` rejects it
        // before this point, so nothing here has to guess.
        sweep: match inner {
            Ramp::Sweep { from, to } => Some((*from as f32, *to as f32)),
            _ => None,
        },
        window,
    })
}

pub fn append_pattern_fx(
    transform: &Transform,
    cycle_seconds: f64,
    travel: LineTravel,
    effects: &mut Vec<FxSpec>,
    ramps: &mut Vec<FxRamp>,
) -> Result<(), String> {
    // The filter this transform becomes takes the next slot in the chain, which
    // is the index an automation has to name.
    let chain_index = effects.len();
    let plain = |name: &str, value: f64, minimum: f64, maximum: f64| {
        if value.is_finite() && (minimum..=maximum).contains(&value) {
            Ok(value as f32)
        } else {
            Err(format!(
                "{name} value {value} is outside the supported range {minimum}–{maximum}"
            ))
        }
    };
    // Collects one resolved parameter, remembering its travel if it has any.
    macro_rules! param {
        ($key:literal, $name:literal, $ramp:expr, $min:expr, $max:expr) => {{
            let resolved = audio_param($name, $ramp, $min, $max, travel)?;
            if let Some((from, to)) = resolved.sweep {
                ramps.push(FxRamp {
                    chain_index,
                    param: $key.into(),
                    from,
                    to,
                    window: resolved.window,
                });
            }
            ($key.into(), resolved.value)
        }};
    }
    match transform {
        Transform::Gain(factor) => effects.push(FxSpec {
            type_id: "GainFilter".into(),
            params: HashMap::from([param!("factor", "gain", factor, 0.0, 2.0)]),
        }),
        Transform::Pan(direction) => effects.push(FxSpec {
            type_id: "PanFilter".into(),
            params: HashMap::from([param!("direction", "pan", direction, -1.0, 1.0)]),
        }),
        Transform::Fx(call) => {
            let (spec, call_ramps) = resolve_fx_call(call, cycle_seconds, travel)?;
            effects.push(spec);
            ramps.extend(call_ramps.into_iter().map(|mut ramp| {
                ramp.chain_index = chain_index;
                ramp
            }));
        }
        Transform::AutoPan(sweep) => effects.push(FxSpec {
            type_id: "AutoPanFilter".into(),
            params: HashMap::from([
                (
                    "frequency".into(),
                    plain("pan rate", sweep_hertz(sweep, cycle_seconds)?, 0.0, 100.0)?,
                ),
                (
                    "depth".into(),
                    plain(
                        "pan depth",
                        sweep.depth.unwrap_or(DEFAULT_SWEEP_DEPTH),
                        0.0,
                        1.0,
                    )?,
                ),
                ("waveform".into(), wave_ordinal(sweep.wave) as f32),
            ]),
        }),
        Transform::Lpf(cutoff) => effects.push(FxSpec {
            type_id: "LowPassFilter".into(),
            params: HashMap::from([param!("cutoff_frequency", "lpf", cutoff, 1.0, 20_000.0)]),
        }),
        Transform::Hpf(cutoff) => effects.push(FxSpec {
            type_id: "HighPassFilter".into(),
            params: HashMap::from([param!("cutoff_frequency", "hpf", cutoff, 1.0, 20_000.0)]),
        }),
        Transform::Delay(time, feedback, mix) => {
            let mix = mix.clone().unwrap_or(Ramp::Fixed(DEFAULT_DELAY_MIX));
            effects.push(FxSpec {
                type_id: "DelayFilter".into(),
                params: HashMap::from([
                    param!("delay_for", "delay time", time, 0.0, 20.0),
                    param!("feedback", "delay feedback", feedback, 0.0, 0.99),
                    param!("mix", "delay mix", &mix, 0.0, 1.0),
                ]),
            })
        }
        Transform::Reverb(amount) => effects.push(FxSpec {
            type_id: "ReverbFilter".into(),
            params: HashMap::from([param!("amount", "reverb", amount, 0.0, 1.0)]),
        }),
        Transform::Every(_, _) => {}
        Transform::RampSpan { .. } => {}
        // Event-shaping transforms are handled by `apply_transform`.
        Transform::Rev
        | Transform::Fast(_)
        | Transform::Slow(_)
        | Transform::Arp(_)
        | Transform::Scale(_, _)
        | Transform::Oct(_)
        | Transform::Vel(_) => {}
    }
    Ok(())
}

/// Whether a transform reshapes the scheduled events, as opposed to describing
/// a filter in the pattern's DSP chain.
/// How far a ramp has travelled at `cycle`, given where it started and how many
/// cycles it spans. Clamped, so a ramp arrives and then holds.
/// Clamped travel through a window at a (possibly fractional) cycle.
pub fn ramp_progress_at(cycle: f64, origin: u64, span: f64) -> f64 {
    if span <= 0.0 {
        return 1.0;
    }
    ((cycle - origin as f64).max(0.0) / span).clamp(0.0, 1.0)
}

/// Which held stage `progress` falls in, given `count` equal shares.
///
/// The last stage takes everything from its start onwards, so a chain holds its
/// final value once the span has passed.
pub fn stage_index(count: usize, progress: f64) -> usize {
    if count == 0 {
        return 0;
    }
    ((progress * count as f64).floor().max(0.0) as usize).min(count - 1)
}

/// Interpolate between two ends along the line's curve.
///
/// `exp` moves in equal ratio steps, which is what a cutoff needs to sound like
/// a steady opening. A range that touches or crosses zero has no geometric path
/// between its ends, so it falls back to linear travel — the same reading
/// treble-core takes for a degenerate automation, so a value and the sweep that
/// continues it never disagree.
pub fn interpolate(from: f64, to: f64, travel: Travel) -> f64 {
    match travel.curve {
        RampCurve::Exponential if from > 0.0 && to > 0.0 && from != to => {
            from * (to / from).powf(travel.progress)
        }
        _ => from + (to - from) * travel.progress,
    }
}

pub fn travel_f64(ramp: &Ramp<f64>, travel: LineTravel) -> Ramp<f64> {
    let at = travel.of(ramp);
    let inner = match ramp {
        Ramp::Timed { ramp, .. } => ramp.as_ref(),
        other => other,
    };
    Ramp::Fixed(match inner {
        Ramp::Fixed(value) => *value,
        Ramp::Sweep { from, to } => interpolate(*from, *to, at),
        _ => {
            let stages = inner.values();
            stages[stage_index(stages.len(), at.progress)]
        }
    })
}

pub fn travel_u32(ramp: &Ramp<u32>, travel: LineTravel) -> Ramp<u32> {
    let at = travel.of(ramp);
    let inner = match ramp {
        Ramp::Timed { ramp, .. } => ramp.as_ref(),
        other => other,
    };
    Ramp::Fixed(match inner {
        Ramp::Fixed(value) => *value,
        Ramp::Sweep { from, to } => {
            interpolate(*from as f64, *to as f64, at).round().max(0.0) as u32
        }
        _ => {
            let stages = inner.values();
            stages[stage_index(stages.len(), at.progress)]
        }
    })
}

pub fn travel_i32(ramp: &Ramp<i32>, travel: LineTravel) -> Ramp<i32> {
    let at = travel.of(ramp);
    let inner = match ramp {
        Ramp::Timed { ramp, .. } => ramp.as_ref(),
        other => other,
    };
    Ramp::Fixed(match inner {
        Ramp::Fixed(value) => *value,
        Ramp::Sweep { from, to } => interpolate(*from as f64, *to as f64, at).round() as i32,
        _ => {
            let stages = inner.values();
            stages[stage_index(stages.len(), at.progress)]
        }
    })
}

/// Collapse every range in a pattern to the value it holds at `progress`.
///
/// Resolving up front keeps the expansion and transform code ramp-unaware:
/// they only ever see concrete values.
pub fn resolve_notation(notation: &MiniNotation, travel: LineTravel) -> MiniNotation {
    MiniNotation {
        sequence: resolve_sequence(&notation.sequence, travel),
    }
}

pub fn resolve_sequence(sequence: &Sequence, travel: LineTravel) -> Sequence {
    Sequence {
        steps: sequence
            .steps
            .iter()
            .map(|step| Step {
                atom: resolve_atom(&step.atom, travel),
                modifiers: step
                    .modifiers
                    .iter()
                    .map(|modifier| resolve_modifier(modifier, travel))
                    .collect(),
                velocity: step.velocity.as_ref().map(|ramp| travel_f64(ramp, travel)),
            })
            .collect(),
    }
}

pub fn resolve_atom(atom: &Atom, travel: LineTravel) -> Atom {
    match atom {
        Atom::Solo(solo) => Atom::Solo(Solo {
            low: solo.low,
            high: solo.high,
            steps: travel_u32(&solo.steps, travel),
        }),
        Atom::Group(group) => Atom::Group(Group {
            mode: group.mode,
            layers: group
                .layers
                .iter()
                .map(|layer| resolve_sequence(layer, travel))
                .collect(),
        }),
        Atom::Alternation(alternation) => Atom::Alternation(Alternation {
            sequence: resolve_sequence(&alternation.sequence, travel),
        }),
        other => other.clone(),
    }
}

pub fn resolve_modifier(modifier: &Modifier, travel: LineTravel) -> Modifier {
    match modifier {
        Modifier::Repeat(count) => Modifier::Repeat(travel_u32(count, travel)),
        Modifier::Euclidean(onsets, positions, offset) => Modifier::Euclidean(
            travel_u32(onsets, travel),
            travel_u32(positions, travel),
            *offset,
        ),
        Modifier::Drop(probability) => {
            Modifier::Drop(probability.as_ref().map(|ramp| travel_f64(ramp, travel)))
        }
        other => other.clone(),
    }
}

pub fn resolve_transform(transform: &Transform, travel: LineTravel) -> Transform {
    match transform {
        Transform::Fast(amount) => Transform::Fast(travel_f64(amount, travel)),
        Transform::Slow(amount) => Transform::Slow(travel_f64(amount, travel)),
        Transform::Oct(octaves) => Transform::Oct(travel_i32(octaves, travel)),
        Transform::Vel(value) => Transform::Vel(travel_f64(value, travel)),
        Transform::Every(cycles, inner) => {
            Transform::Every(*cycles, Box::new(resolve_transform(inner, travel)))
        }
        other => other.clone(),
    }
}

/// Whether anything on this line travels, and so needs a ramp span.
/// Whether a value asks the *line* for a window: it travels and does not
/// carry a `r(...)` span of its own.
pub fn needs_span<T: Copy>(ramp: &Ramp<T>) -> bool {
    ramp.travels() && ramp.own_window().is_none()
}

/// Whether the notation carries a `r(...)` span anywhere. Event values are
/// rebuilt per cycle against the line's single window, so per-value windows
/// on them are rejected rather than silently ignored.
pub fn notation_timed(notation: &MiniNotation) -> bool {
    fn timed(steps: &Sequence) -> bool {
        steps.steps.iter().any(|step| {
            step.velocity
                .as_ref()
                .is_some_and(|ramp| ramp.own_window().is_some())
                || step.modifiers.iter().any(|modifier| match modifier {
                    Modifier::Repeat(count) => count.own_window().is_some(),
                    Modifier::Euclidean(onsets, positions, _) => {
                        onsets.own_window().is_some() || positions.own_window().is_some()
                    }
                    Modifier::Drop(probability) => probability
                        .as_ref()
                        .is_some_and(|ramp| ramp.own_window().is_some()),
                    _ => false,
                })
                || match &step.atom {
                    Atom::Group(group) => group.layers.iter().any(timed),
                    Atom::Alternation(alternation) => timed(&alternation.sequence),
                    Atom::Solo(solo) => solo.steps.own_window().is_some(),
                    _ => false,
                }
        })
    }
    timed(&notation.sequence)
}

/// Whether an *event* transform carries a `r(...)` span — same rejection as
/// [`notation_timed`], and for the same reason.
pub fn transform_timed_event(transform: &Transform) -> bool {
    match transform {
        Transform::Fast(amount) | Transform::Slow(amount) | Transform::Vel(amount) => {
            amount.own_window().is_some()
        }
        Transform::Oct(octaves) => octaves.own_window().is_some(),
        Transform::Every(_, inner) => transform_timed_event(inner),
        _ => false,
    }
}

pub fn notation_travels(notation: &MiniNotation) -> bool {
    fn travels(steps: &Sequence) -> bool {
        steps.steps.iter().any(|step| {
            step.velocity.as_ref().is_some_and(needs_span)
                || step.modifiers.iter().any(|modifier| match modifier {
                    Modifier::Repeat(count) => needs_span(count),
                    Modifier::Euclidean(onsets, positions, _) => {
                        needs_span(onsets) || needs_span(positions)
                    }
                    Modifier::Drop(probability) => probability.as_ref().is_some_and(needs_span),
                    _ => false,
                })
                || match &step.atom {
                    Atom::Group(group) => group.layers.iter().any(travels),
                    Atom::Alternation(alternation) => travels(&alternation.sequence),
                    Atom::Solo(solo) => needs_span(&solo.steps),
                    _ => false,
                }
        })
    }
    travels(&notation.sequence)
}

/// Whether any chord layer carries a per-note velocity.
///
/// The notes of a chord sound as one strike, so they cannot carry a velocity
/// each. A `:v` on the chord itself works; one inside it is reported rather than
/// silently dropped.
pub fn chord_velocities(notation: &MiniNotation) -> bool {
    fn walk(sequence: &Sequence, inside_chord: bool) -> bool {
        sequence.steps.iter().any(|step| {
            (inside_chord && step.velocity.is_some())
                || match &step.atom {
                    Atom::Group(group) => group
                        .layers
                        .iter()
                        .any(|layer| walk(layer, inside_chord || group.mode == GroupMode::Chord)),
                    Atom::Alternation(alternation) => walk(&alternation.sequence, inside_chord),
                    _ => false,
                }
        })
    }
    walk(&notation.sequence, false)
}

/// Whether a transform reshapes the *events* and travels, so the pattern has to
/// be re-expanded each cycle.
///
/// Audio travel is deliberately excluded: it becomes an automation the render
/// thread evaluates, so an audio-only ramp must not drag the line onto the
/// per-cycle rebuild path.
pub fn event_transform_travels(transform: &Transform) -> bool {
    match transform {
        Transform::Fast(amount) | Transform::Slow(amount) | Transform::Vel(amount) => {
            needs_span(amount)
        }
        Transform::Oct(octaves) => needs_span(octaves),
        Transform::Every(_, inner) => event_transform_travels(inner),
        _ => false,
    }
}

/// Whether a transform has a range on it at all, event or audio. This is what
/// decides whether the line needs a `| ramp` span.
pub fn transform_travels(transform: &Transform) -> bool {
    match transform {
        Transform::Gain(value) | Transform::Pan(value) | Transform::Reverb(value) => {
            needs_span(value)
        }
        Transform::Lpf(cutoff) | Transform::Hpf(cutoff) => needs_span(cutoff),
        Transform::Delay(time, feedback, mix) => {
            needs_span(time) || needs_span(feedback) || mix.as_ref().is_some_and(needs_span)
        }
        Transform::Fx(call) => call.args.iter().any(|arg| match arg {
            FxArg::Positional(value) | FxArg::Named(_, value) => needs_span(value.ramp()),
        }),
        Transform::Every(_, inner) => transform_travels(inner),
        other => event_transform_travels(other),
    }
}

/// Audio parameters written as a held chain (`a>b>c`) rather than a sweep.
///
/// A held chain has no single from→to shape, and a filter parameter takes one
/// automation: a second one on the same parameter would simply overwrite the
/// first at every frame rather than take over at its stage boundary. Stepping it
/// for real would mean rebuilding the graph at each stage, which cuts every
/// sounding voice — so this is reported rather than silently flattened.
pub fn audio_step_chains(transforms: &[Transform]) -> Vec<&'static str> {
    fn chains(ramp: &Ramp<f64>) -> bool {
        match ramp {
            Ramp::Steps { .. } => true,
            Ramp::Timed { ramp, .. } => chains(ramp),
            _ => false,
        }
    }
    transforms
        .iter()
        .filter(|transform| match transform {
            Transform::Gain(value) | Transform::Pan(value) | Transform::Reverb(value) => {
                chains(value)
            }
            Transform::Lpf(cutoff) | Transform::Hpf(cutoff) => chains(cutoff),
            Transform::Delay(time, feedback, mix) => {
                chains(time) || chains(feedback) || mix.as_ref().is_some_and(chains)
            }
            Transform::Fx(call) => call.args.iter().any(|arg| match arg {
                FxArg::Positional(value) | FxArg::Named(_, value) => chains(value.ramp()),
            }),
            _ => false,
        })
        .map(transform_name)
        .collect()
}

pub fn event_transform(transform: &Transform) -> bool {
    matches!(
        transform,
        Transform::Rev
            | Transform::Fast(_)
            | Transform::Slow(_)
            | Transform::Arp(_)
            | Transform::Scale(_, _)
            | Transform::Oct(_)
            | Transform::Vel(_)
    )
}

pub fn transform_name(transform: &Transform) -> &'static str {
    match transform {
        Transform::Rev => "rev",
        Transform::Fast(_) => "fast",
        Transform::Slow(_) => "slow",
        Transform::Every(_, _) => "every",
        Transform::Arp(_) => "arp",
        Transform::Scale(_, _) => "scale",
        Transform::Oct(_) => "oct",
        Transform::Vel(_) => "vel",
        Transform::RampSpan { .. } => "ramp",
        Transform::Gain(_) => "gain",
        Transform::Pan(_) => "pan",
        Transform::AutoPan(_) => "pan",
        Transform::Lpf(_) => "lpf",
        Transform::Hpf(_) => "hpf",
        Transform::Delay(_, _, _) => "delay",
        Transform::Reverb(_) => "reverb",
        Transform::Fx(_) => "fx",
    }
}

/// Lower a `def` block onto the same [`InstrumentSpec`] the JSON and visual
/// editors produce, so a definition written either way behaves identically.
///
/// Defaults follow `LANGUAGE.md` §6: an eight-voice polyphonic gated instrument
/// summing its tones at unity gain.
pub fn lower_instrument_def(
    definition: &InstrumentDef,
    cycle_seconds: f64,
) -> Result<treble::instruments::spec::InstrumentSpec, String> {
    use treble::instruments::spec as core;

    let voice = match definition.voice {
        Some(VoiceDef::Mono {
            track_pitch,
            allocation,
        }) => core::VoiceSpec::Mono {
            track_pitch,
            allocation: match allocation {
                MonoAllocation::Replace => {
                    treble::core::graph::sources::MonophonicAllocationStrategy::Replace
                }
                MonoAllocation::Drop => {
                    treble::core::graph::sources::MonophonicAllocationStrategy::Drop
                }
            },
        },
        Some(VoiceDef::Poly { voices, allocation }) => core::VoiceSpec::Poly {
            voices: voices as usize,
            allocation: lower_poly_allocation(allocation),
        },
        None => core::VoiceSpec::Poly {
            voices: 8,
            allocation: lower_poly_allocation(PolyAllocation::ReplaceOldest),
        },
    };

    let mut fx = Vec::new();
    for call in definition.fx.iter() {
        // A `def` block's own chain is not a pattern line, so it has no `ramp`
        // span to travel across: every value reads as written.
        let (spec, _) = resolve_fx_call(call, cycle_seconds, LineTravel::STILL)?;
        fx.push(spec);
    }

    Ok(core::InstrumentSpec {
        name: definition.name.clone(),
        note_lifecycle: match definition.lifecycle.unwrap_or(Lifecycle::Gated) {
            Lifecycle::OneShot => core::NoteLifecycle::OneShot,
            Lifecycle::Gated => core::NoteLifecycle::Gated,
            Lifecycle::Cutoff => core::NoteLifecycle::Cutoff,
        },
        voice,
        tones: definition
            .tones
            .iter()
            .map(lower_tone)
            .collect::<Result<Vec<_>, String>>()?,
        sample: definition.sample.as_ref().map(|sample| core::SampleSpec {
            path: sample.path.clone().into(),
            root_midi: sample.root_midi.unwrap_or(60).min(127) as u8,
            start_seconds: sample.start_seconds.unwrap_or(0.0) as f32,
            end_seconds: sample.end_seconds.map(|value| value as f32),
            looped: sample.looped,
        }),
        mix_mode: {
            // Not `treble_meta::MixMode`: that separate enum mixes signals at a
            // graph input and has a different variant list.
            use treble::core::generator::prelude::MixMode as CoreMixMode;
            match definition.mix.unwrap_or(MixMode::Sum) {
                MixMode::Sum => CoreMixMode::Sum,
                MixMode::Multiply => CoreMixMode::Multiply,
                MixMode::Max => CoreMixMode::Max,
                MixMode::Average => CoreMixMode::Average,
            }
        },
        pitch_envelope: definition.pitch.as_ref().map(lower_envelope),
        amplitude_envelope: definition.amplitude.as_ref().map(lower_envelope),
        base_frequency: definition.base_frequency.map(|value| value as f32),
        fx,
        gain: definition.gain.unwrap_or(1.0) as f32,
        velocity_sensitivity: definition.velocity_sensitivity.unwrap_or(1.0) as f32,
        mods: Vec::new(),
    })
}

pub fn lower_poly_allocation(
    allocation: PolyAllocation,
) -> treble::core::graph::sources::PolyphonicAllocationStrategy {
    use treble::core::graph::sources::PolyphonicAllocationStrategy as Strategy;
    match allocation {
        PolyAllocation::ReplaceOldest => Strategy::ReplaceOldest,
        PolyAllocation::ReplaceYoungest => Strategy::ReplaceYoungest,
        PolyAllocation::ReplaceLoudest => Strategy::ReplaceLoudest,
        PolyAllocation::ReplaceQuietest => Strategy::ReplaceQuietest,
        PolyAllocation::ReplaceRandom => Strategy::ReplaceRandom,
        PolyAllocation::Drop => Strategy::Drop,
    }
}

pub fn lower_tone(tone: &ToneDef) -> Result<treble::instruments::spec::ToneSpec, String> {
    use treble::core::generator::prelude::{FrequencyRelation, Waveform as CoreWaveform};
    use treble::instruments::spec as core;

    // `gain` is shorthand for a constant amplitude envelope, which is how the
    // built-in instruments express partial levels.
    let envelope = match (&tone.envelope, tone.gain) {
        (Some(_), Some(_)) => {
            return Err(
                "a tone takes either a 'gain' level or its own envelope, not both".to_string(),
            );
        }
        (Some(envelope), None) => Some(lower_envelope(envelope)),
        (None, Some(level)) => Some(core::EnvelopeSpec::Segment(core::SegmentSpec::Constant {
            value: level as f32,
            duration: None,
        })),
        (None, None) => None,
    };

    Ok(core::ToneSpec {
        waveform: match tone.waveform {
            Waveform::Sine => CoreWaveform::Sine,
            Waveform::Square => CoreWaveform::Square,
            Waveform::Saw => CoreWaveform::Sawtooth,
            Waveform::Triangle => CoreWaveform::Triangle,
            Waveform::SquareRaw => CoreWaveform::SquareRaw,
            Waveform::SawRaw => CoreWaveform::SawRaw,
            Waveform::TriangleRaw => CoreWaveform::TriangleRaw,
            Waveform::Noise => CoreWaveform::WhiteNoise,
            Waveform::PinkNoise => CoreWaveform::PinkNoise,
            Waveform::Blank => CoreWaveform::Blank,
        },
        // A tone with neither a fixed frequency nor a relation follows the
        // played note. Without this it would keep the generator's default
        // frequency forever, because `update_frequency` only acts when a
        // relation is present — a silent drone at 440 Hz.
        frequency_relation: match (tone.relation, tone.frequency) {
            (Some(relation), _) => Some(match relation {
                Relation::Identity => FrequencyRelation::Identity,
                Relation::Harmonic(n) => FrequencyRelation::Harmonic(n.min(255) as u8),
                Relation::Ratio(value) => FrequencyRelation::Ratio(value as f32),
                Relation::Offset(value) => FrequencyRelation::Offset(value as f32),
                Relation::Semitones(value) => FrequencyRelation::Semitones(value),
                Relation::Constant(value) => FrequencyRelation::Constant(value as f32),
            }),
            // An explicit `freq` is a fixed partial, as percussion needs.
            (None, Some(_)) => None,
            (None, None) => Some(FrequencyRelation::Identity),
        },
        frequency: tone.frequency.map(|value| value as f32),
        amplitude_envelope: envelope,
    })
}

pub fn lower_envelope(envelope: &EnvelopeDef) -> treble::instruments::spec::EnvelopeSpec {
    use treble::instruments::spec as core;
    match envelope {
        EnvelopeDef::Adsr {
            attack,
            decay,
            sustain,
            release,
        } => core::EnvelopeSpec::Adsr {
            attack: *attack as f32,
            decay: *decay as f32,
            sustain: *sustain as f32,
            release: *release as f32,
        },
        EnvelopeDef::Single(segment) => core::EnvelopeSpec::Segment(lower_segment(segment)),
        EnvelopeDef::Stages {
            attack,
            decay,
            sustain,
            release,
        } => {
            // A missing stage becomes a zero-length hold rather than an error,
            // so a partial envelope still compiles to something audible.
            let silent = || core::SegmentSpec::Constant {
                value: 0.0,
                duration: None,
            };
            core::EnvelopeSpec::Segments {
                attack: attack.as_ref().map(lower_segment).unwrap_or_else(silent),
                decay: decay.as_ref().map(lower_segment).unwrap_or_else(silent),
                sustain: sustain.as_ref().map(lower_segment),
                release: release.as_ref().map(lower_segment).unwrap_or_else(silent),
            }
        }
    }
}

pub fn lower_segment(segment: &SegmentDef) -> treble::instruments::spec::SegmentSpec {
    use treble::instruments::spec as core;
    match segment {
        SegmentDef::Linear { from, to, duration } => core::SegmentSpec::Linear {
            from: *from as f32,
            to: *to as f32,
            duration: *duration as f32,
        },
        SegmentDef::Bezier {
            from,
            to,
            duration,
            control,
        } => core::SegmentSpec::Bezier {
            from: *from as f32,
            to: *to as f32,
            duration: *duration as f32,
            control: (control.0 as f32, control.1 as f32),
        },
        SegmentDef::Constant { value, duration } => core::SegmentSpec::Constant {
            value: *value as f32,
            duration: duration.map(|value| value as f32),
        },
    }
}

/// A short spelling for one engine filter, with its positional parameter order.
pub struct FilterAlias {
    pub keyword: &'static str,
    pub type_id: &'static str,
    /// Declared parameter names, in the order positional arguments fill them.
    pub params: &'static [&'static str],
}

/// Aliases for the filters worth reaching for mid-performance. Anything else in
/// Aliases for the filters worth reaching for mid-performance. Anything else in
/// the registry is still reachable through `fx <name>`.
pub const FILTER_ALIASES: &[FilterAlias] = &[
    FilterAlias {
        keyword: "trem",
        type_id: "Tremolo",
        params: &["frequency", "depth"],
    },
    FilterAlias {
        keyword: "bpf",
        type_id: "BandPass",
        params: &["low", "high"],
    },
    FilterAlias {
        keyword: "rbpf",
        type_id: "ResonantBandpassFilter",
        params: &["center_frequency", "quality"],
    },
    FilterAlias {
        keyword: "avg",
        type_id: "MovingAverage",
        params: &["size"],
    },
    FilterAlias {
        keyword: "clip",
        type_id: "Clipper",
        params: &["max_ampl"],
    },
    FilterAlias {
        keyword: "comp",
        type_id: "Compressor",
        params: &["threshold", "ratio", "attack", "release"],
    },
    FilterAlias {
        keyword: "limit",
        type_id: "Limiter",
        params: &["threshold", "attack", "release"],
    },
    // The curated transform spellings, so a `def` block's chain and a pattern's
    // read alike. In a pattern line the dedicated transforms match first and
    // keep their own ranges and defaults.
    FilterAlias {
        keyword: "lpf",
        type_id: "LowPassFilter",
        params: &["cutoff_frequency"],
    },
    FilterAlias {
        keyword: "hpf",
        type_id: "HighPassFilter",
        params: &["cutoff_frequency"],
    },
    FilterAlias {
        keyword: "delay",
        type_id: "DelayFilter",
        params: &["delay_for", "feedback", "mix"],
    },
    FilterAlias {
        keyword: "reverb",
        type_id: "ReverbFilter",
        params: &["amount"],
    },
    FilterAlias {
        keyword: "pan",
        type_id: "PanFilter",
        params: &["direction"],
    },
    FilterAlias {
        keyword: "gain",
        type_id: "GainFilter",
        params: &["factor"],
    },
];

pub const RATE_PARAMS: &[(&str, &str)] =
    &[("Tremolo", "frequency"), ("AutoPanFilter", "frequency")];

/// Owned by the engine and injected at build time, so never settable from a
/// pattern line.
pub const ENGINE_OWNED_PARAMS: &[&str] = &["sample_rate"];

/// Whether a parameter belongs to the engine rather than to the performer.
///
/// Shared with the manual's drift check, which must not require a page to
/// document a value nobody can set.
pub fn is_engine_owned(param: &str) -> bool {
    ENGINE_OWNED_PARAMS.contains(&param)
}

pub fn is_rate_param(type_id: &str, param: &str) -> bool {
    RATE_PARAMS
        .iter()
        .any(|(filter, name)| *filter == type_id && *name == param)
}

/// Resolve an `fx` call against the engine's filter registry.
/// Resolve a filter call into a spec plus the travel of any ranged argument.
///
/// The returned [`FxRamp`]s carry a placeholder `chain_index`; the caller knows
/// where the filter lands in the chain and rewrites it.
pub fn resolve_fx_call(
    call: &FxCall,
    cycle_seconds: f64,
    travel: LineTravel,
) -> Result<(FxSpec, Vec<FxRamp>), String> {
    let alias = FILTER_ALIASES
        .iter()
        .find(|alias| alias.keyword == call.filter);
    let requested = alias.map_or(call.filter.as_str(), |alias| alias.type_id);

    let info = treble::meta::get_filters()
        .into_iter()
        .find(|info| {
            info.type_id.eq_ignore_ascii_case(requested)
                || info.name.eq_ignore_ascii_case(requested)
        })
        .ok_or_else(|| {
            format!(
                "unknown filter '{}'. Available: {}",
                call.filter,
                available_filters().join(", ")
            )
        })?;

    // Declared parameters, minus the ones the engine owns, in declaration order.
    let declared: Vec<(String, treble::meta::Parameter<&'static str>)> = info
        .inputs
        .iter()
        .filter_map(|input| input.parameter.clone())
        .map(|parameter| (param_name(&parameter).to_string(), parameter))
        .filter(|(name, _)| !is_engine_owned(name))
        .collect();

    // An alias fixes its own positional order; otherwise declaration order wins.
    let positional: Vec<String> = match alias {
        Some(alias) => alias.params.iter().map(|name| name.to_string()).collect(),
        None => declared.iter().map(|(name, _)| name.clone()).collect(),
    };

    let mut params: HashMap<String, f32> = HashMap::new();
    let mut ramps: Vec<FxRamp> = Vec::new();
    let mut next_positional = 0usize;
    for arg in call.args.iter() {
        let (name, value) = match arg {
            FxArg::Positional(value) => {
                let name = positional.get(next_positional).ok_or_else(|| {
                    format!(
                        "{} takes at most {} positional argument(s): {}",
                        call.filter,
                        positional.len(),
                        positional.join(", ")
                    )
                })?;
                next_positional += 1;
                (name.clone(), value)
            }
            FxArg::Named(name, value) => {
                if is_engine_owned(name) {
                    return Err(format!(
                        "{}: '{name}' is set by the engine and cannot be given here",
                        call.filter
                    ));
                }
                (name.clone(), value)
            }
        };

        let Some((_, parameter)) = declared.iter().find(|(declared, _)| *declared == name) else {
            return Err(format!(
                "{} has no parameter '{name}'. Available: {}",
                call.filter,
                declared
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        };
        // Every value the argument passes through has to be legal for the
        // filter, not only the one the graph is built with — a sweep that runs
        // out of range would be refused mid-performance instead of on save.
        let mut checked = Vec::with_capacity(2);
        for end in value.ramp().values() {
            let resolved =
                resolve_fx_value(&call.filter, &name, info.type_id, value, end, cycle_seconds)?;
            checked.push(check_parameter(&call.filter, &name, parameter, resolved)?);
        }
        let held = resolve_fx_value(
            &call.filter,
            &name,
            info.type_id,
            value,
            travel_f64(value.ramp(), travel).start(),
            cycle_seconds,
        )?;
        params.insert(
            name.clone(),
            check_parameter(&call.filter, &name, parameter, held)?,
        );
        // A rate argument is inverted on the way in, so the automation has to
        // travel between the *converted* endpoints, not the written ones.
        let sweeps = match value.ramp() {
            Ramp::Sweep { .. } => true,
            Ramp::Timed { ramp, .. } => matches!(ramp.as_ref(), Ramp::Sweep { .. }),
            _ => false,
        };
        if sweeps {
            ramps.push(FxRamp {
                chain_index: 0,
                param: name,
                from: checked[0],
                to: checked[1],
                window: value.ramp().own_window(),
            });
        }
    }

    Ok((
        FxSpec {
            type_id: info.type_id.to_string(),
            params,
        },
        ramps,
    ))
}

/// A bare number on an LFO rate is a period in cycles; everywhere else it is a
/// literal parameter value. An `hz` suffix is always absolute.
pub fn resolve_fx_value(
    filter: &str,
    param: &str,
    type_id: &str,
    value: &FxValue,
    number: f64,
    cycle_seconds: f64,
) -> Result<f64, String> {
    match value {
        FxValue::Hertz(_) => {
            if is_rate_param(type_id, param) {
                Ok(number)
            } else {
                Err(format!(
                    "{filter}: '{param}' is not a rate, so it cannot take an 'hz' value"
                ))
            }
        }
        FxValue::Plain(_) => {
            if !is_rate_param(type_id, param) {
                return Ok(number);
            }
            if !number.is_finite() || number <= 0.0 {
                return Err(format!(
                    "{filter}: '{param}' of {number} must be a positive number of cycles"
                ));
            }
            Ok(1.0 / (number * cycle_seconds.max(f64::EPSILON)))
        }
    }
}

/// Validate against the range the filter itself declares, so the language does
/// not keep a second copy of every limit.
pub fn check_parameter(
    filter: &str,
    param: &str,
    parameter: &treble::meta::Parameter<&'static str>,
    value: f64,
) -> Result<f32, String> {
    use treble::meta::Parameter;
    if !value.is_finite() {
        return Err(format!("{filter}: '{param}' must be a finite number"));
    }
    let outside = |minimum: f64, maximum: f64| {
        format!("{filter}: '{param}' value {value} is outside {minimum}-{maximum}")
    };
    match parameter {
        Parameter::Range { min, max, .. } => {
            let (min, max) = (*min as f64, *max as f64);
            if !(min..=max).contains(&value) {
                return Err(outside(min, max));
            }
        }
        Parameter::Int { min, max, .. } => {
            let minimum = min.map(f64::from).unwrap_or(f64::MIN);
            let maximum = max.map(f64::from).unwrap_or(f64::MAX);
            if !(minimum..=maximum).contains(&value) {
                return Err(outside(minimum, maximum));
            }
        }
        // Float and Toggle declare no bounds; a finite value is all we can check.
        Parameter::Float { .. } | Parameter::Toggle { .. } | Parameter::List { .. } => {}
    }
    Ok(value as f32)
}

pub fn param_name<'a>(parameter: &'a treble::meta::Parameter<&'static str>) -> &'a str {
    use treble::meta::Parameter;
    match parameter {
        Parameter::Toggle { field_name, .. }
        | Parameter::Range { field_name, .. }
        | Parameter::Float { field_name, .. }
        | Parameter::Int { field_name, .. }
        | Parameter::List { field_name, .. } => field_name,
    }
}

/// Every filter a pattern line can name, aliases first.
pub fn available_filters() -> Vec<String> {
    let mut names: Vec<String> = FILTER_ALIASES
        .iter()
        .map(|alias| format!("{} ({})", alias.keyword, alias.type_id))
        .collect();
    let mut registered: Vec<String> = treble::meta::get_filters()
        .into_iter()
        .filter(|info| {
            !FILTER_ALIASES
                .iter()
                .any(|alias| alias.type_id == info.type_id)
        })
        .map(|info| info.type_id.to_string())
        .collect();
    registered.sort();
    names.extend(registered);
    names
}

/// A sweep's rate in hertz. A cycle-relative rate is a *period*, so it is
/// inverted against the cycle length; a hertz rate passes straight through.
pub fn sweep_hertz(sweep: &PanSweep, cycle_seconds: f64) -> Result<f64, String> {
    match sweep.rate {
        LfoRate::Hertz(hertz) => Ok(hertz),
        LfoRate::Cycles(cycles) => {
            if !cycles.is_finite() || cycles <= 0.0 {
                return Err(format!(
                    "pan rate {cycles} must be a positive number of cycles"
                ));
            }
            Ok(1.0 / (cycles * cycle_seconds.max(f64::EPSILON)))
        }
    }
}

pub fn wave_ordinal(wave: LfoWave) -> u8 {
    match wave {
        LfoWave::Sine => 0,
        LfoWave::Triangle => 1,
        LfoWave::Square => 2,
        LfoWave::Saw => 3,
        LfoWave::Random => 4,
    }
}

pub fn wave_name(wave: LfoWave) -> &'static str {
    match wave {
        LfoWave::Sine => "sine",
        LfoWave::Triangle => "tri",
        LfoWave::Square => "sq",
        LfoWave::Saw => "saw",
        LfoWave::Random => "rand",
    }
}

#[derive(Clone)]
pub struct Slot {
    pub weight: f64,
    pub event: Event,
    /// `Some(p)` when a `?` applies, carrying its drop probability.
    pub drop: Option<f64>,
    pub source_step: usize,
    /// The written velocity for this slot — the innermost `:v` or `X` that
    /// covers it. `None` takes the line's `| vel`.
    pub velocity: Option<f32>,
}

pub fn expand_sequence(
    notation: &MiniNotation,
    sequence: &Sequence,
    weight: f64,
    scale: Option<(PitchRoot, ScaleMode)>,
    out: &mut Vec<Slot>,
    cycle_factor: &mut f64,
    parent_step: Option<usize>,
) {
    let total: f64 = sequence.steps.iter().map(step_weight).sum::<f64>().max(1.0);
    for (step_index, step) in sequence.steps.iter().enumerate() {
        let step_weight = weight * step_weight(step) / total;
        expand_step(
            notation,
            step,
            step_weight,
            scale,
            out,
            cycle_factor,
            parent_step.unwrap_or(step_index),
        );
    }
}

pub fn expand_step(
    notation: &MiniNotation,
    step: &Step,
    weight: f64,
    scale: Option<(PitchRoot, ScaleMode)>,
    out: &mut Vec<Slot>,
    cycle_factor: &mut f64,
    source_step: usize,
) {
    // `/N` is pattern-wide rather than per-step; see the manual's
    // implementation notes.
    for modifier in step.modifiers.iter() {
        if let Modifier::Slow(amount) = modifier {
            *cycle_factor *= (*amount).max(1) as f64;
        }
    }

    // A tie extends whatever sounded last instead of occupying its own slot. A
    // group is self-contained, so a leading `_` inside one has nothing to hold
    // and stays a rest.
    if matches!(step.atom, Atom::Hold) {
        match out.last_mut() {
            Some(previous) => previous.weight += weight,
            None => out.push(Slot {
                weight,
                event: Event::Rest,
                drop: None,
                source_step,
                velocity: None,
            }),
        }
        return;
    }

    let mut slots = base_slots(notation, step, weight, scale, cycle_factor, source_step);

    // Slot-generating modifiers apply left-to-right in written order; `?` then
    // applies to whatever they produced, so `x*8?` is eight chances and not one.
    for modifier in step.modifiers.iter() {
        match modifier {
            Modifier::Repeat(count) => slots = repeat_slots(slots, count.start(), true),
            Modifier::Replicate(count) => slots = repeat_slots(slots, *count, false),
            Modifier::Euclidean(onsets, positions, offset) => {
                slots = euclidean_slots(
                    slots,
                    onsets.start(),
                    positions.start(),
                    offset.unwrap_or(0),
                );
            }
            Modifier::Drop(probability) => {
                let probability = probability
                    .as_ref()
                    .map_or(DEFAULT_DROP, |ramp| ramp.start());
                for slot in slots.iter_mut() {
                    slot.drop = Some(probability);
                }
            }
            // Folded into the pattern above, or read by `step_weight`.
            Modifier::Slow(_) | Modifier::Weight(_) => {}
        }
    }
    // A velocity is a property of the step, not of the slots a modifier
    // produced, so it is applied after the slot-generating modifiers have run
    // and reaches every slot they made — `X*4` is four accents. An inner step
    // that named its own keeps it: `[c4 e4:0.4]:0.9` is a loud C and a quiet E.
    if let Some(written) = step.velocity.as_ref().map(|ramp| ramp.start() as f32) {
        let written = written.clamp(0.0, 1.0);
        for slot in slots.iter_mut() {
            slot.velocity = slot.velocity.or(Some(written));
        }
    }
    out.extend(slots);
}

/// The slots an atom produces before any stacking modifier is applied.
pub fn base_slots(
    notation: &MiniNotation,
    step: &Step,
    weight: f64,
    scale: Option<(PitchRoot, ScaleMode)>,
    cycle_factor: &mut f64,
    source_step: usize,
) -> Vec<Slot> {
    let mut slots = Vec::new();
    let event = match &step.atom {
        Atom::Group(group) => match group.mode {
            GroupMode::Subdivide => {
                if let Some(layer) = group.layers.first() {
                    expand_sequence(
                        notation,
                        layer,
                        weight,
                        scale,
                        &mut slots,
                        cycle_factor,
                        Some(source_step),
                    );
                }
                return slots;
            }
            GroupMode::Chord => {
                let notes = group
                    .layers
                    .iter()
                    .flat_map(|layer| layer.steps.iter())
                    .flat_map(|step| event_notes(atom_event(&step.atom, scale)))
                    .collect();
                Event::Notes(notes)
            }
            GroupMode::Random => compile_choices(notation, &group.layers, scale, source_step),
        },
        Atom::Alternation(alternation) => {
            compile_alternatives(notation, &alternation.sequence, scale, source_step)
        }
        other => atom_event(other, scale),
    };
    slots.push(Slot {
        weight,
        event,
        drop: None,
        source_step,
        velocity: None,
    });
    slots
}

/// `*N` subdivides the slot (total duration unchanged); `!N` replicates it into
/// N sibling steps (total duration multiplied).
pub fn repeat_slots(slots: Vec<Slot>, count: u32, subdivide: bool) -> Vec<Slot> {
    let count = count.max(1);
    let divisor = if subdivide { count as f64 } else { 1.0 };
    let mut repeated = Vec::with_capacity(slots.len() * count as usize);
    for _ in 0..count {
        repeated.extend(slots.iter().cloned().map(|mut slot| {
            slot.weight /= divisor;
            slot
        }));
    }
    repeated
}

/// Distribute the base slots over `steps` positions according to the Euclidean
/// onset counts, resting where a position gets none. A position that receives
/// more than one onset subdivides, so a single figure can mix note values.
/// The total duration is preserved either way.
pub fn euclidean_slots(slots: Vec<Slot>, beats: u32, steps: u32, offset: u32) -> Vec<Slot> {
    let onsets = euclidean(beats, steps, offset);
    if onsets.is_empty() {
        return slots;
    }
    let positions = onsets.len() as f64;
    let source_step = slots.first().map(|slot| slot.source_step).unwrap_or(0);
    let position_weight = slots.iter().map(|slot| slot.weight).sum::<f64>() / positions;
    let mut expanded = Vec::with_capacity(onsets.len() * slots.len());
    for count in onsets {
        if count == 0 {
            expanded.push(Slot {
                weight: position_weight,
                event: Event::Rest,
                drop: None,
                source_step,
                velocity: None,
            });
            continue;
        }
        // Each onset is a full copy of the payload, compressed to its share of
        // the position.
        let divisor = positions * count as f64;
        for _ in 0..count {
            expanded.extend(slots.iter().cloned().map(|mut slot| {
                slot.weight /= divisor;
                slot
            }));
        }
    }
    expanded
}

pub fn normalize(slots: Vec<Slot>) -> Vec<Segment> {
    let total = slots
        .iter()
        .map(|slot| slot.weight)
        .sum::<f64>()
        .max(f64::EPSILON);
    let mut cursor = 0.0;
    slots
        .into_iter()
        .map(|slot| {
            let start = cursor / total;
            cursor += slot.weight;
            Segment {
                start,
                end: cursor / total,
                event: slot.event,
                drop: slot.drop,
                source_step: slot.source_step,
                velocity: slot.velocity,
            }
        })
        .collect()
}

/// `[a|b|c]` — compile each layer into a timed sub-sequence. Unlike an
/// alternation, the option is chosen by hash rather than by cycle order.
pub fn compile_choices(
    notation: &MiniNotation,
    layers: &[Sequence],
    scale: Option<(PitchRoot, ScaleMode)>,
    source_step: usize,
) -> Event {
    let options = layers
        .iter()
        .map(|layer| {
            let mut slots = Vec::new();
            let mut cycle_factor = 1.0;
            expand_sequence(
                notation,
                layer,
                1.0,
                scale,
                &mut slots,
                &mut cycle_factor,
                Some(source_step),
            );
            timed_events(normalize(slots), cycle_factor)
        })
        .collect();
    Event::Choice(options)
}

pub fn timed_events(segments: Vec<Segment>, cycle_factor: f64) -> Vec<TimedEvent> {
    segments
        .into_iter()
        .map(|segment| TimedEvent {
            start: segment.start * cycle_factor,
            end: segment.end * cycle_factor,
            event: segment.event,
            drop: segment.drop,
            velocity: segment.velocity,
        })
        .collect()
}

/// Map a solo's degree range onto the active scale once, at compile time — the
/// same moment plain degrees are resolved — so the walk itself only ever deals
/// in table indices.
pub fn compile_solo(solo: &Solo, scale: Option<(PitchRoot, ScaleMode)>) -> Event {
    let (root, mode) = scale.unwrap_or((default_root(), ScaleMode::Major));
    Event::Solo {
        notes: (solo.low..=solo.high)
            .map(|degree| degree_to_midi(degree, root, mode))
            .collect(),
        steps: solo.steps.start().max(1),
    }
}

pub fn compile_alternatives(
    notation: &MiniNotation,
    sequence: &Sequence,
    scale: Option<(PitchRoot, ScaleMode)>,
    source_step: usize,
) -> Event {
    let options = sequence
        .steps
        .iter()
        .map(|step| {
            let mut slots = Vec::new();
            let mut cycle_factor = 1.0;
            expand_step(
                notation,
                step,
                1.0,
                scale,
                &mut slots,
                &mut cycle_factor,
                source_step,
            );
            timed_events(normalize(slots), cycle_factor)
        })
        .collect();
    Event::Alternatives(options)
}

pub fn apply_transform(
    segments: &mut Vec<Segment>,
    cycle_factor: &mut f64,
    velocity: &mut f32,
    transform: &Transform,
) {
    match transform {
        Transform::Rev => {
            for segment in segments.iter_mut() {
                (segment.start, segment.end) = (1.0 - segment.end, 1.0 - segment.start);
            }
            segments.reverse();
        }
        // Ramped values arrive already resolved for this cycle, so only the
        // `from` end is ever read here.
        Transform::Fast(amount) => *cycle_factor /= amount.start().max(f64::EPSILON),
        Transform::Slow(amount) => *cycle_factor *= amount.start().max(f64::EPSILON),
        Transform::Oct(octaves) => {
            let shift = octaves.start() * 12;
            map_notes(segments, move |note| {
                (note as i32 + shift).clamp(0, 127) as u8
            })
        }
        Transform::Vel(velocity_value) => {
            *velocity = (velocity_value.start() as f32).clamp(0.0, 1.0)
        }
        Transform::Scale(root, mode) => map_notes(segments, |note| quantise(note, *root, *mode)),
        Transform::Arp(mode) => arpeggiate(segments, *mode),
        Transform::Every(_, _) => {}
        // The span is read when the pattern is compiled, not per transform.
        Transform::RampSpan { .. } => {}
        // Audio transforms are compiled into the pattern's DSP chain instead.
        Transform::Gain(_)
        | Transform::Pan(_)
        | Transform::AutoPan(_)
        | Transform::Lpf(_)
        | Transform::Hpf(_)
        | Transform::Delay(_, _, _)
        | Transform::Reverb(_)
        | Transform::Fx(_) => {}
    }
}

/// `every N <audio fx>` parses but cannot take effect, because changing the DSP
/// chain needs a graph rebuild rather than a per-cycle decision. Callers turn
/// this into a diagnostic. `every N vel …` is unaffected — velocity is an event
/// property, not a filter.
pub fn conditional_audio_transform(transform: &Transform) -> bool {
    matches!(
        transform,
        Transform::Every(
            _,
            inner
        ) if matches!(
            inner.as_ref(),
            Transform::Gain(_)
                | Transform::Pan(_)
                | Transform::AutoPan(_)
                | Transform::Lpf(_)
                | Transform::Hpf(_)
                | Transform::Delay(_, _, _)
                | Transform::Reverb(_)
                | Transform::Fx(_)
        )
    )
}

pub fn map_notes(segments: &mut [Segment], map: impl Fn(u8) -> u8 + Copy) {
    for segment in segments {
        map_event_notes(&mut segment.event, map);
    }
}

pub fn map_event_notes(event: &mut Event, map: impl Fn(u8) -> u8 + Copy) {
    match event {
        Event::Notes(notes) => notes.iter_mut().for_each(|note| *note = map(*note)),
        Event::Alternatives(options) | Event::Choice(options) => {
            for event in options.iter_mut().flatten() {
                map_event_notes(&mut event.event, map);
            }
        }
        Event::Solo { notes, .. } => notes.iter_mut().for_each(|note| *note = map(*note)),
        Event::Rest | Event::Trigger => {}
    }
}

pub fn arpeggiate(segments: &mut Vec<Segment>, mode: ArpMode) {
    let mut expanded = Vec::new();
    for segment in segments.drain(..) {
        let Event::Notes(mut notes) = segment.event else {
            expanded.push(segment);
            continue;
        };
        if notes.len() < 2 {
            expanded.push(Segment {
                event: Event::Notes(notes),
                ..segment
            });
            continue;
        }
        notes.sort_unstable();
        match mode {
            ArpMode::Down => notes.reverse(),
            ArpMode::UpDown => {
                let tail: Vec<_> = notes
                    .iter()
                    .rev()
                    .skip(1)
                    .take(notes.len() - 2)
                    .copied()
                    .collect();
                notes.extend(tail);
            }
            ArpMode::Random => {
                let len = notes.len();
                notes.rotate_left((segment.start * 1000.0) as usize % len);
            }
            ArpMode::Up => {}
        }
        let duration = (segment.end - segment.start) / notes.len() as f64;
        for (index, note) in notes.into_iter().enumerate() {
            expanded.push(Segment {
                start: segment.start + duration * index as f64,
                end: segment.start + duration * (index + 1) as f64,
                event: Event::Notes(vec![note]),
                drop: segment.drop,
                source_step: segment.source_step,
                velocity: segment.velocity,
            });
        }
    }
    *segments = expanded;
}

pub fn atom_event(atom: &Atom, scale: Option<(PitchRoot, ScaleMode)>) -> Event {
    match atom {
        Atom::Trigger => Event::Trigger,
        Atom::Rest | Atom::Hold | Atom::Group(_) | Atom::Alternation(_) => Event::Rest,
        Atom::Note(note) => Event::Notes(vec![lang_note_to_midi(note)]),
        Atom::Degree(degree) => {
            let (root, mode) = scale.unwrap_or((default_root(), ScaleMode::Major));
            Event::Notes(vec![degree_to_midi(*degree, root, mode)])
        }
        Atom::Solo(solo) => compile_solo(solo, scale),
    }
}

pub fn event_notes(event: Event) -> Vec<u8> {
    match event {
        Event::Notes(notes) => notes,
        _ => Vec::new(),
    }
}

pub fn step_weight(step: &Step) -> f64 {
    step.modifiers
        .iter()
        .find_map(|modifier| match modifier {
            Modifier::Weight(weight) => Some((*weight).max(1) as f64),
            _ => None,
        })
        .unwrap_or(1.0)
}

/// How many onsets each of `steps` positions receives, rotated by `offset`.
///
/// `beats` is a count of onsets, not a count of sounding positions, so it may
/// exceed `steps`. Every position gets `beats / steps` onsets, and Bjorklund
/// spreads the `beats % steps` remainder over the positions that get one more.
/// For `beats <= steps` the quotient is zero and this reduces to the classic
/// one-or-nothing Euclidean rhythm.
pub fn euclidean(beats: u32, steps: u32, offset: u32) -> Vec<u32> {
    if steps == 0 {
        return Vec::new();
    }
    let base = beats / steps;
    let extra = bjorklund(beats % steps, steps);
    let mut pattern: Vec<u32> = extra
        .into_iter()
        .map(|carries| base + u32::from(carries))
        .collect();
    let len = pattern.len();
    pattern.rotate_left(offset as usize % len);
    pattern
}

/// Bjorklund's algorithm — the canonical even spread of `beats` across `steps`.
///
/// Repeatedly pairs the hit groups with the rest groups until at most one rest
/// group is left, which spreads the hits as evenly as the step count allows.
pub fn bjorklund(beats: u32, steps: u32) -> Vec<bool> {
    let beats = beats.min(steps);
    if beats == 0 {
        return vec![false; steps as usize];
    }
    if beats == steps {
        return vec![true; steps as usize];
    }
    let mut hits: Vec<Vec<bool>> = vec![vec![true]; beats as usize];
    let mut rests: Vec<Vec<bool>> = vec![vec![false]; (steps - beats) as usize];
    while rests.len() > 1 {
        let pairs = hits.len().min(rests.len());
        let mut paired = Vec::with_capacity(pairs);
        for index in 0..pairs {
            let mut group = hits[index].clone();
            group.extend_from_slice(&rests[index]);
            paired.push(group);
        }
        rests = if hits.len() > pairs {
            hits[pairs..].to_vec()
        } else {
            rests[pairs..].to_vec()
        };
        hits = paired;
    }
    hits.into_iter().flatten().chain(rests.concat()).collect()
}

pub fn resolve_events(
    event: &Event,
    cycle: u64,
    pattern_name: &str,
    seed: u64,
) -> Vec<ResolvedEvent> {
    resolve_events_at(event, cycle, cycle, pattern_name, seed)
}

/// [`resolve_events`], with the alternation clock stated separately — see
/// [`cycle_strikes_at`] for why the two differ inside a piece.
pub fn resolve_events_at(
    event: &Event,
    cycle: u64,
    alternation_cycle: u64,
    pattern_name: &str,
    seed: u64,
) -> Vec<ResolvedEvent> {
    match event {
        Event::Trigger => vec![ResolvedEvent {
            start: 0.0,
            end: 1.0,
            notes: vec![60],
            velocity: None,
        }],
        Event::Notes(notes) => vec![ResolvedEvent {
            start: 0.0,
            end: 1.0,
            notes: notes.clone(),
            velocity: None,
        }],
        Event::Alternatives(options) if !options.is_empty() => {
            let option = &options[alternation_cycle as usize % options.len()];
            resolve_option_at(option, cycle, alternation_cycle, pattern_name, seed)
        }
        Event::Choice(options) if !options.is_empty() => {
            let option = &options[choice_index(cycle, pattern_name, seed, options.len())];
            resolve_option_at(option, cycle, alternation_cycle, pattern_name, seed)
        }
        Event::Solo { notes, steps } if !notes.is_empty() => {
            solo_events(notes, *steps, cycle, pattern_name, seed)
        }
        Event::Rest | Event::Alternatives(_) | Event::Choice(_) | Event::Solo { .. } => Vec::new(),
    }
}

/// One cycle of a generated solo: a weighted random walk over the note table.
///
/// Deterministic in (pattern, cycle, seed) — the phrase evolves every cycle and
/// replays identically from the same buffer. Three musical biases keep it from
/// sounding like dice: movement is favoured over repetition, the walk is pushed
/// back when it nears the range edges, and the final note cadences toward the
/// middle of the range.
pub fn solo_events(
    notes: &[u8],
    steps: u32,
    cycle: u64,
    pattern_name: &str,
    seed: u64,
) -> Vec<ResolvedEvent> {
    let len = notes.len() as i64;
    let steps = steps.max(1) as usize;
    let roll = |tag: &str, index: usize| -> f64 {
        let mut hasher = DefaultHasher::new();
        ("solo", tag, cycle, pattern_name, seed, index).hash(&mut hasher);
        unit_from_hash(hasher.finish())
    };

    // Start somewhere in the middle third, so there is room to move both ways.
    let third = (len / 3).max(1);
    let mut index = third + (roll("start", 0) * third as f64) as i64;

    let mut events = Vec::with_capacity(steps);
    for i in 0..steps {
        events.push(ResolvedEvent {
            start: i as f64 / steps as f64,
            end: (i + 1) as f64 / steps as f64,
            notes: vec![notes[index.clamp(0, len - 1) as usize]],
            velocity: None,
        });
        // Movement-biased deltas: mostly step by one, sometimes leap, rarely
        // repeat. Cumulative weights over [-2, -1, 0, +1, +2].
        let r = roll("step", i);
        let mut delta: i64 = if r < 0.12 {
            -2
        } else if r < 0.42 {
            -1
        } else if r < 0.58 {
            0
        } else if r < 0.88 {
            1
        } else {
            2
        };
        // Edge gravity: near a boundary, reflect motion back into the range.
        if index + delta >= len - 1 && delta > 0 {
            delta = -delta;
        }
        if index + delta <= 0 && delta < 0 {
            delta = -delta;
        }
        // Cadence: aim the last note toward the middle of the range.
        if i + 2 == steps {
            let centre = len / 2;
            delta = (centre - index).clamp(-2, 2);
        }
        index = (index + delta).clamp(0, len - 1);
    }
    events
}

pub fn resolve_option(
    option: &[TimedEvent],
    cycle: u64,
    pattern_name: &str,
    seed: u64,
) -> Vec<ResolvedEvent> {
    resolve_option_at(option, cycle, cycle, pattern_name, seed)
}

/// [`resolve_option`], carrying the alternation clock down to nested `<a b c>`.
pub fn resolve_option_at(
    option: &[TimedEvent],
    cycle: u64,
    alternation_cycle: u64,
    pattern_name: &str,
    seed: u64,
) -> Vec<ResolvedEvent> {
    let mut resolved = Vec::new();
    for (index, timed) in option.iter().enumerate() {
        if timed
            .drop
            .is_some_and(|probability| should_drop(cycle, pattern_name, index, probability))
        {
            continue;
        }
        let duration = timed.end - timed.start;
        for nested in resolve_events_at(
            &timed.event,
            cycle,
            alternation_cycle,
            pattern_name,
            mix_seed(seed, index),
        ) {
            resolved.push(ResolvedEvent {
                start: timed.start + nested.start * duration,
                end: timed.start + nested.end * duration,
                notes: nested.notes,
                // An inner step's own velocity wins; otherwise the enclosing
                // step's carries down into everything it sounds.
                velocity: nested.velocity.or(timed.velocity),
            });
        }
    }
    resolved
}

/// Deterministic drop decision: the same performance replayed from the same
/// buffer always drops the same hits.
pub fn should_drop(cycle: u64, name: &str, segment: usize, probability: f64) -> bool {
    if probability <= 0.0 {
        return false;
    }
    if probability >= 1.0 {
        return true;
    }
    let mut hasher = DefaultHasher::new();
    (cycle, name, segment).hash(&mut hasher);
    unit_from_hash(hasher.finish()) < probability
}

/// Pick one `[a|b|c]` option for this cycle, deterministically.
pub fn choice_index(cycle: u64, name: &str, seed: u64, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let mut hasher = DefaultHasher::new();
    ("choice", cycle, name, seed).hash(&mut hasher);
    (hasher.finish() % len as u64) as usize
}

/// Map a hash onto `[0, 1)` using the 53 bits an f64 can hold exactly.
pub fn unit_from_hash(hash: u64) -> f64 {
    (hash >> 11) as f64 / (1u64 << 53) as f64
}

/// Mix a child index into a seed so nested choices stay independent.
pub fn mix_seed(seed: u64, index: usize) -> u64 {
    seed.wrapping_mul(0x100_0000_01b3)
        .wrapping_add(index as u64 + 1)
}

pub fn lang_note_to_midi(note: &crate::ast::Note) -> u8 {
    let root = PitchRoot {
        name: note.letter,
        accidental: note.accidental,
    };
    ((note.octave as i32 + 1) * 12 + root_semitone(root)).clamp(0, 127) as u8
}

pub fn degree_to_midi(degree: i32, root: PitchRoot, mode: ScaleMode) -> u8 {
    let intervals = mode_intervals(mode);
    let len = intervals.len() as i32;
    let octave = degree.div_euclid(len);
    let interval = intervals[degree.rem_euclid(len) as usize];
    (60 + root_semitone(root) + interval + octave * 12).clamp(0, 127) as u8
}

pub fn quantise(midi: u8, root: PitchRoot, mode: ScaleMode) -> u8 {
    let intervals = mode_intervals(mode);
    let root = root_semitone(root).rem_euclid(12);
    let mut best = midi;
    let mut distance = i32::MAX;
    for candidate in 0..=127 {
        if intervals.contains(&((candidate - root).rem_euclid(12))) {
            let candidate_distance = (candidate - midi as i32).abs();
            if candidate_distance < distance {
                distance = candidate_distance;
                best = candidate as u8;
            }
        }
    }
    best
}

pub fn default_root() -> PitchRoot {
    PitchRoot {
        name: NoteLetter::C,
        accidental: Accidental::Natural,
    }
}

pub fn root_semitone(root: PitchRoot) -> i32 {
    let natural = match root.name {
        NoteLetter::C => 0,
        NoteLetter::D => 2,
        NoteLetter::E => 4,
        NoteLetter::F => 5,
        NoteLetter::G => 7,
        NoteLetter::A => 9,
        NoteLetter::B => 11,
    };
    natural
        + match root.accidental {
            Accidental::Natural => 0,
            Accidental::Sharp => 1,
            Accidental::DoubleSharp => 2,
            Accidental::Flat => -1,
            Accidental::DoubleFlat => -2,
        }
}

pub fn mode_intervals(mode: ScaleMode) -> &'static [i32] {
    match mode {
        ScaleMode::Major => &[0, 2, 4, 5, 7, 9, 11],
        ScaleMode::Minor | ScaleMode::Aeolian => &[0, 2, 3, 5, 7, 8, 10],
        ScaleMode::Dorian => &[0, 2, 3, 5, 7, 9, 10],
        ScaleMode::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
        ScaleMode::Lydian => &[0, 2, 4, 6, 7, 9, 11],
        ScaleMode::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
        ScaleMode::Locrian => &[0, 1, 3, 5, 6, 8, 10],
        ScaleMode::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        ScaleMode::Pentatonic => &[0, 2, 4, 7, 9],
        ScaleMode::Blues => &[0, 3, 5, 6, 7, 10],
    }
}

/// Sweep depth used by `pan <wave> <rate>` when no depth is given.
pub const DEFAULT_SWEEP_DEPTH: f64 = 1.0;

/// The pattern's audio transforms, in source order, as the performer wrote
/// them. This is what the mixer strip displays instead of offering sliders.
/// A ramp as the performer wrote it — `4`, `4..16` or `2>4>8>16`.
///
/// The mixer strip echoes the source rather than a derived value, so a range has
/// to render as a range and not as whichever end it happens to hold now.
pub fn render_ramp(ramp: &Ramp<f64>) -> String {
    match ramp {
        Ramp::Fixed(value) => value.to_string(),
        Ramp::Sweep { from, to } => format!("{from}..{to}"),
        Ramp::Steps { first, rest } => {
            let mut text = first.to_string();
            for value in rest {
                text.push('>');
                text.push_str(&value.to_string());
            }
            text
        }
        Ramp::Timed {
            ramp,
            span_divisions,
            curve,
        } => format!(
            "r({},{span_divisions},{})",
            render_ramp(ramp),
            curve_name(*curve)
        ),
    }
}

pub fn describe_audio_chain(transforms: &[Transform]) -> Vec<String> {
    transforms
        .iter()
        .filter_map(|transform| match transform {
            Transform::Gain(factor) => Some(format!("gain {}", render_ramp(factor))),
            Transform::Pan(direction) => Some(format!("pan {}", render_ramp(direction))),
            Transform::AutoPan(sweep) => Some(match (sweep.rate, sweep.depth) {
                (LfoRate::Cycles(cycles), None) => {
                    format!("pan {} {cycles}", wave_name(sweep.wave))
                }
                (LfoRate::Cycles(cycles), Some(depth)) => {
                    format!("pan {} {cycles} {depth}", wave_name(sweep.wave))
                }
                (LfoRate::Hertz(hertz), None) => {
                    format!("pan {} {hertz}hz", wave_name(sweep.wave))
                }
                (LfoRate::Hertz(hertz), Some(depth)) => {
                    format!("pan {} {hertz}hz {depth}", wave_name(sweep.wave))
                }
            }),
            Transform::Lpf(cutoff) => Some(format!("lpf {}", render_ramp(cutoff))),
            Transform::Hpf(cutoff) => Some(format!("hpf {}", render_ramp(cutoff))),
            Transform::Delay(time, feedback, mix) => Some(format!(
                "delay {} {} {}",
                render_ramp(time),
                render_ramp(feedback),
                render_ramp(&mix.clone().unwrap_or(Ramp::Fixed(DEFAULT_DELAY_MIX)))
            )),
            Transform::Reverb(amount) => Some(format!("reverb {}", render_ramp(amount))),
            Transform::Fx(call) => Some(describe_fx_call(call)),
            _ => None,
        })
        .collect()
}

pub fn describe_fx_call(call: &FxCall) -> String {
    let mut text = call.filter.clone();
    for arg in call.args.iter() {
        let render = |value: &FxValue| match value {
            FxValue::Plain(number) => render_ramp(number),
            FxValue::Hertz(hertz) => format!("{}hz", render_ramp(hertz)),
        };
        match arg {
            FxArg::Positional(value) => text.push_str(&format!(" {}", render(value))),
            FxArg::Named(name, value) => text.push_str(&format!(" {name}={}", render(value))),
        }
    }
    text
}

pub fn curve_name(curve: RampCurve) -> String {
    match curve {
        RampCurve::Linear => "lin".to_string(),
        RampCurve::Exponential => "exp".to_string(),
        RampCurve::Oscillate => "osc".to_string(),
    }
}
