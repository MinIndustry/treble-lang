//! Help while writing a line: what fits inside the notation, what a
//! transform's arguments can be, which argument is being typed, and what
//! note or chord each step plays.

use serde_json::{Value, json};

use super::language::{DEF_KEYWORDS, TRANSFORMS};
use super::text::column;
use super::theory::{self, Spelled};
use super::workspace::{Analysis, LineAt};
use crate::ast::{Atom, GroupMode, PatternDef, PitchRoot, ScaleMode, Sequence, Transform};
use crate::render::compile::{
    FILTER_ALIASES, default_root, is_engine_owned, mode_intervals, param_name,
};

const KIND_VALUE: u32 = 12;
const KIND_KEYWORD: u32 = 14;
const KIND_SNIPPET: u32 = 15;
const KIND_OPERATOR: u32 = 24;
const KIND_PROPERTY: u32 = 10;

/// A pattern line, what it says, and the scale and octave shift its degrees
/// are read in.
struct Context<'a> {
    line: &'a PatternDef,
    root: PitchRoot,
    mode: ScaleMode,
    octaves: i32,
}

fn context<'a>(analysis: &'a Analysis, index: usize) -> Option<Context<'a>> {
    let at = analysis.line_on(index)?;
    let section_name = at.section.as_deref()?;
    let section = analysis
        .piece
        .as_ref()?
        .sections
        .iter()
        .find(|section| section.name == section_name)?;
    let line = section
        .patterns
        .iter()
        .find(|line| line.name == at.name_text)?;
    Some(context_of(line, section.scale))
}

fn context_of<'a>(line: &'a PatternDef, scale: Option<(PitchRoot, ScaleMode)>) -> Context<'a> {
    let (mut root, mut mode) = scale.unwrap_or((default_root(), ScaleMode::Major));
    let mut octaves = 0;
    for transform in &line.transforms {
        match transform {
            Transform::Scale(r, m) => (root, mode) = (*r, *m),
            Transform::Oct(ramp) => octaves = ramp.start(),
            _ => {}
        }
    }
    Context {
        line,
        root,
        mode,
        octaves,
    }
}

fn mode_name(root: PitchRoot, mode: ScaleMode) -> String {
    super::language::scale_name(Some((root, mode))).unwrap_or_default()
}

// ---- inside the notation

/// What fits at the cursor inside a pattern's quotes, if the cursor is there.
pub fn notation_items(
    analysis: &Analysis,
    at: &LineAt,
    offset: usize,
    lines: &[&str],
) -> Option<Vec<Value>> {
    let context = context(analysis, at.line)?;
    let start = at.notation.0;
    if offset < start || offset > at.notation.1 {
        return None;
    }
    let inner = &at.statement.text[start..offset];
    let token_at = inner
        .rfind(|c: char| c.is_whitespace() || "[<,|".contains(c))
        .map_or(0, |at| at + 1);
    let token = &inner[token_at..];
    let opener = inner[..token_at].chars().last();
    if token.ends_with([']', '>', 'x', 'X', '~']) {
        return Some(modifier_items(&at.range(lines, offset, offset)));
    }
    if !token.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return Some(Vec::new());
    }
    let edit = at.range(lines, start + token_at, offset);
    Some(step_items(&context, &edit, opener == Some('[')))
}

fn item(label: &str, kind: u32, detail: &str, doc: &str, edit: &Value, sort: usize) -> Value {
    json!({
        "label": label,
        "kind": kind,
        "detail": detail,
        "documentation": {"kind": "markdown", "value": doc},
        "textEdit": {"range": edit, "newText": label},
        "filterText": label,
        "sortText": format!("{sort:04}"),
    })
}

fn step_items(context: &Context, edit: &Value, inside_group: bool) -> Vec<Value> {
    let size = mode_intervals(context.mode).len() as i32;
    let scale = mode_name(context.root, context.mode);
    let mut items = Vec::new();
    // Two octaves up from the root first, then a few below it.
    let degrees: Vec<i32> = (0..2 * size).chain(-(size / 2).max(1)..0).collect();
    for (order, degree) in degrees.into_iter().enumerate() {
        let note = theory::degree(degree, context.root, context.mode, context.octaves);
        items.push(item(
            &degree.to_string(),
            KIND_VALUE,
            &note.name_with_octave(),
            &format!("Degree {degree} of {scale}: {}.", note.name_with_octave()),
            edit,
            order,
        ));
    }
    // A chord on each degree of the first octave: triads, then sevenths.
    for (shape, offset) in [(&[0, 2, 4][..], 100), (&[0, 2, 4, 6][..], 200)] {
        for base in 0..size.min(7) {
            let degrees: Vec<i32> = shape.iter().map(|step| base + step).collect();
            let notes: Vec<Spelled> = degrees
                .iter()
                .map(|&d| theory::degree(d, context.root, context.mode, context.octaves))
                .collect();
            let Some(name) = theory::chord(&notes) else {
                continue;
            };
            let list = degrees
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let label = if inside_group {
                list
            } else {
                format!("[{list}]")
            };
            let spelled = notes
                .iter()
                .map(Spelled::name)
                .collect::<Vec<_>>()
                .join(" ");
            items.push(item(
                &label,
                KIND_SNIPPET,
                &name,
                &format!("{name} in {scale}: {spelled}."),
                edit,
                offset + base as usize,
            ));
        }
    }
    if !inside_group {
        for (order, (symbol, what)) in [
            ("~", "A rest for this step."),
            ("_", "Hold the previous note through this step."),
            ("x", "A hit, at the line's `vel`."),
            ("X", "An accented hit, at full velocity."),
        ]
        .into_iter()
        .enumerate()
        {
            items.push(item(symbol, KIND_OPERATOR, what, what, edit, 300 + order));
        }
    }
    items
}

