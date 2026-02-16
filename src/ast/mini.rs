//! Mini-notation AST — the pattern language inside double quotes.

use super::program::{Accidental, NoteLetter};

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

/// A single step in a sequence: an atom with an optional modifier.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub atom: Atom,
    pub modifier: Option<Modifier>,
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
    /// A grouped subsequence: `[c4 e4 g4]`
    /// Also used for chords when inner sequences are comma-separated.
    Group(Group),
    /// Cycle through alternatives: `<c4 e4 g4>`
    Alternation(Alternation),
}

/// A pitched note with letter, accidental, and octave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Note {
    pub letter: NoteLetter,
    pub accidental: Accidental,
    pub octave: u8,
}

/// A bracketed group `[...]`.
/// If `layers.len() == 1`, it's a simple subdivision.
/// If `layers.len() > 1`, the layers play simultaneously (chord).
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
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
    /// `*N` — repeat within the time slot.
    Repeat(u32),
    /// `/N` — stretch over N cycles.
    Slow(u32),
    /// `!N` — replicate as N separate equal steps.
    Replicate(u32),
    /// `(beats, steps[, offset])` — Euclidean rhythm.
    Euclidean(u32, u32, Option<u32>),
    /// `?` — 50% chance of silence.
    Drop,
    /// `@N` — proportional duration weight.
    Weight(u32),
}
