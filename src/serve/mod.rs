//! `treble-lang serve`: a language server with a player behind it.
//!
//! An editor starts this as a child process and speaks LSP over stdio. The
//! standard methods give diagnostics, completion, hover, go-to-definition and
//! document symbols; `treble/outline`, `treble/timeline`, `treble/play` and
//! `treble/stop` add what a music editor needs on top, and the server sends
//! `treble/playhead` and `treble/ended` back while something plays. Quill's
//! `PROTOCOL.md` is the contract.
//!
//! Behind the `serve` feature, which needs `render`: the player streams what
//! the offline renderer produces, so a line heard here is the line as the
//! piece renders it.

mod assist;
mod format;
mod language;
mod player;
mod rpc;
mod text;
mod theory;
mod workspace;

use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use player::Player;
use rpc::{INVALID_PARAMS, METHOD_NOT_FOUND, Outgoing, REQUEST_FAILED};
use workspace::{Analysis, Workspace, diagnostic, is_library};

/// Serve one client over stdin and stdout until it says `exit` or goes away.
pub fn run() -> Result<(), String> {
    let out = Outgoing::stdout();
    let mut server = Server {
        workspace: Workspace::default(),
        analyses: HashMap::new(),
        player: Player::new(out.clone()),
        out,
    };
    let mut input = BufReader::new(std::io::stdin().lock());
    while let Some(message) = rpc::read(&mut input)? {
        if !server.handle(message) {
            break;
        }
    }
    Ok(())
}

type Failure = (i64, String);

struct Server {
    workspace: Workspace,
    /// The latest analysis of every open piece.
    analyses: HashMap<PathBuf, Arc<Analysis>>,
    player: Player,
    out: Outgoing,
}

impl Server {
    /// Handle one message. `false` when the client said `exit`.
    fn handle(&mut self, message: Value) -> bool {
        // A response carries no method; this server asks the client nothing,
        // so there is none to wait for.
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return true;
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match (method, message.get("id").cloned()) {
            ("exit", _) => return false,
            (method, Some(id)) => match self.request(method, &params) {
                Ok(result) => self.out.respond(id, result),
                Err((code, message)) => self.out.fail(id, code, message),
            },
            (method, None) => self.notification(method, &params),
        }
        true
    }

