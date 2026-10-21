//! Statements: what the line parser reads, made from the physical lines of a
//! buffer (§4.8).
//!
//! Most statements are one line. A pattern line may also break inside its
//! quotes, and its transform chain may continue on following lines that start
//! with `|`; those lines are joined, with a space where each break was, into
//! one statement. Comments are stripped on the way: `--` starts one anywhere
//! outside a `def` block, inside the quotes too. A statement remembers where
//! each of its pieces came from, so a position in the joined text — a step's
//! [`Source`](crate::ast::Source), an error — maps back to a line and column.
//!
//! `def` blocks are left exactly as written: they have their own block
//! parser, which reads its lines one by one.

use super::instrument::count_braces;

/// One statement and where its text came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// 0-based physical line the statement starts on.
    pub first_line: usize,
    /// 0-based physical line it ends on (the same for most statements).
    pub last_line: usize,
    /// The joined text, comments removed. The first line keeps its
    /// indentation, so offsets on it are that line's own byte columns.
    pub text: String,
    /// Whether a comment was written anywhere inside it. Re-flowing such a
    /// statement would drop the comment, so a formatter leaves it alone.
    pub comments: bool,
    pieces: Vec<Piece>,
}

/// A run of the joined text copied from one physical line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Piece {
    /// Byte offset in [`Statement::text`].
    at: usize,
    line: usize,
    /// Byte column in that line.
    column: usize,
    len: usize,
}

impl Statement {
    fn starting(line: usize) -> Self {
        Self {
            first_line: line,
            last_line: line,
            text: String::new(),
            comments: false,
            pieces: Vec::new(),
        }
    }

    fn push(&mut self, text: &str, line: usize, column: usize) {
        if text.is_empty() {
            return;
        }
        if !self.text.is_empty() {
            self.text.push(' ');
        }
        self.pieces.push(Piece {
            at: self.text.len(),
            line,
            column,
            len: text.len(),
        });
        self.text.push_str(text);
        self.last_line = line;
    }

    /// The physical `(line, byte column)` of byte `offset` in [`Self::text`].
    /// An offset on a joining space maps to the end of the piece before it.
    pub fn locate(&self, offset: usize) -> (usize, usize) {
        let Some(piece) = self.pieces.iter().rev().find(|piece| piece.at <= offset) else {
            return (self.first_line, offset);
        };
        (
            piece.line,
            piece.column + (offset - piece.at).min(piece.len),
        )
    }

    /// The byte offset in [`Self::text`] of a physical position, when that
    /// position falls on the statement's text. A column past a piece (in its
    /// comment, say) maps to the piece's end.
    pub fn offset_of(&self, line: usize, column: usize) -> Option<usize> {
        let piece = self
            .pieces
            .iter()
            .rev()
            .find(|piece| piece.line == line && piece.column <= column)?;
        Some(piece.at + (column - piece.column).min(piece.len))
    }

    /// Whether physical line `line` belongs to this statement.
    pub fn covers(&self, line: usize) -> bool {
        (self.first_line..=self.last_line).contains(&line)
    }
}

/// Where a line's comment starts, and whether a quote is still open there.
/// `quoted` is the quote state the line starts in.
fn scan(line: &str, mut quoted: bool) -> (usize, bool) {
    let bytes = line.as_bytes();
    for (index, &byte) in bytes.iter().enumerate() {
        match byte {
            b'"' => quoted = !quoted,
            b'-' if bytes.get(index + 1) == Some(&b'-') => return (index, quoted),
            _ => {}
        }
    }
    (line.len(), quoted)
}

fn blank_or_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.is_empty() || trimmed.starts_with("--")
}

fn opens_def(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("def ") || trimmed.trim_end() == "def"
}

/// A statement whose chain may continue on a `|` line: a pattern (its quotes
/// are closed) or a group's closing brace.
fn takes_a_chain(statement: &Statement) -> bool {
    let text = statement.text.trim_start();
    text.starts_with('}') || text.matches('"').count() >= 2
}

