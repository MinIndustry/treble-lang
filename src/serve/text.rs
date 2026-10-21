//! Positions the way LSP counts them, and file URIs.
//!
//! LSP columns are UTF-16 code units; everything else in this crate counts
//! bytes. The conversion happens here and nowhere else.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The UTF-16 column of byte offset `byte` in `line`.
pub fn column(line: &str, byte: usize) -> u32 {
    let byte = floor_char_boundary(line, byte);
    line[..byte].encode_utf16().count() as u32
}

/// The byte offset in `line` at UTF-16 column `column`, clamped to the line.
pub fn byte_at(line: &str, column: u32) -> usize {
    let mut units = 0u32;
    for (byte, character) in line.char_indices() {
        if units >= column {
            return byte;
        }
        units += character.len_utf16() as u32;
    }
    line.len()
}

fn floor_char_boundary(line: &str, byte: usize) -> usize {
    let mut byte = byte.min(line.len());
    while !line.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

/// An LSP range over bytes `from..to` of line `index`.
pub fn range(lines: &[&str], index: usize, from: usize, to: usize) -> Value {
    let line = lines.get(index).copied().unwrap_or("");
    json!({
        "start": {"line": index, "character": column(line, from)},
        "end": {"line": index, "character": column(line, to)},
    })
}

/// An LSP range over the whole of line `index`, whose text is `line`.
pub fn whole_line(index: usize, line: &str) -> Value {
    json!({
        "start": {"line": index, "character": 0},
        "end": {"line": index, "character": column(line, line.len())},
    })
}

/// An LSP range from the start of line `from` to the end of line `to`.
pub fn lines_range(lines: &[&str], from: usize, to: usize) -> Value {
    let last = lines.get(to).copied().unwrap_or("");
    json!({
        "start": {"line": from, "character": 0},
        "end": {"line": to, "character": column(last, last.len())},
    })
}

/// A line's written content as a range: from its first non-blank character
/// to its end, which is where a diagnostic without a column belongs.
pub fn content_range(lines: &[&str], index: usize) -> Value {
    let line = lines.get(index).copied().unwrap_or("");
    let start = line.len() - line.trim_start().len();
    range(lines, index, start, line.trim_end().len().max(start))
}

/// Split source text into lines the way the parser does, `\r\n` included.
pub fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

/// The byte where a `--` comment starts, or the line's length.
pub fn comment_at(line: &str) -> usize {
    let mut quoted = false;
    let bytes = line.as_bytes();
    for (index, &byte) in bytes.iter().enumerate() {
        match byte {
            b'"' => quoted = !quoted,
            b'-' if !quoted && bytes.get(index + 1) == Some(&b'-') => return index,
            _ => {}
        }
    }
    line.len()
}

/// The filesystem path a `file://` URI names.
pub fn path_of(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?.as_bytes();
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        let byte = encoded[index];
        if byte == b'%' && index + 2 < encoded.len() {
            let hex = std::str::from_utf8(&encoded[index + 1..index + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(byte);
            index += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(decoded).ok()?))
}

/// The `file://` URI for a path, percent-encoding everything but the
/// characters RFC 3986 leaves unreserved and the path separator.
pub fn uri_of(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(byte as char)
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_count_utf16_units() {
        let line = "é x";
        assert_eq!(column(line, 3), 2, "é is two bytes but one unit");
        assert_eq!(byte_at(line, 2), 3);
    }

    #[test]
    fn uris_round_trip() {
        let path = Path::new("/tmp/a b/01-sleepless.rt");
        let uri = uri_of(path);
        assert_eq!(uri, "file:///tmp/a%20b/01-sleepless.rt");
        assert_eq!(path_of(&uri).as_deref(), Some(path));
    }

    #[test]
    fn comments_inside_notation_are_not_comments() {
        assert_eq!(comment_at("a sine \"0 -- 1\" -- here"), 16);
    }
}
