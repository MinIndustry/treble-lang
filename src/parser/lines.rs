//! Line-level parser for Treble Live DSL.
//!
//! Each source line is parsed independently into a [`SourceLine`].

use super::mini::parse_mini;
use crate::ast::program::*;

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
    if let Some(rest) = strip_keyword(trimmed, "phrase") {
        return parse_phrase(rest);
    }
    if let Some(rest) = strip_keyword(trimmed, "scale") {
        return parse_scale(rest);
    }
    if let Some(rest) = strip_keyword(trimmed, "load") {
        return parse_load(rest);
    }
    if let Some(rest) = strip_keyword(trimmed, "include") {
        return parse_include(rest);
    }
    // Early builds used `use`; keep it as a parse-compatible alias while all
    // diagnostics and documentation point to the clearer `include` spelling.
    if let Some(rest) = strip_keyword(trimmed, "use") {
        return parse_include(rest);
    }

    // Group markers. `group` is a reserved word: a pattern cannot be named it.
    if trimmed == "group" || strip_keyword(trimmed, "group").is_some() {
        return parse_group_header(trimmed, false);
    }
    if let Some(rest) = trimmed.strip_prefix('}') {
        return parse_group_footer(rest);
    }

    // Muted pattern
    if let Some(rest) = trimmed.strip_prefix(';') {
        let rest = rest.trim_start();
        if rest == "group" || strip_keyword(rest, "group").is_some() {
            return parse_group_header(rest, true);
        }
        return parse_pattern_line(rest, true);
    }

    // Pattern line
    parse_pattern_line(trimmed, false)
}

/// `[;] group <name> {` — the shared filters live on the closing `}` line.
fn parse_group_header(line: &str, muted: bool) -> Result<SourceLine, String> {
    let rest = strip_keyword(line, "group")
        .ok_or_else(|| "group: expected 'group <name> {'".to_string())?;
    let Some(name) = rest.strip_suffix('{') else {
        return Err(
            "group: the header must end with '{' (filters go on the closing '}')".to_string(),
        );
    };
    let name = name.trim();
    validate_identifier(name).map_err(|error| format!("group: {error}"))?;
    Ok(SourceLine::GroupStart {
        muted,
        name: name.to_string(),
    })
}

/// `}` closing a group, optionally followed by `| transform | transform`.
fn parse_group_footer(rest: &str) -> Result<SourceLine, String> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok(SourceLine::GroupEnd(Vec::new()));
    }
    let Some(pipeline) = rest.strip_prefix('|') else {
        return Err(format!(
            "group: expected '| transform' after '}}', got '{rest}'"
        ));
    };
    Ok(SourceLine::GroupEnd(parse_transforms(pipeline.trim())?))
}

/// Strip a keyword prefix followed by whitespace. Returns the rest.
fn strip_keyword<'a>(input: &'a str, keyword: &str) -> Option<&'a str> {
    if let Some(rest) = input.strip_prefix(keyword)
        && rest.starts_with(char::is_whitespace)
    {
        Some(rest.trim_start())
    } else {
        None
    }
}

// --- Directive parsers ---

/// `phrase <cycles>` — the unit changes are quantised to.
///
/// A drop has to land on the top of a phrase, not merely on the next cycle, so
/// this is what the consumer aligns its snapshot swaps to.
fn parse_phrase(input: &str) -> Result<SourceLine, String> {
    let cycles: u32 = input.trim().parse().map_err(|_| {
        format!(
            "phrase: expected a number of cycles, got '{}'",
            input.trim()
        )
    })?;
    if cycles == 0 {
        return Err("phrase: a phrase is at least one cycle".to_string());
    }
    Ok(SourceLine::Phrase(cycles))
}

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
        return Err(format!(
            "expected time signature N/D, got '{}'",
            rest.trim()
        ));
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
    if den == 0 {
        return Err("time signature denominator must be > 0".to_string());
    }
    Ok(SourceLine::Sig(num, den))
}

