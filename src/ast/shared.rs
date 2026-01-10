//! Shared AST nodes between RusticScore and RusticLive

/// Musical pitch representation
#[derive(Debug, Clone, PartialEq)]
pub enum Pitch {
    Note {
        name: NoteName,
        accidental: Accidental,
        octave: u8,
    },
    Rest,
}

/// Note names (C, D, E, F, G, A, B)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteName {
    C,
    D,
    E,
    F,
    G,
    A,
    B,
}

/// Accidentals (sharp, flat, natural)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accidental {
    Natural,
    Sharp,
    Flat,
}

/// Articulation markings
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Articulation {
    Accent,   // !
    Staccato, // :
    Tie,      // -
}

/// Duration representation
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Duration {
    pub value: u32,   // 1, 2, 4, 8, 16, 32...
    pub dotted: bool, // true if dotted (1.5x duration)
}

/// Instrument definition
#[derive(Debug, Clone)]
pub struct InstrumentDef {
    pub name: String,
    pub generator: Option<GeneratorExpr>,
    pub envelope: Option<EnvelopeExpr>,
    pub effects: Vec<EffectExpr>,
    pub pitch_envelope: Option<EnvelopeExpr>,
}

/// Generator expression (can be combined)
#[derive(Debug, Clone)]
pub enum GeneratorExpr {
    Waveform(WaveformExpr),
    Add(Box<GeneratorExpr>, Box<GeneratorExpr>),
    Multiply(Box<GeneratorExpr>, f32),
}

/// Single waveform generator
#[derive(Debug, Clone)]
pub struct WaveformExpr {
    pub waveform: WaveformType,
    pub amplitude: f32,
    pub freq_relation: Option<FrequencyRelation>,
}

/// Waveform types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveformType {
    Sine,
    Square,
    Saw,
    Sawtooth,
    Triangle,
    Noise,
    WhiteNoise,
    PinkNoise,
}

/// Frequency relation to base frequency
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrequencyRelation {
    Constant(f32),
    Harmonic(u8),
    Ratio(f32),
    Semitones(i32),
    Offset(f32),
}

/// Envelope expression
#[derive(Debug, Clone, PartialEq)]
pub enum EnvelopeExpr {
    ADSR {
        attack: f32,
        decay: f32,
        sustain: f32,
        release: f32,
    },
    Bezier {
        start: f32,
        end: f32,
        duration: f32,
    },
    Linear {
        start: f32,
        end: f32,
        duration: f32,
    },
}

/// Effect/filter expression
#[derive(Debug, Clone, PartialEq)]
pub enum EffectExpr {
    LowPass {
        cutoff: f32,
        resonance: Option<f32>,
    },
    HighPass {
        cutoff: f32,
        resonance: Option<f32>,
    },
    BandPass {
        center: f32,
        q: Option<f32>,
    },
    Delay {
        time: f32,
        feedback: f32,
    },
    Reverb {
        amount: f32,
    },
    Gain {
        amount: f32,
    },
    Tremolo {
        rate: f32,
        depth: f32,
    },
}
