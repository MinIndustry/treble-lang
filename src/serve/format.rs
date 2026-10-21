//! Laying a pattern statement out over lines and back (§4.8).
//!
//! Two jobs. `toggle` joins a statement written over several lines onto one,
//! or opens a one-line statement by a level: the chain onto `|` lines, a
//! top-level `[...]` or `<...>` with one child per line. `fit` breaks only
//! what is longer than a limit, as little as it can. Neither touches a
//! statement with comments inside, since re-flowing it would lose them.
//!
//! The text of every step is kept exactly as written; only the whitespace
//! between steps, brackets and transforms changes.

use serde_json::{Value, json};

use super::text::lines_range;
use super::workspace::{Analysis, LineAt};
use crate::ast::{Atom, GroupMode, Sequence, Step};
use crate::parser::parse_mini;

/// One indentation step inside a broken statement.
const INDENT: &str = "  ";

pub enum Mode {
    Toggle,
    Fit(usize),
}

/// The edits for one request, and why nothing changed when that is useful.
pub fn format(
    analysis: &Analysis,
    mode: Mode,
    line: Option<usize>,
) -> (Vec<Value>, Option<String>) {
    let lines = analysis.lines();
    let targets: Vec<&LineAt> = match line {
        Some(line) => analysis.line_on(line).into_iter().collect(),
        None => analysis.layout.lines.iter().collect(),
    };
    if targets.is_empty() {
        return (
            Vec::new(),
            line.map(|_| "The cursor isn't on a pattern line.".to_string()),
        );
    }
    let mut edits = Vec::new();
    let mut message = None;
    for at in targets {
        let laid = match mode {
            Mode::Toggle => toggle(at),
            Mode::Fit(limit) => fit(at, limit),
        };
        match laid {
            Ok(Some(text)) => edits.push(json!({
                "range": lines_range(&lines, at.line, at.last_line()),
                "newText": text,
            })),
            Ok(None) => {}
            Err(reason) => message = Some(reason),
        }
    }
    // A whole-document pass reports nothing per line: it skips quietly.
    (edits, if line.is_some() { message } else { None })
}

/// Collapse a broken statement, or open a one-line one by a level.
fn toggle(at: &LineAt) -> Result<Option<String>, String> {
    let parts = Parts::of(at)?;
    if at.last_line() > at.line {
        if at.statement.comments {
            return Err("This line has comments inside; joining it would drop them.".into());
        }
        return Ok(Some(parts.one_line()));
    }
    if at.statement.comments {
        return Err("This line ends with a comment; breaking it would move the comment.".into());
    }
    let opened = parts.broken(Some(1), None);
    if opened == parts.one_line() {
        return Err("There's nothing on this line to break.".into());
    }
    Ok(Some(opened))
}

/// Break a one-line statement longer than `limit`, as little as needed.
fn fit(at: &LineAt, limit: usize) -> Result<Option<String>, String> {
    if at.last_line() > at.line || at.statement.comments {
        return Ok(None);
    }
    let parts = Parts::of(at)?;
    let one = parts.one_line();
    if one.chars().count() <= limit {
        return Ok(None);
    }
    // The chain onto its own lines first: often that is enough.
    let chained = parts.chained();
    if chained.lines().all(|line| line.chars().count() <= limit) {
        return Ok(Some(chained));
    }
    Ok(Some(parts.broken(None, Some(limit))))
}

/// A pattern statement taken apart.
struct Parts<'a> {
    indent: &'a str,
    /// `[;] name instrument`, spaced singly.
    head: String,
    /// Between the quotes.
    inner: &'a str,
    steps: Sequence,
    /// `@ 3..`, or empty.
    span: String,
    chain: Vec<String>,
}

impl<'a> Parts<'a> {
    fn of(at: &'a LineAt) -> Result<Self, String> {
        let text = at.statement.text.as_str();
        let indent = &text[..text.len() - text.trim_start().len()];
        let head = text[indent.len()..at.notation.0.saturating_sub(1)]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace("; ", ";")
            .replacen(';', "; ", 1);
        let inner = &text[at.notation.0..at.notation.1];
        let notation = parse_mini(inner)
            .map_err(|error| format!("The notation doesn't parse yet: {error}."))?;
        let rest = text.get(at.notation.1 + 1..).unwrap_or("");
        let mut segments = rest.split('|');
        let span = segments
            .next()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let chain = segments
            .map(|segment| segment.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|segment| !segment.is_empty())
            .collect();
        Ok(Self {
            indent,
            head,
            inner,
            steps: notation.sequence,
            span,
            chain,
        })
    }

    fn tail(&self) -> String {
        if self.span.is_empty() {
            String::new()
        } else {
            format!(" {}", self.span)
        }
    }

