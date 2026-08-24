//! `def` block parser.
//!
//! Blocks are the one multi-line construct in the language. `parser::mod`
//! collects a block's raw lines, then hands them here as a whole.

use crate::ast::instrument::*;
use crate::parser::lines::parse_fx_line;

/// Parse a complete `def` block, braces included.
pub fn parse_instrument_def(lines: &[&str]) -> Result<InstrumentDef, String> {
    let joined = lines.join("\n");
    let open = joined
        .find('{')
        .ok_or_else(|| "def: expected '{' to open the block".to_string())?;
    let close = joined
        .rfind('}')
        .ok_or_else(|| "def: expected '}' to close the block".to_string())?;
    if close < open {
        return Err("def: '}' appears before '{'".to_string());
    }

    let header = joined[..open].trim();
    let name = header
        .strip_prefix("def")
        .ok_or_else(|| "def: expected the block to start with 'def'".to_string())?
        .trim();
    if !is_identifier(name) {
        return Err(format!(
            "def: '{name}' is not a valid instrument name; use letters, digits and underscores, starting with a letter or underscore"
        ));
    }

    let mut definition = InstrumentDef {
        name: name.to_string(),
        ..InstrumentDef::default()
    };
    // Stage lines accumulate, so they are gathered before being folded in.
    let mut amplitude_stages = StageSet::default();
    let mut pitch_stages = StageSet::default();

    let body = &joined[open + 1..close];
    let mut remaining: Vec<&str> = body.lines().collect();
    let mut index = 0;
    while index < remaining.len() {
        let line = remaining[index].trim();
        index += 1;
        if line.is_empty() || line.starts_with("--") {
            continue;
        }
        // A tone may open a nested block for its own envelope stages.
        if let Some(head) = line.strip_suffix('{') {
            let mut nested = Vec::new();
            let mut depth = 1usize;
            while index < remaining.len() && depth > 0 {
                let inner = remaining[index].trim();
                index += 1;
                depth += count_braces(inner, '{');
                depth -= count_braces(inner, '}').min(depth);
                if depth > 0 {
                    nested.push(inner);
                }
            }
            if depth > 0 {
                return Err(format!("{}: nested block is not closed", definition.name));
            }
            definition
                .tones
                .push(parse_tone(head.trim(), &nested, &definition.name)?);
            continue;
        }
        apply_line(
            line,
            &mut definition,
            &mut amplitude_stages,
            &mut pitch_stages,
        )?;
    }

    definition.amplitude = merge_envelope(
        definition.amplitude,
        amplitude_stages,
        "env",
        &definition.name,
    )?;
    definition.pitch = merge_envelope(
        definition.pitch,
        pitch_stages,
        "pitchenv",
        &definition.name,
    )?;
    if definition.tones.is_empty() && definition.sample.is_none() {
        return Err(format!(
            "{}: a definition needs at least one 'tone' line or a 'sample'",
            definition.name
        ));
    }
    remaining.clear();
    Ok(definition)
}

/// Stage segments gathered from `env attack …`-style lines.
#[derive(Default)]
struct StageSet {
    attack: Option<SegmentDef>,
    decay: Option<SegmentDef>,
    sustain: Option<SegmentDef>,
    release: Option<SegmentDef>,
    seen: bool,
}

impl StageSet {
    fn into_envelope(self) -> Option<EnvelopeDef> {
        if !self.seen {
            return None;
        }
        Some(EnvelopeDef::Stages {
            attack: self.attack,
            decay: self.decay,
            sustain: self.sustain,
            release: self.release,
        })
    }
}

fn merge_envelope(
    direct: Option<EnvelopeDef>,
    stages: StageSet,
    keyword: &str,
    name: &str,
) -> Result<Option<EnvelopeDef>, String> {
    match (direct, stages.into_envelope()) {
        (Some(_), Some(_)) => Err(format!(
            "{name}: '{keyword}' uses either 'adsr'/'segment' or explicit stage lines, not both"
        )),
        (Some(envelope), None) | (None, Some(envelope)) => Ok(Some(envelope)),
        (None, None) => Ok(None),
    }
}