    fn request(&mut self, method: &str, params: &Value) -> Result<Value, Failure> {
        match method {
            "initialize" => Ok(json!({
                "capabilities": {
                    "textDocumentSync": 1,
                    "completionProvider": {"triggerCharacters": [" ", "|"]},
                    "hoverProvider": true,
                    "definitionProvider": true,
                    "documentSymbolProvider": true,
                    "signatureHelpProvider": {"triggerCharacters": [" "], "retriggerCharacters": [" "]},
                    "inlayHintProvider": true,
                },
                "serverInfo": {"name": "treble-lang", "version": env!("CARGO_PKG_VERSION")},
            })),
            "shutdown" => {
                self.player.stop();
                Ok(Value::Null)
            }
            "textDocument/completion" => {
                let path = document(params)?;
                let (line, character) = position(params)?;
                let text = self.workspace.text(&path).unwrap_or_default();
                let library = is_library(&path);
                let analysis = (!library).then(|| self.analysis(&path));
                Ok(language::completion(
                    analysis.as_deref(),
                    &text,
                    library,
                    line,
                    character,
                ))
            }
            "textDocument/hover" => {
                let path = document(params)?;
                let (line, character) = position(params)?;
                if is_library(&path) {
                    return Ok(Value::Null);
                }
                Ok(language::hover(&self.analysis(&path), line, character))
            }
            "textDocument/definition" => {
                let path = document(params)?;
                let (line, character) = position(params)?;
                if is_library(&path) {
                    return Ok(Value::Null);
                }
                let analysis = self.analysis(&path);
                Ok(match language::definition(&analysis, line, character) {
                    Some((file, line, header)) => json!({
                        "uri": self.workspace.uri(&file),
                        "range": text::whole_line(line, &header),
                    }),
                    None => Value::Null,
                })
            }
            "textDocument/signatureHelp" => {
                let path = document(params)?;
                let (line, character) = position(params)?;
                let text = self.workspace.text(&path).unwrap_or_default();
                let lines = text::split_lines(&text);
                let current = lines.get(line).copied().unwrap_or("");
                let byte = text::byte_at(current, character);
                let library = is_library(&path);
                // On a pattern statement the chain may sit on a later line;
                // read it from the statement's start.
                if !library {
                    let analysis = self.analysis(&path);
                    if let Some(at) = analysis.line_on(line)
                        && let Some(offset) = at.statement.offset_of(line, byte)
                        && offset > at.notation.1
                    {
                        return Ok(assist::signature_help(&at.statement.text[..offset], false));
                    }
                }
                Ok(assist::signature_help(&current[..byte], library))
            }
            "textDocument/inlayHint" => {
                let path = document(params)?;
                if is_library(&path) {
                    return Ok(json!([]));
                }
                let from = params["range"]["start"]["line"].as_u64().unwrap_or(0) as usize;
                let to = params["range"]["end"]["line"].as_u64().unwrap_or(u64::MAX) as usize;
                Ok(assist::inlay_hints(&self.analysis(&path), from, to))
            }
            "textDocument/documentSymbol" => {
                let path = document(params)?;
                if is_library(&path) {
                    return Ok(json!([]));
                }
                Ok(language::symbols(&self.analysis(&path)))
            }
            "treble/outline" => {
                let path = document(params)?;
                let (analysis, stale) = self.playable(&path);
                Ok(language::outline(&analysis, stale, |file| {
                    self.workspace.uri(file)
                }))
            }
            "treble/timeline" => {
                let path = document(params)?;
                let section = string(params, "section")?;
                let lines = names(params)?;
                let (analysis, _) = self.playable(&path);
                language::timeline(&analysis, &section, lines.as_deref())
                    .map_err(|message| (REQUEST_FAILED, message))
            }
            "treble/play" => {
                let path = document(params)?;
                let section = string(params, "section")?;
                let lines = names(params)?;
                let looping = params.get("loop").and_then(Value::as_bool).unwrap_or(false);
                let from = params["from"].as_u64().map(|cycle| cycle as u32);
                let analysis = self.analysis(&path);
                self.player
                    .play(path, section, lines, looping, from, analysis)
                    .map(|id| json!({"playId": id}))
                    .map_err(|message| (REQUEST_FAILED, message))
            }
            "treble/format" => {
                let path = document(params)?;
                let line = params["line"].as_u64().map(|line| line as usize);
                let limit = params["maxLength"].as_u64().unwrap_or(100).clamp(20, 1000) as usize;
                let mode = match params["mode"].as_str() {
                    Some("toggle") if line.is_some() => format::Mode::Toggle,
                    Some("toggle") => return Err((INVALID_PARAMS, "toggle needs a `line`".into())),
                    Some("fit") => format::Mode::Fit(limit),
                    _ => return Err((INVALID_PARAMS, "`mode` is \"toggle\" or \"fit\"".into())),
                };
                if is_library(&path) {
                    return Ok(
                        json!({"edits": [], "message": "Instrument libraries aren't formatted."}),
                    );
                }
                let (edits, message) = format::format(&self.analysis(&path), mode, line);
                Ok(json!({"edits": edits, "message": message}))
            }
            "treble/seek" => {
                let cycle = params["cycle"]
                    .as_u64()
                    .ok_or((INVALID_PARAMS, "expected `cycle`".to_string()))?;
                self.player
                    .seek(cycle as u32)
                    .map(|cycle| json!({"cycle": cycle}))
                    .ok_or((REQUEST_FAILED, "Nothing is playing.".to_string()))
            }
            "treble/stop" => {
                self.player.stop();
                Ok(Value::Null)
            }
            other => Err((
                METHOD_NOT_FOUND,
                format!("{other} isn't something this server does."),
            )),
        }
    }

    fn notification(&mut self, method: &str, params: &Value) {
        match method {
            "textDocument/didOpen" => {
                let item = &params["textDocument"];
                let (Some(uri), Some(text)) = (item["uri"].as_str(), item["text"].as_str()) else {
                    return;
                };
                let Some(path) = text::path_of(uri) else {
                    return;
                };
                self.workspace
                    .open(path.clone(), uri.to_string(), text.to_string());
                self.refresh(&path);
            }
            "textDocument/didChange" => {
                let Ok(path) = document(params) else {
                    return;
                };
                // Full sync: the last change holds the whole document.
                let Some(text) = params["contentChanges"]
                    .as_array()
                    .and_then(|changes| changes.last())
                    .and_then(|change| change["text"].as_str())
                else {
                    return;
                };
                self.workspace.change(&path, text.to_string());
                self.refresh(&path);
            }
            "textDocument/didClose" => {
                let Ok(path) = document(params) else {
                    return;
                };
                let uri = self.workspace.uri(&path);
                self.workspace.close(&path);
                self.analyses.remove(&path);
                self.out.notify(
                    "textDocument/publishDiagnostics",
                    json!({"uri": uri, "diagnostics": []}),
                );
                // A closed library falls back to its copy on disk.
                if is_library(&path) {
                    self.refresh(&path);
                }
            }
            // `initialized`, `$/cancelRequest`, `didSave` and the rest need
            // nothing from a server that answers every request at once.
            _ => {}
        }
    }

