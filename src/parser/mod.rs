//! Parser for Treble Live DSL.
//!
//! The parser is line-oriented: each source line is parsed independently.
//! `def` blocks are the sole exception — [`parse_program`] gathers a block's
//! lines and parses them as a unit. Mini-notation (inside double quotes) is
//! parsed with nom combinators.

mod instrument;
mod lines;
mod mini;

pub use instrument::parse_instrument_def;
pub use lines::parse_line;
pub use mini::parse_mini;

use instrument::count_braces;

use crate::ast::{Program, SourceLine};
use crate::error::{CompileError, CompileErrorKind, SourceLocation};

/// Parse an entire source string into a [`Program`].
///
/// Each line is parsed independently. Lines that fail to parse produce a
/// [`CompileError`] in the error list but do not prevent other lines from
/// being parsed (best-effort / error-recovery).
pub fn parse_program(source: &str) -> (Program, Vec<CompileError>) {
    let mut lines = Vec::new();
    let mut errors = Vec::new();
    // A `def` block spans lines, so it is collected before being parsed.
    let mut block: Option<(usize, Vec<&str>)> = None;
    let mut depth = 0usize;
    // The group currently open, if any. Group members stay ordinary lines;
    // only their `group` tag records the membership.
    let mut open_group: Option<(usize, String)> = None;

    let report = |errors: &mut Vec<CompileError>, line: usize, message: String| {
        errors.push(CompileError {
            kind: CompileErrorKind::ParseError,
            location: SourceLocation {
                line,
                column: 1,
                file: None,
            },
            message,
            suggestion: None,
        });
    };

    for (line_idx, raw) in source.lines().enumerate() {
        if let Some((_, collected)) = &mut block {
            depth += count_braces(raw, '{');
            depth -= count_braces(raw, '}').min(depth);
            collected.push(raw);
            if depth == 0 {
                let (start, collected) = block.take().expect("inside a block");
                match parse_instrument_def(&collected) {
                    Ok(definition) => lines.push(SourceLine::Def(Box::new(definition))),
                    Err(message) => {
                        report(&mut errors, start + 1, message);
                        // Preserve the block's text so nothing is silently lost.
                        for line in collected {
                            lines.push(SourceLine::Comment(line.to_string()));
                        }
                    }
                }
            }
            continue;
        }

        if raw.trim_start().starts_with("def ") || raw.trim() == "def" {
            if let Some((_, group)) = &open_group {
                report(
                    &mut errors,
                    line_idx + 1,
                    format!("def: an instrument definition cannot live inside group '{group}'"),
                );
            }
            depth = count_braces(raw, '{');
            depth -= count_braces(raw, '}').min(depth);
            if depth == 0 {
                // A block opened and closed on one line.
                match parse_instrument_def(&[raw]) {
                    Ok(definition) => lines.push(SourceLine::Def(Box::new(definition))),
                    Err(message) => {
                        report(&mut errors, line_idx + 1, message);
                        lines.push(SourceLine::Comment(raw.to_string()));
                    }
                }
            } else {
                block = Some((line_idx, vec![raw]));
            }
            continue;
        }

        match parse_line(raw) {
            Ok(source_line) => {
                match source_line {
                    SourceLine::GroupStart { muted, name } => {
                        if let Some((_, outer)) = &open_group {
                            report(
                                &mut errors,
                                line_idx + 1,
                                format!("group: groups don't nest; '{outer}' is still open"),
                            );
                        }
                        open_group = Some((line_idx, name.clone()));
                        lines.push(SourceLine::GroupStart { muted, name });
                    }
                    SourceLine::GroupEnd(transforms) => {
                        if open_group.take().is_none() {
                            report(
                                &mut errors,
                                line_idx + 1,
                                "group: '}' without an open group".to_string(),
                            );
                        }
                        lines.push(SourceLine::GroupEnd(transforms));
                    }
                    SourceLine::Pattern(mut def) => {
                        def.group = open_group.as_ref().map(|(_, name)| name.clone());
                        lines.push(SourceLine::Pattern(def));
                    }
                    // A group holds patterns; a directive inside one would
                    // silently act on the whole session, so it is refused.
                    directive @ (SourceLine::Bpm(_)
                    | SourceLine::Sig(_, _)
                    | SourceLine::Phrase(_)
                    | SourceLine::Scale(_, _)
                    | SourceLine::Load(_)
                    | SourceLine::Include(_))
                        if open_group.is_some() =>
                    {
                        report(
                            &mut errors,
                            line_idx + 1,
                            "group: directives cannot live inside a group — move this above it"
                                .to_string(),
                        );
                        lines.push(directive);
                    }
                    other => lines.push(other),
                }
            }
            Err(msg) => {
                report(&mut errors, line_idx + 1, msg);
                // Keep the line as a comment so we don't lose it
                lines.push(SourceLine::Comment(raw.to_string()));
            }
        }
    }

    if let Some((start, name)) = open_group {
        report(
            &mut errors,
            start + 1,
            format!("group: '{name}' is never closed with '}}'"),
        );
    }

    if let Some((start, collected)) = block {
        report(
            &mut errors,
            start + 1,
            "def: the block is never closed with '}'".to_string(),
        );
        for line in collected {
            lines.push(SourceLine::Comment(line.to_string()));
        }
    }

    (Program { lines }, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

    #[test]
    fn test_group_block() {
        let source = "\
include kick
include hihat

group drums {
  kick kick \"x ~ x ~\"
  ; hat hihat \"x*8\"
} | lpf 1800 | gain 0.9

lead bass \"0 3 5 3\"";
        let (prog, errs) = parse_program(source);
        assert!(errs.is_empty(), "{errs:?}");
        let patterns: Vec<&PatternDef> = prog
            .lines
            .iter()
            .filter_map(|line| match line {
                SourceLine::Pattern(def) => Some(def),
                _ => None,
            })
            .collect();
        // Members are tagged; the line after the close is not.
        assert_eq!(patterns[0].group.as_deref(), Some("drums"));
        assert_eq!(patterns[1].group.as_deref(), Some("drums"));
        assert!(patterns[1].muted, "member mute survives inside a group");
        assert_eq!(patterns[2].group, None);
        // The close carries the shared chain.
        let end = prog
            .lines
            .iter()
            .find_map(|line| match line {
                SourceLine::GroupEnd(transforms) => Some(transforms),
                _ => None,
            })
            .unwrap();
        assert_eq!(end.len(), 2);
    }

    #[test]
    fn test_muted_group_and_bare_close() {
        let (prog, errs) = parse_program("; group drums {\nkick kick \"x\"\n}");
        assert!(errs.is_empty(), "{errs:?}");
        assert!(matches!(
            &prog.lines[0],
            SourceLine::GroupStart { muted: true, name } if name == "drums"
        ));
        assert!(matches!(&prog.lines[2], SourceLine::GroupEnd(t) if t.is_empty()));
    }

    #[test]
    fn test_group_structural_errors() {
        // Unclosed, stray close, nesting, directives and defs inside.
        assert!(!parse_program("group drums {").1.is_empty());
        assert!(!parse_program("}").1.is_empty());
        assert!(
            !parse_program("group a {\ngroup b {\n}\n}").1.is_empty(),
            "groups must not nest"
        );
        assert!(!parse_program("group a {\nbpm 130\n}").1.is_empty());
        assert!(
            !parse_program("group a {\ndef x { tone sine }\n}")
                .1
                .is_empty(),
            "defs must not live inside groups"
        );
        // Header must end with the brace; members go on their own lines.
        assert!(
            !parse_program("group drums { kick kick \"x\" }")
                .1
                .is_empty()
        );
    }

    #[test]
    fn test_parse_empty_program() {
        let (prog, errs) = parse_program("");
        assert!(errs.is_empty());
        // "".lines() yields an empty iterator, so 0 lines
        assert_eq!(prog.lines.len(), 0);
    }

    #[test]
    fn test_parse_full_example() {
        let source = "\
-- techno
bpm 128
sig 4/4

kick kick \"x ~ x ~\"
; snare snare \"~ x ~ x\"
lead saw \"c4 eb4 g4\" | rev | slow 2";

        let (prog, errs) = parse_program(source);
        assert!(errs.is_empty(), "errors: {:?}", errs);
        assert_eq!(prog.lines.len(), 7);
        assert!(matches!(prog.lines[0], SourceLine::Comment(_)));
        assert!(matches!(prog.lines[1], SourceLine::Bpm(128)));
        assert!(matches!(prog.lines[2], SourceLine::Sig(4, 4)));
        assert!(matches!(prog.lines[3], SourceLine::Blank));
        // kick pattern
        if let SourceLine::Pattern(ref p) = prog.lines[4] {
            assert_eq!(p.name, "kick");
            assert_eq!(p.instrument, "kick");
            assert!(!p.muted);
            assert!(p.transforms.is_empty());
        } else {
            panic!("expected pattern, got {:?}", prog.lines[4]);
        }
        // muted snare
        if let SourceLine::Pattern(ref p) = prog.lines[5] {
            assert!(p.muted);
            assert_eq!(p.name, "snare");
        } else {
            panic!("expected muted pattern, got {:?}", prog.lines[5]);
        }
        // lead with transforms
        if let SourceLine::Pattern(ref p) = prog.lines[6] {
            assert_eq!(p.name, "lead");
            assert_eq!(p.transforms.len(), 2);
            assert_eq!(p.transforms[0], Transform::Rev);
            assert_eq!(p.transforms[1], Transform::Slow(Ramp::fixed(2.0)));
        } else {
            panic!("expected pattern, got {:?}", prog.lines[6]);
        }
    }

    #[test]
    fn test_error_recovery() {
        let source = "\
bpm 128
this is garbage ???
kick kick \"x ~ x ~\"";
        let (prog, errs) = parse_program(source);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].location.line, 2);
        // The valid lines are still parsed
        assert!(matches!(prog.lines[0], SourceLine::Bpm(128)));
        // Error line kept as comment
        assert!(matches!(prog.lines[1], SourceLine::Comment(_)));
        // Third line still valid
        assert!(matches!(prog.lines[2], SourceLine::Pattern(_)));
    }
}