fn modifier_items(edit: &Value) -> Vec<Value> {
    [
        (
            ":0.8",
            "velocity",
            "This step's own velocity, 0–1; beats the line's `vel`.",
        ),
        (
            "*2",
            "repeat",
            "Repeat within the step's time: `x*4` is four hits in one step.",
        ),
        (
            "!2",
            "replicate",
            "Copy into separate steps: `c4!3 e4` is four equal steps.",
        ),
        (
            "(3,8)",
            "euclid",
            "Spread 3 onsets over 8 positions as evenly as possible.",
        ),
        (
            "?",
            "drop",
            "A 50% chance of silence each cycle; `?0.25` sets the chance.",
        ),
        (
            "@2",
            "weight",
            "Make this step twice as long as the others.",
        ),
        ("/2", "slow", "Stretch this step over 2 cycles."),
    ]
    .iter()
    .enumerate()
    .map(|(order, (label, detail, doc))| item(label, KIND_OPERATOR, detail, doc, edit, order))
    .collect()
}

// ---- transform arguments

const WAVES: &[(&str, &str)] = &[
    ("sine", "Starts centred, moves right, smooth."),
    ("tri", "Starts centred, moves right, straight lines."),
    ("sq", "Alternates hard between the sides."),
    ("saw", "Ramps left to right, then jumps back."),
    ("rand", "Holds a random position each period."),
];
const ARP_MODES: &[&str] = &["up", "down", "updown", "random"];
const ROOTS: &[&str] = &[
    "C", "C#", "Db", "D", "Eb", "E", "F", "F#", "Gb", "G", "Ab", "A", "Bb", "B",
];
const MODES: &[&str] = &[
    "major",
    "minor",
    "dorian",
    "phrygian",
    "lydian",
    "mixolydian",
    "aeolian",
    "locrian",
    "pentatonic",
    "blues",
    "chromatic",
];
const CURVES: &[(&str, &str)] = &[
    ("lin", "Equal steps in value. The default."),
    (
        "exp",
        "Equal steps in ratio: a filter that opens evenly to the ear.",
    ),
    (
        "osc",
        "Go back and forth between the ends, once per span, forever.",
    ),
];
const EVENT_TRANSFORMS: &[&str] = &["rev", "fast", "slow", "arp", "scale", "oct", "vel"];

fn simple(label: &str, kind: u32, detail: &str) -> Value {
    json!({"label": label, "kind": kind, "detail": detail, "insertText": format!("{label} ")})
}

/// The words of the transform being written after the last `|`, and whether
/// the cursor sits after a space (starting a new word).
fn transform_words(before: &str) -> Option<(Vec<&str>, bool)> {
    if before.matches('"').count() % 2 == 1 {
        return None;
    }
    let segment = &before[before.rfind('|')? + 1..];
    let words: Vec<&str> = segment.split_whitespace().collect();
    let blank = segment.ends_with(char::is_whitespace);
    Some((words, blank))
}

/// Completion for a transform's arguments, when the cursor is in them.
pub fn argument_items(before: &str) -> Option<Vec<Value>> {
    let (words, blank) = transform_words(before)?;
    let name = *words.first()?;
    // How many arguments are complete before the one being typed.
    let done = if blank {
        words.len() - 1
    } else {
        words.len().checked_sub(2)?
    };
    let items = match (name, done) {
        ("pan", 0) => WAVES
            .iter()
            .map(|(wave, what)| simple(wave, KIND_KEYWORD, what))
            .collect(),
        ("arp", 0) => ARP_MODES
            .iter()
            .map(|mode| simple(mode, KIND_KEYWORD, "arpeggio"))
            .collect(),
        ("scale", 0) => ROOTS
            .iter()
            .map(|root| simple(root, KIND_VALUE, "root"))
            .collect(),
        ("scale", 1) => MODES
            .iter()
            .map(|mode| simple(mode, KIND_KEYWORD, "mode"))
            .collect(),
        ("ramp", 1) => CURVES
            .iter()
            .map(|(curve, what)| simple(curve, KIND_KEYWORD, what))
            .collect(),
        ("every", 1) => EVENT_TRANSFORMS
            .iter()
            .map(|transform| simple(transform, KIND_KEYWORD, "event transform"))
            .collect(),
        ("fx", 0) => filter_items(),
        ("fx", _) => parameter_items(words[1]),
        _ => return None,
    };
    Some(items)
}