fn parse_scale(rest: &str) -> Result<SourceLine, String> {
    let mut tokens = rest.split_whitespace();
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

fn parse_include(rest: &str) -> Result<SourceLine, String> {
    let name = rest.trim();
    validate_identifier(name)?;
    Ok(SourceLine::Include(name.to_string()))
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
        group: None,
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
    let transform = parse_transform_keyword(keyword, &mut parts)?;
    // `every` and the filter calls consume the rest of the segment themselves;
    // everything else has a fixed arity, and silently ignoring extra words
    // hides typos.
    if let Some(extra) = parts.next() {
        return Err(format!("{}: unexpected argument '{}'", keyword, extra));
    }
    Ok(transform)
}

fn parse_transform_keyword(
    keyword: &str,
    parts: &mut std::str::SplitWhitespace,
) -> Result<Transform, String> {
    match keyword {
        "rev" => Ok(Transform::Rev),
        "fast" => Ok(Transform::Fast(parse_transform_ramp(parts, "fast")?)),
        "slow" => Ok(Transform::Slow(parse_transform_ramp(parts, "slow")?)),
        "ramp" => {
            let text = parts
                .next()
                .ok_or_else(|| "ramp: expected a number of cycles".to_string())?;
            let cycles: u32 = text
                .parse()
                .map_err(|_| format!("ramp: invalid number of cycles '{}'", text))?;
            if cycles == 0 {
                return Err("ramp: a span needs at least one cycle".to_string());
            }
            // Omitting the curve means `lin`, so every buffer written before
            // curves existed keeps its meaning.
            let curve = match parts.next() {
                None => RampCurve::Linear,
                Some("lin") => RampCurve::Linear,
                Some("exp") => RampCurve::Exponential,
                Some(other) => {
                    return Err(format!(
                        "ramp: '{other}' is not a curve; use 'lin' (the default) or 'exp'"
                    ));
                }
            };
            Ok(Transform::RampSpan { cycles, curve })
        }
        "every" => {
            let n_str = parts
                .next()
                .ok_or_else(|| "every: expected cycle count".to_string())?;
            let n: u32 = n_str
                .parse()
                .map_err(|_| format!("every: invalid number '{}'", n_str))?;
            let rest: String = parts.by_ref().collect::<Vec<&str>>().join(" ");
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
            let text = parts
                .next()
                .ok_or_else(|| "oct: expected offset".to_string())?;
            let parse = |part: &str| {
                part.parse::<i32>()
                    .map_err(|_| format!("oct: invalid offset '{}'", part))
            };
            Ok(Transform::Oct(ramp_from_text(text, "oct", parse)?))
        }
        "vel" => {
            let vel = parse_transform_ramp(parts, "vel")?;
            validate_within("vel", &vel, 0.0, 1.0)?;
            Ok(Transform::Vel(vel))
        }
        "gain" => {
            let gain = parse_transform_ramp(parts, "gain")?;
            validate_within("gain", &gain, 0.0, 2.0)?;
            Ok(Transform::Gain(gain))
        }
        "pan" => {
            let first = parts
                .next()
                .ok_or_else(|| "pan: expected a position or a waveform".to_string())?;
            // A number, or a range of them, is a fixed position; anything else
            // must name a waveform. The first character decides, so `-0.4..0.4`
            // is read as a position rather than as an unknown waveform.
            if first.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+' || c == '.') {
                let position = ramp_from_text(first, "pan", |part| {
                    part.parse::<f64>()
                        .map_err(|_| format!("pan: invalid position '{}'", part))
                })?;
                validate_within("pan", &position, -1.0, 1.0)?;
                Ok(Transform::Pan(position))
            } else {
                Ok(Transform::AutoPan(parse_pan_sweep(first, parts)?))
            }
        }
        "lpf" => {
            let cutoff = parse_transform_ramp(parts, "lpf")?;
            validate_at_least("lpf", &cutoff, 0.0)?;
            Ok(Transform::Lpf(cutoff))
        }
        "hpf" => {
            let cutoff = parse_transform_ramp(parts, "hpf")?;
            validate_at_least("hpf", &cutoff, 0.0)?;
            Ok(Transform::Hpf(cutoff))
        }
        "delay" => {
            let time = parse_transform_ramp(parts, "delay time")?;
            validate_at_least("delay time", &time, 0.0)?;
            let feedback = parse_transform_ramp(parts, "delay feedback")?;
            validate_within("delay feedback", &feedback, 0.0, 0.99)?;
            let mix = match parts.next() {
                Some(text) => {
                    let mix = ramp_from_text(text, "delay mix", |part| {
                        part.parse::<f64>()
                            .map_err(|_| format!("delay mix: invalid number '{}'", part))
                    })?;
                    validate_within("delay mix", &mix, 0.0, 1.0)?;
                    Some(mix)
                }
                None => None,
            };
            Ok(Transform::Delay(time, feedback, mix))
        }
        "reverb" => {
            let amount = parse_transform_ramp(parts, "reverb")?;
            validate_within("reverb", &amount, 0.0, 1.0)?;
            Ok(Transform::Reverb(amount))
        }
        // A raw engine filter, named explicitly.
        "fx" => {
            let filter = parts
                .next()
                .ok_or_else(|| "fx: expected a filter name".to_string())?;
            Ok(Transform::Fx(parse_fx_call(filter, parts)?))
        }
        // Short aliases for the filters worth reaching for mid-performance.
        // Only the spelling lives here; what each one maps to is the
        // consumer's business.
        keyword if FX_ALIASES.contains(&keyword) => {
            Ok(Transform::Fx(parse_fx_call(keyword, parts)?))
        }
        other => Err(format!("unknown transform: '{}'", other)),
    }
}

