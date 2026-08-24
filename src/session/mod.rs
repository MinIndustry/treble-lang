//! Live session engine.
//!
//! The [`Session`] holds the live state: active patterns, tempo, time
//! signature, etc.  The TUI calls [`Session::evaluate`] on save, which
//! parses the source, diffs against the previous state, and queues
//! changes for the next loop boundary.

use std::collections::HashMap;

use crate::ast::{GroupDef, InstrumentDef, PatternDef, PitchRoot, Program, ScaleMode, SourceLine};
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
    /// A `def` instrument was added.
    AddInstrument(String),
    /// A `def` instrument's definition changed.
    ModifyInstrument(String),
    /// A `def` instrument was removed from the buffer.
    RemoveInstrument(String),
    /// A group was added.
    AddGroup(String),
    /// A group's members, filters, or mute flag changed.
    ModifyGroup(String),
    /// A group was removed.
    RemoveGroup(String),
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

fn group_error(line: usize, message: String) -> CompileError {
    CompileError {
        kind: crate::error::CompileErrorKind::ParseError,
        location: crate::error::SourceLocation {
            line,
            column: 1,
            file: None,
        },
        message,
        suggestion: None,
    }
}

/// Live session state.
#[derive(Clone)]
pub struct Session {
    /// Current BPM.
    pub bpm: u32,
    /// Current time signature (numerator, denominator).
    pub sig: (u8, u8),
    /// How many cycles a phrase spans. Changes are quantised to it, so `1` is
    /// "apply at the next cycle" and `16` is "apply at the top of the phrase".
    pub phrase: u32,
    /// Default scale for degree patterns (`scale` directive).
    pub scale: Option<(PitchRoot, ScaleMode)>,
    /// Active patterns by name.
    patterns: HashMap<String, PatternDef>,
    /// Instruments defined in the buffer by `def`, by name.
    definitions: HashMap<String, InstrumentDef>,
    /// Instrument groups by name.
    groups: HashMap<String, GroupDef>,
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
            phrase: 1,
            scale: None,
            patterns: HashMap::new(),
            definitions: HashMap::new(),
            groups: HashMap::new(),
            pending: Vec::new(),
            last_program: None,
        }
    }

    /// Evaluate a source string, parse it, diff against previous state,
    /// and return the result.
    pub fn evaluate(&mut self, source: &str) -> EvalResult {
        let (program, errors) = parse_program(source);

        // Evaluation is transactional: a partially parsed buffer must never
        // poison the state used to diff the next stage-safe edit.
        if !errors.is_empty() {
            return EvalResult {
                errors,
                deltas: Vec::new(),
                patterns_active: self.patterns.values().filter(|p| !p.muted).count(),
                patterns_muted: self.patterns.values().filter(|p| p.muted).count(),
            };
        }

        // Extract state from the new program
        let mut new_bpm = self.bpm;
        let mut new_sig = self.sig;
        let mut new_phrase = self.phrase;
        let mut new_scale = self.scale;
        let mut new_patterns: HashMap<String, PatternDef> = HashMap::new();
        let mut new_definitions: HashMap<String, InstrumentDef> = HashMap::new();
        let mut new_groups: HashMap<String, GroupDef> = HashMap::new();
        // The open group's name and mute flag while walking the lines; the
        // parser has already verified pairing, so this only pairs the header
        // with its `}` transforms.
        let mut open_group: Option<(String, bool)> = None;
        let mut group_errors: Vec<CompileError> = Vec::new();

        for (line_idx, line) in program.lines.iter().enumerate() {
            match line {
                SourceLine::Bpm(val) => new_bpm = *val,
                SourceLine::Sig(num, den) => new_sig = (*num, *den),
                SourceLine::Phrase(cycles) => new_phrase = *cycles,
                SourceLine::Scale(root, mode) => new_scale = Some((*root, *mode)),
                SourceLine::Pattern(def) => {
                    new_patterns.insert(def.name.clone(), def.clone());
                }
                SourceLine::Def(definition) => {
                    new_definitions.insert(definition.name.clone(), (**definition).clone());
                }
                SourceLine::GroupStart { muted, name } => {
                    if new_groups.contains_key(name) {
                        group_errors.push(group_error(
                            line_idx + 1,
                            format!("group '{name}' is declared twice"),
                        ));
                    }
                    open_group = Some((name.clone(), *muted));
                }
                SourceLine::GroupEnd(transforms) => {
                    if let Some((name, muted)) = open_group.take() {
                        new_groups.insert(
                            name.clone(),
                            GroupDef {
                                muted,
                                name,
                                transforms: transforms.clone(),
                            },
                        );
                    }
                }
                _ => {}
            }
        }
        // A group and a pattern sharing a name would collide in the mixer.
        for name in new_groups.keys() {
            if new_patterns.contains_key(name) {
                group_errors.push(group_error(
                    1,
                    format!("'{name}' names both a group and a pattern; pick two names"),
                ));
            }
        }
        if !group_errors.is_empty() {
            return EvalResult {
                errors: group_errors,
                deltas: Vec::new(),
                patterns_active: self.patterns.values().filter(|p| !p.muted).count(),
                patterns_muted: self.patterns.values().filter(|p| p.muted).count(),
            };
        }

        // Compute deltas
        let mut deltas = self.diff(&new_patterns);
        deltas.extend(self.diff_definitions(&new_definitions));
        deltas.extend(self.diff_groups(&new_groups));

        // Apply immediate directives
        self.bpm = new_bpm;
        self.sig = new_sig;
        self.phrase = new_phrase;
        self.scale = new_scale;

        // Update pattern state
        self.pending = deltas.clone();
        self.patterns = new_patterns;
        self.definitions = new_definitions;
        self.groups = new_groups;
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

    /// Diff `def` blocks against current state.
    fn diff_definitions(&self, new_definitions: &HashMap<String, InstrumentDef>) -> Vec<Delta> {
        let mut deltas = Vec::new();
        for (name, definition) in new_definitions {
            match self.definitions.get(name) {
                None => deltas.push(Delta::AddInstrument(name.clone())),
                Some(previous) if previous != definition => {
                    deltas.push(Delta::ModifyInstrument(name.clone()));
                }
                Some(_) => {}
            }
        }
        for name in self.definitions.keys() {
            if !new_definitions.contains_key(name) {
                deltas.push(Delta::RemoveInstrument(name.clone()));
            }
        }
        deltas
    }

    fn diff_groups(&self, new_groups: &HashMap<String, GroupDef>) -> Vec<Delta> {
        let mut deltas = Vec::new();
        for (name, group) in new_groups {
            match self.groups.get(name) {
                None => deltas.push(Delta::AddGroup(name.clone())),
                Some(previous) if previous != group => {
                    deltas.push(Delta::ModifyGroup(name.clone()));
                }
                Some(_) => {}
            }
        }
        for name in self.groups.keys() {
            if !new_groups.contains_key(name) {
                deltas.push(Delta::RemoveGroup(name.clone()));
            }
        }
        deltas
    }

    /// Instruments defined in the buffer, by name.
    pub fn definitions(&self) -> &HashMap<String, InstrumentDef> {
        &self.definitions
    }

    /// Instrument groups in the buffer, by name.
    pub fn groups(&self) -> &HashMap<String, GroupDef> {
        &self.groups
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
    use crate::ast::{Accidental, NoteLetter, PitchRoot, ScaleMode};

    #[test]
    fn test_groups_are_tracked_and_diffed() {
        let mut session = Session::new();
        let source = "group drums {\nkick kick \"x ~\"\n} | lpf 800";
        let result = session.evaluate(source);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(result.deltas.contains(&Delta::AddGroup("drums".into())));
        let group = &session.groups()["drums"];
        assert!(!group.muted);
        assert_eq!(group.transforms.len(), 1);

        // Changing the shared chain is a modify; dropping the block a remove.
        let result = session.evaluate("group drums {\nkick kick \"x ~\"\n} | lpf 400");
        assert!(result.deltas.contains(&Delta::ModifyGroup("drums".into())));
        let result = session.evaluate("kick kick \"x ~\"");
        assert!(result.deltas.contains(&Delta::RemoveGroup("drums".into())));
        assert!(session.groups().is_empty());
    }

    #[test]
    fn test_group_name_collisions_are_rejected() {
        let mut session = Session::new();
        let twice = "group a {\nkick kick \"x\"\n}\ngroup a {\nsn snare \"x\"\n}";
        assert!(!session.evaluate(twice).errors.is_empty());
        let shadowing = "group kick {\nkick kick \"x\"\n}";
        assert!(!session.evaluate(shadowing).errors.is_empty());
        // A failed evaluation must not poison the session.
        assert!(session.groups().is_empty());
    }

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
        assert!(
            result
                .deltas
                .iter()
                .any(|d| *d == Delta::Remove("bass".into()))
        );
    }

    #[test]
    fn test_evaluate_detects_modify() {
        let mut session = Session::new();
        session.evaluate("kick kick \"x ~ x ~\"");

        let result = session.evaluate("kick kick \"x x x x\"");
        assert!(
            result
                .deltas
                .iter()
                .any(|d| *d == Delta::Modify("kick".into()))
        );
    }

    #[test]
    fn test_evaluate_detects_mute() {
        let mut session = Session::new();
        session.evaluate("kick kick \"x ~ x ~\"");

        let result = session.evaluate("; kick kick \"x ~ x ~\"");
        assert!(
            result
                .deltas
                .iter()
                .any(|d| *d == Delta::Mute("kick".into()))
        );
        assert_eq!(result.patterns_muted, 1);
        assert_eq!(result.patterns_active, 0);
    }

    #[test]
    fn test_evaluate_detects_unmute() {
        let mut session = Session::new();
        session.evaluate("; kick kick \"x ~ x ~\"");

        let result = session.evaluate("kick kick \"x ~ x ~\"");
        assert!(
            result
                .deltas
                .iter()
                .any(|d| *d == Delta::Unmute("kick".into()))
        );
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
        session.evaluate("bpm 130\nkick kick \"x ~ x ~\"");
        let result = session.evaluate("bpm 140\nthis is broken ???\nsnare snare \"x ~ x ~\"");
        assert_eq!(result.errors.len(), 1);
        assert_eq!(session.bpm, 130);
        assert_eq!(result.patterns_active, 1);
        assert!(session.all_patterns().contains_key("kick"));
        assert!(!session.all_patterns().contains_key("snare"));
    }

    #[test]
    fn test_evaluate_scale_directive() {
        let mut session = Session::new();
        let result = session.evaluate("scale C minor\nkick kick \"x ~ x ~\"");
        assert!(result.errors.is_empty());
        assert_eq!(
            session.scale,
            Some((
                PitchRoot {
                    name: NoteLetter::C,
                    accidental: Accidental::Natural,
                },
                ScaleMode::Minor,
            ))
        );
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
