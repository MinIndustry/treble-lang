//! Lowering the language onto the engine, and rendering a piece offline.
//!
//! Two halves. [`compile`] turns a parsed buffer into what the engine needs —
//! events, instrument specs, resolved filters — and is shared with the live
//! front end so a rendered piece and a played performance cannot drift apart.
//! [`offline`] drives that output through the engine to produce samples,
//! which is what a piece needs and a performance does not.

pub mod compile;
pub mod offline;
pub mod wav;

pub use compile::PatternGate;
pub use offline::{
    Progress, RenderTelemetry, RenderedPiece, ScheduledNote, render, render_with_progress,
    scheduled_notes,
};
