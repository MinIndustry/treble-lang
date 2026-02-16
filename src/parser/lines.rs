//! Line-level parser for Rustic Live DSL.
//!
//! Each source line is parsed independently into a [`SourceLine`].

use crate::ast::program::*;
use super::mini::parse_mini;

/// Parse a single source line into a [`SourceLine`].
pub fn parse_line(line: &str) -> Result<SourceLine, String> {
    let trimmed = line.trim();

    // Blank
    if trimmed.is_empty() {
        return Ok(SourceLine::Blank);
    }

    // Comment
    if trimmed.starts_with("--") {
        return Ok(SourceLine::Comment(trimmed.to_string()));
    }

    // Directives
    if let Some(rest) = strip_keyword(trimmed, "bpm") {
        return parse_bpm(rest);
    }
    if let Some(rest) = strip_keyword(trimmed, "sig") {
        return parse_sig(rest);
    }
    if let Some(rest) = strip_keyword(trimmed, "scale") {
        return parse_scale(rest);
    }
    if let Some(rest) = strip_keyword(trimmed, "load") {
        return parse_load(rest);
    }

    // Muted pattern
    if trimmed.starts_with(';') {
        let rest = trimmed[1..].trim_start();
        return parse_pattern_line(rest, true);
    }

    // Pattern line
    parse_pattern_line(trimmed, false)
}

/// Strip a keyword prefix followed by whitespace. Returns the rest.
fn strip_keyword<'a>(input: &'a str, keyword: &str) -> Option<&'a str> {
    if input.starts_with(keyword) {
        let rest = &input[keyword.len()..];
        if rest.starts_with(char::is_whitespace) {
            Some(rest.trim_start())
        } else {
            None
        }
    } else {
        None
    }
}

// --- Directive parsers ---

fn parse_bpm(rest: &str) -> Result<SourceLine, String> {
    let val: u32 = rest
        .trim()
        .parse()
        .map_err(|_| format!("invalid bpm value: '{}'", rest.trim()))?;
    if !(20..=999).contains(&val) {
        return Err(format!("bpm must be between 20 and 999, got {}", val));
    }
    Ok(SourceLine::Bpm(val))
}

fn parse_sig(rest: &str) -> Result<SourceLine, String> {
    let parts: Vec<&str> = rest.trim().split('/').collect();
    if parts.len() != 2 {
        return Err(format!("expected time signature N/D, got '{}'", rest.trim()));
    }
    let num: u8 = parts[0]
        .trim()
        .parse()
        .map_err(|_| format!("invalid numerator: '{}'", parts[0].trim()))?;
    let den: u8 = parts[1]
        .trim()
        .parse()
        .map_err(|_| format!("invalid denominator: '{}'", parts[1].trim()))?;
    if num == 0 {
        return Err("time signature numerator must be > 0".to_string());
    }
    if !den.is_power_of_two() || den == 0 {
        return Err(format!(
            "time signature denominator must be a power of 2, got {}",
            den
        ));
    }
    Ok(SourceLine::Sig(num, den))
}

fn parse_scale(rest: &str) -> Result<SourceLine, String> {
    let mut tokens = rest.trim().split_whitespace();
    let root_str = tokens
        .next()
        .ok_or_else(|| "expected scale root note".to_string())?;
    let mode_str = tokens
        .next()
        .ok_or_else(|| "expected scale mode".to_string())?;

    let root = parse_pitch_root(root_str)?;
    let mode = parse_scale_mode(mode_str)?;
    Ok(SourceLine::Scale(root, mode))
}

fn parse_load(rest: &str) -> Result<SourceLine, String> {
    let trimmed = rest.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        let path = &trimmed[1..trimmed.len() - 1];
        Ok(SourceLine::Load(path.to_string()))
    } else {
        Err(format!(
            "expected quoted path after load, got '{}'",
            trimmed
        ))
    }
}

// --- Pattern line parser ---

fn parse_pattern_line(input: &str, muted: bool) -> Result<SourceLine, String> {
    let mut tokens = SplitKeepQuotes::new(input);

    let name = tokens
        .next()
        .ok_or_else(|| "expected pattern name".to_string())?;
    validate_identifier(name)?;

    let instrument = tokens
        .next()
        .ok_or_else(|| "expected instrument name".to_string())?;
    validate_identifier(instrument)?;

    let notation_str = tokens
        .next()
        .ok_or_else(|| "expected quoted mini-notation".to_string())?;

    if !notation_str.starts_with('"') || !notation_str.ends_with('"') || notation_str.len() < 2 {
        return Err(format!(
            "expected double-quoted mini-notation, got '{}'",
            notation_str
        ));
    }
    let inner = &notation_str[1..notation_str.len() - 1];
    let notation = parse_mini(inner)?;

    // Parse transforms: everything after the closing quote, split by `|`
    let remainder: String = tokens.collect::<Vec<&str>>().join(" ");
    let transforms = parse_transforms(remainder.trim())?;

    Ok(SourceLine::Pattern(PatternDef {
        muted,
        name: name.to_string(),
        instrument: instrument.to_string(),
        notation,
        transforms,
    }))
}

