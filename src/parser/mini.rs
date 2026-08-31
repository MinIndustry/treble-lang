//! Mini-notation parser using nom combinators.
//!
//! Parses the content inside double-quoted pattern strings into a
//! [`MiniNotation`] AST.

use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::tag,
    character::complete::{char, digit1, one_of, space0, space1},
    combinator::{map, map_res, opt, recognize, value},
    multi::{many0, separated_list1},
    sequence::{delimited, preceded},
};

use crate::ast::mini::*;
use crate::ast::program::{Accidental, NoteLetter, Ramp};

/// Parse a mini-notation string into a [`MiniNotation`].
pub fn parse_mini(input: &str) -> Result<MiniNotation, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(MiniNotation {
            sequence: Sequence { steps: vec![] },
        });
    }
    match parse_sequence(trimmed) {
        Ok(("", seq)) => {
            let notation = MiniNotation { sequence: seq };
            validate_sequence(&notation.sequence)?;
            Ok(notation)
        }
        Ok((rest, _)) => Err(format!("unexpected trailing input: '{}'", rest)),
        // A non-recoverable failure is always a real authoring mistake rather
        // than a wrong turn in `alt`. nom's error carries only an `ErrorKind`,
        // so the few of them are told apart by which kind they raise.
        Err(nom::Err::Failure(error)) => Err(match error.code {
            FAIL_DOUBLE_VELOCITY => "a step takes one velocity, not two".to_string(),
            FAIL_ACCENT_VELOCITY => {
                "'X' is already a velocity of 1.0; write 'x:v' to choose one".to_string()
            }
            _ => "a group uses ',' for a chord or '|' for a random choice, not both".to_string(),
        }),
        Err(e) => Err(format!("mini-notation parse error: {}", e)),
    }
}

/// Raised by [`parse_group`] when a group mixes `,` and `|`.
const FAIL_MIXED_SEPARATORS: nom::error::ErrorKind = nom::error::ErrorKind::Verify;
/// Raised by [`parse_step`] when one step carries two `:v` suffixes.
const FAIL_DOUBLE_VELOCITY: nom::error::ErrorKind = nom::error::ErrorKind::Many1;
/// Raised by [`parse_step`] for `X:v`, which asks for two velocities at once.
const FAIL_ACCENT_VELOCITY: nom::error::ErrorKind = nom::error::ErrorKind::ManyMN;

