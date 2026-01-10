//! Lexer for RusticScore

pub mod tokens;

pub use tokens::Token;

/// Tokenize a RusticScore source string
pub fn tokenize(_source: &str) -> Result<Vec<Token>, crate::error::CompileError> {
    todo!("Implement tokenization")
}
