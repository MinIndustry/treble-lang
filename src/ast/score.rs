//! RusticScore AST definitions

use super::shared::*;

/// Root node of a RusticScore program
#[derive(Debug, Clone)]
pub struct ScoreProgram {
    pub header: ScoreHeader,
    pub definitions: Vec<Definition>,
}

/// Score header (metadata)
#[derive(Debug, Clone)]
pub struct ScoreHeader {
    pub title: Option<String>,
    pub tempo: u32,
    pub time_signature: TimeSignature,
    pub key: Option<(NoteName, Scale)>,
}

/// Time signature (e.g., 4/4)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSignature {
    pub numerator: u8,
    pub denominator: u8,
}

/// Musical scales
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Major,
    Minor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Aeolian,
    Locrian,
}

/// Top-level definition (instrument or voice)
#[derive(Debug, Clone)]
pub enum Definition {
    Instrument(InstrumentDef),
    Voice(VoiceDef),
}

/// Voice definition (a part/staff)
#[derive(Debug, Clone)]
pub struct VoiceDef {
    pub name: String,
    pub instrument: InstrumentRef,
    pub measures: Vec<Measure>,
}

/// Instrument reference (user-defined or built-in)
#[derive(Debug, Clone, PartialEq)]
pub enum InstrumentRef {
    User(String),
    Builtin(String),
}

/// A measure containing musical events
#[derive(Debug, Clone)]
pub struct Measure {
    pub events: Vec<MeasureEvent>,
}

/// Events within a measure
#[derive(Debug, Clone)]
pub enum MeasureEvent {
    Note {
        pitch: Pitch,
        duration: Duration,
        articulations: Vec<Articulation>,
    },
    Chord {
        pitches: Vec<Pitch>,
        duration: Duration,
        articulations: Vec<Articulation>,
    },
    Rest {
        duration: Duration,
    },
}
