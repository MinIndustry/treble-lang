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
use crate::piece::{self, Piece};

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
    /// A `load "<path>"` needs resolving: the path is new to the buffer, or the
    /// order of the `load` lines changed and with it which file wins a name.
    ///
    /// This crate does no I/O, so it cannot see a loaded file change on disk; a
    /// consumer watching the file raises this same delta for that case (§9.2).
    Load(String),
    /// A `load` line left the buffer, so its definitions go with it.
    Unload(String),
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
    /// The resolved timeline when the buffer is a piece (§8), `None` when it
    /// is a live buffer.
    ///
    /// A piece is not diffed: it is re-resolved from the top, so `deltas` is
    /// empty for everything but the instrument definitions and `load` paths a
    /// consumer still has to act on.
    pub piece: Option<Piece>,
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
    /// `seed` (§8.8) — salts every generative construct in the buffer.
    pub seed: u64,
    /// Default scale for degree patterns (`scale` directive).
    pub scale: Option<(PitchRoot, ScaleMode)>,
    /// Active patterns by name.
    patterns: HashMap<String, PatternDef>,
    /// Instruments defined in the buffer by `def`, by name.
    definitions: HashMap<String, InstrumentDef>,
    /// Instrument groups by name.
    groups: HashMap<String, GroupDef>,
    /// `load` paths in buffer order — the order decides which of two files wins
    /// a name (§2.5), so it is kept rather than reduced to a set.
    loads: Vec<String>,
    /// Pending deltas (queued for next loop boundary).
    pending: Vec<Delta>,
    /// Last successfully parsed program (for diffing).
    last_program: Option<Program>,
    /// The resolved piece, when the buffer has sections (§8).
    piece: Option<Piece>,
}

impl Session {
    pub fn new() -> Self {
        Self {
            bpm: 120,
            sig: (4, 4),
            phrase: 1,
            seed: 0,
            scale: None,
            patterns: HashMap::new(),
            definitions: HashMap::new(),
            groups: HashMap::new(),
            loads: Vec::new(),
            pending: Vec::new(),
            last_program: None,
            piece: None,
        }
    }

    /// The resolved piece, when the buffer has sections (§8).
    ///
    /// This is the authoritative structure in piece mode: [`Session::patterns`]
    /// still holds the union of every section's lines, but only so a consumer
    /// can see which instruments the buffer needs — which line sounds when is
    /// the piece's business, since the same name legitimately appears in every
    /// section.
    pub fn piece(&self) -> Option<&Piece> {
        self.piece.as_ref()
    }

