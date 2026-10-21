//! Naming what a line plays: the note a scale degree lands on, spelled the
//! way the scale spells it, and the name of a stack of notes.
//!
//! Written here rather than borrowed from a music-theory crate because the
//! job is small and specific: Treble writes chords as stacks of degrees, so
//! the vocabulary that matters is triads, sevenths, sixths, suspensions,
//! added ninths and their inversions, spelled in the key they were written in.

use crate::ast::{Accidental, NoteLetter, PitchRoot, ScaleMode};
use crate::render::compile::{degree_to_midi, mode_intervals, root_semitone};

const LETTERS: [char; 7] = ['C', 'D', 'E', 'F', 'G', 'A', 'B'];
const NATURALS: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

/// A pitch with its spelling: letter, accidental in semitones, MIDI number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spelled {
    letter: usize,
    accidental: i32,
    pub midi: i32,
}

impl Spelled {
    /// `E♭`, without the octave.
    pub fn name(&self) -> String {
        let mark = match self.accidental {
            -2 => "𝄫",
            -1 => "♭",
            1 => "♯",
            2 => "𝄪",
            _ => "",
        };
        format!("{}{mark}", LETTERS[self.letter])
    }

    /// `E♭4`. The octave follows the letter, so B♯3 is the C above B3.
    pub fn name_with_octave(&self) -> String {
        let octave = (self.midi - self.accidental).div_euclid(12) - 1;
        format!("{}{octave}", self.name())
    }

    fn pitch_class(&self) -> i32 {
        self.midi.rem_euclid(12)
    }
}

fn letter_index(letter: NoteLetter) -> usize {
    match letter {
        NoteLetter::C => 0,
        NoteLetter::D => 1,
        NoteLetter::E => 2,
        NoteLetter::F => 3,
        NoteLetter::G => 4,
        NoteLetter::A => 5,
        NoteLetter::B => 6,
    }
}

/// Spell a MIDI note with a given letter, if it is within two semitones of it.
fn with_letter(midi: i32, letter: usize) -> Option<Spelled> {
    let difference = (midi.rem_euclid(12) - NATURALS[letter] + 6).rem_euclid(12) - 6;
    (-2..=2).contains(&difference).then_some(Spelled {
        letter,
        accidental: difference,
        midi,
    })
}

/// A note as written in the notation (`eb3`), keeping its own spelling.
pub fn written(note: &crate::ast::Note) -> Spelled {
    let accidental = match note.accidental {
        Accidental::Natural => 0,
        Accidental::Sharp => 1,
        Accidental::DoubleSharp => 2,
        Accidental::Flat => -1,
        Accidental::DoubleFlat => -2,
    };
    let letter = letter_index(note.letter);
    Spelled {
        letter,
        accidental,
        midi: (i32::from(note.octave) + 1) * 12 + NATURALS[letter] + accidental,
    }
}

/// Spell a MIDI note with no scale to go by: naturals, then sharps, or flats
/// when `flats` says the key leans that way.
pub fn plain(midi: i32, flats: bool) -> Spelled {
    let class = midi.rem_euclid(12);
    if let Some(letter) = NATURALS.iter().position(|&natural| natural == class) {
        return Spelled {
            letter,
            accidental: 0,
            midi,
        };
    }
    let (letter, accidental) = if flats {
        (
            NATURALS.iter().position(|&n| n == class + 1).unwrap_or(0),
            -1,
        )
    } else {
        (
            NATURALS.iter().position(|&n| n == class - 1).unwrap_or(0),
            1,
        )
    };
    Spelled {
        letter,
        accidental,
        midi,
    }
}

/// The note scale degree `degree` plays, shifted by `octaves`. A seven-note
/// mode gives every degree its own letter, so D phrygian's second degree is
/// E♭ and not D♯; other modes fall back to the root's leaning.
pub fn degree(degree: i32, root: PitchRoot, mode: ScaleMode, octaves: i32) -> Spelled {
    let midi = i32::from(degree_to_midi(degree, root, mode)) + 12 * octaves;
    let size = mode_intervals(mode).len() as i32;
    if size == 7 {
        let letter = (letter_index(root.name) + degree.rem_euclid(7) as usize) % 7;
        if let Some(spelled) = with_letter(midi, letter) {
            return spelled;
        }
    }
    plain(midi, leans_flat(root, mode))
}

/// Whether a key is written with flats: a flat root, or F major and the keys
/// that share its signature.
pub fn leans_flat(root: PitchRoot, mode: ScaleMode) -> bool {
    if matches!(root.accidental, Accidental::Flat | Accidental::DoubleFlat) {
        return true;
    }
    if matches!(root.accidental, Accidental::Sharp | Accidental::DoubleSharp) {
        return false;
    }
    // The relative major's root: a natural-rooted mode whose major is F leans flat.
    let offset = match mode {
        ScaleMode::Major | ScaleMode::Pentatonic | ScaleMode::Chromatic => 0,
        ScaleMode::Dorian => 10,
        ScaleMode::Phrygian => 8,
        ScaleMode::Lydian => 7,
        ScaleMode::Mixolydian => 5,
        ScaleMode::Minor | ScaleMode::Aeolian | ScaleMode::Blues => 3,
        ScaleMode::Locrian => 1,
    };
    let major = (root_semitone(root) + offset).rem_euclid(12);
    // F, B♭, E♭, A♭, D♭, G♭ majors are written with flats.
    [5, 10, 3, 8, 1, 6].contains(&major)
}

