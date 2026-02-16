//! Rustic Live — a live-coding music DSL.
//!
//! This crate provides the parser and session engine for the Rustic Live
//! language.  See `LANGUAGE.md` for the full specification.

pub mod ast;
pub mod error;
pub mod parser;
pub mod session;

pub use ast::{MiniNotation, PatternDef, Program, SourceLine};
pub use error::{CompileError, CompileErrorKind, SourceLocation};
pub use session::Session;