/// Split a buffer into statements, in order. Blank and comment-only lines are
/// statements of their own, kept verbatim, as are the lines of `def` blocks.
pub fn statements(source: &str) -> Vec<Statement> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    let mut def_depth = 0usize;
    let mut index = 0;
    while index < lines.len() {
        let raw = lines[index];
        if def_depth > 0 || opens_def(raw) {
            def_depth += count_braces(raw, '{');
            def_depth -= count_braces(raw, '}').min(def_depth);
            out.push(verbatim(raw, index));
            index += 1;
            continue;
        }
        if blank_or_comment(raw) {
            out.push(verbatim(raw, index));
            index += 1;
            continue;
        }
        let (cut, mut quoted) = scan(raw, false);
        let mut statement = Statement::starting(index);
        statement.comments = cut < raw.len();
        statement.push(raw[..cut].trim_end(), index, 0);
        let mut next = index + 1;
        loop {
            if quoted {
                // Inside the quotes: every line belongs, blank or not.
                let Some(line) = lines.get(next) else { break };
                let (cut, still) = scan(line, true);
                statement.comments |= cut < line.len();
                let code = &line[..cut];
                let lead = code.len() - code.trim_start().len();
                statement.push(code.trim(), next, lead);
                statement.last_line = next;
                quoted = still;
                next += 1;
                continue;
            }
            // A chain continues on the next `|` line, past blank lines and
            // comments in between.
            let mut ahead = next;
            while ahead < lines.len() && blank_or_comment(lines[ahead]) {
                ahead += 1;
            }
            let continues = lines
                .get(ahead)
                .is_some_and(|line| line.trim_start().starts_with('|'));
            if !continues || !takes_a_chain(&statement) {
                break;
            }
            statement.comments |= lines[next..ahead]
                .iter()
                .any(|line| !line.trim().is_empty());
            let line = lines[ahead];
            let (cut, still) = scan(line, false);
            statement.comments |= cut < line.len();
            let code = &line[..cut];
            let lead = code.len() - code.trim_start().len();
            statement.push(code.trim(), ahead, lead);
            quoted = still;
            next = ahead + 1;
        }
        out.push(statement);
        index = next;
    }
    out
}

fn verbatim(line: &str, index: usize) -> Statement {
    let mut statement = Statement::starting(index);
    statement.comments = line.trim_start().starts_with("--");
    statement.pieces.push(Piece {
        at: 0,
        line: index,
        column: 0,
        len: line.len(),
    });
    statement.text = line.to_string();
    statement
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(source: &str) -> Vec<String> {
        statements(source).into_iter().map(|s| s.text).collect()
    }

    #[test]
    fn a_line_is_a_statement() {
        assert_eq!(
            texts("bpm 92\n  k kick \"x ~\" | gain 0.9\n"),
            ["bpm 92", "  k kick \"x ~\" | gain 0.9"]
        );
    }

    #[test]
    fn notation_may_break_inside_its_quotes() {
        let source = "  pad pad_dark \"<\n    [0,2,4]   -- Em\n\n    [5,7,9]\n  >\" @ 3..\n";
        let all = statements(source);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].text, "  pad pad_dark \"< [0,2,4] [5,7,9] >\" @ 3..");
        assert_eq!((all[0].first_line, all[0].last_line), (0, 4));
        assert!(all[0].comments);
        let at = all[0].text.find("[5").unwrap();
        assert_eq!(all[0].locate(at), (3, 4));
        assert_eq!(all[0].offset_of(3, 4), Some(at));
    }

    #[test]
    fn a_chain_may_continue_on_pipe_lines() {
        let source = "  bass bass \"0 2\"\n    | oct -2\n    -- the filter opens\n    | lpf 600..1800 | ramp 4\nbpm 90\n";
        assert_eq!(
            texts(source),
            [
                "  bass bass \"0 2\" | oct -2 | lpf 600..1800 | ramp 4",
                "bpm 90"
            ]
        );
        let group = "  } | reverb 0.1\n    | gain 0.9\n";
        assert_eq!(texts(group), ["  } | reverb 0.1 | gain 0.9"]);
    }

    #[test]
    fn a_pipe_line_does_not_join_a_directive() {
        assert_eq!(texts("bpm 90\n| gain 2\n"), ["bpm 90", "| gain 2"]);
    }

    #[test]
    fn trailing_comments_are_dropped_and_comment_lines_kept() {
        let all = statements("-- intro\n  k kick \"x ~\" -- hits\n");
        assert_eq!(all[0].text, "-- intro");
        assert_eq!(all[1].text, "  k kick \"x ~\"");
        assert!(all[1].comments);
    }

    #[test]
    fn def_blocks_are_left_as_written() {
        let source = "def thump {\n  tone sine gain 0.5\n}\n  k thump \"x\"\n";
        assert_eq!(
            texts(source),
            [
                "def thump {",
                "  tone sine gain 0.5",
                "}",
                "  k thump \"x\""
            ]
        );
    }
}
