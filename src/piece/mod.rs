//! Piece resolution (§8).
//!
//! A [`Piece`] is what a buffer means once its `section` blocks have been put
//! in the order its `arrange` line names: a flat timeline of section
//! occurrences with an absolute starting cycle, and therefore a definite
//! length. That is the whole difference between a piece and a live buffer —
//! a piece can be compiled up front and rendered to a file, where a live
//! buffer only ever knows what is sounding now.
//!
//! This module resolves and validates; it does not schedule. Turning the
//! timeline into audio is the consumer's half, exactly as it is for the live
//! language.

use crate::ast::{
    ArrangeItem, GroupDef, PatternDef, PitchRoot, Program, ScaleMode, SourceLine, Transform,
};
use crate::error::{CompileError, CompileErrorKind, SourceLocation};

/// One resolved section: its members and the state in force while it plays.
///
/// Everything a section sounds like lives here rather than on the
/// [`Occurrence`], because a section sounds the same wherever the arrangement
/// puts it (§8.5). The only thing an occurrence adds is *when*.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub name: String,
    /// A muted section still takes its time — it is a rest of its own length.
    pub muted: bool,
    /// How many cycles it lasts. Always >= 1.
    pub cycles: u32,
    /// Tempo in force, after any `bpm` written inside the section (§8.5).
    pub bpm: u32,
    /// Time signature in force.
    pub sig: (u8, u8),
    /// Default scale in force.
    pub scale: Option<(PitchRoot, ScaleMode)>,
    /// Member pattern lines, in source order.
    pub patterns: Vec<PatternDef>,
    /// Groups declared inside the section, in source order.
    pub groups: Vec<GroupDef>,
}

impl Section {
    /// How long one of this section's cycles lasts, in seconds.
    pub fn cycle_seconds(&self) -> f64 {
        cycle_seconds(self.bpm, self.sig)
    }

    /// How long the whole section lasts, in seconds.
    pub fn seconds(&self) -> f64 {
        self.cycle_seconds() * f64::from(self.cycles)
    }

    /// The members audible on `cycle` (1-based, within this section), skipping
    /// the muted ones and those whose span (§8.3) excludes the cycle.
    pub fn audible_on(&self, cycle: u32) -> impl Iterator<Item = &PatternDef> {
        let cycles = self.cycles;
        let muted = self.muted;
        self.patterns.iter().filter(move |pattern| {
            !muted && !pattern.muted && pattern.span.is_none_or(|span| span.contains(cycle, cycles))
        })
    }
}

/// One playing of a section: which one, and where it lands in the piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Occurrence {
    /// Index into [`Piece::sections`].
    pub section: usize,
    /// The absolute cycle the occurrence starts on, counted from 0 at the
    /// start of the piece.
    pub start_cycle: u64,
}

/// A buffer resolved into a timeline (§8).
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    /// Every section, in source order.
    pub sections: Vec<Section>,
    /// The arrangement as written, or the sections in source order when the
    /// buffer has no `arrange` line (§8.4).
    pub arrangement: Vec<ArrangeItem>,
    /// The flat timeline: one entry per section occurrence, in playing order.
    pub timeline: Vec<Occurrence>,
    /// Lines outside every section. They sound for the whole piece — a drone,
    /// a click track — and carry no span.
    pub throughout: Vec<PatternDef>,
    /// How long a render rings out past the last cycle, in seconds (§8.7).
    pub tail: f64,
    /// Salt for the generative constructs (§8.8).
    pub seed: u64,
    /// Sections the arrangement never plays. Legal — a sketch is not an error
    /// — but worth reporting, so it is surfaced rather than swallowed.
    pub unused: Vec<String>,
}

/// The default `tail` when a piece does not set one (§8.7).
pub const DEFAULT_TAIL: f64 = 2.0;

impl Piece {
    /// How many cycles the arrangement lasts.
    pub fn total_cycles(&self) -> u64 {
        self.timeline
            .iter()
            .map(|occurrence| u64::from(self.sections[occurrence.section].cycles))
            .sum()
    }

    /// How long the arrangement lasts in seconds, excluding [`Piece::tail`].
    ///
    /// Summed per occurrence rather than per cycle because a `sig` or `bpm`
    /// scoped to a section changes how long its cycles are (§8.5).
    pub fn seconds(&self) -> f64 {
        self.timeline
            .iter()
            .map(|occurrence| self.sections[occurrence.section].seconds())
            .sum()
    }

    /// How long a render lasts: the arrangement plus its tail.
    pub fn render_seconds(&self) -> f64 {
        self.seconds() + self.tail
    }

    /// Walk the timeline, yielding each occurrence with its section and the
    /// second it starts at.
    ///
    /// The start times are accumulated here rather than stored, so a `bpm`
    /// edit inside one section cannot leave a stale offset behind on another.
    pub fn walk(&self) -> impl Iterator<Item = PlacedSection<'_>> {
        let mut at_seconds = 0.0;
        self.timeline.iter().map(move |occurrence| {
            let section = &self.sections[occurrence.section];
            let placed = PlacedSection {
                section,
                start_cycle: occurrence.start_cycle,
                start_seconds: at_seconds,
            };
            at_seconds += section.seconds();
            placed
        })
    }
}