fn apply_line(
    line: &str,
    definition: &mut InstrumentDef,
    amplitude: &mut StageSet,
    pitch: &mut StageSet,
) -> Result<(), String> {
    let name = definition.name.clone();
    let mut parts = line.split_whitespace();
    let keyword = parts.next().expect("blank lines are skipped");
    match keyword {
        "voice" => {
            once(&definition.voice, "voice", &name)?;
            definition.voice = Some(parse_voice(&mut parts, &name)?);
        }
        "lifecycle" => {
            once(&definition.lifecycle, "lifecycle", &name)?;
            definition.lifecycle = Some(match required(&mut parts, "lifecycle", &name)? {
                "oneshot" => Lifecycle::OneShot,
                "gated" => Lifecycle::Gated,
                "cutoff" => Lifecycle::Cutoff,
                other => {
                    return Err(format!(
                        "{name}: '{other}' is not a lifecycle (oneshot, gated, cutoff)"
                    ));
                }
            });
        }
        "tone" => {
            let rest: Vec<&str> = parts.collect();
            definition
                .tones
                .push(parse_tone(&rest.join(" "), &[], &name)?);
            return Ok(());
        }
        "mix" => {
            once(&definition.mix, "mix", &name)?;
            definition.mix = Some(match required(&mut parts, "mix", &name)? {
                "sum" => MixMode::Sum,
                "multiply" => MixMode::Multiply,
                "max" => MixMode::Max,
                "average" => MixMode::Average,
                other => {
                    return Err(format!(
                        "{name}: '{other}' is not a mix mode (sum, multiply, max, average)"
                    ));
                }
            });
        }
        "env" | "pitchenv" => {
            let stages = if keyword == "env" { amplitude } else { pitch };
            let slot = if keyword == "env" {
                &mut definition.amplitude
            } else {
                &mut definition.pitch
            };
            let rest: Vec<&str> = parts.collect();
            parse_envelope_line(&rest, keyword, &name, slot, stages)?;
            return Ok(());
        }
        "sample" => {
            once(&definition.sample, "sample", &name)?;
            // Reads the raw line for the quoted path, so `parts` is untouched.
            definition.sample = Some(parse_sample(line, &name)?);
            return Ok(());
        }
        "gain" => {
            once(&definition.gain, "gain", &name)?;
            definition.gain = Some(number(required(&mut parts, "gain", &name)?, "gain", &name)?);
        }
        "velsens" => {
            once(&definition.velocity_sensitivity, "velsens", &name)?;
            definition.velocity_sensitivity =
                Some(number(required(&mut parts, "velsens", &name)?, "velsens", &name)?);
        }
        "base" => {
            once(&definition.base_frequency, "base", &name)?;
            definition.base_frequency =
                Some(number(required(&mut parts, "base", &name)?, "base", &name)?);
        }
        // The filter grammar of §4.5, shared with pattern transforms.
        _ => {
            let call = parse_fx_line(line).map_err(|error| format!("{name}: {error}"))?;
            definition.fx.push(call);
            return Ok(());
        }
    }
    if let Some(extra) = parts.next() {
        return Err(format!("{name}: '{keyword}' has an extra argument '{extra}'"));
    }
    Ok(())
}

fn parse_voice(
    parts: &mut std::str::SplitWhitespace,
    name: &str,
) -> Result<VoiceDef, String> {
    match required(parts, "voice", name)? {
        "mono" => {
            let mut track_pitch = true;
            let mut allocation = MonoAllocation::Replace;
            for token in parts.by_ref() {
                match token {
                    "notrack" => track_pitch = false,
                    "replace" => allocation = MonoAllocation::Replace,
                    "drop" => allocation = MonoAllocation::Drop,
                    other => {
                        return Err(format!(
                            "{name}: '{other}' is not a mono voice option (notrack, replace, drop)"
                        ));
                    }
                }
            }
            Ok(VoiceDef::Mono {
                track_pitch,
                allocation,
            })
        }
        "poly" => {
            let count = required(parts, "voice poly", name)?;
            let voices: u32 = count
                .parse()
                .map_err(|_| format!("{name}: '{count}' is not a voice count"))?;
            if voices == 0 {
                return Err(format!("{name}: a poly voice needs at least one voice"));
            }
            let mut allocation = PolyAllocation::ReplaceOldest;
            for token in parts.by_ref() {
                allocation = match token {
                    "replaceoldest" => PolyAllocation::ReplaceOldest,
                    "replaceyoungest" => PolyAllocation::ReplaceYoungest,
                    "replaceloudest" => PolyAllocation::ReplaceLoudest,
                    "replacequietest" => PolyAllocation::ReplaceQuietest,
                    "replacerandom" => PolyAllocation::ReplaceRandom,
                    "drop" => PolyAllocation::Drop,
                    other => {
                        return Err(format!(
                            "{name}: '{other}' is not a poly allocation strategy"
                        ));
                    }
                };
            }
            Ok(VoiceDef::Poly { voices, allocation })
        }
        other => Err(format!(
            "{name}: '{other}' is not a voice kind (mono, poly)"
        )),
    }
}

