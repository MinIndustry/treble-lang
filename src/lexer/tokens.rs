//! Token definitions for RusticScore lexer

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Keywords - Structural
    KwTitle,
    KwTempo,
    KwTime,
    KwKey,
    KwMajor,
    KwMinor,
    KwInstrument,
    KwVoice,
    KwGenerator,
    KwEnvelope,
    KwEffects,
    KwPitchEnvelope,
    KwBuiltin,

    // Keywords - Waveforms
    KwSine,
    KwSquare,
    KwSaw,
    KwSawtooth,
    KwTriangle,
    KwNoise,
    KwWhiteNoise,
    KwPinkNoise,

    // Keywords - Frequency Relations
    KwFreq,
    KwHarmonic,
    KwRatio,
    KwSemitones,
    KwOffset,

    // Keywords - Envelopes
    KwAdsr,
    KwBezier,
    KwLinear,

    // Keywords - Effects
    KwLowpass,
    KwHighpass,
    KwBandpass,
    KwDelay,
    KwReverb,
    KwGain,
    KwTremolo,

    // Keywords - Scales
    KwDorian,
    KwPhrygian,
    KwLydian,
    KwMixolydian,
    KwAeolian,
    KwLocrian,

    // Note names
    NoteC,
    NoteD,
    NoteE,
    NoteF,
    NoteG,
    NoteA,
    NoteB,

    // Accidentals
    Sharp,  // s
    Flat,   // f
    Natural, // n

    // Literals
    Identifier(String),
    Integer(i64),
    Float(f64),
    String(String),

    // Symbols
    Pipe,       // |
    LBracket,   // [
    RBracket,   // ]
    LBrace,     // {
    RBrace,     // }
    LParen,     // (
    RParen,     // )
    Colon,      // :
    Comma,      // ,
    Plus,       // +
    Star,       // *
    Dot,        // .
    Bang,       // !
    Dash,       // -
    Slash,      // /

    // Special
    Newline,
    Eof,
}
