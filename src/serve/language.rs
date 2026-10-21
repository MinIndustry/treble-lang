//! The read-only half of the server: what a piece contains and where, what
//! fits at the cursor, and what an instrument name refers to.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::text::{byte_at, comment_at, lines_range, range, split_lines};
use super::workspace::{Analysis, KEYWORDS};
use crate::ast::{Accidental, NoteLetter, PitchRoot, ScaleMode};
use crate::render::section_strikes;

/// The built-in instruments (LANGUAGE.md §5), for completion and hover.
const BUILT_IN: &[(&str, &str)] = &[
    ("kick", "Bass drum"),
    ("snare", "Snare drum"),
    ("hihat", "Closed hi-hat"),
    ("clap", "Handclap"),
    ("rim", "Rimshot"),
    ("tom", "Tom-tom"),
    ("sine", "Pure sine wave"),
    ("saw", "Sawtooth wave"),
    ("square", "Square wave"),
    ("triangle", "Triangle wave"),
    ("piano", "Piano-like: saw, ADSR, low-pass"),
    ("bass", "Bass synth: square and sub-oscillator"),
    ("pad", "Detuned saws, slow ADSR"),
    ("pluck", "Plucked string, short envelope"),
    ("bell", "Additive harmonics"),
];

/// The transforms (LANGUAGE.md §4.3): name, arguments, what it does.
pub(super) const TRANSFORMS: &[(&str, &str, &str)] = &[
    (
        "vel",
        "<0–1 | a..b>",
        "How hard each step is struck. Steps with `X` or `:v` keep their own.",
    ),
    (
        "gain",
        "<0–2 | a..b>",
        "Level of the signal leaving the instrument.",
    ),
    (
        "pan",
        "<-1–1> | <wave> <rate> [depth]",
        "A fixed position, or swept by sine, tri, sq, saw or rand. The rate is cycles per sweep; `hz` makes it a frequency.",
    ),
    ("oct", "<±n>", "Shift by whole octaves."),
    (
        "ramp",
        "<cycles> [lin|exp]",
        "How long the line's ranges take to travel, from where the line starts.",
    ),
    ("lpf", "<Hz | a..b>", "Low-pass filter."),
    ("hpf", "<Hz | a..b>", "High-pass filter."),
    (
        "delay",
        "<s> <feedback> [mix]",
        "Delay: time in seconds, feedback 0–0.99, wet mix (default 0.35).",
    ),
    ("reverb", "<0–1>", "Reverb mix."),
    ("rev", "", "Reverse the pattern."),
    ("fast", "<n>", "Speed up by n."),
    ("slow", "<n>", "Slow down by n."),
    (
        "every",
        "<n> <transform>",
        "Apply an event transform every nth cycle.",
    ),
    ("arp", "<up|down|updown|random>", "Arpeggiate chords."),
    ("scale", "<root> <mode>", "Quantise this line to a scale."),
    ("fx", "<Filter> <args…>", "Any engine filter by name."),
];

/// What a line inside a section can start with besides a pattern.
const SECTION_KEYWORDS: &[(&str, &str, &str)] = &[
    (
        "group",
        "<name> {",
        "Route several lines through one shared chain.",
    ),
    ("bpm", "<n>", "Tempo for this section only."),
    ("sig", "<n>/<d>", "Time signature for this section only."),
    ("scale", "<root> <mode>", "Scale for this section only."),
];

/// What a line outside every section can start with.
const TOP_KEYWORDS: &[(&str, &str, &str)] = &[
    (
        "section",
        "<name> <cycles> {",
        "A named part of the piece with a stated length.",
    ),
    ("bpm", "<n>", "Tempo."),
    ("sig", "<n>/<d>", "Time signature."),
    (
        "scale",
        "<root> <mode>",
        "The scale degrees are counted in.",
    ),
    ("seed", "<n>", "Salt for the random choices."),
    (
        "tail",
        "<seconds>",
        "How long a render rings out after the end.",
    ),
    ("meta", "<key> \"<value>\"", "A tag written into the WAV."),
    (
        "load",
        "\"<file.trbl>\"",
        "An instrument library beside this file.",
    ),
    (
        "arrange",
        "<section>…",
        "The playing order. Without it, source order.",
    ),
];

