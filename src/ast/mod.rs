//! Abstract Syntax Tree for Rustic Live DSL.
//!
//! This module defines the IR produced by the parser and consumed by the
//! Session evaluator.  It closely mirrors the language grammar from
//! LANGUAGE.md.

pub mod mini;
pub mod program;

pub use mini::*;
pub use program::*;