fn validate_identifier(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("identifier cannot be empty".to_string());
    }
    let first = s.chars().next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return Err(format!(
            "identifier must start with a letter or underscore, got '{}'",
            s
        ));
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("invalid identifier: '{}'", s));
    }
    Ok(())
}

// --- Transform parser ---

fn parse_transforms(input: &str) -> Result<Vec<Transform>, String> {
    if input.is_empty() {
        return Ok(Vec::new());
    }

    let mut transforms = Vec::new();
    // Split by `|` (the transform pipe, outside of quotes)
    let segments: Vec<&str> = input.split('|').collect();
    for seg in segments {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        transforms.push(parse_single_transform(seg)?);
    }
    Ok(transforms)
}

fn parse_single_transform(input: &str) -> Result<Transform, String> {
    let mut parts = input.split_whitespace();
    let keyword = parts
        .next()
        .ok_or_else(|| "expected transform keyword".to_string())?;

    match keyword {
        "rev" => Ok(Transform::Rev),
        "fast" => {
            let val = parse_transform_f64(&mut parts, "fast")?;
            Ok(Transform::Fast(val))
        }
        "slow" => {
            let val = parse_transform_f64(&mut parts, "slow")?;
            Ok(Transform::Slow(val))
        }
        "every" => {
            let n_str = parts
                .next()
                .ok_or_else(|| "every: expected cycle count".to_string())?;
            let n: u32 = n_str
                .parse()
                .map_err(|_| format!("every: invalid number '{}'", n_str))?;
            let rest: String = parts.collect::<Vec<&str>>().join(" ");
            let inner = parse_single_transform(rest.trim())?;
            Ok(Transform::Every(n, Box::new(inner)))
        }
        "arp" => {
            let mode_str = parts
                .next()
                .ok_or_else(|| "arp: expected mode (up/down/updown/random)".to_string())?;
            let mode = match mode_str {
                "up" => ArpMode::Up,
                "down" => ArpMode::Down,
                "updown" => ArpMode::UpDown,
                "random" => ArpMode::Random,
                other => return Err(format!("arp: unknown mode '{}'", other)),
            };
            Ok(Transform::Arp(mode))
        }
        "scale" => {
            let root_str = parts
                .next()
                .ok_or_else(|| "scale: expected root note".to_string())?;
            let mode_str = parts
                .next()
                .ok_or_else(|| "scale: expected mode".to_string())?;
            let root = parse_pitch_root(root_str)?;
            let mode = parse_scale_mode(mode_str)?;
            Ok(Transform::Scale(root, mode))
        }
        "oct" => {
            let val_str = parts
                .next()
                .ok_or_else(|| "oct: expected offset".to_string())?;
            let val: i32 = val_str
                .parse()
                .map_err(|_| format!("oct: invalid offset '{}'", val_str))?;
            Ok(Transform::Oct(val))
        }
        "gain" => {
            let val = parse_transform_f64(&mut parts, "gain")?;
            Ok(Transform::Gain(val))
        }
        "lpf" => {
            let val = parse_transform_f64(&mut parts, "lpf")?;
            Ok(Transform::Lpf(val))
        }
        "hpf" => {
            let val = parse_transform_f64(&mut parts, "hpf")?;
            Ok(Transform::Hpf(val))
        }
        "delay" => {
            let time = parse_transform_f64(&mut parts, "delay time")?;
            let fb = parse_transform_f64(&mut parts, "delay feedback")?;
            Ok(Transform::Delay(time, fb))
        }
        "reverb" => {
            let val = parse_transform_f64(&mut parts, "reverb")?;
            Ok(Transform::Reverb(val))
        }
        other => Err(format!("unknown transform: '{}'", other)),
    }
}

fn parse_transform_f64(
    parts: &mut std::str::SplitWhitespace,
    name: &str,
) -> Result<f64, String> {
    let val_str = parts
        .next()
        .ok_or_else(|| format!("{}: expected number", name))?;
    val_str
        .parse::<f64>()
        .map_err(|_| format!("{}: invalid number '{}'", name, val_str))
}