/// What a line inside a `def` block can start with (LANGUAGE.md §6).
pub(super) const DEF_KEYWORDS: &[(&str, &str, &str)] = &[
    (
        "voice",
        "mono | poly <n> <steal>",
        "Polyphony and voice stealing.",
    ),
    (
        "lifecycle",
        "gated | oneshot | cutoff",
        "Whether a note holds until released.",
    ),
    (
        "tone",
        "<wave> gain <g> | { env … }",
        "One partial. It takes a gain or an envelope block, never both.",
    ),
    ("mix", "sum", "How the tones combine."),
    ("env", "adsr <a> <d> <s> <r>", "The amplitude envelope."),
    ("pitchenv", "segment …", "The pitch envelope."),
    (
        "fx",
        "<Filter> k=v…",
        "An engine filter in the instrument's chain.",
    ),
    ("lpf", "<Hz>", "Low-pass filter."),
    ("hpf", "<Hz>", "High-pass filter."),
    (
        "comp",
        "<threshold> <ratio> <attack> <release>",
        "Compressor.",
    ),
    ("trem", "<rate> <depth>", "Tremolo."),
    ("reverb", "<mix>", "Reverb."),
    ("gain", "<g>", "Output level."),
    ("velsens", "<0–1>", "How much velocity changes the level."),
];

const KIND_FUNCTION: u32 = 3;
const KIND_VARIABLE: u32 = 6;
const KIND_CLASS: u32 = 7;
const KIND_KEYWORD: u32 = 14;
const KIND_SNIPPET: u32 = 15;

pub fn scale_name(scale: Option<(PitchRoot, ScaleMode)>) -> Option<String> {
    let (root, mode) = scale?;
    let letter = match root.name {
        NoteLetter::C => "C",
        NoteLetter::D => "D",
        NoteLetter::E => "E",
        NoteLetter::F => "F",
        NoteLetter::G => "G",
        NoteLetter::A => "A",
        NoteLetter::B => "B",
    };
    let accidental = match root.accidental {
        Accidental::Natural => "",
        Accidental::Sharp => "#",
        Accidental::DoubleSharp => "##",
        Accidental::Flat => "b",
        Accidental::DoubleFlat => "bb",
    };
    Some(format!(
        "{letter}{accidental} {}",
        format!("{mode:?}").to_lowercase()
    ))
}