/// Check the invariants the grammar itself cannot express.
///
/// Runs after a successful parse so the messages can name the offending value,
/// rather than surfacing a generic nom error.
fn validate_sequence(sequence: &Sequence) -> Result<(), String> {
    for step in sequence.steps.iter() {
        for modifier in step.modifiers.iter() {
            validate_modifier(modifier)?;
        }
        validate_velocity(step)?;
        match &step.atom {
            Atom::Group(group) => {
                for layer in group.layers.iter() {
                    // A chord layer is a fixed set of notes; a generated walk
                    // has no single note to contribute to one.
                    if group.mode == GroupMode::Chord
                        && layer
                            .steps
                            .iter()
                            .any(|step| matches!(step.atom, Atom::Solo(_)))
                    {
                        return Err(
                            "solo(..) can't be a chord layer; put it in its own step".to_string()
                        );
                    }
                    validate_sequence(layer)?;
                }
            }
            Atom::Alternation(alternation) => validate_sequence(&alternation.sequence)?,
            Atom::Solo(solo) => {
                if solo.high <= solo.low {
                    return Err(format!(
                        "solo({}..{}) needs at least two degrees to walk between",
                        solo.low, solo.high
                    ));
                }
                if solo.steps.values().contains(&0) {
                    return Err("solo needs at least one step per cycle".to_string());
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// `:v` (and the `X` it desugars from) must be a playable strike, on something
/// that actually strikes.
fn validate_velocity(step: &Step) -> Result<(), String> {
    let Some(velocity) = &step.velocity else {
        return Ok(());
    };
    match step.atom {
        // Silence has no strike, and a hold sustains whatever it extends, so
        // neither has a velocity of its own to set. Ignoring the suffix would
        // hide the misunderstanding instead of correcting it.
        Atom::Rest => {
            return Err(
                "a rest can't carry a velocity: '~' is silence, so ':v' has nothing to strike"
                    .to_string(),
            );
        }
        Atom::Hold => {
            return Err("a hold can't carry a velocity: '_' sustains the event before it, which was already struck".to_string());
        }
        _ => {}
    }
    for value in velocity.values() {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(format!(
                "velocity {value} is outside the supported range 0.0-1.0"
            ));
        }
    }
    Ok(())
}

fn validate_modifier(modifier: &Modifier) -> Result<(), String> {
    let positive = |value: u32, sigil: &str| {
        if value == 0 {
            Err(format!(
                "'{sigil}0' is not a count; use at least '{sigil}1'"
            ))
        } else {
            Ok(())
        }
    };
    match modifier {
        Modifier::Repeat(count) => {
            // Every value a ramp passes through has to be legal, not just the
            // one it starts on.
            for value in count.values() {
                positive(value, "*")?;
            }
            Ok(())
        }
        Modifier::Replicate(0) => Err("'!0' is not a count; use at least '!1'".to_string()),
        Modifier::Slow(0) => Err("'/0' is not a count; use at least '/1'".to_string()),
        Modifier::Weight(0) => Err("'@0' is not a weight; use at least '@1'".to_string()),
        Modifier::Euclidean(_, positions, _) => {
            if positions.values().contains(&0) {
                return Err("a Euclidean rhythm needs at least one step".to_string());
            }
            Ok(())
        }
        Modifier::Drop(Some(probability)) => {
            for value in probability.values() {
                if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                    return Err(format!(
                        "drop probability {value} is outside the supported range 0.0-1.0"
                    ));
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

// --- Sequence ---

fn parse_sequence(input: &str) -> IResult<&str, Sequence> {
    let (input, _) = space0(input)?;
    let (input, steps) = separated_list1(space1, parse_step).parse(input)?;
    let (input, _) = space0(input)?;
    Ok((input, Sequence { steps }))
}

// --- Step = Atom { Modifier } ---

fn parse_step(input: &str) -> IResult<&str, Step> {
    // `X` is stored as `x:1.0` rather than as an atom of its own, so a consumer
    // only has to implement velocity once.
    let (mut input, accented) = match parse_accent(input) {
        Ok((rest, ())) => (rest, true),
        Err(_) => (input, false),
    };
    let (rest, atom) = if accented {
        (input, Atom::Trigger)
    } else {
        parse_atom(input)?
    };
    input = rest;
    let mut modifiers = Vec::new();
    let mut velocity = accented.then(|| Ramp::fixed(1.0));
    // Modifiers and the velocity suffix may be written in either order, and a
    // velocity means the same thing wherever it sits (see [`Step`]), so both
    // are taken in one pass. Every one of them consumes at least its sigil, so
    // the loop always advances.
    loop {
        if let Ok((rest, next)) = parse_velocity(input) {
            if velocity.is_some() {
                let code = if accented {
                    FAIL_ACCENT_VELOCITY
                } else {
                    FAIL_DOUBLE_VELOCITY
                };
                return Err(nom::Err::Failure(nom::error::Error::new(input, code)));
            }
            velocity = Some(next);
            input = rest;
            continue;
        }
        match parse_modifier(input) {
            Ok((rest, modifier)) => {
                modifiers.push(modifier);
                input = rest;
            }
            Err(_) => break,
        }
    }
    Ok((
        input,
        Step {
            atom,
            modifiers,
            velocity,
        },
    ))
}

/// `X` — an accented trigger. Unambiguous because note letters are `a`–`g`.
fn parse_accent(input: &str) -> IResult<&str, ()> {
    let (input, _) = char('X').parse(input)?;
    if input
        .chars()
        .next()
        .map(|c| c.is_ascii_alphanumeric())
        .unwrap_or(false)
    {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Char,
        )));
    }
    Ok((input, ()))
}

/// `:0.6`, `:0.3..0.9` — an explicit velocity for this step.
fn parse_velocity(input: &str) -> IResult<&str, Ramp<f64>> {
    preceded(char(':'), parse_ramp_f64).parse(input)
}

// --- Atom ---

fn parse_atom(input: &str) -> IResult<&str, Atom> {
    alt((
        parse_solo,
        parse_group,
        parse_alternation,
        parse_note_atom,
        parse_degree_atom,
        parse_trigger,
        parse_rest,
        parse_hold,
    ))
    .parse(input)
}

fn parse_note_atom(input: &str) -> IResult<&str, Atom> {
    map(parse_note, Atom::Note).parse(input)
}

fn parse_note(input: &str) -> IResult<&str, Note> {
    let (input, letter) = parse_note_letter(input)?;
    let (input, accidental) = parse_accidental(input, letter)?;
    let (input, octave) = parse_octave(input)?;
    Ok((
        input,
        Note {
            letter,
            accidental,
            octave,
        },
    ))
}

fn parse_note_letter(input: &str) -> IResult<&str, NoteLetter> {
    alt((
        value(NoteLetter::C, char('c')),
        value(NoteLetter::D, char('d')),
        value(NoteLetter::E, char('e')),
        value(NoteLetter::F, char('f')),
        value(NoteLetter::G, char('g')),
        value(NoteLetter::A, char('a')),
        value(NoteLetter::B, char('b')),
    ))
    .parse(input)
}

/// Parse accidental after a note letter.
///
/// Context-sensitive for the note B: `bb3` = B-flat 3, `b4` = B natural 4.
fn parse_accidental(input: &str, letter: NoteLetter) -> IResult<&str, Accidental> {
    if letter == NoteLetter::B {
        // After 'b': '#'/'##' for sharp
        if let Ok((rest, _)) = tag::<&str, &str, nom::error::Error<&str>>("##").parse(input) {
            return Ok((rest, Accidental::DoubleSharp));
        }
        if let Ok((rest, _)) = char::<&str, nom::error::Error<&str>>('#').parse(input) {
            return Ok((rest, Accidental::Sharp));
        }
        // 'b' followed by digit = flat
        if let Some(after_b) = input.strip_prefix('b')
            && after_b.starts_with(|c: char| c.is_ascii_digit())
        {
            return Ok((after_b, Accidental::Flat));
        }
        Ok((input, Accidental::Natural))
    } else {
        // Non-B note
        let result: IResult<&str, Accidental> = alt((
            value(Accidental::DoubleSharp, tag("##")),
            value(Accidental::Sharp, char('#')),
            value(Accidental::DoubleFlat, tag("bb")),
            value(Accidental::Flat, char('b')),
        ))
        .parse(input);
        match result {
            Ok(r) => Ok(r),
            Err(_) => Ok((input, Accidental::Natural)),
        }
    }
}

fn parse_octave(input: &str) -> IResult<&str, u8> {
    map_res(one_of("0123456789"), |c: char| {
        c.to_digit(10).map(|d| d as u8).ok_or("invalid octave")
    })
    .parse(input)
}

fn parse_degree_atom(input: &str) -> IResult<&str, Atom> {
    // Negative degrees: -N
    if input.starts_with('-') {
        let (input, _) = char('-').parse(input)?;
        let (input, digits) = digit1(input)?;
        let n: i32 = digits.parse().map_err(|_| {
            nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Digit))
        })?;
        return Ok((input, Atom::Degree(-n)));
    }
    // Positive degrees: just digits
    let (input, digits) = digit1(input)?;
    let n: i32 = digits.parse().map_err(|_| {
        nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Digit))
    })?;
    Ok((input, Atom::Degree(n)))
}

fn parse_trigger(input: &str) -> IResult<&str, Atom> {
    let (input, _) = char('x').parse(input)?;
    // Ensure not followed by an alphanumeric char
    if input
        .chars()
        .next()
        .map(|c| c.is_ascii_alphanumeric())
        .unwrap_or(false)
    {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Char,
        )));
    }
    Ok((input, Atom::Trigger))
}