// --- Shared helpers ---

fn parse_pitch_root(s: &str) -> Result<PitchRoot, String> {
    let mut chars = s.chars();
    let letter_ch = chars.next().ok_or("expected note letter")?;
    let letter = match letter_ch {
        'C' | 'c' => NoteLetter::C,
        'D' | 'd' => NoteLetter::D,
        'E' | 'e' => NoteLetter::E,
        'F' | 'f' => NoteLetter::F,
        'G' | 'g' => NoteLetter::G,
        'A' | 'a' => NoteLetter::A,
        'B' | 'b' => NoteLetter::B,
        other => return Err(format!("invalid note letter: '{}'", other)),
    };
    let rest: String = chars.collect();
    let accidental = match rest.as_str() {
        "#" => Accidental::Sharp,
        "##" => Accidental::DoubleSharp,
        "b" => Accidental::Flat,
        "bb" => Accidental::DoubleFlat,
        "" => Accidental::Natural,
        other => return Err(format!("invalid accidental: '{}'", other)),
    };
    Ok(PitchRoot { name: letter, accidental })
}

fn parse_scale_mode(s: &str) -> Result<ScaleMode, String> {
    match s {
        "major" => Ok(ScaleMode::Major),
        "minor" => Ok(ScaleMode::Minor),
        "dorian" => Ok(ScaleMode::Dorian),
        "phrygian" => Ok(ScaleMode::Phrygian),
        "lydian" => Ok(ScaleMode::Lydian),
        "mixolydian" => Ok(ScaleMode::Mixolydian),
        "aeolian" => Ok(ScaleMode::Aeolian),
        "locrian" => Ok(ScaleMode::Locrian),
        "chromatic" => Ok(ScaleMode::Chromatic),
        "pentatonic" => Ok(ScaleMode::Pentatonic),
        "blues" => Ok(ScaleMode::Blues),
        other => Err(format!("unknown scale mode: '{}'", other)),
    }
}

/// A simple splitter that keeps quoted strings as single tokens.
struct SplitKeepQuotes<'a> {
    rest: &'a str,
}

impl<'a> SplitKeepQuotes<'a> {
    fn new(input: &'a str) -> Self {
        Self { rest: input }
    }
}

