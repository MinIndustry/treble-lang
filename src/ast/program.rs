//! Top-level program AST nodes (directives, pattern lines).

use super::mini::MiniNotation;

/// A complete parsed source file.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub lines: Vec<SourceLine>,
}

/// A single parsed line from the source.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceLine {
    /// `bpm <integer>`
    Bpm(u32),
    /// `sig <num>/<den>`
    Sig(u8, u8),
    /// `scale <root> <mode>`
    Scale(PitchRoot, ScaleMode),
    /// `load "<path>"`
    Load(String),
    /// A pattern line (possibly muted).
    Pattern(PatternDef),
    /// A comment (kept for round-tripping, not evaluated).
    Comment(String),
    /// An empty/blank line.
    Blank,
}

/// A pattern definition: `[;] <name> <instrument> "<mini>" [| transform ...]`
#[derive(Debug, Clone, PartialEq)]
pub struct PatternDef {
    pub muted: bool,
    pub name: String,
    pub instrument: String,
    pub notation: MiniNotation,
    pub transforms: Vec<Transform>,
}

/// A pitch root (for scales/directives) — uppercase, with optional accidental.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PitchRoot {
    pub name: NoteLetter,
    pub accidental: Accidental,
}

/// One of the seven note letters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteLetter {
    C,
    D,
    E,
    F,
    G,
    A,
    B,
}

/// Accidental applied to a pitch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accidental {
    Natural,
    Sharp,
    DoubleSharp,
    Flat,
    DoubleFlat,
}

impl Default for Accidental {
    fn default() -> Self {
        Accidental::Natural
    }
}

/// Musical scale / mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleMode {
    Major,
    Minor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Aeolian,
    Locrian,
    Chromatic,
    Pentatonic,
    Blues,
}

/// A transform applied after the mini-notation via `|`.
#[derive(Debug, Clone, PartialEq)]
pub enum Transform {
    Rev,
    Fast(f64),
    Slow(f64),
    Every(u32, Box<Transform>),
    Arp(ArpMode),
    Scale(PitchRoot, ScaleMode),
    Oct(i32),
    Gain(f64),
    Lpf(f64),
    Hpf(f64),
    Delay(f64, f64),
    Reverb(f64),
}

/// Arpeggiator mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArpMode {
    Up,
    Down,
    UpDown,
    Random,
}