/// Aliases accepted in place of `fx <filter>`.
///
/// Listing them keeps an unknown keyword an unknown keyword, rather than
/// silently turning a typo into a lookup for a filter that does not exist.
/// The curated transform spellings (`lpf`, `pan`, …) are included so a `def`
/// block's effect chain reads the same as a pattern's. In a pattern line the
/// dedicated `Transform` arms match first, so those keep their own semantics.
pub const FX_ALIASES: &[&str] = &[
    "trem", "bpf", "rbpf", "avg", "clip", "comp", "limit", "lpf", "hpf", "delay", "reverb", "pan",
    "gain",
];

/// Parse a whole `fx …` or alias line, for reuse inside `def` blocks (§6.7).
pub(crate) fn parse_fx_line(line: &str) -> Result<FxCall, String> {
    let mut parts = line.split_whitespace();
    let keyword = parts
        .next()
        .ok_or_else(|| "expected a filter name".to_string())?;
    match keyword {
        "fx" => {
            let filter = parts
                .next()
                .ok_or_else(|| "fx: expected a filter name".to_string())?;
            parse_fx_call(filter, &mut parts)
        }
        alias if FX_ALIASES.contains(&alias) => parse_fx_call(alias, &mut parts),
        other => Err(format!(
            "'{other}' is not a filter line; use 'fx <filter> …' or a short alias"
        )),
    }
}

fn parse_fx_call(filter: &str, parts: &mut std::str::SplitWhitespace) -> Result<FxCall, String> {
    if filter.is_empty() {
        return Err("fx: expected a filter name".to_string());
    }
    let mut args = Vec::new();
    let mut seen_named = false;
    for token in parts.by_ref() {
        match token.split_once('=') {
            Some((name, value)) => {
                if name.is_empty() {
                    return Err(format!("{filter}: '{token}' is missing a parameter name"));
                }
                seen_named = true;
                args.push(FxArg::Named(
                    name.to_string(),
                    parse_fx_value(filter, name, value)?,
                ));
            }
            None => {
                // Positional arguments count from the front, so one appearing
                // after a named argument has no well-defined slot.
                if seen_named {
                    return Err(format!(
                        "{filter}: '{token}' comes after a named argument; give it a name too"
                    ));
                }
                args.push(FxArg::Positional(parse_fx_value(
                    filter, "argument", token,
                )?));
            }
        }
    }
    Ok(FxCall {
        filter: filter.to_string(),
        args,
    })
}

fn parse_fx_value(filter: &str, label: &str, text: &str) -> Result<FxValue, String> {
    let lowered = text.to_ascii_lowercase();
    let (number, hertz) = match lowered.strip_suffix("hz") {
        Some(number) => (number, true),
        None => (lowered.as_str(), false),
    };
    let name = format!("{filter} {label}");
    let ramp = ramp_from_text(number, &name, |part| {
        part.parse::<f64>().map_err(|_| {
            if hertz {
                format!("{name}: invalid frequency '{text}'")
            } else {
                format!("{name}: invalid number '{text}'")
            }
        })
    })?;
    // Nothing range-checks these: which interval a filter's parameter accepts
    // is the consumer's registry to know, not this crate's (§4.5).
    Ok(if hertz {
        FxValue::Hertz(ramp)
    } else {
        FxValue::Plain(ramp)
    })
}

fn parse_pan_sweep(wave: &str, parts: &mut std::str::SplitWhitespace) -> Result<PanSweep, String> {
    let wave = parse_lfo_wave(wave)?;
    let rate_text = parts
        .next()
        .ok_or_else(|| "pan: expected a rate after the waveform".to_string())?;
    let rate = parse_lfo_rate(rate_text)?;
    let depth = match parts.next() {
        Some(text) => Some(
            text.parse::<f64>()
                .map_err(|_| format!("pan depth: invalid number '{}'", text))?,
        ),
        None => None,
    };
    Ok(PanSweep { wave, rate, depth })
}