    /// The latest analysis of a piece, made now if there is none yet.
    fn analysis(&mut self, path: &Path) -> Arc<Analysis> {
        if let Some(analysis) = self.analyses.get(path) {
            return analysis.clone();
        }
        let analysis = self.workspace.analyse(path);
        self.analyses.insert(path.to_path_buf(), analysis.clone());
        analysis
    }

    /// The analysis to draw from: the latest when it resolved, the last one
    /// that did otherwise — with `true` to say it is out of date.
    fn playable(&mut self, path: &Path) -> (Arc<Analysis>, bool) {
        let analysis = self.analysis(path);
        if analysis.piece.is_some() {
            return (analysis, false);
        }
        match self.workspace.last_good(path) {
            Some(good) => (good, true),
            None => (analysis, true),
        }
    }

    /// Re-analyse whatever a changed file affects, publish its problems and
    /// let the player pick up the change.
    fn refresh(&mut self, path: &Path) {
        let pieces: Vec<PathBuf> = if is_library(path) {
            self.workspace
                .open_pieces()
                .into_iter()
                .filter(|piece| {
                    self.analyses
                        .get(piece)
                        .is_some_and(|analysis| analysis.loads.iter().any(|load| load.path == path))
                })
                .collect()
        } else {
            vec![path.to_path_buf()]
        };
        if pieces.is_empty() && is_library(path) {
            self.check_library(path);
            return;
        }
        for piece in pieces {
            let analysis = self.workspace.analyse(&piece);
            for (file, diagnostics) in &analysis.diagnostics {
                self.out.notify(
                    "textDocument/publishDiagnostics",
                    json!({"uri": self.workspace.uri(file), "diagnostics": diagnostics}),
                );
            }
            self.analyses.insert(piece, analysis.clone());
            self.player.update(analysis);
        }
    }

    /// A library open on its own, with no piece loading it: parse problems only.
    fn check_library(&self, path: &Path) {
        let source = self.workspace.text(path).unwrap_or_default();
        let lines = text::split_lines(&source);
        let (_, errors) = crate::parser::parse_program(&source);
        let diagnostics: Vec<Value> = errors
            .iter()
            .map(|error| {
                diagnostic(
                    text::content_range(&lines, error.location.line.saturating_sub(1)),
                    &error.message,
                )
            })
            .collect();
        self.out.notify(
            "textDocument/publishDiagnostics",
            json!({"uri": self.workspace.uri(path), "diagnostics": diagnostics}),
        );
    }
}

/// The file a request is about, from `textDocument.uri` or a bare `uri`.
fn document(params: &Value) -> Result<PathBuf, Failure> {
    let uri = params["textDocument"]["uri"]
        .as_str()
        .or_else(|| params["uri"].as_str())
        .ok_or((INVALID_PARAMS, "expected a document uri".to_string()))?;
    text::path_of(uri).ok_or((INVALID_PARAMS, format!("{uri} isn't a file:// uri")))
}

fn position(params: &Value) -> Result<(usize, u32), Failure> {
    let at = &params["position"];
    match (at["line"].as_u64(), at["character"].as_u64()) {
        (Some(line), Some(character)) => Ok((line as usize, character as u32)),
        _ => Err((INVALID_PARAMS, "expected a position".to_string())),
    }
}

fn string(params: &Value, key: &str) -> Result<String, Failure> {
    params[key]
        .as_str()
        .map(str::to_string)
        .ok_or((INVALID_PARAMS, format!("expected `{key}`")))
}

/// `lines`: `null` for every line, or a list of names.
fn names(params: &Value) -> Result<Option<Vec<String>>, Failure> {
    match &params["lines"] {
        Value::Null => Ok(None),
        Value::Array(names) => names
            .iter()
            .map(|name| {
                name.as_str()
                    .map(str::to_string)
                    .ok_or((INVALID_PARAMS, "`lines` holds names".to_string()))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        _ => Err((
            INVALID_PARAMS,
            "`lines` is null or a list of names".to_string(),
        )),
    }
}
