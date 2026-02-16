//! Parser for Rustic Live DSL.
//!
//! The parser is line-oriented: each source line is parsed independently.
//! Mini-notation (inside double quotes) is parsed with nom combinators.

mod lines;
mod mini;

pub use lines::parse_line;
pub use mini::parse_mini;

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

    for (line_idx, raw) in source.lines().enumerate() {
        match parse_line(raw) {
            Ok(source_line) => lines.push(source_line),
            Err(msg) => {
                errors.push(CompileError {
                    kind: CompileErrorKind::ParseError,
                    location: SourceLocation {
                        line: line_idx + 1,
                        column: 1,
                        file: None,
                    },
                    message: msg,
                    suggestion: None,
                });
                // Keep the line as a comment so we don't lose it
                lines.push(SourceLine::Comment(raw.to_string()));
            }
        }
    }

    (Program { lines }, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

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
            assert_eq!(p.transforms[1], Transform::Slow(2.0));
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
