//! Instrument compilation from AST to rustic Instrument

use crate::ast::shared::InstrumentDef;
use crate::error::CompileError;

/// Build a rustic Instrument from an InstrumentDef AST node
pub fn build_instrument(_def: &InstrumentDef) -> Result<Box<dyn rustic::prelude::Instrument>, CompileError> {
    todo!("Implement instrument builder")
}