    fn chain_inline(&self) -> String {
        self.chain
            .iter()
            .map(|transform| format!(" | {transform}"))
            .collect()
    }

    fn chain_lines(&self) -> Vec<String> {
        self.chain
            .iter()
            .map(|transform| format!("{}{INDENT}| {transform}", self.indent))
            .collect()
    }

    fn one_line(&self) -> String {
        format!(
            "{}{} \"{}\"{}{}",
            self.indent,
            self.head,
            tidy(self.inner),
            self.tail(),
            self.chain_inline()
        )
    }

    fn chained(&self) -> String {
        let mut lines = vec![format!(
            "{}{} \"{}\"{}",
            self.indent,
            self.head,
            tidy(self.inner),
            self.tail()
        )];
        lines.extend(self.chain_lines());
        lines.join("\n")
    }

    /// The notation broken open. `depth` limits how many levels open (the
    /// toggle opens one); `limit` opens only what is too long (fit).
    fn broken(&self, depth: Option<usize>, limit: Option<usize>) -> String {
        let inner_indent = format!("{}{INDENT}", self.indent);
        let mut lines = Vec::new();
        let steps = &self.steps.steps;
        let single = match steps.as_slice() {
            [only] => opening(self.inner, only),
            _ => None,
        };
        match single {
            // One bracketed step: its brackets sit against the quotes.
            Some(open) => {
                lines.push(format!("{}{} \"{}", self.indent, self.head, open.opener));
                for child in &open.children {
                    lines.extend(lay(
                        self.inner,
                        child,
                        &inner_indent,
                        depth.map(|d| d - 1),
                        limit,
                    ));
                }
                lines.push(format!(
                    "{}{}{}\"{}",
                    self.indent,
                    open.closer,
                    open.suffix,
                    self.tail()
                ));
            }
            None if steps.len() > 1 => {
                lines.push(format!("{}{} \"", self.indent, self.head));
                for step in steps {
                    let child = Child::Step(step);
                    lines.extend(lay(
                        self.inner,
                        &child,
                        &inner_indent,
                        depth.map(|d| d - 1),
                        limit,
                    ));
                }
                lines.push(format!("{}\"{}", self.indent, self.tail()));
            }
            None => lines.push(format!(
                "{}{} \"{}\"{}",
                self.indent,
                self.head,
                tidy(self.inner),
                self.tail()
            )),
        }
        lines.extend(self.chain_lines());
        lines.join("\n")
    }
}

/// What goes on a line of a broken bracket: a step, or a whole layer of a
/// chord or random choice with its separator.
enum Child<'s> {
    Step(&'s Step),
    Layer(String),
}

struct Opening<'s> {
    opener: char,
    children: Vec<Child<'s>>,
    closer: char,
    /// Modifiers written after the closing bracket: `*2`, `:0.8`, `(3,8)`.
    suffix: String,
}

/// A bracketed step taken apart, when it is one and its brackets are found.
fn opening<'s>(source: &str, step: &'s Step) -> Option<Opening<'s>> {
    let (opener, layers): (char, Vec<&Sequence>) = match &step.atom {
        Atom::Alternation(alternation) => ('<', vec![&alternation.sequence]),
        Atom::Group(group) => ('[', group.layers.iter().collect()),
        _ => return None,
    };
    let last_end = layers
        .iter()
        .flat_map(|layer| layer.steps.last())
        .map(|inner| inner.source.end as usize)
        .max()?;
    let after = &source[last_end..step.source.end as usize];
    let offset = after.find(|c: char| !c.is_whitespace())?;
    let closer = after[offset..].chars().next()?;
    if closer != if opener == '[' { ']' } else { '>' } {
        return None;
    }
    let suffix = after[offset + 1..].trim().to_string();
    let children = match &step.atom {
        Atom::Group(group) if group.mode != GroupMode::Subdivide => {
            let separator = if group.mode == GroupMode::Chord {
                ","
            } else {
                " |"
            };
            let count = group.layers.len();
            group
                .layers
                .iter()
                .enumerate()
                .map(|(index, layer)| {
                    let text = layer
                        .steps
                        .iter()
                        .map(|inner| tidy(written(source, inner)))
                        .collect::<Vec<_>>()
                        .join(" ");
                    Child::Layer(if index + 1 < count {
                        format!("{text}{separator}")
                    } else {
                        text
                    })
                })
                .collect()
        }
        _ => layers[0].steps.iter().map(Child::Step).collect(),
    };
    Some(Opening {
        opener,
        children,
        closer,
        suffix,
    })
}

fn written<'s>(source: &'s str, step: &Step) -> &'s str {
    &source[step.source.start as usize..step.source.end as usize]
}