fn filter_items() -> Vec<Value> {
    let mut items: Vec<Value> = FILTER_ALIASES
        .iter()
        .map(|alias| simple(alias.keyword, KIND_KEYWORD, alias.type_id))
        .collect();
    let mut filters = treble::meta::get_filters();
    filters.sort_by_key(|info| info.type_id);
    for info in filters {
        items.push(json!({
            "label": info.type_id,
            "kind": KIND_KEYWORD,
            "detail": info.description,
            "insertText": format!("{} ", info.type_id),
        }));
    }
    items
}

/// A filter's own parameters, by alias or name: `(name, range)`.
fn filter_parameters(filter: &str) -> Option<(String, Vec<(String, String)>)> {
    let alias = FILTER_ALIASES.iter().find(|alias| alias.keyword == filter);
    let wanted = alias.map_or(filter, |alias| alias.type_id);
    let info = treble::meta::get_filters().into_iter().find(|info| {
        info.type_id.eq_ignore_ascii_case(wanted) || info.name.eq_ignore_ascii_case(wanted)
    })?;
    let parameters = info
        .inputs
        .iter()
        .filter_map(|input| input.parameter.clone())
        .filter(|parameter| !is_engine_owned(param_name(parameter)))
        .map(|parameter| {
            use treble::meta::Parameter;
            let range = match &parameter {
                Parameter::Range {
                    min, max, default, ..
                } => format!("{min}–{max}, default {default}"),
                Parameter::Int {
                    min, max, default, ..
                } => match (min, max) {
                    (Some(min), Some(max)) => format!("{min}–{max}, default {default}"),
                    _ => format!("whole number, default {default}"),
                },
                Parameter::Float { default, .. } => format!("number, default {default}"),
                Parameter::Toggle { default, .. } => {
                    format!("0 or 1, default {}", u8::from(*default))
                }
                Parameter::List { .. } => "list".to_string(),
            };
            (param_name(&parameter).to_string(), range)
        })
        .collect();
    Some((info.type_id.to_string(), parameters))
}

fn parameter_items(filter: &str) -> Vec<Value> {
    let Some((_, parameters)) = filter_parameters(filter) else {
        return Vec::new();
    };
    parameters
        .into_iter()
        .map(|(name, range)| {
            json!({
                "label": format!("{name}="),
                "kind": KIND_PROPERTY,
                "detail": range,
                "insertText": format!("{name}="),
            })
        })
        .collect()
}

// ---- signature help

/// The signature of the transform (or `def` line) being written, with the
/// argument under the cursor.
pub fn signature_help(before: &str, library: bool) -> Value {
    let (label, arguments, doc, done) = if library {
        let mut words = before.split_whitespace();
        let Some(name) = words.next() else {
            return Value::Null;
        };
        let Some((_, arguments, doc)) =
            DEF_KEYWORDS.iter().find(|(keyword, _, _)| *keyword == name)
        else {
            return Value::Null;
        };
        let typed = before.split_whitespace().count();
        let blank = before.ends_with(char::is_whitespace);
        if typed == 1 && !blank {
            return Value::Null;
        }
        let done = if blank { typed - 1 } else { typed - 2 };
        (
            name.to_string(),
            split_arguments(arguments),
            doc.to_string(),
            done,
        )
    } else {
        let Some((words, blank)) = transform_words(before) else {
            return Value::Null;
        };
        let Some(&name) = words.first() else {
            return Value::Null;
        };
        if words.len() == 1 && !blank {
            return Value::Null;
        }
        let done = if blank {
            words.len() - 1
        } else {
            words.len() - 2
        };
        if name == "fx" && words.len() >= 2 && (words.len() > 2 || blank) {
            let Some((filter, parameters)) = filter_parameters(words[1]) else {
                return Value::Null;
            };
            let arguments = parameters
                .iter()
                .map(|(name, range)| format!("{name}=<{}>", range.split(',').next().unwrap_or("")))
                .collect();
            (
                format!("fx {filter}"),
                arguments,
                "Positional values fill these in order; `name=value` sets one.".to_string(),
                done - 1,
            )
        } else {
            let Some((_, arguments, doc)) = TRANSFORMS
                .iter()
                .find(|(transform, _, _)| *transform == name)
            else {
                return Value::Null;
            };
            if arguments.is_empty() {
                return Value::Null;
            }
            (
                name.to_string(),
                split_arguments(arguments),
                doc.to_string(),
                done,
            )
        }
    };
    let mut text = label;
    let mut parameters = Vec::new();
    for argument in &arguments {
        text.push(' ');
        let start = column(&text, text.len());
        text.push_str(argument);
        parameters.push(json!({"label": [start, column(&text, text.len())]}));
    }
    json!({
        "signatures": [{
            "label": text,
            "documentation": {"kind": "markdown", "value": doc},
            "parameters": parameters,
        }],
        "activeSignature": 0,
        "activeParameter": done.min(arguments.len().saturating_sub(1)),
    })
}