/// One occurrence, resolved to its section and its position in real time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedSection<'a> {
    pub section: &'a Section,
    /// Absolute cycle the occurrence starts on, counted from 0.
    pub start_cycle: u64,
    /// Absolute time the occurrence starts at, in seconds.
    pub start_seconds: f64,
}

/// How long one cycle lasts at `bpm` in `signature`, in seconds.
pub fn cycle_seconds(bpm: u32, signature: (u8, u8)) -> f64 {
    let quarters = f64::from(signature.0) * 4.0 / f64::from(signature.1.max(1));
    quarters * 60.0 / f64::from(bpm.max(1))
}

/// Whether a program is a piece: it has at least one `section` (§8.1).
pub fn is_piece(program: &Program) -> bool {
    program
        .lines
        .iter()
        .any(|line| matches!(line, SourceLine::SectionStart { .. }))
}

fn error(line: usize, message: String) -> CompileError {
    CompileError {
        kind: CompileErrorKind::ParseError,
        location: SourceLocation {
            line,
            column: 1,
            file: None,
        },
        message,
        suggestion: None,
    }
}

/// State carried down the buffer while resolving, so a section can inherit the
/// values in force above it and restore them when it ends (§8.5).
#[derive(Clone, Copy)]
struct Ambient {
    bpm: u32,
    sig: (u8, u8),
    scale: Option<(PitchRoot, ScaleMode)>,
}

/// Resolve a parsed program into a [`Piece`].
///
/// `defaults` is the state a buffer starts in — the session's own `bpm`, `sig`
/// and `scale` — which top-level directives then override.
///
/// Returns the piece alongside any errors. A piece is returned even when the
/// errors are non-empty so a consumer can still show the structure it did
/// understand; whether to play it is the consumer's call.
pub fn resolve(
    program: &Program,
    defaults: (u32, (u8, u8), Option<(PitchRoot, ScaleMode)>),
) -> (Piece, Vec<CompileError>) {
    let mut errors = Vec::new();
    let mut ambient = Ambient {
        bpm: defaults.0,
        sig: defaults.1,
        scale: defaults.2,
    };

    let mut sections: Vec<Section> = Vec::new();
    let mut throughout: Vec<PatternDef> = Vec::new();
    let mut arrangement: Option<Vec<ArrangeItem>> = None;
    let mut tail = DEFAULT_TAIL;
    let mut seed = 0u64;

    // The section being filled, and the ambient state to restore when it ends.
    let mut open: Option<(Section, Ambient)> = None;
    // The group being filled inside the current section.
    let mut open_group: Option<(bool, String, usize)> = None;

    for (index, line) in program.lines.iter().enumerate() {
        let at = index + 1;
        match line {
            SourceLine::Bpm(value) => match &mut open {
                Some((section, _)) => section.bpm = *value,
                None => ambient.bpm = *value,
            },
            SourceLine::Sig(num, den) => match &mut open {
                Some((section, _)) => section.sig = (*num, *den),
                None => ambient.sig = (*num, *den),
            },
            SourceLine::Scale(root, mode) => match &mut open {
                Some((section, _)) => section.scale = Some((*root, *mode)),
                None => ambient.scale = Some((*root, *mode)),
            },
            SourceLine::Phrase(_) => {
                errors.push(error(
                    at,
                    "phrase: a piece is not edited while it plays, so it has no boundary to \
                     land a change on — drop the line (§2.6)"
                        .to_string(),
                ));
            }
            SourceLine::Tail(seconds) => tail = *seconds,
            SourceLine::Seed(value) => seed = *value,
            SourceLine::Arrange(items) => {
                if arrangement.is_some() {
                    errors.push(error(
                        at,
                        "arrange: the piece already has an arrangement — one line names the \
                         whole order"
                            .to_string(),
                    ));
                } else {
                    arrangement = Some(items.clone());
                }
            }
            SourceLine::SectionStart {
                muted,
                name,
                cycles,
            } => {
                if sections.iter().any(|section| &section.name == name) {
                    errors.push(error(at, format!("section '{name}' is declared twice")));
                }
                open = Some((
                    Section {
                        name: name.clone(),
                        muted: *muted,
                        cycles: *cycles,
                        bpm: ambient.bpm,
                        sig: ambient.sig,
                        scale: ambient.scale,
                        patterns: Vec::new(),
                        groups: Vec::new(),
                    },
                    ambient,
                ));
            }
            SourceLine::SectionEnd => {
                if let Some((section, restore)) = open.take() {
                    // A directive inside a section is scoped to it (§8.5).
                    ambient = restore;
                    sections.push(section);
                }
            }
            SourceLine::GroupStart { muted, name } => {
                open_group = Some((*muted, name.clone(), at));
            }
            SourceLine::GroupEnd(transforms) => {
                if let Some((muted, name, _)) = open_group.take()
                    && let Some((section, _)) = &mut open
                {
                    section.groups.push(GroupDef {
                        muted,
                        name,
                        transforms: transforms.clone(),
                    });
                }
            }
            SourceLine::Pattern(def) => match &mut open {
                Some((section, _)) => {
                    if let Some(span) = def.span
                        && let Some(reason) = span.conflict(section.cycles)
                    {
                        errors.push(error(
                            at,
                            format!("'{}': {reason} — the line can never sound", def.name),
                        ));
                    }
                    if section.patterns.iter().any(|other| other.name == def.name) {
                        errors.push(error(
                            at,
                            format!(
                                "'{}' is defined twice in section '{}' — the second would \
                                 replace the first",
                                def.name, section.name
                            ),
                        ));
                    }
                    section.patterns.push(def.clone());
                }
                None => throughout.push(def.clone()),
            },
            _ => {}
        }
    }

    errors.extend(check_group_chains(&sections));
    errors.extend(check_name_collisions(&sections, &throughout));

    // With no `arrange`, the arrangement is the sections in source order (§8.4).
    let arrangement = arrangement.unwrap_or_else(|| {
        sections
            .iter()
            .map(|section| ArrangeItem {
                section: section.name.clone(),
                repeat: 1,
            })
            .collect()
    });

    let (timeline, arrange_errors) = lay_out(&sections, &arrangement);
    errors.extend(arrange_errors);

    let played: Vec<&str> = timeline
        .iter()
        .map(|occurrence| sections[occurrence.section].name.as_str())
        .collect();
    let unused = sections
        .iter()
        .filter(|section| !played.contains(&section.name.as_str()))
        .map(|section| section.name.clone())
        .collect();

    (
        Piece {
            sections,
            arrangement,
            timeline,
            throughout,
            tail,
            seed,
            unused,
        },
        errors,
    )
}