fn parse_lfo_wave(text: &str) -> Result<LfoWave, String> {
    match text {
        "sine" | "sin" => Ok(LfoWave::Sine),
        "tri" | "triangle" => Ok(LfoWave::Triangle),
        "sq" | "square" => Ok(LfoWave::Square),
        "saw" => Ok(LfoWave::Saw),
        "rand" | "random" => Ok(LfoWave::Random),
        other => Err(format!(
            "pan: '{}' is not a position or a waveform (sine, tri, sq, saw, rand)",
            other
        )),
    }
}

/// A bare number is a period in cycles; an `hz` suffix is absolute frequency.
fn parse_lfo_rate(text: &str) -> Result<LfoRate, String> {
    let lowered = text.to_ascii_lowercase();
    match lowered.strip_suffix("hz") {
        Some(number) => number
            .parse::<f64>()
            .map(LfoRate::Hertz)
            .map_err(|_| format!("pan rate: invalid frequency '{}'", text)),
        None => text
            .parse::<f64>()
            .map(LfoRate::Cycles)
            .map_err(|_| format!("pan rate: invalid number of cycles '{}'", text)),
    }
}

/// `0.4` or `0.4..1.0` — an argument that may travel across the line's `ramp`.
fn parse_transform_ramp(
    parts: &mut std::str::SplitWhitespace,
    name: &str,
) -> Result<Ramp<f64>, String> {
    let text = parts
        .next()
        .ok_or_else(|| format!("{}: expected number", name))?;
    ramp_from_text(text, name, |part| {
        part.parse::<f64>()
            .map_err(|_| format!("{}: invalid number '{}'", name, part))
    })
}