fn parse_tone(header: &str, nested: &[&str], name: &str) -> Result<ToneDef, String> {
    let mut parts = header.split_whitespace();
    // A `tone` keyword is present when the line came straight from the block.
    let first = parts
        .next()
        .ok_or_else(|| format!("{name}: 'tone' expects a waveform"))?;
    let waveform_text = if first == "tone" {
        parts
            .next()
            .ok_or_else(|| format!("{name}: 'tone' expects a waveform"))?
    } else {
        first
    };
    let mut tone = ToneDef {
        waveform: parse_waveform(waveform_text, name)?,
        ..ToneDef::default()
    };

    while let Some(option) = parts.next() {
        match option {
            "gain" => tone.gain = Some(number(required(&mut parts, "tone gain", name)?, "tone gain", name)?),
            "freq" => {
                tone.frequency =
                    Some(number(required(&mut parts, "tone freq", name)?, "tone freq", name)?)
            }
            "identity" => tone.relation = Some(Relation::Identity),
            "harmonic" => {
                let text = required(&mut parts, "tone harmonic", name)?;
                tone.relation = Some(Relation::Harmonic(text.parse().map_err(|_| {
                    format!("{name}: '{text}' is not a harmonic number")
                })?));
            }
            "ratio" => {
                tone.relation = Some(Relation::Ratio(number(
                    required(&mut parts, "tone ratio", name)?,
                    "tone ratio",
                    name,
                )?))
            }
            "offset" => {
                tone.relation = Some(Relation::Offset(number(
                    required(&mut parts, "tone offset", name)?,
                    "tone offset",
                    name,
                )?))
            }
            "semitones" => {
                let text = required(&mut parts, "tone semitones", name)?;
                tone.relation = Some(Relation::Semitones(text.parse().map_err(|_| {
                    format!("{name}: '{text}' is not a semitone count")
                })?));
            }
            "const" => {
                tone.relation = Some(Relation::Constant(number(
                    required(&mut parts, "tone const", name)?,
                    "tone const",
                    name,
                )?))
            }
            "env" => {
                let rest: Vec<&str> = parts.by_ref().collect();
                tone.envelope = Some(parse_inline_envelope(&rest, name)?);
            }
            other => {
                return Err(format!(
                    "{name}: '{other}' is not a tone option (gain, freq, identity, harmonic, ratio, offset, semitones, const, env)"
                ));
            }
        }
    }

    if tone.frequency.is_some() && tone.relation.is_some() {
        return Err(format!(
            "{name}: a tone takes either a fixed 'freq' or a note relation, not both"
        ));
    }

    if !nested.is_empty() {
        if tone.envelope.is_some() {
            return Err(format!(
                "{name}: a tone gives its envelope inline or in a block, not both"
            ));
        }
        let mut stages = StageSet::default();
        let mut slot = None;
        for line in nested {
            let line = line.trim();
            if line.is_empty() || line.starts_with("--") {
                continue;
            }
            let mut parts = line.split_whitespace();
            match parts.next() {
                Some("env") => {
                    let rest: Vec<&str> = parts.collect();
                    parse_envelope_line(&rest, "env", name, &mut slot, &mut stages)?;
                }
                Some(other) => {
                    return Err(format!(
                        "{name}: '{other}' is not allowed in a tone block; only 'env' lines are"
                    ));
                }
                None => continue,
            }
        }
        tone.envelope = merge_envelope(slot, stages, "env", name)?;
    }
    Ok(tone)
}

