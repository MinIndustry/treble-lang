//! Rustic DSL - Domain-Specific Language for the rustic music synthesizer
//!
//! This crate provides RusticScore (.rt), a composition DSL for writing musical scores.

pub mod ast;
pub mod compiler;
pub mod error;
pub mod lexer;
pub mod parser;
pub mod runtime;

pub use error::{CompileError, CompileErrorKind, SourceLocation};

// Public API (to be implemented)

/// Compile a .rt file to a rustic Score
pub fn compile_file(_path: impl AsRef<std::path::Path>) -> Result<rustic::prelude::Score, CompileError> {
    todo!("Implement compile_file")
}

/// Compile a string to a rustic Score
pub fn compile_string(_source: &str) -> Result<rustic::prelude::Score, CompileError> {
    todo!("Implement compile_string")
}

/// Check syntax without compiling
pub fn check_syntax(_source: &str) -> Result<(), CompileError> {
    todo!("Implement check_syntax")
}

/// Parse to AST (for tooling)
pub fn parse(_source: &str) -> Result<ast::score::ScoreProgram, CompileError> {
    todo!("Implement parse")
}

/// Start interactive REPL
pub fn repl() -> Result<(), Box<dyn std::error::Error>> {
    todo!("Implement REPL")
}