fn parse_rest(input: &str) -> IResult<&str, Atom> {
    value(Atom::Rest, char('~')).parse(input)
}

fn parse_hold(input: &str) -> IResult<&str, Atom> {
    value(Atom::Hold, char('_')).parse(input)
}

// --- Solo: solo(low..high, steps) ---

fn parse_solo(input: &str) -> IResult<&str, Atom> {
    let (input, _) = tag("solo(").parse(input)?;
    let (input, _) = space0(input)?;
    let (input, low) = parse_i32(input)?;
    let (input, _) = tag("..").parse(input)?;
    let (input, high) = parse_i32(input)?;
    let (input, _) = delimited(space0, char(','), space0).parse(input)?;
    let (input, steps) = parse_ramp_u32(input)?;
    let (input, _) = space0(input)?;
    let (input, _) = char(')').parse(input)?;
    Ok((input, Atom::Solo(Solo { low, high, steps })))
}

fn parse_i32(input: &str) -> IResult<&str, i32> {
    let (rest, negative) = opt(char('-')).parse(input)?;
    let (rest, digits) = digit1(rest)?;
    let value: i32 = digits
        .parse()
        .map_err(|_| nom::Err::Error(nom::error::Error::new(rest, nom::error::ErrorKind::Digit)))?;
    Ok((rest, if negative.is_some() { -value } else { value }))
}

// --- Group: [ sequence { ("," | "|") sequence } ] ---

fn parse_group(input: &str) -> IResult<&str, Atom> {
    let (input, _) = char('[').parse(input)?;
    // `parse_sequence` eats the surrounding whitespace of every layer.
    let (mut input, first) = parse_sequence(input)?;
    let mut layers = vec![first];
    let mut chord = false;
    let mut random = false;
    while let Ok((rest, separator)) =
        one_of::<&str, &str, nom::error::Error<&str>>(",|").parse(input)
    {
        let (rest, layer) = parse_sequence(rest)?;
        match separator {
            ',' => chord = true,
            _ => random = true,
        }
        layers.push(layer);
        input = rest;
    }
    let (input, _) = char(']').parse(input)?;
    if chord && random {
        // Non-recoverable on purpose: `alt` in `parse_atom` must not retry this
        // as some other atom, and `parse_mini` turns it into a real message.
        return Err(nom::Err::Failure(nom::error::Error::new(
            input,
            FAIL_MIXED_SEPARATORS,
        )));
    }
    let mode = match (chord, random) {
        (true, _) => GroupMode::Chord,
        (_, true) => GroupMode::Random,
        _ => GroupMode::Subdivide,
    };
    Ok((input, Atom::Group(Group { mode, layers })))
}

// --- Alternation: < sequence > ---

fn parse_alternation(input: &str) -> IResult<&str, Atom> {
    let (input, _) = char('<').parse(input)?;
    let (input, _) = space0(input)?;
    let (input, sequence) = parse_sequence(input)?;
    let (input, _) = space0(input)?;
    let (input, _) = char('>').parse(input)?;
    Ok((input, Atom::Alternation(Alternation { sequence })))
}

// --- Modifiers ---

fn parse_modifier(input: &str) -> IResult<&str, Modifier> {
    alt((
        parse_repeat,
        parse_slow_mod,
        parse_replicate,
        parse_euclidean,
        parse_drop,
        parse_weight,
    ))
    .parse(input)
}

fn parse_repeat(input: &str) -> IResult<&str, Modifier> {
    let (input, _) = char('*').parse(input)?;
    let (input, n) = parse_ramp_u32(input)?;
    Ok((input, Modifier::Repeat(n)))
}

fn parse_slow_mod(input: &str) -> IResult<&str, Modifier> {
    let (input, _) = char('/').parse(input)?;
    let (input, n) = parse_u32(input)?;
    Ok((input, Modifier::Slow(n)))
}

fn parse_replicate(input: &str) -> IResult<&str, Modifier> {
    let (input, _) = char('!').parse(input)?;
    let (input, n) = parse_u32(input)?;
    Ok((input, Modifier::Replicate(n)))
}

fn parse_euclidean(input: &str) -> IResult<&str, Modifier> {
    let (input, _) = char('(').parse(input)?;
    let (input, _) = space0(input)?;
    let (input, beats) = parse_ramp_u32(input)?;
    let (input, _) = delimited(space0, char(','), space0).parse(input)?;
    let (input, steps) = parse_ramp_u32(input)?;
    let (input, offset) =
        opt(preceded(delimited(space0, char(','), space0), parse_u32)).parse(input)?;
    let (input, _) = space0(input)?;
    let (input, _) = char(')').parse(input)?;
    Ok((input, Modifier::Euclidean(beats, steps, offset)))
}

fn parse_drop(input: &str) -> IResult<&str, Modifier> {
    let (input, _) = char('?').parse(input)?;
    // No space is allowed before the probability, so `x? 0.5` still reads as a
    // bare drop followed by a separate step.
    let (input, probability) = opt(parse_ramp_f64).parse(input)?;
    Ok((input, Modifier::Drop(probability)))
}

