//! Error types for rustic-lang

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileErrorKind {
    LexError,      // Invalid token
    ParseError,    // Syntax error
    SemanticError, // Logical error (wrong duration, undefined instrument, etc.)
    IoError,       // File I/O error
}

#[derive(Debug, Clone)]
pub struct CompileError {
    pub kind: CompileErrorKind,
    pub location: SourceLocation,
    pub message: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub line: usize,
    pub column: usize,
    pub file: Option<PathBuf>,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?} at {}:{}: {}", self.kind, self.location.line, self.location.column, self.message)
    }
}

impl std::error::Error for CompileError {}