/// Chord shapes by their intervals above the root, and the suffix each takes.
const SHAPES: &[(&[i32], &str)] = &[
    (&[0, 4, 7], ""),
    (&[0, 3, 7], "m"),
    (&[0, 3, 6], "dim"),
    (&[0, 4, 8], "aug"),
    (&[0, 2, 7], "sus2"),
    (&[0, 5, 7], "sus4"),
    (&[0, 7], "5"),
    (&[0, 4, 7, 11], "maj7"),
    (&[0, 3, 7, 10], "m7"),
    (&[0, 4, 7, 10], "7"),
    (&[0, 3, 6, 10], "m7♭5"),
    (&[0, 3, 6, 9], "dim7"),
    (&[0, 3, 7, 11], "m(maj7)"),
    (&[0, 4, 8, 11], "maj7♯5"),
    (&[0, 5, 7, 10], "7sus4"),
    (&[0, 4, 7, 9], "6"),
    (&[0, 3, 7, 9], "m6"),
    (&[0, 2, 4, 7], "add9"),
    (&[0, 2, 3, 7], "m(add9)"),
    (&[0, 2, 4, 7, 11], "maj9"),
    (&[0, 2, 3, 7, 10], "m9"),
    (&[0, 2, 4, 7, 10], "9"),
];

/// The name of a stack of spelled notes: `Em`, `Cmaj7`, `C/E`, `Gsus4`.
/// `None` when it is a single note or no shape in the table fits.
pub fn chord(notes: &[Spelled]) -> Option<String> {
    let mut classes: Vec<i32> = notes.iter().map(Spelled::pitch_class).collect();
    classes.sort_unstable();
    classes.dedup();
    if classes.len() < 2 {
        return None;
    }
    let bass = notes.iter().min_by_key(|note| note.midi)?;
    // Try the bass as the root first, so a root-position chord is never named
    // as an inversion of something else.
    let mut candidates: Vec<&Spelled> = vec![bass];
    candidates.extend(
        notes
            .iter()
            .filter(|note| note.pitch_class() != bass.pitch_class()),
    );
    for root in candidates {
        let mut intervals: Vec<i32> = classes
            .iter()
            .map(|class| (class - root.pitch_class()).rem_euclid(12))
            .collect();
        intervals.sort_unstable();
        if let Some((_, suffix)) = SHAPES
            .iter()
            .find(|(shape, _)| *shape == intervals.as_slice())
        {
            let name = format!("{}{suffix}", root.name());
            return Some(if root.pitch_class() == bass.pitch_class() {
                name
            } else {
                format!("{name}/{}", bass.name())
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: NoteLetter, accidental: Accidental) -> PitchRoot {
        PitchRoot { name, accidental }
    }

    fn stack(degrees: &[i32], key: PitchRoot, mode: ScaleMode) -> Option<String> {
        let notes: Vec<Spelled> = degrees.iter().map(|&d| degree(d, key, mode, 0)).collect();
        chord(&notes)
    }

    #[test]
    fn degrees_are_spelled_in_their_scale() {
        let d = root(NoteLetter::D, Accidental::Natural);
        let names: Vec<String> = (0..7)
            .map(|step| degree(step, d, ScaleMode::Phrygian, 0).name())
            .collect();
        assert_eq!(names, ["D", "E♭", "F", "G", "A", "B♭", "C"]);
        let e = root(NoteLetter::E, Accidental::Natural);
        assert_eq!(degree(0, e, ScaleMode::Aeolian, 0).name_with_octave(), "E4");
        assert_eq!(
            degree(3, e, ScaleMode::Aeolian, -2).name_with_octave(),
            "A2"
        );
        assert_eq!(
            degree(-1, e, ScaleMode::Aeolian, 0).name_with_octave(),
            "D4"
        );
    }

    #[test]
    fn stacks_are_named() {
        let c = root(NoteLetter::C, Accidental::Natural);
        let e = root(NoteLetter::E, Accidental::Natural);
        assert_eq!(
            stack(&[0, 2, 4], e, ScaleMode::Aeolian).as_deref(),
            Some("Em")
        );
        assert_eq!(
            stack(&[5, 7, 9], e, ScaleMode::Aeolian).as_deref(),
            Some("C")
        );
        assert_eq!(
            stack(&[0, 2, 4, 6], c, ScaleMode::Major).as_deref(),
            Some("Cmaj7")
        );
        assert_eq!(
            stack(&[4, 6, 8, 10], c, ScaleMode::Major).as_deref(),
            Some("G7")
        );
        assert_eq!(
            stack(&[6, 8, 10], c, ScaleMode::Major).as_deref(),
            Some("Bdim")
        );
        assert_eq!(
            stack(&[2, 4, 7], c, ScaleMode::Major).as_deref(),
            Some("C/E")
        );
        assert_eq!(
            stack(&[0, 3, 4], c, ScaleMode::Major).as_deref(),
            Some("Csus4")
        );
        assert_eq!(stack(&[0, 4], c, ScaleMode::Major).as_deref(), Some("C5"));
        assert_eq!(stack(&[0], c, ScaleMode::Major), None);
    }

    #[test]
    fn flat_keys_spell_with_flats_outside_seven_note_modes() {
        let f = root(NoteLetter::F, Accidental::Natural);
        assert!(leans_flat(f, ScaleMode::Major));
        assert_eq!(plain(70, true).name(), "B♭");
        assert_eq!(plain(70, false).name(), "A♯");
    }
}