/// `<s> <feedback> [mix]` into its arguments, keeping `<a | b>` together.
fn split_arguments(arguments: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    for c in arguments.chars() {
        match c {
            '<' | '[' | '{' => depth += 1,
            '>' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if c == ' ' && depth == 0 {
            if !current.is_empty() && current != "|" {
                parts.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() && current != "|" {
        parts.push(current);
    }
    parts
}

// ---- inlay hints

/// The note each degree plays and the name of each chord, for pattern lines
/// between `from` and `to` (inclusive, 0-based).
pub fn inlay_hints(analysis: &Analysis, from: usize, to: usize) -> Value {
    let Some(piece) = &analysis.piece else {
        return json!([]);
    };
    let lines = analysis.lines();
    let mut hints = Vec::new();
    for section in &piece.sections {
        for line in &section.patterns {
            let Some(at) = analysis.line_at(&section.name, &line.name) else {
                continue;
            };
            if at.last_line() < from || at.line > to {
                continue;
            }
            let context = context_of(line, section.scale);
            let mut found = Vec::new();
            walk(&context.line.notation.sequence, &context, &mut found);
            for (end, label) in found {
                let position = at.range(&lines, at.notation.0 + end as usize, 0)["start"].clone();
                hints.push(json!({
                    "position": position,
                    "label": label,
                    "kind": 1,
                    "paddingLeft": true,
                }));
            }
        }
    }
    json!(hints)
}

fn walk(sequence: &Sequence, context: &Context, found: &mut Vec<(u32, String)>) {
    for step in &sequence.steps {
        match &step.atom {
            Atom::Degree(degree) => {
                let note = theory::degree(*degree, context.root, context.mode, context.octaves);
                found.push((step.source.end, note.name_with_octave()));
            }
            Atom::Group(group) if group.mode == GroupMode::Chord => {
                let notes: Vec<Spelled> = group
                    .layers
                    .iter()
                    .flat_map(|layer| &layer.steps)
                    .filter_map(|inner| match &inner.atom {
                        Atom::Degree(degree) => Some(theory::degree(
                            *degree,
                            context.root,
                            context.mode,
                            context.octaves,
                        )),
                        Atom::Note(note) => Some(theory::written(note)),
                        _ => None,
                    })
                    .collect();
                if let Some(name) = theory::chord(&notes) {
                    found.push((step.source.end, name));
                }
            }
            Atom::Group(group) => {
                for layer in &group.layers {
                    walk(layer, context, found);
                }
            }
            Atom::Alternation(alternation) => walk(&alternation.sequence, context, found),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_split_on_spaces_outside_brackets() {
        assert_eq!(
            split_arguments("<s> <feedback> [mix]"),
            ["<s>", "<feedback>", "[mix]"]
        );
        assert_eq!(
            split_arguments("<-1–1> | <wave> <rate> [depth]"),
            ["<-1–1>", "<wave>", "<rate>", "[depth]"]
        );
    }

    #[test]
    fn signature_help_tracks_the_argument_being_typed() {
        let help = signature_help("  bass sine \"0\" | delay 0.65 ", false);
        assert_eq!(help["signatures"][0]["label"], "delay <s> <feedback> [mix]");
        assert_eq!(help["activeParameter"], 1);
        assert!(signature_help("  bass sine \"0\" | del", false).is_null());
        assert!(
            signature_help("  bass sine \"0 | x", false).is_null(),
            "inside the quotes"
        );
    }

    #[test]
    fn arguments_complete_by_transform() {
        let labels = |before: &str| -> Vec<String> {
            argument_items(before)
                .unwrap_or_default()
                .iter()
                .map(|item| item["label"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(labels("a sine \"0\" | pan ")[0], "sine");
        assert!(labels("a sine \"0\" | scale E ").contains(&"phrygian".to_string()));
        assert_eq!(labels("a sine \"0\" | ramp 16 "), ["lin", "exp", "osc"]);
        assert!(argument_items("a sine \"0\" | lpf ").is_none());
    }
}
