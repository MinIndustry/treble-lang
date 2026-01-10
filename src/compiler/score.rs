//! Score compilation from AST to rustic::Score

use crate::ast::score::ScoreProgram;
use crate::error::CompileError;

/// Compile a ScoreProgram AST to a rustic::Score
pub fn compile_score(_program: &ScoreProgram) -> Result<rustic::prelude::Score, CompileError> {
    todo!("Implement score compiler")
}