/// `treble/outline`.
pub fn outline(analysis: &Analysis, stale: bool, uri_of: impl Fn(&PathBuf) -> String) -> Value {
    let lines = analysis.lines();
    let Some(piece) = &analysis.piece else {
        return json!({"stale": stale, "meta": {}, "top": {}, "loads": [], "sections": []});
    };
    let meta: serde_json::Map<String, Value> = piece
        .metadata
        .iter()
        .map(|(key, value)| (key.clone(), json!(value)))
        .collect();
    let top = &analysis.layout.top;
    let sig: Option<Vec<u32>> = top
        .get("sig")
        .and_then(|sig| sig.split_once('/'))
        .and_then(|(n, d)| Some(vec![n.trim().parse().ok()?, d.trim().parse().ok()?]));
    let sections: Vec<Value> = piece
        .sections
        .iter()
        .map(|section| {
            let at = analysis
                .layout
                .sections
                .iter()
                .find(|at| at.name == section.name);
            let members: Vec<Value> = section
                .patterns
                .iter()
                .map(|line| {
                    let written = analysis.line_at(&section.name, &line.name);
                    let ranges = written.map(|at| {
                        (
                            range(&lines, at.line, at.name.0, at.name.1),
                            range(&lines, at.line, at.instrument.0, at.instrument.1),
                            at.range(&lines, at.notation.0, at.notation.1),
                            at.whole(&lines),
                        )
                    });
                    // What silences the line, if anything, innermost first.
                    let group_muted = line.group.as_ref().is_some_and(|name| {
                        section
                            .groups
                            .iter()
                            .any(|group| &group.name == name && group.muted)
                    });
                    let muted_by = if line.muted {
                        Some("line")
                    } else if group_muted {
                        Some("group")
                    } else if section.muted {
                        Some("section")
                    } else {
                        None
                    };
                    json!({
                        "name": line.name,
                        "instrument": line.instrument,
                        "muted": line.muted,
                        "mutedBy": muted_by,
                        "group": line.group,
                        "span": line.span.map(|span| json!({"from": span.from, "to": span.to})),
                        "range": ranges.as_ref().map(|r| r.3.clone()),
                        "nameRange": ranges.as_ref().map(|r| r.0.clone()),
                        "instrumentRange": ranges.as_ref().map(|r| r.1.clone()),
                        "notationRange": ranges.as_ref().map(|r| r.2.clone()),
                    })
                })
                .collect();
            json!({
                "name": section.name,
                "muted": section.muted,
                "range": at.map(|at| lines_range(&lines, at.header, at.end)),
                "cycles": section.cycles,
                "bpm": section.bpm,
                "sig": [section.sig.0, section.sig.1],
                "scale": scale_name(section.scale),
                "cycleSeconds": section.cycle_seconds(),
                "seconds": section.seconds(),
                "lines": members,
                "groups": section.groups.iter().map(|group| json!({
                    "name": group.name,
                    "muted": group.muted,
                    "chain": crate::render::compile::describe_audio_chain(&group.transforms),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "stale": stale,
        "meta": meta,
        "top": {
            "bpm": top.get("bpm").and_then(|bpm| bpm.parse::<u32>().ok()),
            "sig": sig,
            "scale": top.get("scale"),
        },
        "loads": analysis.loads.iter().map(|load| json!({
            "file": load.file,
            "uri": uri_of(&load.path),
            "defs": load.defs,
        })).collect::<Vec<_>>(),
        "sections": sections,
    })
}

/// `treble/timeline`: the notes of some lines of one section, as the renderer
/// schedules them.
pub fn timeline(
    analysis: &Analysis,
    section: &str,
    wanted: Option<&[String]>,
) -> Result<Value, String> {
    let piece = analysis
        .piece
        .as_ref()
        .ok_or("This piece doesn't parse yet, so it has no notes to show.")?;
    let index = piece
        .sections
        .iter()
        .position(|placed| placed.name == section)
        .ok_or_else(|| format!("There's no section named {section}."))?;
    let placed = &piece.sections[index];
    // A line on an unknown instrument cannot be compiled. It still gets its
    // lane, empty and marked, rather than vanishing or taking the whole
    // picture down with one typo.
    let wanted_lines: Vec<&crate::PatternDef> = placed
        .patterns
        .iter()
        .filter(|line| wanted.is_none_or(|wanted| wanted.contains(&line.name)))
        .collect();
    let names: Vec<String> = placed
        .patterns
        .iter()
        .filter(|line| wanted.is_none_or(|wanted| wanted.contains(&line.name)))
        .filter(|line| analysis.registry.get(&line.instrument).is_some())
        .map(|line| line.name.clone())
        .collect();
    let lanes = section_strikes(piece, &analysis.registry, index, Some(&names))?;
    let lines = analysis.lines();
    let cycle = placed.cycle_seconds();
    let mut compiled = lanes.into_iter();
    let lanes: Vec<Value> = wanted_lines
        .iter()
        .map(|line| {
            if analysis.registry.get(&line.instrument).is_none() {
                return json!({
                    "line": line.name,
                    "instrument": line.instrument,
                    "events": [],
                    "automations": [],
                    "undefined": true,
                });
            }
            let lane = compiled.next().expect("a lane per compiled line, in order");
            let written = analysis.line_at(section, &lane.line);
            let mut events = Vec::new();
            for (within, strike) in &lane.strikes {
                let source = written.map(|at| {
                    let start = at.notation.0 + strike.source.start as usize;
                    let end = at.notation.0 + strike.source.end as usize;
                    at.range(&lines, start, end)
                });
                let start = (f64::from(*within) + strike.start) * cycle;
                let length = (strike.end - strike.start) * cycle;
                let pitches: Vec<Option<u8>> = if strike.trigger {
                    vec![None]
                } else {
                    strike.notes.iter().copied().map(Some).collect()
                };
                for midi in pitches {
                    events.push(json!({
                        "t": start,
                        "d": length,
                        "midi": midi,
                        "velocity": strike.velocity,
                        "cycle": within,
                        "source": source,
                    }));
                }
            }
            let automations: Vec<Value> = lane
                .automations
                .iter()
                .map(|automation| {
                    json!({
                        "label": automation.label,
                        "unit": automation.unit,
                        "low": automation.low,
                        "high": automation.high,
                        "points": automation.points.iter().map(|(within, value)| {
                            json!({"t": within * cycle, "v": value})
                        }).collect::<Vec<_>>(),
                    })
                })
                .collect();
            json!({
                "line": lane.line,
                "instrument": lane.instrument,
                "events": events,
                "automations": automations,
            })
        })
        .collect();
    Ok(json!({
        "section": placed.name,
        "cycles": placed.cycles,
        "cycleSeconds": cycle,
        "divisions": placed.sig.0,
        "lanes": lanes,
    }))
}

/// `textDocument/documentSymbol`: sections, with their lines inside.
pub fn symbols(analysis: &Analysis) -> Value {
    let lines = analysis.lines();
    let symbols: Vec<Value> = analysis
        .layout
        .sections
        .iter()
        .map(|section| {
            let detail = analysis
                .piece
                .as_ref()
                .and_then(|piece| piece.sections.iter().find(|placed| placed.name == section.name))
                .map(|placed| format!("{} bars · {:.1} s", placed.cycles, placed.seconds()))
                .unwrap_or_default();
            let children: Vec<Value> = analysis
                .layout
                .lines
                .iter()
                .filter(|line| line.section.as_deref() == Some(section.name.as_str()))
                .map(|line| {
                    let name = range(&lines, line.line, line.name.0, line.name.1);
                    let instrument = &lines[line.line][line.instrument.0..line.instrument.1];
                    json!({
                        "name": line.name_text,
                        "detail": instrument,
                        "kind": 13,
                        "range": line.whole(&lines),
                        "selectionRange": name,
                    })
                })
                .collect();
            let header = &lines[section.header];
            let name_start = header.find(&section.name).unwrap_or(0);
            json!({
                "name": section.name,
                "detail": detail,
                "kind": 3,
                "range": lines_range(&lines, section.header, section.end),
                "selectionRange": range(&lines, section.header, name_start, name_start + section.name.len()),
                "children": children,
            })
        })
        .collect();
    json!(symbols)
}

/// The instrument named at a position in a piece, if the cursor is on one.
pub fn instrument_at(analysis: &Analysis, line: usize, character: u32) -> Option<String> {
    let lines = analysis.lines();
    let text = lines.get(line)?;
    let byte = byte_at(text, character);
    let at = analysis.line_on(line).filter(|at| at.line == line)?;
    (at.instrument.0..=at.instrument.1)
        .contains(&byte)
        .then(|| text[at.instrument.0..at.instrument.1].to_string())
}

/// `textDocument/hover` over an instrument name.
pub fn hover(analysis: &Analysis, line: usize, character: u32) -> Value {
    let Some(name) = instrument_at(analysis, line, character) else {
        return Value::Null;
    };
    let markdown = match analysis.defs.get(&name) {
        Some(site) => {
            let file = site
                .path
                .file_name()
                .map_or_else(String::new, |file| file.to_string_lossy().into_owned());
            format!(
                "**{name}** · {}\n\n```treble\n{}\n```\n\n{file}:{}",
                site.summary,
                site.text,
                site.line + 1
            )
        }
        None => match BUILT_IN.iter().find(|(built_in, _)| *built_in == name) {
            Some((_, description)) => format!("**{name}** · built-in\n\n{description}."),
            None => format!("**{name}** isn't defined anywhere this piece can see."),
        },
    };
    json!({"contents": {"kind": "markdown", "value": markdown}})
}

/// `textDocument/definition` over an instrument name: its file, line, and
/// the header line's text (for the range's end).
pub fn definition(
    analysis: &Analysis,
    line: usize,
    character: u32,
) -> Option<(PathBuf, usize, String)> {
    let name = instrument_at(analysis, line, character)?;
    let site = analysis.defs.get(&name)?;
    Some((
        site.path.clone(),
        site.line,
        site.text.lines().next()?.to_string(),
    ))
}

/// `textDocument/completion`.
///
/// `analysis` is the piece the text belongs to; `library` says the cursor is
/// in a `.trbl`, where only `def` keywords fit.
pub fn completion(
    analysis: Option<&Analysis>,
    text: &str,
    library: bool,
    line: usize,
    character: u32,
) -> Value {
    let lines = split_lines(text);
    let current = lines.get(line).copied().unwrap_or("");
    let byte = byte_at(current, character);
    let before = &current[..byte];
    if comment_at(before) < before.len() {
        return json!({"isIncomplete": false, "items": []});
    }
    let empty = json!({"isIncomplete": false, "items": []});
    // On a pattern statement, which may span lines, the statement decides:
    // inside its quotes the notation, after them its chain.
    if let Some(analysis) = analysis.filter(|_| !library)
        && let Some(at) = analysis.line_on(line)
        && let Some(offset) = at.statement.offset_of(line, byte)
    {
        if (at.notation.0..=at.notation.1).contains(&offset) {
            let items =
                super::assist::notation_items(analysis, at, offset, &lines).unwrap_or_default();
            return json!({"isIncomplete": false, "items": items});
        }
        if offset > at.notation.1 {
            let prefix = &at.statement.text[..offset];
            let items = super::assist::argument_items(prefix)
                .unwrap_or_else(|| piece_items(Some(analysis), prefix, line));
            return json!({"isIncomplete": false, "items": items});
        }
    }
    if before.matches('"').count() % 2 == 1 {
        return empty;
    }
    if !library && let Some(items) = super::assist::argument_items(before) {
        return json!({"isIncomplete": false, "items": items});
    }
    let items = if library {
        def_items(before)
    } else {
        piece_items(analysis, before, line)
    };
    json!({"isIncomplete": false, "items": items})
}

fn keyword_items(table: &[(&str, &str, &str)]) -> Vec<Value> {
    table
        .iter()
        .map(|(name, arguments, what)| {
            json!({
                "label": name,
                "kind": KIND_KEYWORD,
                "detail": arguments,
                "documentation": {"kind": "markdown", "value": format!("`{name} {arguments}`. {what}")},
                "insertText": format!("{name} "),
            })
        })
        .collect()
}

fn def_items(before: &str) -> Vec<Value> {
    let word = before.trim_start();
    let indented = before.len() > word.len();
    if indented && is_ident(word) {
        keyword_items(DEF_KEYWORDS)
    } else {
        Vec::new()
    }
}

fn is_ident(word: &str) -> bool {
    word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn piece_items(analysis: Option<&Analysis>, before: &str, line: usize) -> Vec<Value> {
    // After a pipe: a transform.
    if let Some(pipe) = before.rfind('|') {
        let word = before[pipe + 1..].trim_start();
        if is_ident(word) {
            return TRANSFORMS
                .iter()
                .map(|(name, arguments, what)| {
                    json!({
                        "label": name,
                        "kind": KIND_FUNCTION,
                        "detail": arguments,
                        "documentation": {"kind": "markdown", "value": format!("`{name} {arguments}`. {what}")},
                        "insertText": format!("{name} "),
                    })
                })
                .collect();
        }
    }
    let Some(analysis) = analysis else {
        return Vec::new();
    };
    let section = analysis
        .section_at(line)
        .filter(|section| section.header != line);
    let body = before.trim_start().trim_start_matches(';').trim_start();
    let words: Vec<&str> = body.split_whitespace().collect();
    let ends_blank = body.ends_with(char::is_whitespace);
    let typing_second = (words.len() == 1 && ends_blank) || (words.len() == 2 && !ends_blank);
    match section {
        // `name ▮` — the instrument it plays.
        Some(_) if typing_second && is_ident(words[0]) && !KEYWORDS.contains(&words[0]) => {
            instrument_items(analysis)
        }
        // The start of a line in a section: a line from another section, or a keyword.
        Some(section) if words.len() <= 1 && !ends_blank => {
            let mut items = line_items(analysis, &section.name);
            items.extend(keyword_items(SECTION_KEYWORDS));
            items
        }
        None if words.len() <= 1 && !ends_blank && before == body => keyword_items(TOP_KEYWORDS),
        _ => Vec::new(),
    }
}

fn instrument_items(analysis: &Analysis) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();
    let mut names: Vec<&String> = analysis.defs.keys().collect();
    names.sort();
    for name in names {
        let site = &analysis.defs[name];
        let file = site
            .path
            .file_name()
            .map_or_else(String::new, |file| file.to_string_lossy().into_owned());
        items.push(json!({
            "label": name,
            "kind": KIND_CLASS,
            "detail": site.summary,
            "documentation": {"kind": "markdown", "value": format!("```treble\n{}\n```\n\n{file}:{}", site.text, site.line + 1)},
            "insertText": format!("{name} "),
        }));
    }
    for (name, description) in BUILT_IN {
        if !analysis.defs.contains_key(*name) {
            items.push(json!({
                "label": name,
                "kind": KIND_CLASS,
                "detail": format!("built-in · {description}"),
                "insertText": format!("{name} "),
                "sortText": format!("~{name}"),
            }));
        }
    }
    items
}

/// Lines written in other sections that this one does not have yet: start a
/// line on the same instrument, or copy it whole.
fn line_items(analysis: &Analysis, here: &str) -> Vec<Value> {
    let lines = analysis.lines();
    let present: Vec<&str> = analysis
        .layout
        .lines
        .iter()
        .filter(|line| line.section.as_deref() == Some(here))
        .map(|line| line.name_text.as_str())
        .collect();
    let mut seen: Vec<&str> = Vec::new();
    let mut items = Vec::new();
    // The latest writing of a line is the one most likely wanted, so walk
    // backwards and keep the first of each name.
    for at in analysis.layout.lines.iter().rev() {
        let Some(section) = at.section.as_deref() else {
            continue;
        };
        let name = at.name_text.as_str();
        if section == here || present.contains(&name) || seen.contains(&name) {
            continue;
        }
        seen.push(name);
        let instrument = &lines[at.line][at.instrument.0..at.instrument.1];
        // A line written over several lines copies as one; its comments stay
        // where they were.
        let written = at
            .statement
            .text
            .trim()
            .trim_start_matches(';')
            .trim_start()
            .to_string();
        items.push(json!({
            "label": name,
            "kind": KIND_VARIABLE,
            "detail": format!("{instrument} · new line"),
            "insertText": format!("{name} {instrument} \""),
        }));
        items.push(json!({
            "label": name,
            "kind": KIND_SNIPPET,
            "detail": format!("copy from {section}"),
            "documentation": {"kind": "markdown", "value": format!("```treble\n{written}\n```")},
            "insertText": written,
            "data": {"copyLine": written},
        }));
    }
    items
}