/// `4`, `4..16` or `2>4>8>16`, from one whitespace-delimited argument.
///
/// The two spellings mean different things, so mixing them in one argument is
/// an error rather than a guess.
fn ramp_from_text<T: Copy>(
    text: &str,
    name: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<Ramp<T>, String> {
    let sweeps = text.contains("..");
    let steps = text.contains('>');
    if sweeps && steps {
        return Err(format!(
            "{name}: '{text}' mixes '..' and '>'. Use '..' to sweep smoothly, or '>' to hold each value in turn."
        ));
    }
    if sweeps {
        let mut ends = text.split("..");
        let from = parse(ends.next().unwrap_or_default())?;
        let to = parse(ends.next().unwrap_or_default())?;
        if ends.next().is_some() {
            return Err(format!(
                "{name}: '{text}' has more than two ends. Use '>' for a sequence of held values."
            ));
        }
        return Ok(Ramp::Sweep { from, to });
    }
    let mut stages = text.split('>');
    let first = parse(stages.next().unwrap_or_default())?;
    let rest = stages.map(parse).collect::<Result<Vec<T>, String>>()?;
    Ok(Ramp::steps(first, rest))
}

/// Every value a ramp passes through has to be legal, not only the one it
/// starts on — the whole travel is played, so the whole travel is checked.
fn validate_within(name: &str, ramp: &Ramp<f64>, low: f64, high: f64) -> Result<(), String> {
    for value in ramp.values() {
        if !value.is_finite() || !(low..=high).contains(&value) {
            return Err(format!(
                "{name}: {value} is outside the supported range {low}-{high}"
            ));
        }
    }
    Ok(())
}

/// The half-open case: a cutoff in hertz or a delay time has a floor but no
/// ceiling this crate is entitled to invent.
fn validate_at_least(name: &str, ramp: &Ramp<f64>, low: f64) -> Result<(), String> {
    for value in ramp.values() {
        if !value.is_finite() || value < low {
            return Err(format!("{name}: {value} is below the minimum of {low}"));
        }
    }
    Ok(())
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
    Ok(PitchRoot {
        name: letter,
        accidental,
    })
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
        assert_eq!(result, SourceLine::Comment("-- hello world".to_string()));
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
        assert!(parse_line("sig 0/4").is_err()); // numerator 0
        assert!(parse_line("sig 4/0").is_err()); // denominator 0
        assert!(parse_line("sig abc").is_err());
    }

    #[test]
    fn test_sig_accepts_non_binary_denominator() {
        assert_eq!(parse_line("sig 4/3").unwrap(), SourceLine::Sig(4, 3));
        assert_eq!(parse_line("sig 5/6").unwrap(), SourceLine::Sig(5, 6));
    }

    #[test]
    fn test_explicit_instrument_include() {
        assert_eq!(
            parse_line("include hihat").unwrap(),
            SourceLine::Include("hihat".into())
        );
        assert_eq!(
            parse_line("use hihat").unwrap(),
            SourceLine::Include("hihat".into())
        );
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
        let result = parse_line("lead saw \"c4 eb4 g4 bb4\" | rev | slow 2").unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(p.name, "lead");
            assert_eq!(p.transforms.len(), 2);
            assert_eq!(p.transforms[0], Transform::Rev);
            assert_eq!(p.transforms[1], Transform::Slow(Ramp::fixed(2.0)));
        } else {
            panic!("expected pattern");
        }
    }

    #[test]
    fn test_pattern_with_every_transform() {
        let result = parse_line("hats hihat \"x*8\" | every 4 rev").unwrap();
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
        let result = parse_line("arp piano \"[c3,e3,g3]\" | arp up").unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(p.transforms[0], Transform::Arp(ArpMode::Up));
        } else {
            panic!("expected pattern");
        }
    }

    #[test]
    fn test_pattern_with_scale_transform() {
        let result = parse_line("mel saw \"0 2 4 6\" | scale C minor").unwrap();
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
        let result = parse_line(
            "pad pad \"[c3,eb3,g3]\" | gain 0.5 | lpf 800 | delay 0.25 0.4 | reverb 0.3",
        )
        .unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(p.transforms.len(), 4);
            assert_eq!(p.transforms[0], Transform::Gain(Ramp::fixed(0.5)));
            assert_eq!(p.transforms[1], Transform::Lpf(Ramp::fixed(800.0)));
            assert_eq!(
                p.transforms[2],
                Transform::Delay(Ramp::fixed(0.25), Ramp::fixed(0.4), None)
            );
            assert_eq!(p.transforms[3], Transform::Reverb(Ramp::fixed(0.3)));
        } else {
            panic!("expected pattern");
        }
    }

    #[test]
    fn test_pan_and_vel_transforms() {
        let result =
            parse_line("lead saw \"c4 e4\" | pan -1.0 | vel 0.42 | gain 1.4 | delay 0.2 0.3 0.5")
                .unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(p.transforms[0], Transform::Pan(Ramp::fixed(-1.0)));
            assert_eq!(p.transforms[1], Transform::Vel(Ramp::fixed(0.42)));
            assert_eq!(p.transforms[2], Transform::Gain(Ramp::fixed(1.4)));
            assert_eq!(
                p.transforms[3],
                Transform::Delay(Ramp::fixed(0.2), Ramp::fixed(0.3), Some(Ramp::fixed(0.5)))
            );
        } else {
            panic!("expected pattern");
        }
    }

    #[test]
    fn test_pan_sweep_variants() {
        let sweep = |source: &str| match parse_line(source).unwrap() {
            SourceLine::Pattern(p) => match p.transforms[0] {
                Transform::AutoPan(sweep) => sweep,
                ref other => panic!("expected a swept pan, got {other:?}"),
            },
            other => panic!("expected a pattern, got {other:?}"),
        };

        let basic = sweep("hat hihat \"x*8\" | pan sine 4");
        assert_eq!(basic.wave, LfoWave::Sine);
        assert_eq!(basic.rate, LfoRate::Cycles(4.0));
        assert_eq!(basic.depth, None);

        let deep = sweep("hat hihat \"x*8\" | pan sq 1 0.6");
        assert_eq!(deep.wave, LfoWave::Square);
        assert_eq!(deep.rate, LfoRate::Cycles(1.0));
        assert_eq!(deep.depth, Some(0.6));

        // An `hz` suffix opts out of tempo-relative timing.
        let absolute = sweep("hat hihat \"x*8\" | pan tri 0.5hz");
        assert_eq!(absolute.wave, LfoWave::Triangle);
        assert_eq!(absolute.rate, LfoRate::Hertz(0.5));

        assert_eq!(sweep("p pad \"0\" | pan saw 2").wave, LfoWave::Saw);
        assert_eq!(sweep("p pad \"0\" | pan rand 1").wave, LfoWave::Random);
        // Long spellings and mixed case on the suffix both work.
        assert_eq!(
            sweep("p pad \"0\" | pan triangle 2HZ").rate,
            LfoRate::Hertz(2.0)
        );
    }

    #[test]
    fn test_fixed_pan_still_parses_as_a_position() {
        let result = parse_line("kick kick \"x ~\" | pan -0.3").unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(p.transforms[0], Transform::Pan(Ramp::fixed(-0.3)));
        } else {
            panic!("expected pattern");
        }
    }

    #[test]
    fn test_pan_sweep_errors_are_specific() {
        let message = |source: &str| parse_line(source).unwrap_err();
        assert!(
            message("p pad \"0\" | pan wobble 4").contains("not a position or a waveform"),
            "got: {}",
            message("p pad \"0\" | pan wobble 4")
        );
        assert!(message("p pad \"0\" | pan sine").contains("expected a rate"));
        assert!(message("p pad \"0\" | pan sine fast").contains("invalid number of cycles"));
        assert!(message("p pad \"0\" | pan sine 4 loud").contains("invalid number"));
        assert!(message("p pad \"0\" | pan sine 4hz 0.5 extra").contains("unexpected argument"));
    }

    fn fx_call(source: &str) -> FxCall {
        match parse_line(source).unwrap() {
            SourceLine::Pattern(p) => match p.transforms[0].clone() {
                Transform::Fx(call) => call,
                other => panic!("expected a filter call, got {other:?}"),
            },
            other => panic!("expected a pattern, got {other:?}"),
        }
    }

    #[test]
    fn test_fx_call_positional_and_named_arguments() {
        let call = fx_call("p pad \"0\" | fx Compressor 0.3 8");
        assert_eq!(call.filter, "Compressor");
        assert_eq!(
            call.args,
            vec![
                FxArg::Positional(FxValue::Plain(Ramp::fixed(0.3))),
                FxArg::Positional(FxValue::Plain(Ramp::fixed(8.0))),
            ]
        );

        let call = fx_call("p pad \"0\" | fx Limiter threshold=0.8 release=0.3");
        assert_eq!(
            call.args,
            vec![
                FxArg::Named("threshold".into(), FxValue::Plain(Ramp::fixed(0.8))),
                FxArg::Named("release".into(), FxValue::Plain(Ramp::fixed(0.3))),
            ]
        );

        // Positional first, then named, is allowed.
        let call = fx_call("p pad \"0\" | fx Compressor 0.3 ratio=8");
        assert_eq!(
            call.args,
            vec![
                FxArg::Positional(FxValue::Plain(Ramp::fixed(0.3))),
                FxArg::Named("ratio".into(), FxValue::Plain(Ramp::fixed(8.0))),
            ]
        );

        // A filter taking no arguments at all is still a valid call.
        assert!(fx_call("p pad \"0\" | fx Limiter").args.is_empty());
    }

    #[test]
    fn test_fx_aliases_parse_as_filter_calls() {
        for (source, filter) in [
            ("p pad \"0\" | trem 4 0.6", "trem"),
            ("p pad \"0\" | bpf 200 1200", "bpf"),
            ("p pad \"0\" | rbpf 900 12", "rbpf"),
            ("p pad \"0\" | avg 5", "avg"),
            ("p pad \"0\" | clip 0.4", "clip"),
            ("p pad \"0\" | comp 0.3 8", "comp"),
            ("p pad \"0\" | limit 0.9", "limit"),
        ] {
            assert_eq!(fx_call(source).filter, filter);
        }
    }

    #[test]
    fn test_fx_hz_suffix_is_kept_distinct_from_a_bare_number() {
        let call = fx_call("p pad \"0\" | trem 0.5hz");
        assert_eq!(
            call.args,
            vec![FxArg::Positional(FxValue::Hertz(Ramp::fixed(0.5)))]
        );
        let call = fx_call("p pad \"0\" | trem 4");
        assert_eq!(
            call.args,
            vec![FxArg::Positional(FxValue::Plain(Ramp::fixed(4.0)))]
        );
        // Mixed case on the suffix too.
        let call = fx_call("p pad \"0\" | fx Tremolo frequency=2HZ");
        assert_eq!(
            call.args,
            vec![FxArg::Named(
                "frequency".into(),
                FxValue::Hertz(Ramp::fixed(2.0))
            )]
        );
    }

    #[test]
    fn test_fx_argument_errors_are_specific() {
        let message = |source: &str| parse_line(source).unwrap_err();
        assert!(message("p pad \"0\" | fx").contains("expected a filter name"));
        assert!(message("p pad \"0\" | trem loud").contains("invalid number"));
        assert!(message("p pad \"0\" | trem =4").contains("missing a parameter name"));
        assert!(
            message("p pad \"0\" | fx Compressor ratio=8 0.3").contains("comes after a named"),
            "got: {}",
            message("p pad \"0\" | fx Compressor ratio=8 0.3")
        );
        // An unknown keyword stays an unknown keyword rather than becoming a
        // lookup for a filter nobody registered.
        assert!(message("p pad \"0\" | revv").contains("unknown transform"));
    }

    #[test]
    fn test_ramp_span_and_ranged_transforms() {
        let result = parse_line("sn snare \"x(4..16,4)\" | vel 0.4..1.0 | ramp 16").unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(
                p.transforms[0],
                Transform::Vel(Ramp::Sweep { from: 0.4, to: 1.0 })
            );
            assert_eq!(
                p.transforms[1],
                Transform::RampSpan {
                    cycles: 16,
                    curve: RampCurve::Linear
                }
            );
        } else {
            panic!("expected pattern");
        }

        let result = parse_line("p pad \"0\" | oct 0..-2 | fast 1..4 | slow 2..1").unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(
                p.transforms[0],
                Transform::Oct(Ramp::Sweep { from: 0, to: -2 })
            );
            assert_eq!(
                p.transforms[1],
                Transform::Fast(Ramp::Sweep { from: 1.0, to: 4.0 })
            );
            assert_eq!(
                p.transforms[2],
                Transform::Slow(Ramp::Sweep { from: 2.0, to: 1.0 })
            );
        } else {
            panic!("expected pattern");
        }
    }

    #[test]
    fn test_step_chains_in_transform_arguments() {
        let result = parse_line("sn snare \"x(2>4>8>16,4)\" | vel 0.3>0.6>1.0 | ramp 16").unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(
                p.transforms[0],
                Transform::Vel(Ramp::Steps {
                    first: 0.3,
                    rest: vec![0.6, 1.0]
                })
            );
            assert_eq!(
                p.transforms[1],
                Transform::RampSpan {
                    cycles: 16,
                    curve: RampCurve::Linear
                }
            );
        } else {
            panic!("expected pattern");
        }

        // Negative stages read correctly.
        let result = parse_line("p pad \"0\" | oct 0>-1>-2").unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(
                p.transforms[0],
                Transform::Oct(Ramp::Steps {
                    first: 0,
                    rest: vec![-1, -2]
                })
            );
        } else {
            panic!("expected pattern");
        }
    }

    #[test]
    fn test_mixing_the_two_travel_spellings_is_rejected() {
        let error = parse_line("p pad \"0\" | vel 0.2..0.5>1.0").unwrap_err();
        assert!(error.contains("mixes"), "{error}");
        // And the message says what each one does.
        assert!(error.contains("sweep") && error.contains("hold"), "{error}");

        let error = parse_line("p pad \"0\" | vel 0.2..0.5..1.0").unwrap_err();
        assert!(error.contains("more than two ends"), "{error}");
    }

    fn transforms(source: &str) -> Vec<Transform> {
        match parse_line(source).unwrap() {
            SourceLine::Pattern(p) => p.transforms,
            other => panic!("expected a pattern, got {other:?}"),
        }
    }

    #[test]
    fn test_audio_transforms_take_ranges() {
        // The line the engine's sweep mechanism exists for.
        assert_eq!(
            transforms("p pad \"0\" | lpf 300..9000 | ramp 16"),
            vec![
                Transform::Lpf(Ramp::Sweep {
                    from: 300.0,
                    to: 9000.0
                }),
                Transform::RampSpan {
                    cycles: 16,
                    curve: RampCurve::Linear
                },
            ]
        );

        assert_eq!(
            transforms("p pad \"0\" | hpf 40>200>800"),
            vec![Transform::Hpf(Ramp::Steps {
                first: 40.0,
                rest: vec![200.0, 800.0]
            })]
        );
        assert_eq!(
            transforms("p pad \"0\" | gain 0.2..1.0"),
            vec![Transform::Gain(Ramp::Sweep { from: 0.2, to: 1.0 })]
        );
        assert_eq!(
            transforms("p pad \"0\" | reverb 0.0..0.6"),
            vec![Transform::Reverb(Ramp::Sweep { from: 0.0, to: 0.6 })]
        );
        // A fixed position that travels, still told apart from a waveform.
        assert_eq!(
            transforms("p pad \"0\" | pan -0.8..0.8"),
            vec![Transform::Pan(Ramp::Sweep {
                from: -0.8,
                to: 0.8
            })]
        );
        // All three of delay's numbers travel independently.
        assert_eq!(
            transforms("p pad \"0\" | delay 0.1..0.4 0.2>0.6 0.3..0.9"),
            vec![Transform::Delay(
                Ramp::Sweep { from: 0.1, to: 0.4 },
                Ramp::Steps {
                    first: 0.2,
                    rest: vec![0.6]
                },
                Some(Ramp::Sweep { from: 0.3, to: 0.9 }),
            )]
        );
    }

    #[test]
    fn test_fx_arguments_take_ranges() {
        // `hz` still marks an absolute frequency, range or not.
        let call = fx_call("p pad \"0\" | trem 2..8hz 0.7");
        assert_eq!(
            call.args,
            vec![
                FxArg::Positional(FxValue::Hertz(Ramp::Sweep { from: 2.0, to: 8.0 })),
                FxArg::Positional(FxValue::Plain(Ramp::fixed(0.7))),
            ]
        );
        let FxArg::Positional(ref rate) = call.args[0] else {
            panic!("expected a positional argument");
        };
        assert!(rate.travels(), "a ranged fx argument needs a ramp span");
        assert!(!matches!(&call.args[1], FxArg::Positional(v) if v.travels()));

        let call = fx_call("p pad \"0\" | fx Tremolo depth=0.2>0.9");
        assert_eq!(
            call.args,
            vec![FxArg::Named(
                "depth".into(),
                FxValue::Plain(Ramp::Steps {
                    first: 0.2,
                    rest: vec![0.9]
                })
            )]
        );
    }

    #[test]
    fn test_travels_reports_on_every_ranged_argument() {
        let fixed = transforms("p pad \"0\" | lpf 900");
        assert_eq!(fixed, vec![Transform::Lpf(Ramp::fixed(900.0))]);
        let Transform::Lpf(ref cutoff) = fixed[0] else {
            panic!("expected lpf");
        };
        assert!(!cutoff.travels(), "a plain value must not ask for a ramp");

        let swept = transforms("p pad \"0\" | lpf 300..9000");
        let Transform::Lpf(ref cutoff) = swept[0] else {
            panic!("expected lpf");
        };
        assert!(cutoff.travels());
        assert_eq!(cutoff.start(), 300.0);
        assert_eq!(cutoff.values(), vec![300.0, 9000.0]);
    }

    #[test]
    fn test_every_value_of_an_audio_range_is_validated() {
        // The far end of the travel is played too, so it is checked too.
        let error = parse_line("p pad \"0\" | reverb 0.5..1.4").unwrap_err();
        assert!(error.contains("1.4"), "{error}");
        assert!(error.contains("0-1"), "{error}");

        assert!(
            parse_line("p pad \"0\" | gain 0.5>1.0>2.5")
                .unwrap_err()
                .contains("2.5")
        );
        assert!(
            parse_line("p pad \"0\" | pan -0.5..1.5")
                .unwrap_err()
                .contains("1.5")
        );
        assert!(
            parse_line("p pad \"0\" | vel 0.4..1.2")
                .unwrap_err()
                .contains("1.2")
        );
        assert!(
            parse_line("p pad \"0\" | delay 0.2 0.4..1.0")
                .unwrap_err()
                .contains("delay feedback")
        );
        assert!(
            parse_line("p pad \"0\" | lpf 900..-20")
                .unwrap_err()
                .contains("below the minimum")
        );
        // A filter's own limits stay the consumer's business (§4.5), so a wild
        // `fx` argument is not rejected here.
        assert!(parse_line("p pad \"0\" | trem 0..900").is_ok());
    }

    #[test]
    fn test_ramp_span_must_be_at_least_one_cycle() {
        assert!(
            parse_line("p pad \"0\" | ramp 0")
                .unwrap_err()
                .contains("at least one cycle")
        );
        assert!(
            parse_line("p pad \"0\" | ramp")
                .unwrap_err()
                .contains("expected a number")
        );
    }

    #[test]
    fn test_ramp_span_carries_a_curve() {
        // Omitted means linear, so old buffers keep their meaning.
        assert_eq!(
            transforms("p pad \"0\" | lpf 300..9000 | ramp 16"),
            transforms("p pad \"0\" | lpf 300..9000 | ramp 16 lin")
        );
        assert_eq!(
            transforms("p pad \"0\" | ramp 8 exp"),
            vec![Transform::RampSpan {
                cycles: 8,
                curve: RampCurve::Exponential
            }]
        );
        assert_eq!(RampCurve::default(), RampCurve::Linear);
    }

    #[test]
    fn test_ramp_curve_errors_are_specific() {
        let error = parse_line("p pad \"0\" | ramp 8 sine").unwrap_err();
        assert!(error.contains("'sine' is not a curve"), "{error}");
        assert!(error.contains("lin") && error.contains("exp"), "{error}");
        // The arity is still fixed: one curve, not two.
        assert!(
            parse_line("p pad \"0\" | ramp 8 exp lin")
                .unwrap_err()
                .contains("unexpected argument")
        );
    }

    #[test]
    fn test_trailing_transform_argument_is_rejected() {
        let error = parse_line("lead saw \"c4\" | pan -1.0 0.5").unwrap_err();
        assert!(error.contains("unexpected argument"), "got: {error}");
    }

    #[test]
    fn test_every_still_accepts_a_nested_transform() {
        let result = parse_line("lead saw \"c4\" | every 4 every 2 rev").unwrap();
        if let SourceLine::Pattern(p) = result {
            assert_eq!(
                p.transforms[0],
                Transform::Every(4, Box::new(Transform::Every(2, Box::new(Transform::Rev))))
            );
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
