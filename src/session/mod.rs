//! Live session engine.
//!
//! The [`Session`] holds the live state: active patterns, tempo, time
//! signature, etc.  The TUI calls [`Session::evaluate`] on save, which
//! parses the source, diffs against the previous state, and queues
//! changes for the next loop boundary.

use std::collections::HashMap;

use crate::ast::{PatternDef, Program, SourceLine};
use crate::error::CompileError;
use crate::parser::parse_program;

/// A change that will be applied at the next loop boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    /// A new pattern was added.
    Add(String),
    /// An existing pattern was modified.
    Modify(String),
    /// A pattern was removed.
    Remove(String),
    /// A pattern was muted.
    Mute(String),
    /// A pattern was unmuted.
    Unmute(String),
}

/// Result returned by [`Session::evaluate`].
#[derive(Debug, Clone)]
pub struct EvalResult {
    /// Parse errors (per-line, non-fatal).
    pub errors: Vec<CompileError>,
    /// Changes detected vs. previous state.
    pub deltas: Vec<Delta>,
    /// Summary counts.
    pub patterns_active: usize,
    pub patterns_muted: usize,
}

/// Live session state.
pub struct Session {
    /// Current BPM.
    pub bpm: u32,
    /// Current time signature (numerator, denominator).
    pub sig: (u8, u8),
    /// Active patterns by name.
    patterns: HashMap<String, PatternDef>,
    /// Pending deltas (queued for next loop boundary).
    pending: Vec<Delta>,
    /// Last successfully parsed program (for diffing).
    last_program: Option<Program>,
}

impl Session {
    pub fn new() -> Self {
        Self {
            bpm: 120,
            sig: (4, 4),
            patterns: HashMap::new(),
            pending: Vec::new(),
            last_program: None,
        }
    }

    /// Evaluate a source string, parse it, diff against previous state,
    /// and return the result.
    pub fn evaluate(&mut self, source: &str) -> EvalResult {
        let (program, errors) = parse_program(source);

        // Extract state from the new program
        let mut new_bpm = self.bpm;
        let mut new_sig = self.sig;
        let mut new_patterns: HashMap<String, PatternDef> = HashMap::new();

        for line in &program.lines {
            match line {
                SourceLine::Bpm(val) => new_bpm = *val,
                SourceLine::Sig(num, den) => new_sig = (*num, *den),
                SourceLine::Pattern(def) => {
                    new_patterns.insert(def.name.clone(), def.clone());
                }
                _ => {}
            }
        }

        // Compute deltas
        let deltas = self.diff(&new_patterns);

        // Apply immediate directives
        self.bpm = new_bpm;
        self.sig = new_sig;

        // Update pattern state
        self.pending = deltas.clone();
        self.patterns = new_patterns;
        self.last_program = Some(program);

        let patterns_active = self.patterns.values().filter(|p| !p.muted).count();
        let patterns_muted = self.patterns.values().filter(|p| p.muted).count();

        EvalResult {
            errors,
            deltas,
            patterns_active,
            patterns_muted,
        }
    }

    /// Diff new patterns against current state.
    fn diff(&self, new_patterns: &HashMap<String, PatternDef>) -> Vec<Delta> {
        let mut deltas = Vec::new();

        // Check for added or modified patterns
        for (name, new_def) in new_patterns {
            match self.patterns.get(name) {
                None => deltas.push(Delta::Add(name.clone())),
                Some(old_def) => {
                    if old_def.muted && !new_def.muted {
                        deltas.push(Delta::Unmute(name.clone()));
                    } else if !old_def.muted && new_def.muted {
                        deltas.push(Delta::Mute(name.clone()));
                    } else if old_def != new_def {
                        deltas.push(Delta::Modify(name.clone()));
                    }
                }
            }
        }

        // Check for removed patterns
        for name in self.patterns.keys() {
            if !new_patterns.contains_key(name) {
                deltas.push(Delta::Remove(name.clone()));
            }
        }

        deltas
    }

    /// Get the currently pending deltas.
    pub fn pending_deltas(&self) -> &[Delta] {
        &self.pending
    }

    /// Apply pending deltas (called by the TUI at loop boundary).
    pub fn apply_pending(&mut self) {
        self.pending.clear();
    }

