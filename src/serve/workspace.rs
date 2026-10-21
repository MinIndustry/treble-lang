//! What the server knows: the files a client has open, and what each piece
//! resolves to — its timeline, its instruments, its problems, and where each
//! of its lines sits in the text.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use treble::instruments::prelude::InstrumentRegistry;

use super::text::{column, comment_at, content_range, lines_range, range, split_lines, uri_of};
use crate::ast::InstrumentDef;
use crate::parser::{Statement, statements};
use crate::render::compile::lower_instrument_def;
use crate::{Piece, Session, SourceLine};

/// Words a line can start with that make it something other than a pattern.
pub const KEYWORDS: &[&str] = &[
    "section", "group", "def", "bpm", "sig", "scale", "seed", "tail", "meta", "load", "arrange",
    "include", "phrase",
];

#[derive(Default)]
pub struct Workspace {
    open: HashMap<PathBuf, Document>,
    /// The last analysis of each piece that resolved, so an outline survives
    /// the moments a buffer is mid-edit and does not parse.
    last_good: HashMap<PathBuf, Arc<Analysis>>,
}

struct Document {
    uri: String,
    text: String,
}

impl Workspace {
    pub fn open(&mut self, path: PathBuf, uri: String, text: String) {
        self.open.insert(path, Document { uri, text });
    }

    pub fn change(&mut self, path: &Path, text: String) {
        if let Some(document) = self.open.get_mut(path) {
            document.text = text;
        }
    }

    pub fn close(&mut self, path: &Path) {
        self.open.remove(path);
        self.last_good.remove(path);
    }

    /// The URI the client used for a file, or one made from its path.
    pub fn uri(&self, path: &Path) -> String {
        self.open
            .get(path)
            .map_or_else(|| uri_of(path), |document| document.uri.clone())
    }

    /// A file's text: the client's copy when it has one open, which may be
    /// unsaved, and the disk's otherwise.
    pub fn text(&self, path: &Path) -> Result<String, String> {
        match self.open.get(path) {
            Some(document) => Ok(document.text.clone()),
            None => std::fs::read_to_string(path).map_err(|error| error.to_string()),
        }
    }

    /// Every open file that is a piece rather than an instrument library.
    pub fn open_pieces(&self) -> Vec<PathBuf> {
        self.open
            .keys()
            .filter(|path| !is_library(path))
            .cloned()
            .collect()
    }

    pub fn last_good(&self, path: &Path) -> Option<Arc<Analysis>> {
        self.last_good.get(path).cloned()
    }

    /// Resolve a piece and everything it loads.
    pub fn analyse(&mut self, path: &Path) -> Arc<Analysis> {
        let analysis = Arc::new(analyse(self, path));
        if analysis.piece.is_some() {
            self.last_good.insert(path.to_path_buf(), analysis.clone());
        }
        analysis
    }
}

/// An instrument library holds `def` blocks only and is never played itself.
pub fn is_library(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "trbl")
}

/// A piece, resolved.
pub struct Analysis {
    pub path: PathBuf,
    pub text: String,
    pub piece: Option<Piece>,
    pub registry: Arc<InstrumentRegistry>,
    /// Nothing anywhere is wrong: the piece resolves, every load reads, every
    /// instrument builds and exists. Only a clean analysis replaces audio that
    /// is already playing.
    pub clean: bool,
    /// Problems by file: the piece's own first, then each library it loads,
    /// listed even when empty so a client clears what was fixed.
    pub diagnostics: Vec<(PathBuf, Vec<Value>)>,
    pub loads: Vec<Load>,
    /// Every instrument definition in reach, by name; a later one (the
    /// buffer's own `def`) replaces an earlier one, as in the registry.
    pub defs: HashMap<String, DefSite>,
    pub layout: Layout,
}

impl Analysis {
    pub fn lines(&self) -> Vec<&str> {
        split_lines(&self.text)
    }

    /// Where the line `name` of section `section` is written.
    pub fn line_at(&self, section: &str, name: &str) -> Option<&LineAt> {
        self.layout
            .lines
            .iter()
            .find(|line| line.section.as_deref() == Some(section) && line.name_text == name)
    }