    /// Whether the buffer is a piece rather than a live session (§8.1).
    pub fn is_piece(&self) -> bool {
        self.piece.is_some()
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
                piece: self.piece.clone(),
            };
        }

        // A buffer with sections is a piece (§8.1). The two modes share every
        // line kind but differ in what a line means, so the mode is settled
        // once here and the rest of the walk consults it.
        let is_piece = piece::is_piece(&program);

        // Extract state from the new program
        let mut new_bpm = self.bpm;
        let mut new_sig = self.sig;
        let mut new_phrase = self.phrase;
        let mut new_seed = self.seed;
        let mut new_scale = self.scale;
        let mut new_patterns: HashMap<String, PatternDef> = HashMap::new();
        let mut new_definitions: HashMap<String, InstrumentDef> = HashMap::new();
        let mut new_groups: HashMap<String, GroupDef> = HashMap::new();
        let mut new_loads: Vec<String> = Vec::new();
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
                SourceLine::Load(path) => new_loads.push(path.clone()),
                SourceLine::Seed(value) => new_seed = *value,
                SourceLine::Meta(_, _) => {}
                directive @ (SourceLine::Arrange(_) | SourceLine::Tail(_)) if !is_piece => {
                    group_errors.push(group_error(
                        line_idx + 1,
                        format!(
                            "{}: a live buffer has no sections to arrange or tail — add a \
                             'section' block to make this a piece (§8.1)",
                            match directive {
                                SourceLine::Arrange(_) => "arrange",
                                _ => "tail",
                            }
                        ),
                    ));
                }
                SourceLine::Pattern(def) => {
                    new_patterns.insert(def.name.clone(), def.clone());
                }
                SourceLine::Def(definition) => {
                    new_definitions.insert(definition.name.clone(), (**definition).clone());
                }
                SourceLine::GroupStart { muted, name } => {
                    // In a piece the same bus is declared in every section that
                    // feeds it, which is not a redeclaration; that the chains
                    // agree is checked when the piece is resolved (§8.2).
                    if !is_piece && new_groups.contains_key(name) {
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
        // A group and a pattern sharing a name would collide in the mixer. In a
        // piece the namespaces are per-section, so the piece resolver checks it
        // there rather than against the flattened union here.
        for name in new_groups.keys().filter(|_| !is_piece) {
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
                piece: self.piece.clone(),
            };
        }

        // Compute deltas
        let mut deltas = self.diff(&new_patterns);
        deltas.extend(self.diff_definitions(&new_definitions));
        deltas.extend(self.diff_groups(&new_groups));
        deltas.extend(self.diff_loads(&new_loads));

        // Resolve the arrangement into a timeline. A piece is not diffed — it
        // is re-resolved from the top (§8.9) — so this replaces the pattern
        // deltas rather than adding to them.
        let mut piece = None;
        if is_piece {
            let (resolved, piece_errors) = piece::resolve(&program, (new_bpm, new_sig, new_scale));
            if !piece_errors.is_empty() {
                return EvalResult {
                    errors: piece_errors,
                    deltas: Vec::new(),
                    patterns_active: self.patterns.values().filter(|p| !p.muted).count(),
                    patterns_muted: self.patterns.values().filter(|p| p.muted).count(),
                    piece: self.piece.clone(),
                };
            }
            piece = Some(resolved);
        }

        // Apply immediate directives
        self.bpm = new_bpm;
        self.sig = new_sig;
        self.phrase = new_phrase;
        self.seed = new_seed;
        self.scale = new_scale;

        // Update pattern state
        self.pending = deltas.clone();
        self.patterns = new_patterns;
        self.definitions = new_definitions;
        self.groups = new_groups;
        self.loads = new_loads;
        self.last_program = Some(program);
        self.piece = piece.clone();

        let patterns_active = self.patterns.values().filter(|p| !p.muted).count();
        let patterns_muted = self.patterns.values().filter(|p| p.muted).count();

        EvalResult {
            errors,
            deltas,
            patterns_active,
            patterns_muted,
            piece,
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

    /// Diff `load` paths against current state.
    ///
    /// A path is compared by the text the performer wrote, not by a resolved
    /// filename: resolution is the consumer's (§2.5), and it is the only side
    /// that can tell two spellings of one file apart.
    fn diff_loads(&self, new_loads: &[String]) -> Vec<Delta> {
        let mut deltas = Vec::new();
        for path in new_loads {
            if !self.loads.contains(path) {
                deltas.push(Delta::Load(path.clone()));
            }
        }
        for path in self.loads.iter() {
            if !new_loads.contains(path) {
                deltas.push(Delta::Unload(path.clone()));
            }
        }
        // A pure reorder changes which file wins a name, so the whole set has
        // to be resolved again even though nothing was added or removed.
        if deltas.is_empty() && self.loads != new_loads {
            deltas.extend(new_loads.iter().cloned().map(Delta::Load));
        }
        deltas
    }

    /// `load` paths in the buffer, in the order they were written.
    pub fn loads(&self) -> &[String] {
        &self.loads
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
    fn test_loads_are_tracked_and_diffed() {
        let mut session = Session::new();
        let result = session.evaluate("load \"pads.trbl\"\nkick kick \"x ~\"");
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(result.deltas.contains(&Delta::Load("pads.trbl".into())));
        assert_eq!(session.loads(), ["pads.trbl"]);

        // Unchanged buffer, nothing to resolve again.
        let result = session.evaluate("load \"pads.trbl\"\nkick kick \"x ~\"");
        assert!(result.deltas.is_empty(), "{:?}", result.deltas);

        // A second file is one new load, not a re-resolve of both.
        let result = session.evaluate("load \"pads.trbl\"\nload \"keys.trbl\"\nkick kick \"x ~\"");
        assert_eq!(result.deltas, vec![Delta::Load("keys.trbl".into())]);

        // Swapping the order changes which file wins a name, so both reload.
        let result = session.evaluate("load \"keys.trbl\"\nload \"pads.trbl\"\nkick kick \"x ~\"");
        assert_eq!(
            result.deltas,
            vec![
                Delta::Load("keys.trbl".into()),
                Delta::Load("pads.trbl".into())
            ]
        );

        let result = session.evaluate("kick kick \"x ~\"");
        assert!(result.deltas.contains(&Delta::Unload("pads.trbl".into())));
        assert!(result.deltas.contains(&Delta::Unload("keys.trbl".into())));
        assert!(session.loads().is_empty());
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