impl<'a> Iterator for SplitKeepQuotes<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        self.rest = self.rest.trim_start();
        if self.rest.is_empty() {
            return None;
        }
        if self.rest.starts_with('"') {
            // Find the closing quote
            if let Some(end) = self.rest[1..].find('"') {
                let token = &self.rest[..end + 2]; // include both quotes
                self.rest = &self.rest[end + 2..];
                Some(token)
            } else {
                // Unclosed quote — return rest
                let token = self.rest;
                self.rest = "";
                Some(token)
            }
        } else {
            // Regular whitespace-delimited token
            let end = self
                .rest
                .find(char::is_whitespace)
                .unwrap_or(self.rest.len());
            let token = &self.rest[..end];
            self.rest = &self.rest[end..];
            Some(token)
        }
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

    // ---- Comments & blanks ----

    #[test]
    fn test_blank_line() {
        assert_eq!(parse_line("").unwrap(), SourceLine::Blank);
        assert_eq!(parse_line("   ").unwrap(), SourceLine::Blank);
    }

    #[test]
    fn test_comment() {
        let result = parse_line("-- hello world").unwrap();
        assert_eq!(
            result,
            SourceLine::Comment("-- hello world".to_string())
        );
    }

    // ---- Directives ----

    #[test]
    fn test_bpm() {
        assert_eq!(parse_line("bpm 120").unwrap(), SourceLine::Bpm(120));
        assert_eq!(parse_line("bpm 60").unwrap(), SourceLine::Bpm(60));
    }

    #[test]
    fn test_bpm_invalid() {
        assert!(parse_line("bpm abc").is_err());
        assert!(parse_line("bpm 10").is_err()); // below 20
        assert!(parse_line("bpm 1000").is_err()); // above 999
    }

    #[test]
    fn test_sig() {
        assert_eq!(parse_line("sig 4/4").unwrap(), SourceLine::Sig(4, 4));
        assert_eq!(parse_line("sig 3/4").unwrap(), SourceLine::Sig(3, 4));
        assert_eq!(parse_line("sig 7/8").unwrap(), SourceLine::Sig(7, 8));
    }

    #[test]
    fn test_sig_invalid() {
        assert!(parse_line("sig 4/3").is_err()); // 3 not power of 2
        assert!(parse_line("sig 0/4").is_err()); // numerator 0
        assert!(parse_line("sig abc").is_err());
    }

    #[test]
    fn test_scale() {
        let result = parse_line("scale C minor").unwrap();
        assert_eq!(
            result,
            SourceLine::Scale(
                PitchRoot {
                    name: NoteLetter::C,
                    accidental: Accidental::Natural,
                },
                ScaleMode::Minor
            )
        );
    }

    #[test]
    fn test_scale_with_accidental() {
        let result = parse_line("scale Eb dorian").unwrap();
        assert_eq!(
            result,
            SourceLine::Scale(
                PitchRoot {
                    name: NoteLetter::E,
                    accidental: Accidental::Flat,
                },
                ScaleMode::Dorian
            )
        );
    }

    #[test]
    fn test_load() {
        let result = parse_line("load \"pads.rt\"").unwrap();
        assert_eq!(result, SourceLine::Load("pads.rt".to_string()));
    }

    #[test]
    fn test_load_unquoted() {
        assert!(parse_line("load pads.rt").is_err());
    }

    // ---- Pattern lines ----

    #[test]
    fn test_simple_pattern() {
        let result = parse_line("kick drums \"x ~ x ~\"").unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(p.name, "kick");
            assert_eq!(p.instrument, "drums");
            assert!(!p.muted);
            assert!(p.transforms.is_empty());
            assert_eq!(p.notation.sequence.steps.len(), 4);
        } else {
            panic!("expected pattern");
        }
    }

    #[test]
    fn test_muted_pattern() {
        let result = parse_line("; bass sine \"c2 ~ eb2 ~\"").unwrap();
        if let SourceLine::Pattern(p) = result {
            assert!(p.muted);
            assert_eq!(p.name, "bass");
            assert_eq!(p.instrument, "sine");
        } else {
            panic!("expected muted pattern");
        }
    }

    #[test]
    fn test_pattern_with_transforms() {
        let result =
            parse_line("lead saw \"c4 eb4 g4 bb4\" | rev | slow 2").unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(p.name, "lead");
            assert_eq!(p.transforms.len(), 2);
            assert_eq!(p.transforms[0], Transform::Rev);
            assert_eq!(p.transforms[1], Transform::Slow(2.0));
        } else {
            panic!("expected pattern");
        }
    }

    #[test]
    fn test_pattern_with_every_transform() {
        let result =
            parse_line("hats hihat \"x*8\" | every 4 rev").unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(
                p.transforms[0],
                Transform::Every(4, Box::new(Transform::Rev))
            );
        } else {
            panic!("expected pattern");
        }
    }

    #[test]
    fn test_pattern_with_arp() {
        let result =
            parse_line("arp piano \"[c3,e3,g3]\" | arp up").unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(p.transforms[0], Transform::Arp(ArpMode::Up));
        } else {
            panic!("expected pattern");
        }
    }

    #[test]
    fn test_pattern_with_scale_transform() {
        let result =
            parse_line("mel saw \"0 2 4 6\" | scale C minor").unwrap();
        if let SourceLine::Pattern(p) = result {
            if let Transform::Scale(root, mode) = &p.transforms[0] {
                assert_eq!(root.name, NoteLetter::C);
                assert_eq!(*mode, ScaleMode::Minor);
            } else {
                panic!("expected scale transform");
            }
        } else {
            panic!("expected pattern");
        }
    }

    #[test]
    fn test_pattern_with_effects() {
        let result =
            parse_line("pad pad \"[c3,eb3,g3]\" | gain 0.5 | lpf 800 | delay 0.25 0.4 | reverb 0.3")
                .unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(p.transforms.len(), 4);
            assert_eq!(p.transforms[0], Transform::Gain(0.5));
            assert_eq!(p.transforms[1], Transform::Lpf(800.0));
            assert_eq!(p.transforms[2], Transform::Delay(0.25, 0.4));
            assert_eq!(p.transforms[3], Transform::Reverb(0.3));
        } else {
            panic!("expected pattern");
        }
    }

    // ---- Error cases ----

    #[test]
    fn test_unknown_keyword_is_error() {
        // "foobar" with no matching pattern syntax
        assert!(parse_line("foobar").is_err());
    }

    #[test]
    fn test_pattern_missing_quotes() {
        assert!(parse_line("kick drums x ~ x ~").is_err());
    }

    // ---- SplitKeepQuotes ----

    #[test]
    fn test_split_keep_quotes() {
        let input = r#"kick drums "x ~ x ~" | rev"#;
        let tokens: Vec<&str> = SplitKeepQuotes::new(input).collect();
        assert_eq!(tokens, vec!["kick", "drums", "\"x ~ x ~\"", "|", "rev"]);
    }
}
