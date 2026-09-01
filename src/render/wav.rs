//! Writing a rendered piece to a WAV, metadata and all.
//!
//! `hound` writes the audio; the `LIST`/`INFO` chunk is appended afterwards.
//! RIFF is a chunk container, so a reader that does not care about `INFO`
//! walks straight past it to `data`, and one that does — a tagger, a media
//! player, a DAW's browser — shows the piece's own title and credits rather
//! than a filename.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use crate::piece::Piece;
use crate::render::RenderedPiece;

/// Well-known `meta` keys mapped to their RIFF `INFO` tag.
///
/// Anything not listed still travels: an unrecognised key is written under
/// `ICMT` alongside the comment, because dropping what the author wrote would
/// be worse than filing it loosely.
const INFO_TAGS: &[(&str, &str)] = &[
    ("title", "INAM"),
    ("artist", "IART"),
    ("composer", "IART"),
    ("album", "IPRD"),
    ("genre", "IGNR"),
    ("year", "ICRD"),
    ("date", "ICRD"),
    ("comment", "ICMT"),
    ("copyright", "ICOP"),
    ("engineer", "IENG"),
    ("source", "ISRC"),
    ("software", "ISFT"),
    ("subject", "ISBJ"),
    ("keywords", "IKEY"),
];

/// Write `rendered` to `path` as a 32-bit float stereo WAV.
///
/// Refuses to overwrite, like the app's own export: a render is cheap to redo
/// and a take is not.
pub fn write(
    path: &Path,
    rendered: &RenderedPiece,
    piece: &Piece,
    overwrite: bool,
) -> Result<(), String> {
    if path.exists() && !overwrite {
        return Err(format!(
            "{} already exists — pass --force to overwrite it",
            path.display()
        ));
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rendered.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for sample in &rendered.samples {
        writer
            .write_sample(sample.clamp(-1.0, 1.0))
            .map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;

    append_info_chunk(path, piece, rendered)
}

/// The `INFO` fields this piece should carry, resolved from its `meta` tags.
fn info_fields(piece: &Piece, rendered: &RenderedPiece) -> Vec<(&'static str, String)> {
    let mut by_tag: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let mut loose: Vec<String> = Vec::new();

    for (key, value) in &piece.metadata {
        match INFO_TAGS.iter().find(|(name, _)| name == key) {
            Some((_, tag)) => by_tag.entry(tag).or_default().push(value.clone()),
            None => loose.push(format!("{key}: {value}")),
        }
    }
    if !loose.is_empty() {
        by_tag.entry("ICMT").or_default().extend(loose);
    }
    // Said last so a piece that sets its own `software` wins over this.
    by_tag
        .entry("ISFT")
        .or_insert_with(|| vec![format!("treble-lang {}", env!("CARGO_PKG_VERSION"))]);

    let mut fields: Vec<(&'static str, String)> = by_tag
        .into_iter()
        .map(|(tag, values)| (tag, values.join("; ")))
        .collect();
    // A render that says how long it is and what it played is easier to file.
    fields.push((
        "ITCH",
        format!(
            "{:.2}s, {} occurrences, {} notes, {} Hz",
            rendered.rendered_seconds, rendered.occurrences, rendered.notes, rendered.sample_rate
        ),
    ));
    fields
}

/// Append a `LIST`/`INFO` chunk and correct the RIFF size.
fn append_info_chunk(path: &Path, piece: &Piece, rendered: &RenderedPiece) -> Result<(), String> {
    let fields = info_fields(piece, rendered);
    if fields.is_empty() {
        return Ok(());
    }

    let mut list: Vec<u8> = b"INFO".to_vec();
    for (tag, value) in fields {
        // NUL-terminated, and padded to an even length: RIFF chunks are
        // word-aligned and a reader that trusts that will desync without it.
        let mut bytes = value.into_bytes();
        bytes.push(0);
        if bytes.len() % 2 == 1 {
            bytes.push(0);
        }
        list.extend_from_slice(tag.as_bytes());
        list.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        list.extend_from_slice(&bytes);
    }

    let mut chunk: Vec<u8> = b"LIST".to_vec();
    chunk.extend_from_slice(&(list.len() as u32).to_le_bytes());
    chunk.extend_from_slice(&list);

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let existing = file.metadata().map_err(|e| e.to_string())?.len();
    file.set_len(existing).map_err(|e| e.to_string())?;
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    file.write_all(&chunk).map_err(|e| e.to_string())?;

    // RIFF's size field counts everything after it, so it grows by the chunk.
    let riff_size = (existing - 8 + chunk.len() as u64) as u32;
    file.seek(SeekFrom::Start(4)).map_err(|e| e.to_string())?;
    file.write_all(&riff_size.to_le_bytes())
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered() -> RenderedPiece {
        RenderedPiece {
            sample_rate: 44_100,
            samples: vec![0.25, -0.25, 0.5, -0.5],
            seconds: 1.0,
            rendered_seconds: 2.0,
            occurrences: 3,
            notes: 12,
            unused: Vec::new(),
            telemetry: Default::default(),
        }
    }

    fn piece_with(metadata: &[(&str, &str)]) -> Piece {
        let source = metadata
            .iter()
            .map(|(k, v)| format!("meta {k} \"{v}\""))
            .collect::<Vec<_>>()
            .join("\n");
        let source = format!("{source}\nsection a 1 {{\n  x kick \"x\"\n}}\n");
        let (program, errors) = crate::parser::parse_program(&source);
        assert!(errors.is_empty(), "{errors:?}");
        crate::piece::resolve(&program, (120, (4, 4), None)).0
    }

    #[test]
    fn well_known_keys_become_info_tags() {
        let piece = piece_with(&[("title", "Drift"), ("composer", "A. N. Other")]);
        let fields = info_fields(&piece, &rendered());
        assert!(fields.contains(&("INAM", "Drift".to_string())));
        assert!(fields.contains(&("IART", "A. N. Other".to_string())));
    }

    #[test]
    fn an_unknown_key_is_kept_rather_than_dropped() {
        let piece = piece_with(&[("tuning", "just intonation")]);
        let fields = info_fields(&piece, &rendered());
        let comment = fields.iter().find(|(tag, _)| *tag == "ICMT").expect("ICMT");
        assert!(
            comment.1.contains("tuning: just intonation"),
            "got {:?}",
            comment.1
        );
    }

    #[test]
    fn the_writer_names_itself_unless_the_piece_does() {
        let fields = info_fields(&piece_with(&[]), &rendered());
        assert!(
            fields
                .iter()
                .any(|(tag, v)| *tag == "ISFT" && v.starts_with("treble-lang"))
        );

        let fields = info_fields(&piece_with(&[("software", "hand-wound")]), &rendered());
        assert!(fields.contains(&("ISFT", "hand-wound".to_string())));
    }

    #[test]
    fn the_wav_stays_readable_with_the_chunk_appended() {
        let path = std::env::temp_dir().join(format!("treble-meta-{}.wav", std::process::id()));
        std::fs::remove_file(&path).ok();
        let piece = piece_with(&[("title", "Drift"), ("comment", "a test")]);
        write(&path, &rendered(), &piece, true).expect("write");

        let mut reader = hound::WavReader::open(&path).expect("readable");
        let samples: Vec<f32> = reader.samples::<f32>().map(|s| s.unwrap()).collect();
        assert_eq!(samples.len(), 4, "the audio must survive the extra chunk");

        // The tag is really in the file, not merely accepted by the writer.
        let raw = std::fs::read(&path).expect("read");
        let needle = b"INAM";
        assert!(
            raw.windows(4).any(|w| w == needle),
            "no INAM tag in the written file"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn an_existing_file_is_not_overwritten_by_accident() {
        let path = std::env::temp_dir().join(format!("treble-keep-{}.wav", std::process::id()));
        std::fs::write(&path, b"precious").expect("seed");
        let error = write(&path, &rendered(), &piece_with(&[]), false).expect_err("must refuse");
        assert!(error.contains("--force"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), b"precious");
        std::fs::remove_file(&path).ok();
    }
}