    /// Get the current active (non-muted) pattern definitions.
    pub fn active_patterns(&self) -> Vec<&PatternDef> {
        self.patterns.values().filter(|p| !p.muted).collect()
    }

    /// Get all pattern definitions (including muted).
    pub fn all_patterns(&self) -> &HashMap<String, PatternDef> {
        &self.patterns
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_initial_state() {
        let session = Session::new();
        assert_eq!(session.bpm, 120);
        assert_eq!(session.sig, (4, 4));
        assert!(session.patterns.is_empty());
    }

    #[test]
    fn test_evaluate_directives() {
        let mut session = Session::new();
        let result = session.evaluate("bpm 140\nsig 3/4");
        assert!(result.errors.is_empty());
        assert_eq!(session.bpm, 140);
        assert_eq!(session.sig, (3, 4));
    }

    #[test]
    fn test_evaluate_adds_patterns() {
        let mut session = Session::new();
        let result = session.evaluate("kick kick \"x ~ x ~\"\nbass sine \"c2 eb2\"");
        assert!(result.errors.is_empty());
        assert_eq!(result.deltas.len(), 2);
        assert!(result.deltas.iter().all(|d| matches!(d, Delta::Add(_))));
        assert_eq!(result.patterns_active, 2);
    }

    #[test]
    fn test_evaluate_detects_removal() {
        let mut session = Session::new();
        session.evaluate("kick kick \"x ~ x ~\"\nbass sine \"c2 eb2\"");

        let result = session.evaluate("kick kick \"x ~ x ~\"");
        assert!(result.deltas.iter().any(|d| *d == Delta::Remove("bass".into())));
    }

    #[test]
    fn test_evaluate_detects_modify() {
        let mut session = Session::new();
        session.evaluate("kick kick \"x ~ x ~\"");

        let result = session.evaluate("kick kick \"x x x x\"");
        assert!(result.deltas.iter().any(|d| *d == Delta::Modify("kick".into())));
    }

    #[test]
    fn test_evaluate_detects_mute() {
        let mut session = Session::new();
        session.evaluate("kick kick \"x ~ x ~\"");

        let result = session.evaluate("; kick kick \"x ~ x ~\"");
        assert!(result.deltas.iter().any(|d| *d == Delta::Mute("kick".into())));
        assert_eq!(result.patterns_muted, 1);
        assert_eq!(result.patterns_active, 0);
    }

    #[test]
    fn test_evaluate_detects_unmute() {
        let mut session = Session::new();
        session.evaluate("; kick kick \"x ~ x ~\"");

        let result = session.evaluate("kick kick \"x ~ x ~\"");
        assert!(result.deltas.iter().any(|d| *d == Delta::Unmute("kick".into())));
    }

    #[test]
    fn test_unchanged_no_delta() {
        let mut session = Session::new();
        session.evaluate("kick kick \"x ~ x ~\"");

        let result = session.evaluate("kick kick \"x ~ x ~\"");
        assert!(result.deltas.is_empty());
    }

    #[test]
    fn test_error_recovery_preserves_valid() {
        let mut session = Session::new();
        let result = session.evaluate("bpm 140\nthis is broken ???\nkick kick \"x ~ x ~\"");
        assert_eq!(result.errors.len(), 1);
        assert_eq!(session.bpm, 140);
        assert_eq!(result.patterns_active, 1);
    }

    #[test]
    fn test_full_example() {
        let mut session = Session::new();
        let source = "\
-- techno loop
bpm 128
sig 4/4

kick kick \"x ~ x ~\"
snare snare \"~ x ~ x\"
hats hihat \"x*8\"
bass saw \"c2 _ eb2 _ g1 _ f2 _\"
lead piano \"c4 eb4 g4 bb4\" | slow 2
; pad pad \"[c3,eb3,g3] ~ [f3,ab3,c4] ~\"";

        let result = session.evaluate(source);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(session.bpm, 128);
        assert_eq!(session.sig, (4, 4));
        assert_eq!(result.patterns_active, 5);
        assert_eq!(result.patterns_muted, 1);
        assert_eq!(result.deltas.len(), 6); // all new = 6 adds
    }
}
