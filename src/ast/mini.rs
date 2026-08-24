//! Mini-notation AST — the pattern language inside double quotes.

use super::program::{Accidental, NoteLetter, Ramp};

/// A generated melody: `solo(low..high, steps)`.
///
/// Walks the scale degrees `low..=high` in a weighted random walk, `steps`
/// notes per cycle. Deterministic per pattern and cycle — it evolves from
/// cycle to cycle but replays identically from the same buffer, like `?` and
/// `[a|b]`. `steps` may ramp (`solo(0..7, 4..16)`), so a solo can densify
/// across the line's `ramp` span.
#[derive(Debug, Clone, PartialEq)]
pub struct Solo {
    pub low: i32,
    pub high: i32,
    pub steps: Ramp<u32>,
}

/// The top-level mini-notation tree (contents of a quoted pattern string).
/// Represents a full cycle that will be looped.
#[derive(Debug, Clone, PartialEq)]
pub struct MiniNotation {
    pub sequence: Sequence,
}

/// An ordered list of steps that share their parent's time equally
/// (unless weights `@N` are present).
#[derive(Debug, Clone, PartialEq)]
pub struct Sequence {
    pub steps: Vec<Step>,
}

/// A single step in a sequence: an atom with zero or more modifiers.
///
/// Modifiers stack and are applied left-to-right in written order, so `x*8?`
/// first expands into eight slots and then gives each of them a drop chance.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub atom: Atom,
    pub modifiers: Vec<Modifier>,
}

/// The core building blocks of the mini-notation.
#[derive(Debug, Clone, PartialEq)]
pub enum Atom {
    /// A pitched note: `c4`, `eb3`, `f#5`
    Note(Note),
    /// A scale degree: `0`, `3`, `7`
    Degree(i32),
    /// A drum trigger: `x`
    Trigger,
    /// Silence for this slot: `~`
    Rest,
    /// Hold/tie the previous event: `_`
    Hold,
    /// A grouped subsequence `[c4 e4 g4]`, a chord `[c4,e4,g4]`, or a random
    /// choice `[c4|e4|g4]` — see [`GroupMode`].
    Group(Group),
    /// Cycle through alternatives: `<c4 e4 g4>`
    Alternation(Alternation),
    /// A generated melody: `solo(0..7, 8)` — see [`Solo`].
    Solo(Solo),
}

/// A pitched note with letter, accidental, and octave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Note {
    pub letter: NoteLetter,
    pub accidental: Accidental,
    pub octave: u8,
}

/// How the layers of a bracketed group relate to each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupMode {
    /// A single layer that subdivides the parent step: `[c4 e4]`.
    Subdivide,
    /// Comma-separated layers sounding together: `[c4,e4,g4]`.
    Chord,
    /// Pipe-separated layers, one picked per cycle: `[c4|e4|g4]`.
    Random,
}

/// A bracketed group `[...]`.
///
/// `mode` records which separator was used. A group carries either commas or
/// pipes, never both, so the two never need precedence against each other.
/// A single-layer group is always [`GroupMode::Subdivide`].
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub mode: GroupMode,
    pub layers: Vec<Sequence>,
}

/// An alternation `<...>` — cycles through the inner sequence steps, one per
/// loop iteration.
#[derive(Debug, Clone, PartialEq)]
pub struct Alternation {
    pub sequence: Sequence,
}

/// Modifiers that can be appended to any atom.
#[derive(Debug, Clone, PartialEq)]
pub enum Modifier {
    /// `*N` — repeat within the time slot. May ramp: `x*4..16`.
    Repeat(Ramp<u32>),
    /// `/N` — stretch over N cycles.
    Slow(u32),
    /// `!N` — replicate as N separate equal steps.
    Replicate(u32),
    /// `(onsets, positions[, offset])` — Euclidean rhythm. Onsets and positions
    /// may ramp, which is how a density crescendo is written: `x(4..16,4)`.
    Euclidean(Ramp<u32>, Ramp<u32>, Option<u32>),
    /// `?` — chance of silence. `None` means the default 50%; `?0.25` carries
    /// an explicit probability in `0.0..=1.0`, which may ramp: `?0.1..0.9`.
    Drop(Option<Ramp<f64>>),
    /// `@N` — proportional duration weight.
    Weight(u32),
}