fn parse_weight(input: &str) -> IResult<&str, Modifier> {
    let (input, _) = char('@').parse(input)?;
    let (input, n) = parse_u32(input)?;
    Ok((input, Modifier::Weight(n)))
}

fn parse_u32(input: &str) -> IResult<&str, u32> {
    map_res(digit1, |s: &str| s.parse::<u32>()).parse(input)
}

/// `4`, `4..16` or `2>4>8>16` — a value that may travel.
///
/// `digit1` followed by an optional fraction means `4..16` backtracks cleanly:
/// the fraction needs a digit after its dot, and the second dot is not one.
/// A `>` chain backtracks too, so the closing `>` of `<x*4>` is left alone.
fn parse_ramp_u32(input: &str) -> IResult<&str, Ramp<u32>> {
    if let Some(result) = parse_r_form(input, |part| {
        part.parse::<u32>().map_err(|error| error.to_string())
    }) {
        return result;
    }
    let (input, first) = parse_u32(input)?;
    if let Ok((rest, to)) = preceded(tag(".."), parse_u32).parse(input) {
        return Ok((rest, Ramp::Sweep { from: first, to }));
    }
    let (input, steps) = many0(preceded(tag(">"), parse_u32)).parse(input)?;
    Ok((input, Ramp::steps(first, steps)))
}

fn parse_ramp_f64(input: &str) -> IResult<&str, Ramp<f64>> {
    if let Some(result) = parse_r_form(input, |part| {
        part.parse::<f64>().map_err(|error| error.to_string())
    }) {
        return result;
    }
    let (input, first) = parse_f64(input)?;
    if let Ok((rest, to)) = preceded(tag(".."), parse_f64).parse(input) {
        return Ok((rest, Ramp::Sweep { from: first, to }));
    }
    let (input, steps) = many0(preceded(tag(">"), parse_f64)).parse(input)?;
    Ok((input, Ramp::steps(first, steps)))
}

/// `r(...)` wherever a ramp value is legal — `x(r(2,16,8),4)` grows a
/// Euclidean count over its own eight-beat window. Written without spaces, so
/// the closing parenthesis is unambiguous; the interpretation is shared with
/// the transform parser.
fn parse_r_form<T: Copy>(
    input: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Option<IResult<&str, Ramp<T>>> {
    let inner = input.strip_prefix("r(")?;
    let close = inner.find(')')?;
    let text = format!("r({})", &inner[..close]);
    Some(
        match crate::parser::lines::ramp_from_text(&text, "r", &parse) {
            Ok(ramp) => Ok((&inner[close + 1..], ramp)),
            Err(_) => Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Verify,
            ))),
        },
    )
}

