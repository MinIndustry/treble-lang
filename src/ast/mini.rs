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

/// A single step in a sequence: an atom, zero or more modifiers, and an
/// optional velocity.
///
/// Modifiers stack and are applied left-to-right in written order, so `x*8?`
/// first expands into eight slots and then gives each of them a drop chance.
///
/// Velocity is a **field rather than a [`Modifier`]** because it is a property
/// of the step, not a transformation of the slots a modifier produced. The
/// slot-generating modifiers (`*N`, `!N`, `(k,n)`) run first and build the
/// slots; a velocity written anywhere in the run — `x:0.6*4` or `x*4:0.6` —
/// therefore applies to every slot the step generates, and cannot be read as
/// applying to only the slots that existed when it was written. It also makes
/// "at most one velocity per step" a type invariant instead of a check.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub atom: Atom,
    pub modifiers: Vec<Modifier>,
    /// `:v`, or `1.0` for the accent spelling `X` — an **absolute** velocity in
    /// `0.0..=1.0` that overrides the line's `vel` for this step. `None` takes
    /// the line's `vel`. May travel across the line's `ramp` span, which is how
    /// a per-step swell (`x:0.3..0.9`) is written.
    ///
    /// On a group or an alternation this is the velocity of everything the step
    /// sounds, except where an inner step names its own.
    pub velocity: Option<Ramp<f64>>,
}

impl Step {
    /// A step with no modifiers and no velocity.
    pub fn bare(atom: Atom) -> Self {
        Self {
            atom,
            modifiers: Vec::new(),
            velocity: None,
        }
    }
}

/// The core building blocks of the mini-notation.
#[derive(Debug, Clone, PartialEq)]
pub enum Atom {
    /// A pitched note: `c4`, `eb3`, `f#5`
    Note(Note),
    /// A scale degree: `0`, `3`, `7`
    Degree(i32),
    /// A drum trigger: `x`, or `X` for the accented spelling — which is stored
    /// as this atom with [`Step::velocity`] set to `1.0`, so a consumer that
    /// honours `:v` honours `X` without knowing it exists.
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