    /// The pattern statement that physical line `index` belongs to.
    pub fn line_on(&self, index: usize) -> Option<&LineAt> {
        self.layout
            .lines
            .iter()
            .find(|line| line.statement.covers(index))
    }

    /// The section whose braces enclose line `index`.
    pub fn section_at(&self, index: usize) -> Option<&SectionAt> {
        self.layout
            .sections
            .iter()
            .find(|section| (section.header..=section.end).contains(&index))
    }
}

pub struct Load {
    /// As written on the `load` line.
    pub file: String,
    pub path: PathBuf,
    pub defs: usize,
}

/// Where a `def` is written.
#[derive(Clone)]
pub struct DefSite {
    pub path: PathBuf,
    /// 0-based line of `def <name> {`.
    pub line: usize,
    /// The block, header to closing brace.
    pub text: String,
    /// "poly · gated · 4 tones: saw+triangle".
    pub summary: String,
}

/// Where things sit in a piece's text, found by reading it the way a person
/// does: one statement per line, sections and groups opened by `{` and closed
/// by `}`. The parser's AST carries no positions above the step, so this is
/// what positions an outline; the AST stays the authority on meaning.
#[derive(Default)]
pub struct Layout {
    pub sections: Vec<SectionAt>,
    pub lines: Vec<LineAt>,
    /// `load "<file>"` lines: the file as written, and the 0-based line.
    pub loads: Vec<(String, usize)>,
    /// `bpm`, `sig` and `scale` as written outside every section.
    pub top: HashMap<&'static str, String>,
}

pub struct SectionAt {
    pub name: String,
    /// 0-based line of the `section` header.
    pub header: usize,
    /// 0-based line of its closing brace (the last line, when unclosed).
    pub end: usize,
}

/// A pattern statement's position. Offsets are bytes in the statement's
/// joined text; on its first line they are that line's own columns, which is
/// where the name and instrument always are.
pub struct LineAt {
    /// The physical line it starts on.
    pub line: usize,
    pub section: Option<String>,
    pub name_text: String,
    pub name: (usize, usize),
    pub instrument: (usize, usize),
    /// Between the quotes, exclusive of them. May span lines.
    pub notation: (usize, usize),
    pub statement: Statement,
}

impl LineAt {
    /// The physical line it ends on.
    pub fn last_line(&self) -> usize {
        self.statement.last_line
    }

    /// An LSP range over statement offsets `from..to`, which may span lines.
    pub fn range(&self, lines: &[&str], from: usize, to: usize) -> Value {
        let (start_line, start) = self.statement.locate(from);
        let (end_line, end) = self.statement.locate(to);
        let column =
            |line: usize, byte: usize| column(lines.get(line).copied().unwrap_or(""), byte);
        json!({
            "start": {"line": start_line, "character": column(start_line, start)},
            "end": {"line": end_line, "character": column(end_line, end)},
        })
    }

    /// The whole statement, first line to last.
    pub fn whole(&self, lines: &[&str]) -> Value {
        lines_range(lines, self.line, self.last_line())
    }
}

impl Layout {
    pub fn scan(text: &str) -> Self {
        let mut layout = Self::default();
        let mut section: Option<(String, usize)> = None;
        // Only whether a group is open matters: its `}` is not a section's.
        let mut in_group = false;
        let all = statements(text);
        let last = all.last().map_or(0, |statement| statement.last_line);
        for statement in all {
            let index = statement.first_line;
            let code = &statement.text[..comment_at(&statement.text)];
            let trimmed = code.trim();
            if trimmed.is_empty() {
                continue;
            }
            let body = trimmed.trim_start_matches(';').trim_start();
            let mut words = body.split_whitespace();
            let first = words.next().unwrap_or("");
            let second = words.next().unwrap_or("").trim_end_matches('{');
            if first == "section" && body.ends_with('{') {
                section = Some((second.to_string(), index));
                in_group = false;
                continue;
            }
            if first == "group" && body.ends_with('{') {
                in_group = true;
                continue;
            }
            if trimmed.starts_with('}') {
                if !std::mem::take(&mut in_group)
                    && let Some((name, header)) = section.take()
                {
                    layout.sections.push(SectionAt {
                        name,
                        header,
                        end: statement.last_line,
                    });
                }
                continue;
            }
            if section.is_none() && !in_group {
                match first {
                    "bpm" | "sig" | "scale" => {
                        let key = match first {
                            "bpm" => "bpm",
                            "sig" => "sig",
                            _ => "scale",
                        };
                        let value = body[first.len()..].trim().to_string();
                        layout.top.insert(key, value);
                    }
                    "load" => {
                        let file = body[4..].trim().trim_matches('"').to_string();
                        layout.loads.push((file, index));
                    }
                    _ => {}
                }
            }
            if let Some((name, instrument, notation)) = pattern_at(code) {
                layout.lines.push(LineAt {
                    line: index,
                    section: section.as_ref().map(|(name, _)| name.clone()),
                    name_text: code[name.0..name.1].to_string(),
                    name,
                    instrument,
                    notation,
                    statement,
                });
            }
        }
        if let Some((name, header)) = section {
            layout.sections.push(SectionAt {
                name,
                header,
                end: last,
            });
        }
        layout
    }
}