/// Lay one child out at `indent`: on one line, or opened when the depth
/// allows it and (with a limit) it does not fit.
fn lay(
    source: &str,
    child: &Child,
    indent: &str,
    depth: Option<usize>,
    limit: Option<usize>,
) -> Vec<String> {
    let step = match child {
        Child::Layer(text) => return vec![format!("{indent}{text}")],
        Child::Step(step) => step,
    };
    let flat = format!("{indent}{}", tidy(written(source, step)));
    let wanted = match (depth, limit) {
        (Some(0), _) => false,
        (_, Some(limit)) => flat.chars().count() > limit,
        (Some(_), None) => true,
        (None, None) => false,
    };
    let Some(open) = wanted.then(|| opening(source, step)).flatten() else {
        return vec![flat];
    };
    let deeper = format!("{indent}{INDENT}");
    let mut lines = vec![format!("{indent}{}", open.opener)];
    for inner in &open.children {
        lines.extend(lay(source, inner, &deeper, depth.map(|d| d - 1), limit));
    }
    lines.push(format!("{indent}{}{}", open.closer, open.suffix));
    lines
}

/// Notation on one line: single spaces between steps, none inside brackets
/// or around commas.
fn tidy(notation: &str) -> String {
    let joined = notation.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(joined.len());
    let chars: Vec<char> = joined.chars().collect();
    for (index, &c) in chars.iter().enumerate() {
        if c == ' ' {
            let before = index.checked_sub(1).map(|i| chars[i]);
            let after = chars.get(index + 1).copied();
            if matches!(before, Some('[' | '<' | ',')) || matches!(after, Some(']' | '>' | ',')) {
                continue;
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::workspace::Layout;

    fn laid(text: &str, mode: Mode) -> Result<Option<String>, String> {
        let layout = Layout::scan(text);
        let at = &layout.lines[0];
        match mode {
            Mode::Toggle => toggle(at),
            Mode::Fit(limit) => fit(at, limit),
        }
    }

    #[test]
    fn toggle_opens_a_bracket_one_child_per_line() {
        let line = "section a 2 {\n  chords pad \"[[0,3,7] [0,4,7]]\" @ 2 | oct -1 | gain 0.8\n}\n";
        let opened = laid(line, Mode::Toggle).unwrap().unwrap();
        assert_eq!(
            opened,
            "  chords pad \"[\n    [0,3,7]\n    [0,4,7]\n  ]\" @ 2\n    | oct -1\n    | gain 0.8"
        );
        let back = format!("section a 2 {{\n{opened}\n}}\n");
        assert_eq!(
            laid(&back, Mode::Toggle).unwrap().unwrap(),
            "  chords pad \"[[0,3,7] [0,4,7]]\" @ 2 | oct -1 | gain 0.8"
        );
    }

    #[test]
    fn toggle_puts_several_steps_one_per_line_and_keeps_modifiers() {
        let line = "section a 1 {\n  ; bass bass \"<[0 ~ 2]*2 [5 4]>:0.8 x\"\n}\n";
        assert_eq!(
            laid(line, Mode::Toggle).unwrap().unwrap(),
            "  ; bass bass \"\n    <[0 ~ 2]*2 [5 4]>:0.8\n    x\n  \""
        );
    }

    #[test]
    fn comments_stop_a_join() {
        let line = "section a 1 {\n  p pad \"<\n    [0,2,4] -- Em\n  >\"\n}\n";
        assert!(laid(line, Mode::Toggle).is_err());
    }

    #[test]
    fn fit_breaks_only_as_much_as_needed() {
        let line = "section a 1 {\n  bass bass \"0 2 4\" @ 13.. | oct -2 | vel 0.5..0.85 | lpf 600..1800 | ramp 4\n}\n";
        assert_eq!(laid(line, Mode::Fit(120)).unwrap(), None, "it fits");
        assert_eq!(
            laid(line, Mode::Fit(40)).unwrap().unwrap(),
            "  bass bass \"0 2 4\" @ 13..\n    | oct -2\n    | vel 0.5..0.85\n    | lpf 600..1800\n    | ramp 4"
        );
        let long =
            "section a 1 {\n  pad pad \"<[0,2,4] [5,7,9] [3,5,7] [0,2,4] [5,7,9] [3,5,7]>\"\n}\n";
        assert_eq!(
            laid(long, Mode::Fit(40)).unwrap().unwrap(),
            "  pad pad \"<\n    [0,2,4]\n    [5,7,9]\n    [3,5,7]\n    [0,2,4]\n    [5,7,9]\n    [3,5,7]\n  >\""
        );
    }

    #[test]
    fn chords_stay_on_one_line() {
        assert_eq!(tidy("[ 0 , 2 ,4 ]  ~"), "[0,2,4] ~");
    }
}
