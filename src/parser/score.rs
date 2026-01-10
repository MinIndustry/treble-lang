//! RusticScore parser

use crate::ast::score::ScoreProgram;
use crate::error::CompileError;
use crate::lexer::Token;

/// Parse a token stream into a ScoreProgram AST
pub fn parse_score_program(_tokens: &[Token]) -> Result<ScoreProgram, CompileError> {
    todo!("Implement parser")
}