fn parse_waveform(text: &str, name: &str) -> Result<Waveform, String> {
    Ok(match text {
        "sine" => Waveform::Sine,
        "square" => Waveform::Square,
        "saw" => Waveform::Saw,
        "triangle" => Waveform::Triangle,
        "squareraw" => Waveform::SquareRaw,
        "sawraw" => Waveform::SawRaw,
        "triangleraw" => Waveform::TriangleRaw,
        "noise" => Waveform::Noise,
        "pinknoise" => Waveform::PinkNoise,
        "blank" => Waveform::Blank,
        other => {
            return Err(format!(
                "{name}: '{other}' is not a waveform (sine, square, saw, triangle, squareraw, sawraw, triangleraw, noise, pinknoise, blank)"
            ));
        }
    })
}

/// Handle one `env`/`pitchenv` line, which is either a whole envelope or a stage.
fn parse_envelope_line(
    tokens: &[&str],
    keyword: &str,
    name: &str,
    slot: &mut Option<EnvelopeDef>,
    stages: &mut StageSet,
) -> Result<(), String> {
    let first = tokens
        .first()
        .ok_or_else(|| format!("{name}: '{keyword}' expects adsr, segment, or a stage name"))?;
    match *first {
        "adsr" | "segment" => {
            if slot.is_some() {
                return Err(format!("{name}: '{keyword}' is given more than once"));
            }
            *slot = Some(parse_inline_envelope(tokens, name)?);
        }
        stage @ ("attack" | "decay" | "sustain" | "release") => {
            let segment = parse_segment(&tokens[1..], name)?;
            let target = match stage {
                "attack" => &mut stages.attack,
                "decay" => &mut stages.decay,
                "sustain" => &mut stages.sustain,
                _ => &mut stages.release,
            };
            if target.is_some() {
                return Err(format!("{name}: '{keyword} {stage}' is given more than once"));
            }
            *target = Some(segment);
            stages.seen = true;
        }
        other => {
            return Err(format!(
                "{name}: '{other}' is not an envelope form (adsr, segment, attack, decay, sustain, release)"
            ));
        }
    }
    Ok(())
}

fn parse_inline_envelope(tokens: &[&str], name: &str) -> Result<EnvelopeDef, String> {
    match tokens.first().copied() {
        Some("adsr") => {
            let values = numbers(&tokens[1..], 4, "env adsr", name)?;
            Ok(EnvelopeDef::Adsr {
                attack: values[0],
                decay: values[1],
                sustain: values[2],
                release: values[3],
            })
        }
        Some("segment") => Ok(EnvelopeDef::Single(parse_segment(&tokens[1..], name)?)),
        Some(other) => Err(format!(
            "{name}: '{other}' is not an envelope form (adsr, segment)"
        )),
        None => Err(format!("{name}: 'env' expects adsr or segment")),
    }
}

fn parse_segment(tokens: &[&str], name: &str) -> Result<SegmentDef, String> {
    match tokens.first().copied() {
        Some("linear") => {
            let values = numbers(&tokens[1..], 3, "linear", name)?;
            Ok(SegmentDef::Linear {
                from: values[0],
                to: values[1],
                duration: values[2],
            })
        }
        Some("bezier") => {
            let values = numbers(&tokens[1..], 5, "bezier", name)?;
            Ok(SegmentDef::Bezier {
                from: values[0],
                to: values[1],
                duration: values[2],
                control: (values[3], values[4]),
            })
        }
        Some("constant") => {
            let rest = &tokens[1..];
            if rest.is_empty() || rest.len() > 2 {
                return Err(format!(
                    "{name}: 'constant' takes a value and an optional duration"
                ));
            }
            Ok(SegmentDef::Constant {
                value: number(rest[0], "constant", name)?,
                duration: match rest.get(1) {
                    Some(text) => Some(number(text, "constant duration", name)?),
                    None => None,
                },
            })
        }
        Some(other) => Err(format!(
            "{name}: '{other}' is not a segment (linear, bezier, constant)"
        )),
        None => Err(format!("{name}: expected a segment after the stage name")),
    }
}