fn parse_f64(input: &str) -> IResult<&str, f64> {
    map_res(recognize((digit1, opt((char('.'), digit1)))), |s: &str| {
        s.parse::<f64>()
    })
    .parse(input)
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::program::RampCurve;
    use pretty_assertions::assert_eq;

    // --- Helper to build notes quickly ---
    fn n(letter: NoteLetter, acc: Accidental, oct: u8) -> Atom {
        Atom::Note(Note {
            letter,
            accidental: acc,
            octave: oct,
        })
    }

    fn nat(letter: NoteLetter, oct: u8) -> Atom {
        n(letter, Accidental::Natural, oct)
    }

    // ---- Notes ----

    #[test]
    fn r_form_ranges_fit_inside_mini_notation() {
        let m = parse_mini("x(r(2,16,8),4)").unwrap();
        let modifiers = &m.sequence.steps[0].modifiers;
        assert_eq!(
            modifiers,
            &vec![Modifier::Euclidean(
                Ramp::Timed {
                    ramp: Box::new(Ramp::Sweep { from: 2, to: 16 }),
                    span_divisions: 8.0,
                    curve: RampCurve::Linear,
                },
                Ramp::fixed(4),
                None
            )]
        );

        let m = parse_mini("x*r(2,8,4,osc)").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Repeat(Ramp::Timed {
                ramp: Box::new(Ramp::Sweep { from: 2, to: 8 }),
                span_divisions: 4.0,
                curve: RampCurve::Oscillate,
            })]
        );

        let m = parse_mini("x:r(0.3,1.0,2)").unwrap();
        assert!(m.sequence.steps[0].velocity.as_ref().unwrap().travels());
    }

    #[test]
    fn test_note_c4() {
        let m = parse_mini("c4").unwrap();
        assert_eq!(m.sequence.steps.len(), 1);
        assert_eq!(m.sequence.steps[0].atom, nat(NoteLetter::C, 4));
    }

    #[test]
    fn test_note_eb3() {
        let m = parse_mini("eb3").unwrap();
        assert_eq!(
            m.sequence.steps[0].atom,
            n(NoteLetter::E, Accidental::Flat, 3)
        );
    }

    #[test]
    fn test_note_f_sharp_5() {
        let m = parse_mini("f#5").unwrap();
        assert_eq!(
            m.sequence.steps[0].atom,
            n(NoteLetter::F, Accidental::Sharp, 5)
        );
    }

    #[test]
    fn test_note_bb3_is_b_flat() {
        let m = parse_mini("bb3").unwrap();
        assert_eq!(
            m.sequence.steps[0].atom,
            n(NoteLetter::B, Accidental::Flat, 3)
        );
    }

    #[test]
    fn test_note_b4_natural() {
        let m = parse_mini("b4").unwrap();
        assert_eq!(
            m.sequence.steps[0].atom,
            n(NoteLetter::B, Accidental::Natural, 4)
        );
    }

    #[test]
    fn test_note_b_sharp() {
        let m = parse_mini("b#4").unwrap();
        assert_eq!(
            m.sequence.steps[0].atom,
            n(NoteLetter::B, Accidental::Sharp, 4)
        );
    }

    // ---- Sequences ----

    #[test]
    fn test_sequence_three_notes() {
        let m = parse_mini("c4 e4 g4").unwrap();
        assert_eq!(m.sequence.steps.len(), 3);
        assert_eq!(m.sequence.steps[0].atom, nat(NoteLetter::C, 4));
        assert_eq!(m.sequence.steps[1].atom, nat(NoteLetter::E, 4));
        assert_eq!(m.sequence.steps[2].atom, nat(NoteLetter::G, 4));
    }

    #[test]
    fn test_rest_and_hold() {
        let m = parse_mini("c4 ~ _ e4").unwrap();
        assert_eq!(m.sequence.steps.len(), 4);
        assert_eq!(m.sequence.steps[0].atom, nat(NoteLetter::C, 4));
        assert_eq!(m.sequence.steps[1].atom, Atom::Rest);
        assert_eq!(m.sequence.steps[2].atom, Atom::Hold);
        assert_eq!(m.sequence.steps[3].atom, nat(NoteLetter::E, 4));
    }

    #[test]
    fn test_trigger() {
        let m = parse_mini("x ~ x ~").unwrap();
        assert_eq!(m.sequence.steps.len(), 4);
        assert_eq!(m.sequence.steps[0].atom, Atom::Trigger);
        assert_eq!(m.sequence.steps[1].atom, Atom::Rest);
        assert_eq!(m.sequence.steps[2].atom, Atom::Trigger);
        assert_eq!(m.sequence.steps[3].atom, Atom::Rest);
    }

    #[test]
    fn test_scale_degrees() {
        let m = parse_mini("0 2 4 6").unwrap();
        assert_eq!(m.sequence.steps.len(), 4);
        assert_eq!(m.sequence.steps[0].atom, Atom::Degree(0));
        assert_eq!(m.sequence.steps[1].atom, Atom::Degree(2));
        assert_eq!(m.sequence.steps[2].atom, Atom::Degree(4));
        assert_eq!(m.sequence.steps[3].atom, Atom::Degree(6));
    }

    // ---- Modifiers ----

    #[test]
    fn test_repeat() {
        let m = parse_mini("x*4").unwrap();
        assert_eq!(m.sequence.steps[0].atom, Atom::Trigger);
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Repeat(Ramp::fixed(4))]
        );
    }

    #[test]
    fn test_slow_modifier() {
        let m = parse_mini("[c4 e4 g4]/2").unwrap();
        assert!(matches!(m.sequence.steps[0].atom, Atom::Group(_)));
        assert_eq!(m.sequence.steps[0].modifiers, vec![Modifier::Slow(2)]);
    }

    #[test]
    fn test_replicate() {
        let m = parse_mini("c4!3").unwrap();
        assert_eq!(m.sequence.steps[0].modifiers, vec![Modifier::Replicate(3)]);
    }

    #[test]
    fn test_euclidean() {
        let m = parse_mini("c4(3,8)").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Euclidean(Ramp::fixed(3), Ramp::fixed(8), None)]
        );
    }

    #[test]
    fn test_euclidean_with_offset() {
        let m = parse_mini("x(5,8,2)").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Euclidean(Ramp::fixed(5), Ramp::fixed(8), Some(2))]
        );
    }

    #[test]
    fn test_drop() {
        let m = parse_mini("c4?").unwrap();
        assert_eq!(m.sequence.steps[0].modifiers, vec![Modifier::Drop(None)]);
    }

    #[test]
    fn test_weight() {
        let m = parse_mini("c4@3 e4").unwrap();
        assert_eq!(m.sequence.steps[0].modifiers, vec![Modifier::Weight(3)]);
        assert!(m.sequence.steps[1].modifiers.is_empty());
    }

    // ---- Groups and chords ----

    #[test]
    fn test_group() {
        let m = parse_mini("[c4 e4] g4").unwrap();
        assert_eq!(m.sequence.steps.len(), 2);
        if let Atom::Group(ref g) = m.sequence.steps[0].atom {
            assert_eq!(g.mode, GroupMode::Subdivide);
            assert_eq!(g.layers.len(), 1);
            assert_eq!(g.layers[0].steps.len(), 2);
        } else {
            panic!("expected group");
        }
        assert_eq!(m.sequence.steps[1].atom, nat(NoteLetter::G, 4));
    }

    #[test]
    fn test_chord() {
        let m = parse_mini("[c3,e3,g3]").unwrap();
        assert_eq!(m.sequence.steps.len(), 1);
        if let Atom::Group(ref g) = m.sequence.steps[0].atom {
            assert_eq!(g.mode, GroupMode::Chord);
            assert_eq!(g.layers.len(), 3);
            assert_eq!(g.layers[0].steps[0].atom, nat(NoteLetter::C, 3));
            assert_eq!(g.layers[1].steps[0].atom, nat(NoteLetter::E, 3));
            assert_eq!(g.layers[2].steps[0].atom, nat(NoteLetter::G, 3));
        } else {
            panic!("expected group/chord");
        }
    }

    #[test]
    fn test_nested_groups() {
        let m = parse_mini("[c4 [e4 g4]] b4").unwrap();
        assert_eq!(m.sequence.steps.len(), 2);
        if let Atom::Group(ref outer) = m.sequence.steps[0].atom {
            assert_eq!(outer.layers[0].steps.len(), 2);
            assert!(matches!(outer.layers[0].steps[1].atom, Atom::Group(_)));
        } else {
            panic!("expected nested group");
        }
    }

    // ---- Alternation ----

    #[test]
    fn test_alternation() {
        let m = parse_mini("<c4 e4 g4>").unwrap();
        assert_eq!(m.sequence.steps.len(), 1);
        if let Atom::Alternation(ref a) = m.sequence.steps[0].atom {
            assert_eq!(a.sequence.steps.len(), 3);
        } else {
            panic!("expected alternation");
        }
    }

    #[test]
    fn test_alternation_in_sequence() {
        let m = parse_mini("c4 <e4 g4> c5").unwrap();
        assert_eq!(m.sequence.steps.len(), 3);
        assert_eq!(m.sequence.steps[0].atom, nat(NoteLetter::C, 4));
        assert!(matches!(m.sequence.steps[1].atom, Atom::Alternation(_)));
        assert_eq!(m.sequence.steps[2].atom, nat(NoteLetter::C, 5));
    }

    // ---- Empty ----

    #[test]
    fn test_empty_notation() {
        let m = parse_mini("").unwrap();
        assert_eq!(m.sequence.steps.len(), 0);
    }

    #[test]
    fn test_whitespace_only() {
        let m = parse_mini("   ").unwrap();
        assert_eq!(m.sequence.steps.len(), 0);
    }

    // ---- Complex patterns ----

    #[test]
    fn test_drum_pattern() {
        let m = parse_mini("x ~ [x x] ~").unwrap();
        assert_eq!(m.sequence.steps.len(), 4);
        assert_eq!(m.sequence.steps[0].atom, Atom::Trigger);
        assert_eq!(m.sequence.steps[1].atom, Atom::Rest);
        assert!(matches!(m.sequence.steps[2].atom, Atom::Group(_)));
        assert_eq!(m.sequence.steps[3].atom, Atom::Rest);
    }

    #[test]
    fn test_chord_sequence() {
        let m = parse_mini("[c3,e3,g3] ~ [f3,a3,c4] ~").unwrap();
        assert_eq!(m.sequence.steps.len(), 4);
        if let Atom::Group(ref g) = m.sequence.steps[0].atom {
            assert_eq!(g.layers.len(), 3);
        } else {
            panic!("expected chord");
        }
    }

    #[test]
    fn test_repeat_group() {
        let m = parse_mini("[c4 e4]*3").unwrap();
        assert_eq!(m.sequence.steps.len(), 1);
        assert!(matches!(m.sequence.steps[0].atom, Atom::Group(_)));
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Repeat(Ramp::fixed(3))]
        );
    }

    // ---- Stacked modifiers ----

    #[test]
    fn test_stacked_repeat_and_drop() {
        let m = parse_mini("x*8?").unwrap();
        assert_eq!(m.sequence.steps.len(), 1);
        assert_eq!(m.sequence.steps[0].atom, Atom::Trigger);
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Repeat(Ramp::fixed(8)), Modifier::Drop(None)]
        );
    }

    #[test]
    fn test_stacked_euclidean_and_drop_keeps_written_order() {
        let m = parse_mini("x(5,8)?0.25").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![
                Modifier::Euclidean(Ramp::fixed(5), Ramp::fixed(8), None),
                Modifier::Drop(Some(Ramp::fixed(0.25)))
            ]
        );
    }

    #[test]
    fn test_stacked_weight_and_repeat() {
        let m = parse_mini("[c4 e4]*2@3 g4").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Repeat(Ramp::fixed(2)), Modifier::Weight(3)]
        );
    }

    // ---- Drop probability ----

    #[test]
    fn test_drop_with_probability() {
        let m = parse_mini("x?0.3").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Drop(Some(Ramp::fixed(0.3)))]
        );
    }

    // ---- Solo ----

    #[test]
    fn test_solo_atom() {
        let m = parse_mini("solo(0..7, 8)").unwrap();
        assert_eq!(
            m.sequence.steps[0].atom,
            Atom::Solo(Solo {
                low: 0,
                high: 7,
                steps: Ramp::fixed(8)
            })
        );
        // Negative degrees walk below the root octave.
        let m = parse_mini("solo(-3..4,6)").unwrap();
        assert_eq!(
            m.sequence.steps[0].atom,
            Atom::Solo(Solo {
                low: -3,
                high: 4,
                steps: Ramp::fixed(6)
            })
        );
        // The step count may travel across the line's ramp span.
        let m = parse_mini("solo(0..12, 4..16)").unwrap();
        assert_eq!(
            m.sequence.steps[0].atom,
            Atom::Solo(Solo {
                low: 0,
                high: 12,
                steps: Ramp::Sweep { from: 4, to: 16 }
            })
        );
    }

    #[test]
    fn test_solo_composes_with_sequences_and_modifiers() {
        // Half a cycle of kick-space, half of solo.
        let m = parse_mini("~ solo(0..7,4)").unwrap();
        assert_eq!(m.sequence.steps.len(), 2);
        assert!(matches!(m.sequence.steps[1].atom, Atom::Solo(_)));
        // A drop chance applies to the whole generated slot.
        let m = parse_mini("solo(0..7,8)?0.3").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Drop(Some(Ramp::fixed(0.3)))]
        );
    }

    #[test]
    fn test_solo_validation() {
        // A walk needs somewhere to go, and at least one note.
        assert!(parse_mini("solo(5..5,8)").is_err());
        assert!(parse_mini("solo(7..0,8)").is_err());
        assert!(parse_mini("solo(0..7,0)").is_err());
        assert!(parse_mini("solo(0..7,4..0)").is_err());
        // A generated walk has no single note to lend a chord, but it can sit
        // beside other choice options or subdivision steps.
        assert!(parse_mini("[c4,solo(0..7,4)]").is_err());
        assert!(parse_mini("[c4|solo(0..7,4)]").is_ok());
        assert!(parse_mini("[c4 solo(0..7,4)]").is_ok());
    }

    // ---- Ramps ----

    #[test]
    fn test_euclidean_onsets_may_ramp() {
        // The density crescendo: four hits growing to sixteen.
        let m = parse_mini("x(4..16,4)").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Euclidean(
                Ramp::Sweep { from: 4, to: 16 },
                Ramp::fixed(4),
                None
            )]
        );
    }

    #[test]
    fn test_ranges_do_not_confuse_the_float_parser() {
        // `4..16` must not read the first dot as a decimal point.
        let m = parse_mini("x?0.1..0.9").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Drop(Some(Ramp::Sweep { from: 0.1, to: 0.9 }))]
        );
        let m = parse_mini("x*2..8").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Repeat(Ramp::Sweep { from: 2, to: 8 })]
        );
        // Positions can travel too, and an offset still follows.
        let m = parse_mini("x(3,4..16,1)").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Euclidean(
                Ramp::fixed(3),
                Ramp::Sweep { from: 4, to: 16 },
                Some(1)
            )]
        );
    }

    #[test]
    fn test_a_step_chain_holds_each_value_in_turn() {
        // The doubling build: not a smooth sweep but four held stages.
        let m = parse_mini("x(2>4>8>16,4)").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Euclidean(
                Ramp::Steps {
                    first: 2,
                    rest: vec![4, 8, 16]
                },
                Ramp::fixed(4),
                None
            )]
        );

        let m = parse_mini("x*4>8>16").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Repeat(Ramp::Steps {
                first: 4,
                rest: vec![8, 16]
            })]
        );

        let m = parse_mini("x?0.2>0.6>0.9").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Drop(Some(Ramp::Steps {
                first: 0.2,
                rest: vec![0.6, 0.9]
            }))]
        );
    }

    #[test]
    fn test_a_step_chain_does_not_swallow_an_alternation_close() {
        // `<x*4>` must still parse: the chain has to backtrack off the `>`.
        let m = parse_mini("<x*4 x*8>").unwrap();
        let Atom::Alternation(ref alternation) = m.sequence.steps[0].atom else {
            panic!(
                "expected an alternation, got {:?}",
                m.sequence.steps[0].atom
            );
        };
        assert_eq!(alternation.sequence.steps.len(), 2);
        assert_eq!(
            alternation.sequence.steps[0].modifiers,
            vec![Modifier::Repeat(Ramp::fixed(4))]
        );
        assert_eq!(
            alternation.sequence.steps[1].modifiers,
            vec![Modifier::Repeat(Ramp::fixed(8))]
        );
    }

    #[test]
    fn test_every_stage_of_a_chain_is_validated() {
        assert!(parse_mini("x*2>0>8").is_err());
        assert!(parse_mini("x(3,4>0)").is_err());
        assert!(parse_mini("x?0.2>2.0").is_err());
    }

    #[test]
    fn test_both_ends_of_a_range_are_validated() {
        assert!(parse_mini("x*1..0").is_err());
        assert!(parse_mini("x(3,4..0)").is_err());
        assert!(parse_mini("x?0.5..2").is_err());
    }

    #[test]
    fn test_drop_probability_out_of_range_is_rejected() {
        let error = parse_mini("x?2").unwrap_err();
        assert!(error.contains("0.0-1.0"), "unexpected message: {error}");
    }

    #[test]
    fn test_bare_drop_then_degree_is_two_steps() {
        // A space means the number is the next step, not the drop probability.
        let m = parse_mini("x? 0").unwrap();
        assert_eq!(m.sequence.steps.len(), 2);
        assert_eq!(m.sequence.steps[0].modifiers, vec![Modifier::Drop(None)]);
        assert_eq!(m.sequence.steps[1].atom, Atom::Degree(0));
    }

    // ---- Velocity and accent ----

    #[test]
    fn test_accent_is_a_full_velocity_trigger() {
        let m = parse_mini("X ~ x ~").unwrap();
        assert_eq!(m.sequence.steps[0].atom, Atom::Trigger);
        assert_eq!(m.sequence.steps[0].velocity, Some(Ramp::fixed(1.0)));
        // Lowercase keeps meaning "normal", i.e. take the line's `vel`.
        assert_eq!(m.sequence.steps[2].atom, Atom::Trigger);
        assert_eq!(m.sequence.steps[2].velocity, None);
        // And it is exactly the `:1.0` spelling.
        assert_eq!(parse_mini("X").unwrap(), parse_mini("x:1.0").unwrap());
    }

    #[test]
    fn test_explicit_velocity_on_any_sounding_atom() {
        let velocity = |source: &str| {
            parse_mini(source).unwrap().sequence.steps[0]
                .velocity
                .clone()
        };
        assert_eq!(velocity("x:0.6"), Some(Ramp::fixed(0.6)));
        assert_eq!(velocity("c4:0.35"), Some(Ramp::fixed(0.35)));
        assert_eq!(velocity("3:0.5"), Some(Ramp::fixed(0.5)));
        assert_eq!(velocity("[c4 e4]:0.8"), Some(Ramp::fixed(0.8)));
        assert_eq!(velocity("<c4 e4>:0.8"), Some(Ramp::fixed(0.8)));
        assert_eq!(velocity("solo(0..7,8):0.7"), Some(Ramp::fixed(0.7)));
        // An inner step keeps its own, so a group's velocity is only a default.
        let m = parse_mini("[c4 e4:0.4]:0.9").unwrap();
        assert_eq!(m.sequence.steps[0].velocity, Some(Ramp::fixed(0.9)));
        let Atom::Group(ref group) = m.sequence.steps[0].atom else {
            panic!("expected a group");
        };
        assert_eq!(group.layers[0].steps[1].velocity, Some(Ramp::fixed(0.4)));
    }

    #[test]
    fn test_velocity_composes_with_slot_generating_modifiers() {
        // Written once, whichever side of the modifier — one velocity for every
        // slot the step generates.
        let m = parse_mini("X*4").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Repeat(Ramp::fixed(4))]
        );
        assert_eq!(m.sequence.steps[0].velocity, Some(Ramp::fixed(1.0)));

        assert_eq!(
            parse_mini("x:0.6*4").unwrap(),
            parse_mini("x*4:0.6").unwrap()
        );

        let m = parse_mini("x:0.6?0.25").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Drop(Some(Ramp::fixed(0.25)))]
        );
        assert_eq!(m.sequence.steps[0].velocity, Some(Ramp::fixed(0.6)));

        let m = parse_mini("x(3,8):0.9").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Euclidean(Ramp::fixed(3), Ramp::fixed(8), None)]
        );
        assert_eq!(m.sequence.steps[0].velocity, Some(Ramp::fixed(0.9)));
    }

    #[test]
    fn test_velocity_may_travel() {
        let m = parse_mini("x:0.3..0.9").unwrap();
        assert_eq!(
            m.sequence.steps[0].velocity,
            Some(Ramp::Sweep { from: 0.3, to: 0.9 })
        );
        assert!(m.sequence.steps[0].velocity.as_ref().unwrap().travels());
        let m = parse_mini("x:0.3>0.6>0.9").unwrap();
        assert_eq!(
            m.sequence.steps[0].velocity,
            Some(Ramp::Steps {
                first: 0.3,
                rest: vec![0.6, 0.9]
            })
        );
        // A closing `>` is still left for the alternation.
        let m = parse_mini("<x:0.4 x:0.8>").unwrap();
        let Atom::Alternation(ref alternation) = m.sequence.steps[0].atom else {
            panic!("expected an alternation");
        };
        assert_eq!(
            alternation.sequence.steps[1].velocity,
            Some(Ramp::fixed(0.8))
        );
    }

    #[test]
    fn test_velocity_out_of_range_is_rejected_at_every_value() {
        let error = parse_mini("x:1.4").unwrap_err();
        assert!(
            error.contains("1.4") && error.contains("0.0-1.0"),
            "{error}"
        );
        assert!(parse_mini("x:0.5..1.2").unwrap_err().contains("1.2"));
        assert!(parse_mini("x:0.2>0.9>1.1").unwrap_err().contains("1.1"));
        // Validation reaches nested layers like every other check.
        assert!(parse_mini("[c4 e4:2]").is_err());
    }

    #[test]
    fn test_velocity_on_a_rest_or_a_hold_is_rejected() {
        let error = parse_mini("~:0.5").unwrap_err();
        assert!(error.contains("rest"), "{error}");
        let error = parse_mini("x _:0.5").unwrap_err();
        assert!(error.contains("hold"), "{error}");
    }

    #[test]
    fn test_one_velocity_per_step() {
        let error = parse_mini("x:0.6:0.8").unwrap_err();
        assert!(error.contains("one velocity"), "{error}");
        // `X` already set one.
        let error = parse_mini("X:0.6").unwrap_err();
        assert!(error.contains("already a velocity of 1.0"), "{error}");
    }

    // ---- Random choice ----

    #[test]
    fn test_random_choice_group() {
        let m = parse_mini("[c4|e4|g4]").unwrap();
        assert_eq!(m.sequence.steps.len(), 1);
        if let Atom::Group(ref g) = m.sequence.steps[0].atom {
            assert_eq!(g.mode, GroupMode::Random);
            assert_eq!(g.layers.len(), 3);
            assert_eq!(g.layers[0].steps[0].atom, nat(NoteLetter::C, 4));
            assert_eq!(g.layers[2].steps[0].atom, nat(NoteLetter::G, 4));
        } else {
            panic!("expected random-choice group");
        }
    }

    #[test]
    fn test_random_choice_layers_may_be_sequences() {
        let m = parse_mini("[x x|x ~ x]").unwrap();
        if let Atom::Group(ref g) = m.sequence.steps[0].atom {
            assert_eq!(g.mode, GroupMode::Random);
            assert_eq!(g.layers[0].steps.len(), 2);
            assert_eq!(g.layers[1].steps.len(), 3);
        } else {
            panic!("expected random-choice group");
        }
    }

    #[test]
    fn test_mixing_chord_and_random_separators_is_rejected() {
        let error = parse_mini("[c4,e4|g4]").unwrap_err();
        assert!(error.contains("not both"), "unexpected message: {error}");
    }

    // ---- Validation ----

    #[test]
    fn test_euclidean_accepts_more_onsets_than_steps() {
        // `(9,8)` is nine onsets over eight slots, so one slot subdivides.
        let m = parse_mini("x(9,8)").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifiers,
            vec![Modifier::Euclidean(Ramp::fixed(9), Ramp::fixed(8), None)]
        );
        assert!(parse_mini("x(20,8)").is_ok());
    }

    #[test]
    fn test_zero_counts_are_rejected() {
        assert!(parse_mini("x*0").is_err());
        assert!(parse_mini("x!0").is_err());
        assert!(parse_mini("x@0").is_err());
        assert!(parse_mini("x(3,0)").is_err());
        // A zero-onset Euclidean is a legitimate silence, not an error.
        assert!(parse_mini("x(0,8)").is_ok());
    }

    #[test]
    fn test_validation_reaches_nested_layers() {
        assert!(parse_mini("[c4 e4*0]").is_err());
        assert!(parse_mini("<c4 e4?3>").is_err());
    }
}
