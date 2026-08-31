//! Treble Live — a live-coding music DSL.
//!
//! This crate provides the parser and session engine for the Treble Live
//! language.  See `LANGUAGE.md` for the full specification.

pub mod ast;
pub mod error;
pub mod parser;
pub mod piece;
pub mod session;

pub use ast::{MiniNotation, PatternDef, Program, SourceLine, Span};
pub use error::{CompileError, CompileErrorKind, SourceLocation};
pub use piece::{Piece, Section};
pub use session::Session;
