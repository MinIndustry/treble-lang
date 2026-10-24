//! JSON-RPC over stdio, framed the way LSP frames it.
//!
//! Hand-written rather than borrowed: the server needs a reader, a writer and
//! four message shapes, and that is less than a dependency's worth of code.

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// Read one `Content-Length`-framed message. `None` at end of input.
pub fn read(input: &mut impl BufRead) -> Result<Option<Value>, String> {
    let mut length = None;
    loop {
        let mut header = String::new();
        let read = input
            .read_line(&mut header)
            .map_err(|error| format!("reading a header: {error}"))?;
        if read == 0 {
            return Ok(None);
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some(value) = header.strip_prefix("Content-Length:") {
            let value = value
                .trim()
                .parse::<usize>()
                .map_err(|error| format!("Content-Length '{}': {error}", value.trim()))?;
            length = Some(value);
        }
    }
    let length = length.ok_or("a message arrived without a Content-Length header")?;
    let mut body = vec![0; length];
    input
        .read_exact(&mut body)
        .map_err(|error| format!("reading a {length}-byte message: {error}"))?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| format!("a message is not JSON: {error}"))
}

/// The way back to the client, shared by the request loop and the player's
/// threads. Each message is written whole under the lock, so two threads can
/// never interleave frames.
#[derive(Clone)]
pub struct Outgoing(Arc<Mutex<std::io::Stdout>>);

impl Outgoing {
    pub fn stdout() -> Self {
        Self(Arc::new(Mutex::new(std::io::stdout())))
    }

    fn send(&self, message: Value) {
        let body = message.to_string();
        let mut out = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // A client that went away takes the server with it at the next read,
        // so a failed write has nothing better to do than be dropped.
        let _ = write!(out, "Content-Length: {}\r\n\r\n{body}", body.len());
        let _ = out.flush();
    }

    pub fn respond(&self, id: Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    pub fn fail(&self, id: Value, code: i64, message: impl Into<String>) {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message.into()},
        }));
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }
}

/// JSON-RPC's "the method does not exist".
pub const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC's "the params are wrong".
pub const INVALID_PARAMS: i64 = -32602;
/// The request was understood but cannot be carried out — an unknown section,
/// a buffer that does not compile, no sound card.
pub const REQUEST_FAILED: i64 = -32803;