fn parse_sample(line: &str, name: &str) -> Result<SampleDef, String> {
    let open = line
        .find('"')
        .ok_or_else(|| format!("{name}: 'sample' expects a quoted path"))?;
    let close = line[open + 1..]
        .find('"')
        .map(|offset| open + 1 + offset)
        .ok_or_else(|| format!("{name}: 'sample' path is missing its closing quote"))?;
    let mut sample = SampleDef {
        path: line[open + 1..close].to_string(),
        ..SampleDef::default()
    };
    if sample.path.is_empty() {
        return Err(format!("{name}: 'sample' path is empty"));
    }
    let mut parts = line[close + 1..].split_whitespace();
    while let Some(option) = parts.next() {
        match option {
            "loop" => sample.looped = true,
            "root" => {
                let text = required(&mut parts, "sample root", name)?;
                sample.root_midi = Some(
                    text.parse()
                        .map_err(|_| format!("{name}: '{text}' is not a MIDI note"))?,
                );
            }
            "start" => {
                sample.start_seconds = Some(number(
                    required(&mut parts, "sample start", name)?,
                    "sample start",
                    name,
                )?)
            }
            "end" => {
                sample.end_seconds = Some(number(
                    required(&mut parts, "sample end", name)?,
                    "sample end",
                    name,
                )?)
            }
            other => {
                return Err(format!(
                    "{name}: '{other}' is not a sample option (root, start, end, loop)"
                ));
            }
        }
    }
    Ok(sample)
}

// --- shared helpers ---

/// Reject a field given twice, which is otherwise a silent last-wins surprise.
fn once<T>(slot: &Option<T>, keyword: &str, name: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{name}: '{keyword}' is given more than once"));
    }
    Ok(())
}

fn required<'a>(
    parts: &mut std::str::SplitWhitespace<'a>,
    keyword: &str,
    name: &str,
) -> Result<&'a str, String> {
    parts
        .next()
        .ok_or_else(|| format!("{name}: '{keyword}' is missing its value"))
}

fn number(text: &str, keyword: &str, name: &str) -> Result<f64, String> {
    text.parse::<f64>()
        .map_err(|_| format!("{name}: '{keyword}' got '{text}', which is not a number"))
}

