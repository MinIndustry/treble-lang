//! Mini-notation parser using nom combinators.
//!
//! Parses the content inside double-quoted pattern strings into a
//! [`MiniNotation`] AST.

use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::tag,
    character::complete::{char, digit1, one_of, space0, space1},
    combinator::{map, map_res, opt, value},
    multi::separated_list1,
    sequence::{delimited, preceded},
};

use crate::ast::mini::*;
use crate::ast::program::{Accidental, NoteLetter};

/// Parse a mini-notation string into a [`MiniNotation`].
pub fn parse_mini(input: &str) -> Result<MiniNotation, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(MiniNotation {
            sequence: Sequence { steps: vec![] },
        });
    }
    match parse_sequence(trimmed) {
        Ok(("", seq)) => Ok(MiniNotation { sequence: seq }),
        Ok((rest, _)) => Err(format!("unexpected trailing input: '{}'", rest)),
        Err(e) => Err(format!("mini-notation parse error: {}", e)),
    }
}

// --- Sequence ---

fn parse_sequence(input: &str) -> IResult<&str, Sequence> {
    let (input, _) = space0(input)?;
    let (input, steps) = separated_list1(space1, parse_step).parse(input)?;
    let (input, _) = space0(input)?;
    Ok((input, Sequence { steps }))
}

// --- Step = Atom [ Modifier ] ---

fn parse_step(input: &str) -> IResult<&str, Step> {
    let (input, atom) = parse_atom(input)?;
    let (input, modifier) = opt(parse_modifier).parse(input)?;
    Ok((input, Step { atom, modifier }))
}

// --- Atom ---

fn parse_atom(input: &str) -> IResult<&str, Atom> {
    alt((
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
        if input.starts_with('b') {
            let after_b = &input[1..];
            if after_b.starts_with(|c: char| c.is_ascii_digit()) {
                return Ok((after_b, Accidental::Flat));
            }
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

// --- Group: [ sequence {, sequence} ] ---

fn parse_group(input: &str) -> IResult<&str, Atom> {
    let (input, _) = char('[').parse(input)?;
    let (input, _) = space0(input)?;
    let (input, layers) =
        separated_list1(delimited(space0, char(','), space0), parse_sequence).parse(input)?;
    let (input, _) = space0(input)?;
    let (input, _) = char(']').parse(input)?;
    Ok((input, Atom::Group(Group { layers })))
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
    let (input, n) = parse_u32(input)?;
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
    let (input, beats) = parse_u32(input)?;
    let (input, _) = delimited(space0, char(','), space0).parse(input)?;
    let (input, steps) = parse_u32(input)?;
    let (input, offset) =
        opt(preceded(delimited(space0, char(','), space0), parse_u32)).parse(input)?;
    let (input, _) = space0(input)?;
    let (input, _) = char(')').parse(input)?;
    Ok((input, Modifier::Euclidean(beats, steps, offset)))
}

fn parse_drop(input: &str) -> IResult<&str, Modifier> {
    value(Modifier::Drop, char('?')).parse(input)
}

fn parse_weight(input: &str) -> IResult<&str, Modifier> {
    let (input, _) = char('@').parse(input)?;
    let (input, n) = parse_u32(input)?;
    Ok((input, Modifier::Weight(n)))
}

fn parse_u32(input: &str) -> IResult<&str, u32> {
    map_res(digit1, |s: &str| s.parse::<u32>()).parse(input)
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(m.sequence.steps[0].modifier, Some(Modifier::Repeat(4)));
    }

    #[test]
    fn test_slow_modifier() {
        let m = parse_mini("[c4 e4 g4]/2").unwrap();
        assert!(matches!(m.sequence.steps[0].atom, Atom::Group(_)));
        assert_eq!(m.sequence.steps[0].modifier, Some(Modifier::Slow(2)));
    }

    #[test]
    fn test_replicate() {
        let m = parse_mini("c4!3").unwrap();
        assert_eq!(m.sequence.steps[0].modifier, Some(Modifier::Replicate(3)));
    }

    #[test]
    fn test_euclidean() {
        let m = parse_mini("c4(3,8)").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifier,
            Some(Modifier::Euclidean(3, 8, None))
        );
    }

    #[test]
    fn test_euclidean_with_offset() {
        let m = parse_mini("x(5,8,2)").unwrap();
        assert_eq!(
            m.sequence.steps[0].modifier,
            Some(Modifier::Euclidean(5, 8, Some(2)))
        );
    }

    #[test]
    fn test_drop() {
        let m = parse_mini("c4?").unwrap();
        assert_eq!(m.sequence.steps[0].modifier, Some(Modifier::Drop));
    }

    #[test]
    fn test_weight() {
        let m = parse_mini("c4@3 e4").unwrap();
        assert_eq!(m.sequence.steps[0].modifier, Some(Modifier::Weight(3)));
        assert_eq!(m.sequence.steps[1].modifier, None);
    }

    // ---- Groups and chords ----

    #[test]
    fn test_group() {
        let m = parse_mini("[c4 e4] g4").unwrap();
        assert_eq!(m.sequence.steps.len(), 2);
        if let Atom::Group(ref g) = m.sequence.steps[0].atom {
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
        assert_eq!(m.sequence.steps[0].modifier, Some(Modifier::Repeat(3)));
    }
}