type Bytes = (usize, usize);

/// `[;] name instrument "notation" …` — the byte ranges of its parts.
fn pattern_at(code: &str) -> Option<(Bytes, Bytes, Bytes)> {
    let bytes = code.as_bytes();
    let mut at = blank_from(bytes, 0);
    if bytes.get(at) == Some(&b';') {
        at = blank_from(bytes, at + 1);
    }
    let name = ident_at(bytes, at)?;
    if KEYWORDS.contains(&&code[name.0..name.1]) {
        return None;
    }
    at = blank_from(bytes, name.1);
    if at == name.1 {
        return None;
    }
    let instrument = ident_at(bytes, at)?;
    at = blank_from(bytes, instrument.1);
    if at == instrument.1 || bytes.get(at) != Some(&b'"') {
        return None;
    }
    let close = code[at + 1..].find('"')? + at + 1;
    Some((name, instrument, (at + 1, close)))
}

fn blank_from(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(|byte| byte.is_ascii_whitespace()) {
        at += 1;
    }
    at
}

fn ident_at(bytes: &[u8], at: usize) -> Option<Bytes> {
    if !bytes.get(at)?.is_ascii_alphabetic() {
        return None;
    }
    let mut end = at + 1;
    while bytes
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        end += 1;
    }
    Some((at, end))
}

/// Every `def` block in a file, by name.
pub fn def_sites(lines: &[&str], path: &Path) -> HashMap<String, DefSite> {
    let mut sites = HashMap::new();
    let mut index = 0;
    while index < lines.len() {
        let code = &lines[index][..comment_at(lines[index])];
        let trimmed = code.trim_start();
        let Some(rest) = trimmed.strip_prefix("def ") else {
            index += 1;
            continue;
        };
        let name: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let start = index;
        let mut depth = 0i32;
        loop {
            let code = &lines[index][..comment_at(lines[index])];
            depth += code.matches('{').count() as i32;
            depth -= code.matches('}').count() as i32;
            if depth <= 0 || index + 1 == lines.len() {
                break;
            }
            index += 1;
        }
        let text = lines[start..=index].join("\n");
        sites.insert(
            name,
            DefSite {
                path: path.to_path_buf(),
                line: start,
                summary: summary(&text),
                text,
            },
        );
        index += 1;
    }
    sites
}