fn numbers(tokens: &[&str], count: usize, keyword: &str, name: &str) -> Result<Vec<f64>, String> {
    if tokens.len() != count {
        return Err(format!(
            "{name}: '{keyword}' takes {count} numbers, got {}",
            tokens.len()
        ));
    }
    tokens
        .iter()
        .map(|text| number(text, keyword, name))
        .collect()
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Count braces outside quoted spans, so a path may contain one.
pub(crate) fn count_braces(line: &str, brace: char) -> usize {
    let mut quoted = false;
    let mut count = 0;
    for character in line.chars() {
        match character {
            '"' => quoted = !quoted,
            found if found == brace && !quoted => count += 1,
            _ => {}
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_program;

    fn parse(source: &str) -> InstrumentDef {
        let (program, errors) = parse_program(source);
        assert!(errors.is_empty(), "errors: {errors:?}");
        program
            .lines
            .iter()
            .find_map(|line| match line {
                crate::ast::SourceLine::Def(definition) => Some((**definition).clone()),
                _ => None,
            })
            .expect("a def block was parsed")
    }

    fn reject(source: &str) -> String {
        let (_, errors) = parse_program(source);
        assert!(!errors.is_empty(), "should have been rejected: {source}");
        errors[0].message.clone()
    }

    #[test]
    fn a_block_collects_across_lines() {
        let definition = parse(
            "def wobble {\n    voice poly 8\n    lifecycle gated\n    tone saw\n    tone sine gain 0.3 ratio 2.01\n    mix sum\n    env adsr 0.01 0.1 0.7 0.3\n    gain 0.8\n}",
        );
        assert_eq!(definition.name, "wobble");
        assert_eq!(
            definition.voice,
            Some(VoiceDef::Poly {
                voices: 8,
                allocation: PolyAllocation::ReplaceOldest
            })
        );
        assert_eq!(definition.lifecycle, Some(Lifecycle::Gated));
        assert_eq!(definition.mix, Some(MixMode::Sum));
        assert_eq!(definition.gain, Some(0.8));
        assert_eq!(definition.tones.len(), 2);
        assert_eq!(definition.tones[0].waveform, Waveform::Saw);
        assert_eq!(definition.tones[1].gain, Some(0.3));
        assert_eq!(definition.tones[1].relation, Some(Relation::Ratio(2.01)));
        assert_eq!(
            definition.amplitude,
            Some(EnvelopeDef::Adsr {
                attack: 0.01,
                decay: 0.1,
                sustain: 0.7,
                release: 0.3
            })
        );
    }

    #[test]
    fn a_block_coexists_with_pattern_lines() {
        let source = "bpm 120\ndef wobble {\n    tone saw\n}\nbass wobble \"0 _ 3 _\"\nkick kick \"x ~\"";
        let (program, errors) = parse_program(source);
        assert!(errors.is_empty(), "{errors:?}");
        // bpm, def, two patterns.
        assert_eq!(program.lines.len(), 4);
        assert!(matches!(program.lines[0], crate::ast::SourceLine::Bpm(120)));
        assert!(matches!(program.lines[1], crate::ast::SourceLine::Def(_)));
        assert!(matches!(
            program.lines[2],
            crate::ast::SourceLine::Pattern(_)
        ));
        assert!(matches!(
            program.lines[3],
            crate::ast::SourceLine::Pattern(_)
        ));
    }

    #[test]
    fn a_block_may_be_written_on_one_line() {
        let definition = parse("def ping { tone sine } ");
        assert_eq!(definition.name, "ping");
        assert_eq!(definition.tones.len(), 1);
    }

    #[test]
    fn comments_and_blank_lines_are_allowed_inside_a_block() {
        let definition = parse("def q {\n\n    -- the body\n    tone sine\n\n}");
        assert_eq!(definition.tones.len(), 1);
    }

    #[test]
    fn every_field_of_the_spec_is_reachable() {
        let definition = parse(
            "def full {\n\
             voice mono notrack drop\n\
             lifecycle oneshot\n\
             tone noise gain 0.82\n\
             tone sine freq 6713 gain 0.14\n\
             tone sine harmonic 3\n\
             tone sine semitones -12\n\
             tone sine offset 4\n\
             tone sine const 440\n\
             tone sine identity\n\
             mix average\n\
             env attack bezier 0 1 0.0008 0 1\n\
             env decay bezier 1 0 0.09 0.16 0.015\n\
             env sustain constant 0\n\
             env release constant 0 0.5\n\
             pitchenv adsr 0 0.08 0 0\n\
             base 440\n\
             sample \"field.wav\" root 36 start 0.01 end 0.4 loop\n\
             lpf 800\n\
             fx Tremolo frequency=5 depth=0.4\n\
             gain 0.8\n\
             velsens 1.0\n\
             }",
        );
        assert_eq!(
            definition.voice,
            Some(VoiceDef::Mono {
                track_pitch: false,
                allocation: MonoAllocation::Drop
            })
        );
        assert_eq!(definition.lifecycle, Some(Lifecycle::OneShot));
        assert_eq!(definition.mix, Some(MixMode::Average));
        assert_eq!(definition.base_frequency, Some(440.0));
        assert_eq!(definition.velocity_sensitivity, Some(1.0));
        assert_eq!(definition.tones.len(), 7);
        assert_eq!(definition.tones[1].frequency, Some(6713.0));
        assert_eq!(definition.tones[2].relation, Some(Relation::Harmonic(3)));
        assert_eq!(definition.tones[3].relation, Some(Relation::Semitones(-12)));
        assert_eq!(definition.tones[4].relation, Some(Relation::Offset(4.0)));
        assert_eq!(definition.tones[5].relation, Some(Relation::Constant(440.0)));
        assert_eq!(definition.tones[6].relation, Some(Relation::Identity));

        // Stage lines accumulate into one envelope.
        let Some(EnvelopeDef::Stages {
            attack,
            decay,
            sustain,
            release,
        }) = definition.amplitude
        else {
            panic!("expected staged envelope, got {:?}", definition.amplitude);
        };
        assert_eq!(
            attack,
            Some(SegmentDef::Bezier {
                from: 0.0,
                to: 1.0,
                duration: 0.0008,
                control: (0.0, 1.0)
            })
        );
        assert!(decay.is_some());
        assert_eq!(
            sustain,
            Some(SegmentDef::Constant {
                value: 0.0,
                duration: None
            })
        );
        assert_eq!(
            release,
            Some(SegmentDef::Constant {
                value: 0.0,
                duration: Some(0.5)
            })
        );

        assert!(matches!(definition.pitch, Some(EnvelopeDef::Adsr { .. })));
        let sample = definition.sample.expect("sample present");
        assert_eq!(sample.path, "field.wav");
        assert_eq!(sample.root_midi, Some(36));
        assert_eq!(sample.start_seconds, Some(0.01));
        assert_eq!(sample.end_seconds, Some(0.4));
        assert!(sample.looped);

        // Both the alias and the long form land in the chain, in order.
        assert_eq!(definition.fx.len(), 2);
        assert_eq!(definition.fx[0].filter, "lpf");
        assert_eq!(definition.fx[1].filter, "Tremolo");
    }

    #[test]
    fn a_tone_may_carry_its_own_staged_envelope() {
        let definition = parse(
            "def hat {\n    tone sine freq 6713 {\n        env attack linear 0 1 0.0008\n        env release constant 0\n    }\n    tone noise gain 0.8\n}",
        );
        assert_eq!(definition.tones.len(), 2);
        assert_eq!(definition.tones[0].frequency, Some(6713.0));
        assert!(matches!(
            definition.tones[0].envelope,
            Some(EnvelopeDef::Stages { .. })
        ));
        assert_eq!(definition.tones[1].envelope, None);
    }

    #[test]
    fn a_tone_may_carry_an_inline_envelope() {
        let definition = parse("def q {\n    tone sine env adsr 0.001 0.2 0 0.05\n}");
        assert!(matches!(
            definition.tones[0].envelope,
            Some(EnvelopeDef::Adsr { .. })
        ));
    }

    #[test]
    fn an_unclosed_block_is_reported_without_losing_the_text() {
        let (program, errors) = parse_program("def broken {\n    tone sine\nkick kick \"x ~\"");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("never closed"), "{errors:?}");
        assert_eq!(errors[0].location.line, 1);
        // The block's lines survive as comments rather than vanishing.
        assert_eq!(program.lines.len(), 3);
        assert!(
            program
                .lines
                .iter()
                .all(|line| matches!(line, crate::ast::SourceLine::Comment(_)))
        );
    }

    #[test]
    fn block_errors_are_specific() {
        assert!(reject("def 9bad {\n tone sine\n}").contains("not a valid instrument name"));
        assert!(reject("def q {\n tone wobble\n}").contains("not a waveform"));
        assert!(reject("def q {\n voice poly 0\n}").contains("at least one voice"));
        assert!(reject("def q {\n voice quad 2\n tone sine\n}").contains("not a voice kind"));
        assert!(reject("def q {\n lifecycle forever\n tone sine\n}").contains("not a lifecycle"));
        assert!(reject("def q {\n mix blend\n tone sine\n}").contains("not a mix mode"));
        assert!(reject("def q {\n tone sine\n gain 1\n gain 2\n}").contains("more than once"));
        assert!(reject("def q {\n}").contains("needs at least one 'tone'"));
        // A fixed frequency and a note relation cannot both win.
        assert!(reject("def q {\n tone sine freq 400 ratio 2\n}").contains("not both"));
        // Nor can a whole envelope and explicit stages.
        assert!(
            reject("def q {\n tone sine\n env adsr 0 0 0 0\n env attack linear 0 1 0.1\n}")
                .contains("not both")
        );
        assert!(reject("def q {\n tone sine\n env attack wiggle 1\n}").contains("not a segment"));
        assert!(reject("def q {\n tone sine\n env adsr 0.1 0.2\n}").contains("takes 4 numbers"));
        assert!(reject("def q {\n tone sine\n sample nopath\n}").contains("quoted path"));
        assert!(reject("def q {\n tone sine\n wibble 4\n}").contains("not a filter line"));
    }

    #[test]
    fn a_sample_only_definition_needs_no_tones() {
        let definition = parse("def field {\n    sample \"rec.wav\"\n}");
        assert!(definition.tones.is_empty());
        assert_eq!(definition.sample.map(|s| s.path), Some("rec.wav".into()));
    }

    #[test]
    fn a_brace_inside_a_quoted_path_does_not_close_the_block() {
        let definition = parse("def odd {\n    sample \"a{b}.wav\"\n    tone sine\n}");
        assert_eq!(definition.sample.map(|s| s.path), Some("a{b}.wav".into()));
        assert_eq!(definition.tones.len(), 1);
    }
}
