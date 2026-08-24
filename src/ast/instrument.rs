//! Instrument definition AST — the contents of a `def` block.
//!
//! These types mirror the shape of an instrument without naming the engine's
//! own types: this crate has no dependency on the audio engine, so the consumer
//! lowers a definition onto whatever it builds instruments from. Every field is
//! optional so a definition can be terse; the consumer supplies the defaults
//! documented in `LANGUAGE.md` §6.

use super::program::FxCall;

/// A `def <name> { ... }` block.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InstrumentDef {
    pub name: String,
    pub voice: Option<VoiceDef>,
    pub lifecycle: Option<Lifecycle>,
    /// Oscillators, in mix order.
    pub tones: Vec<ToneDef>,
    pub mix: Option<MixMode>,
    /// The amplitude envelope (`env`).
    pub amplitude: Option<EnvelopeDef>,
    /// The optional pitch envelope (`pitchenv`).
    pub pitch: Option<EnvelopeDef>,
    pub sample: Option<SampleDef>,
    /// Ordered effect chain; the same call shape as a pattern's `fx` transform.
    pub fx: Vec<FxCall>,
    pub gain: Option<f64>,
    pub velocity_sensitivity: Option<f64>,
    pub base_frequency: Option<f64>,
}

/// `voice mono [notrack] [alloc]` or `voice poly <n> [alloc]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceDef {
    Mono {
        track_pitch: bool,
        allocation: MonoAllocation,
    },
    Poly {
        voices: u32,
        allocation: PolyAllocation,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MonoAllocation {
    #[default]
    Replace,
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolyAllocation {
    #[default]
    ReplaceOldest,
    ReplaceYoungest,
    ReplaceLoudest,
    ReplaceQuietest,
    ReplaceRandom,
    Drop,
}

/// What a note-off does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lifecycle {
    OneShot,
    #[default]
    Gated,
    Cutoff,
}

/// How several tones combine.
///
/// These are the tone-mixing modes of an instrument. Note that the engine also
/// has a separate, similarly named set for combining signals arriving at one
/// graph input; the two are not the same list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MixMode {
    #[default]
    Sum,
    Multiply,
    Max,
    Average,
}

/// One oscillator.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToneDef {
    pub waveform: Waveform,
    /// Mix level, lowered to a constant amplitude envelope.
    pub gain: Option<f64>,
    /// A fixed frequency in hertz, ignoring the played note.
    pub frequency: Option<f64>,
    /// How the tone follows the played note. Mutually exclusive with `frequency`.
    pub relation: Option<Relation>,
    pub envelope: Option<EnvelopeDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Waveform {
    #[default]
    Sine,
    Square,
    Saw,
    Triangle,
    SquareRaw,
    SawRaw,
    TriangleRaw,
    Noise,
    PinkNoise,
    Blank,
}

/// How a tone's frequency relates to the played note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Relation {
    Identity,
    Harmonic(u32),
    Ratio(f64),
    Offset(f64),
    Semitones(i32),
    Constant(f64),
}

/// An envelope, in any of the three spellings of §6.5.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvelopeDef {
    /// `adsr <a> <d> <s> <r>`.
    Adsr {
        attack: f64,
        decay: f64,
        sustain: f64,
        release: f64,
    },
    /// `segment <segment>` — one segment as the whole envelope.
    Single(SegmentDef),
    /// Accumulated `attack`/`decay`/`sustain`/`release` stage lines.
    Stages {
        attack: Option<SegmentDef>,
        decay: Option<SegmentDef>,
        sustain: Option<SegmentDef>,
        release: Option<SegmentDef>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SegmentDef {
    Linear {
        from: f64,
        to: f64,
        duration: f64,
    },
    Bezier {
        from: f64,
        to: f64,
        duration: f64,
        control: (f64, f64),
    },
    Constant {
        value: f64,
        duration: Option<f64>,
    },
}

/// `sample "<path>" [root N] [start S] [end S] [loop]`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SampleDef {
    pub path: String,
    pub root_midi: Option<u32>,
    pub start_seconds: Option<f64>,
    pub end_seconds: Option<f64>,
    pub looped: bool,
}