/// Turn the arrangement into a flat timeline of occurrences.
fn lay_out(
    sections: &[Section],
    arrangement: &[ArrangeItem],
) -> (Vec<Occurrence>, Vec<CompileError>) {
    let mut timeline = Vec::new();
    let mut errors = Vec::new();
    let mut cycle = 0u64;

    for item in arrangement {
        let Some(index) = sections
            .iter()
            .position(|section| section.name == item.section)
        else {
            errors.push(error(
                1,
                format!(
                    "arrange: '{}' is not a section in this buffer",
                    item.section
                ),
            ));
            continue;
        };
        for _ in 0..item.repeat {
            timeline.push(Occurrence {
                section: index,
                start_cycle: cycle,
            });
            cycle += u64::from(sections[index].cycles);
        }
    }

    if timeline.is_empty() && !sections.is_empty() {
        errors.push(error(
            1,
            "arrange: the arrangement plays no section, so the piece has no length".to_string(),
        ));
    }

    (timeline, errors)
}

/// A bus is one bus for the whole piece, so a group of the same name declared
/// in two sections must declare the same chain (§7, §8.2).
///
/// Anything else would mean either rebuilding the bus mid-piece — audible as a
/// reverb tail being cut — or silently picking one of the two chains.
fn check_group_chains(sections: &[Section]) -> Vec<CompileError> {
    let mut errors = Vec::new();
    let mut seen: Vec<(&str, &[Transform], &str)> = Vec::new();

    for section in sections {
        for group in &section.groups {
            match seen
                .iter()
                .find(|(name, _, _)| *name == group.name.as_str())
            {
                Some((_, chain, first)) if *chain != group.transforms.as_slice() => {
                    errors.push(error(
                        1,
                        format!(
                            "group '{}' declares a different chain in '{}' than in '{first}' — \
                             a bus is one bus for the whole piece, so its filters cannot change \
                             between sections",
                            group.name, section.name
                        ),
                    ));
                }
                Some(_) => {}
                None => seen.push((&group.name, &group.transforms, &section.name)),
            }
        }
    }

    errors
}

/// Names must not collide where they share a namespace.
///
/// Within a section, a pattern and a group would collide in the mixer, exactly
/// as they do in a live buffer. Across sections they do not: `kick` appearing
/// in every section is the normal way to write a piece.
fn check_name_collisions(sections: &[Section], throughout: &[PatternDef]) -> Vec<CompileError> {
    let mut errors = Vec::new();

    for section in sections {
        for group in &section.groups {
            if section.patterns.iter().any(|pattern| {
                pattern.name == group.name && pattern.group.as_deref() != Some(&group.name)
            }) {
                errors.push(error(
                    1,
                    format!(
                        "'{}' names both a group and a pattern in section '{}'; pick two names",
                        group.name, section.name
                    ),
                ));
            }
        }
        if let Some(clash) = throughout.iter().find(|line| {
            section
                .patterns
                .iter()
                .any(|member| member.name == line.name)
        }) {
            errors.push(error(
                1,
                format!(
                    "'{}' is both a line that sounds throughout the piece and a member of \
                     section '{}' — one of them would silence the other",
                    clash.name, section.name
                ),
            ));
        }
        if sections
            .iter()
            .any(|other| other.patterns.iter().any(|p| p.name == section.name))
        {
            errors.push(error(
                1,
                format!(
                    "'{}' names both a section and a pattern; pick two names",
                    section.name
                ),
            ));
        }
    }

    errors
}

#[cfg(test)]
mod tests;
