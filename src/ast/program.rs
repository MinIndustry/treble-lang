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
    /// `phrase <cycles>` — how many cycles a musical phrase spans.
    Phrase(u32),
    /// `scale <root> <mode>`
    Scale(PitchRoot, ScaleMode),
    /// `load "<path>"`
    Load(String),
    /// `include <instrument>` — explicitly make a registry instrument available.
    Include(String),
    /// A `def <name> { … }` instrument definition (§6).
    ///
    /// Boxed because a definition is several times the size of any other line,
    /// and every `SourceLine` would otherwise pay for it.
    Def(Box<super::instrument::InstrumentDef>),
    /// A pattern line (possibly muted).
    Pattern(PatternDef),
    /// `[;] group <name> {` — opens an instrument group (§7). The member
    /// pattern lines stay ordinary [`SourceLine::Pattern`]s so line-based
    /// editing keeps working; [`parse_program`](crate::parser::parse_program)
    /// tags each member's [`PatternDef::group`].
    GroupStart { muted: bool, name: String },
    /// `}` closing a group, optionally with `| transform` shared filters.
    GroupEnd(Vec<Transform>),
    /// A comment (kept for round-tripping, not evaluated).
    Comment(String),
    /// An empty/blank line.
    Blank,
}

/// An instrument group (§7): member patterns share one bus filter chain.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupDef {
    /// A muted group silences every member without touching their own flags.
    pub muted: bool,
    pub name: String,
    /// The shared chain after the `}`. The consumer restricts these to audio
    /// transforms (plus `vel`, which distributes to the members).
    pub transforms: Vec<Transform>,
}

/// A pattern definition: `[;] <name> <instrument> "<mini>" [| transform ...]`
#[derive(Debug, Clone, PartialEq)]
pub struct PatternDef {
    pub muted: bool,
    pub name: String,
    pub instrument: String,
    pub notation: MiniNotation,
    pub transforms: Vec<Transform>,
    /// The group this line sits inside, if any — assigned by
    /// [`parse_program`](crate::parser::parse_program), not by the line itself.
    pub group: Option<String>,
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
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accidental {
    #[default]
    Natural,
    Sharp,
    DoubleSharp,
    Flat,
    DoubleFlat,
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
///
/// Transforms split into two families, which matters to consumers: `Rev`,
/// `Fast`, `Slow`, `Arp`, `Scale`, `Oct` and `Vel` reshape the scheduled
/// events, while `Gain`, `Pan`, `Lpf`, `Hpf`, `Delay` and `Reverb` describe an
/// ordered DSP chain.
///
/// Every numeric argument is a [`Ramp`], so any of them may travel across the
/// line's [`Transform::RampSpan`] (§4.6). A parameter that only ever holds one
/// value is a `Ramp::Fixed`, and [`Ramp::travels`] is how a consumer tells the
/// two apart.
#[derive(Debug, Clone, PartialEq)]
pub enum Transform {
    Rev,
    Fast(Ramp<f64>),
    Slow(Ramp<f64>),
    Every(u32, Box<Transform>),
    Arp(ArpMode),
    Scale(PitchRoot, ScaleMode),
    Oct(Ramp<i32>),
    /// `vel <0.0..=1.0>` — the line's default note velocity. A step that names
    /// its own velocity (`X`, `:v` — see [`crate::ast::mini::Step`]) overrides
    /// this for that step.
    Vel(Ramp<f64>),
    /// `ramp <cycles> [lin|exp]` — how long the line's ranges take to travel,
    /// and how they get there. One span governs every range on the line.
    RampSpan {
        cycles: u32,
        curve: RampCurve,
    },
    /// `gain <0.0..=2.0>` — output level, positional in the DSP chain.
    Gain(Ramp<f64>),
    /// `pan <-1.0..=1.0>` — a fixed stereo position, positional in the chain.
    Pan(Ramp<f64>),
    /// `pan <wave> <rate>[hz] [depth]` — a stereo position swept by an LFO.
    AutoPan(PanSweep),
    Lpf(Ramp<f64>),
    Hpf(Ramp<f64>),
    /// `fx <filter> <arg>...`, or one of its short aliases.
    ///
    /// The filter is named but not resolved here: this crate has no knowledge
    /// of the engine's filter set, so the consumer looks the name up in its own
    /// registry and reports an unknown one.
    Fx(FxCall),
    /// `delay <time> <feedback> [mix]` — the wet mix defaults to 0.35.
    Delay(Ramp<f64>, Ramp<f64>, Option<Ramp<f64>>),
    Reverb(Ramp<f64>),
}

/// How a range gets from one end to the other across the `ramp` span.
///
/// The crate records the intent only. `Exp` asks for geometric interpolation —
/// equal ratio steps rather than equal value steps, which is what a filter
/// cutoff needs to sound like a steady opening — but resolving it is the
/// consumer's job, including deciding what to do with a range that touches or
/// crosses zero, where a geometric path does not exist.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RampCurve {
    /// `lin`, and the default when the curve is omitted, so buffers written
    /// before curves existed keep their meaning.
    #[default]
    Linear,
    /// `exp` — equal ratio steps.
    Exponential,
    /// `osc` — oscillate between the ends instead of arriving: a triangle
    /// with the span as its period, `from → to → from`, wrapping forever.
    Oscillate,
}

/// Arpeggiator mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArpMode {
    Up,
    Down,
    UpDown,
    Random,
}