/// A def's character in a few words: voicing, lifecycle and its tones.
fn summary(block: &str) -> String {
    let mut voice = None;
    let mut lifecycle = None;
    let mut tones: Vec<String> = Vec::new();
    for line in block.lines().skip(1) {
        let mut words = line.split_whitespace();
        match (words.next(), words.next()) {
            (Some("voice"), Some(kind)) => voice = Some(kind.to_string()),
            (Some("lifecycle"), Some(kind)) => lifecycle = Some(kind.to_string()),
            (Some("tone"), Some(wave)) => tones.push(wave.to_string()),
            _ => {}
        }
    }
    let mut kinds: Vec<&str> = Vec::new();
    for tone in &tones {
        if !kinds.contains(&tone.as_str()) {
            kinds.push(tone);
        }
    }
    let tones = match tones.len() {
        0 => None,
        1 => Some(format!("1 tone: {}", kinds.join("+"))),
        n => Some(format!("{n} tones: {}", kinds.join("+"))),
    };
    [voice, lifecycle, tones]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

pub fn diagnostic(range: Value, message: &str) -> Value {
    json!({"range": range, "severity": 1, "source": "treble", "message": message})
}

fn analyse(workspace: &Workspace, path: &Path) -> Analysis {
    let text = workspace.text(path).unwrap_or_default();
    let lines = split_lines(&text);
    let layout = Layout::scan(&text);
    let mut session = Session::new();
    let result = session.evaluate(&text);

    let mut own: Vec<Value> = result
        .errors
        .iter()
        .map(|error| {
            let message = match &error.suggestion {
                Some(suggestion) => format!("{} ({suggestion})", error.message),
                None => error.message.clone(),
            };
            diagnostic(
                content_range(&lines, error.location.line.saturating_sub(1)),
                &message,
            )
        })
        .collect();
    let mut libraries: Vec<(PathBuf, Vec<Value>)> = Vec::new();
    let mut loads = Vec::new();
    let mut defs = HashMap::new();
    let mut loaded: Vec<InstrumentDef> = Vec::new();

    // `load` resolves beside the piece, exactly as `treble-lang check` does.
    let base = path.parent().unwrap_or(Path::new("."));
    for load in session.loads() {
        let resolved = if Path::new(load).is_absolute() {
            PathBuf::from(load)
        } else {
            base.join(load)
        };
        let load_line = layout
            .loads
            .iter()
            .find(|(file, _)| file == load)
            .map_or(0, |(_, index)| *index);
        let library = match workspace.text(&resolved) {
            Ok(library) => library,
            Err(error) => {
                own.push(diagnostic(
                    content_range(&lines, load_line),
                    &format!("Can't read {load}: {error}"),
                ));
                continue;
            }
        };
        let library_lines = split_lines(&library);
        let (program, errors) = crate::parser::parse_program(&library);
        let problems: Vec<Value> = errors
            .iter()
            .map(|error| {
                diagnostic(
                    content_range(&library_lines, error.location.line.saturating_sub(1)),
                    &error.message,
                )
            })
            .collect();
        let mut count = 0;
        let mut sounding = false;
        for line in program.lines {
            match line {
                SourceLine::Def(definition) => {
                    count += 1;
                    loaded.push(*definition);
                }
                SourceLine::Comment(_) | SourceLine::Blank => {}
                _ => sounding = true,
            }
        }
        if sounding {
            own.push(diagnostic(
                content_range(&lines, load_line),
                &format!("{load} holds lines other than `def` blocks; those belong in the piece."),
            ));
        }
        if !problems.is_empty() {
            own.push(diagnostic(
                content_range(&lines, load_line),
                &format!(
                    "{load} has {} problem{}.",
                    problems.len(),
                    if problems.len() == 1 { "" } else { "s" }
                ),
            ));
        }
        defs.extend(def_sites(&library_lines, &resolved));
        libraries.push((resolved.clone(), problems));
        loads.push(Load {
            file: load.clone(),
            path: resolved,
            defs: count,
        });
    }
    defs.extend(def_sites(&lines, path));

    // Lower every definition the way the renderer will, so a def that parses
    // but cannot be built is reported where it is written.
    let cycle = result
        .piece
        .as_ref()
        .and_then(|piece| piece.sections.first())
        .map_or(2.0, |section| section.cycle_seconds());
    let mut registry = InstrumentRegistry::built_in();
    let mut own_defs: Vec<&InstrumentDef> = session.definitions().values().collect();
    own_defs.sort_by(|a, b| a.name.cmp(&b.name));
    for definition in loaded.iter().chain(own_defs) {
        let built = lower_instrument_def(definition, cycle)
            .and_then(|spec| registry.register(spec).map_err(|error| error.to_string()));
        if let Err(error) = built {
            let message = format!("def {}: {error}", definition.name);
            match defs.get(&definition.name) {
                Some(site) if site.path != path => {
                    if let Some((_, problems)) = libraries
                        .iter_mut()
                        .find(|(library, _)| *library == site.path)
                    {
                        let library = workspace.text(&site.path).unwrap_or_default();
                        problems.push(diagnostic(
                            content_range(&split_lines(&library), site.line),
                            &message,
                        ));
                    }
                }
                Some(site) => own.push(diagnostic(content_range(&lines, site.line), &message)),
                None => own.push(diagnostic(content_range(&lines, 0), &message)),
            }
        }
    }

    // `check` leaves an unknown instrument to the render; an editor should
    // say so while the line is being written.
    if let Some(piece) = &result.piece {
        for section in &piece.sections {
            for line in &section.patterns {
                if registry.get(&line.instrument).is_some() {
                    continue;
                }
                let Some(at) = layout.lines.iter().find(|at| {
                    at.section.as_deref() == Some(section.name.as_str())
                        && at.name_text == line.name
                }) else {
                    continue;
                };
                let nearest = registry
                    .names()
                    .map(|name| (distance(name, &line.instrument), name.to_string()))
                    .min();
                let hint = match nearest {
                    Some((distance, name)) if distance <= 3 => format!(" Did you mean {name}?"),
                    _ => String::new(),
                };
                own.push(diagnostic(
                    range(&lines, at.line, at.instrument.0, at.instrument.1),
                    &format!(
                        "'{}' isn't a built-in instrument or a def this piece loads.{hint}",
                        line.instrument
                    ),
                ));
            }
        }
    }

    let clean = result.piece.is_some()
        && own.is_empty()
        && libraries.iter().all(|(_, problems)| problems.is_empty());
    let mut diagnostics = vec![(path.to_path_buf(), own)];
    diagnostics.extend(libraries);
    Analysis {
        path: path.to_path_buf(),
        piece: result.piece,
        registry: Arc::new(registry),
        clean,
        diagnostics,
        loads,
        defs,
        layout,
        text,
    }
}

/// Edit distance, for "did you mean".
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous + usize::from(ca != *cb);
            previous = row[j + 1];
            row[j + 1] = substitution.min(row[j] + 1).min(row[j + 1] + 1);
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_finds_lines_inside_sections_and_groups() {
        let text = "bpm 92\nload \"kit.trbl\"\nsection a 4 {\n  ; bass sine \"0 2\" @ 2.. -- low\n  group kit {\n    kk kick \"x ~\"\n  } | gain 0.9\n}\n";
        let lines = split_lines(text);
        let layout = Layout::scan(text);
        assert_eq!(layout.sections.len(), 1);
        assert_eq!((layout.sections[0].header, layout.sections[0].end), (2, 7));
        assert_eq!(layout.loads, vec![("kit.trbl".to_string(), 1)]);
        assert_eq!(layout.top.get("bpm").map(String::as_str), Some("92"));
        let bass = &layout.lines[0];
        assert_eq!(bass.name_text, "bass", "a muted line is still a line");
        assert_eq!(&lines[3][bass.instrument.0..bass.instrument.1], "sine");
        assert_eq!(&lines[3][bass.notation.0..bass.notation.1], "0 2");
        let kick = &layout.lines[1];
        let broken = "section b 2 {\n  pad pad \"<\n    [0,2,4]  -- Em\n    [5,7,9]\n  >\"\n    | oct -1\n}\n";
        let broken_lines = split_lines(broken);
        let layout_b = Layout::scan(broken);
        let pad = &layout_b.lines[0];
        assert_eq!((pad.line, pad.last_line()), (1, 5));
        assert_eq!(layout_b.sections[0].end, 6);
        let range = pad.range(&broken_lines, pad.notation.0, pad.notation.1);
        assert_eq!(range["start"], json!({"line": 1, "character": 11}));
        assert_eq!(range["end"], json!({"line": 4, "character": 3}));
        assert_eq!(
            kick.section.as_deref(),
            Some("a"),
            "a group's closing brace does not close the section"
        );
    }

    #[test]
    fn defs_are_found_with_their_blocks() {
        let text = "-- kit\ndef thump {\n  voice mono\n  tone sine gain 0.5\n  tone noise {\n    env attack linear 0 1 0.01\n  }\n}\n";
        let lines = split_lines(text);
        let sites = def_sites(&lines, Path::new("kit.trbl"));
        let thump = &sites["thump"];
        assert_eq!(thump.line, 1);
        assert!(thump.text.ends_with('}'));
        assert_eq!(thump.summary, "mono · 2 tones: sine+noise");
    }
}