/// A swept stereo position: `pan sine 4`, `pan sq 1 0.6`, `pan tri 0.5hz`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanSweep {
    pub wave: LfoWave,
    pub rate: LfoRate,
    /// How far the sweep reaches, in `0.0..=1.0`. `None` means a full sweep.
    pub depth: Option<f64>,
}

/// LFO shapes available to a swept parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfoWave {
    Sine,
    Triangle,
    Square,
    Saw,
    Random,
}

/// How fast an LFO runs.
///
/// A bare number is musical — a period measured in cycles, which follows the
/// tempo. An `hz` suffix opts into absolute time instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LfoRate {
    /// Period in cycles: `4` is one sweep every four cycles.
    Cycles(f64),
    /// Frequency in hertz: `0.5hz` is one sweep every two seconds.
    Hertz(f64),
}

/// A call on an engine filter: the name the performer wrote, plus its arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct FxCall {
    /// Either a filter name or a short alias, exactly as written.
    pub filter: String,
    pub args: Vec<FxArg>,
}

/// One argument to an [`FxCall`].
#[derive(Debug, Clone, PartialEq)]
pub enum FxArg {
    /// Fills the next of the filter's declared parameters.
    Positional(FxValue),
    /// Sets one parameter by name, leaving the rest at their defaults.
    Named(String, FxValue),
}

/// An argument's value, which may travel across the line's `ramp` span.
///
/// The distinction is syntactic on purpose. Whether a bare number is a literal
/// parameter value or a period in cycles depends on the parameter it lands on,
/// which only the consumer knows. The legal interval is likewise the filter's
/// own business, so nothing here range-checks the values.
#[derive(Debug, Clone, PartialEq)]
pub enum FxValue {
    /// A bare number: `0.6`, `2..8`.
    Plain(Ramp<f64>),
    /// A number with an `hz` suffix, so always an absolute frequency.
    Hertz(Ramp<f64>),
}

impl FxValue {
    /// The ramp inside, whichever spelling it arrived in.
    pub fn ramp(&self) -> &Ramp<f64> {
        match self {
            Self::Plain(ramp) | Self::Hertz(ramp) => ramp,
        }
    }

    /// Whether this argument travels, and so needs the line to have a `ramp`.
    pub fn travels(&self) -> bool {
        self.ramp().travels()
    }
}

/// A value that may travel over the line's [`Transform::RampSpan`].
///
/// Three shapes, because a build is not always a smooth sweep: `4` stays put,
/// `4..16` sweeps continuously, and `2>4>8>16` holds each value in turn. All of
/// them hold their final value once the span has passed — a crescendo arrives
/// and stays until the performer changes it.
#[derive(Debug, Clone, PartialEq)]
pub enum Ramp<T> {
    /// A plain value: `4`.
    Fixed(T),
    /// `4..16` — travels continuously from one end to the other.
    Sweep { from: T, to: T },
    /// `2>4>8>16` — each value held for an equal share of the span.
    ///
    /// Split into head and tail so the sequence can never be empty.
    Steps { first: T, rest: Vec<T> },
}

impl<T: Copy> Ramp<T> {
    pub fn fixed(value: T) -> Self {
        Self::Fixed(value)
    }

    /// Build a stepped ramp, collapsing to a fixed value when nothing follows.
    pub fn steps(first: T, rest: Vec<T>) -> Self {
        if rest.is_empty() {
            Self::Fixed(first)
        } else {
            Self::Steps { first, rest }
        }
    }

    /// The value held before the ramp starts moving.
    pub fn start(&self) -> T {
        match self {
            Self::Fixed(value) => *value,
            Self::Sweep { from, .. } => *from,
            Self::Steps { first, .. } => *first,
        }
    }

    /// Whether this value travels, and so needs a ramp span.
    pub fn travels(&self) -> bool {
        !matches!(self, Self::Fixed(_))
    }

    /// Every value the ramp passes through, for range checking.
    pub fn values(&self) -> Vec<T> {
        match self {
            Self::Fixed(value) => vec![*value],
            Self::Sweep { from, to } => vec![*from, *to],
            Self::Steps { first, rest } => {
                let mut all = vec![*first];
                all.extend(rest.iter().copied());
                all
            }
        }
    }
}
