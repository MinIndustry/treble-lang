//! Tests for piece resolution (§8).

use crate::ast::{Accidental, NoteLetter, ScaleMode, Span};
use crate::parser::parse_program;
use crate::session::Session;

/// Resolve a source that is expected to parse and arrange cleanly.
fn piece(source: &str) -> crate::piece::Piece {
    let (program, errors) = parse_program(source);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let (piece, errors) = super::resolve(&program, (120, (4, 4), None));
    assert!(errors.is_empty(), "resolve errors: {errors:?}");
    piece
}

/// Collect the messages from resolving a source, whether they come from the
/// parser or from the resolver.
fn complaints(source: &str) -> Vec<String> {
    let (program, parse_errors) = parse_program(source);
    let (_, resolve_errors) = super::resolve(&program, (120, (4, 4), None));
    parse_errors
        .into_iter()
        .chain(resolve_errors)
        .map(|error| error.message)
        .collect()
}

fn assert_complains(source: &str, needle: &str) {
    let messages = complaints(source);
    assert!(
        messages.iter().any(|message| message.contains(needle)),
        "expected a complaint containing {needle:?}, got {messages:?}"
    );
}

// --- Mode detection ---

#[test]
fn a_buffer_without_sections_is_not_a_piece() {
    let (program, _) = parse_program("bpm 120\nkick kick \"x ~ x ~\"\n");
    assert!(!super::is_piece(&program));

    let mut session = Session::new();
    session.evaluate("bpm 120\nkick kick \"x ~ x ~\"\n");
    assert!(!session.is_piece());
    assert!(session.piece().is_none());
}

#[test]
fn one_section_makes_it_a_piece() {
    let mut session = Session::new();
    let result = session.evaluate("section verse 8 {\n  kick kick \"x ~ x ~\"\n}\n");
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(session.is_piece());
    assert_eq!(session.piece().unwrap().total_cycles(), 8);
}

// --- Sections ---

#[test]
fn a_section_states_its_length() {
    let piece = piece("section verse 16 {\n  kick kick \"x ~ x ~\"\n}\n");
    assert_eq!(piece.sections.len(), 1);
    assert_eq!(piece.sections[0].name, "verse");
    assert_eq!(piece.sections[0].cycles, 16);
    assert_eq!(piece.sections[0].patterns.len(), 1);
    assert_eq!(piece.total_cycles(), 16);
}

#[test]
fn a_section_without_a_length_is_refused() {
    assert_complains("section verse {\n}\n", "needs a length in cycles");
}

#[test]
fn a_zero_length_section_is_refused() {
    assert_complains("section verse 0 {\n}\n", "at least one cycle");
}

#[test]
fn members_are_tagged_with_their_section() {
    let piece = piece("section verse 8 {\n  kick kick \"x ~ x ~\"\n}\n");
    assert_eq!(
        piece.sections[0].patterns[0].section.as_deref(),
        Some("verse")
    );
}

#[test]
fn a_line_outside_every_section_sounds_throughout() {
    let piece = piece("drone pad \"c2\"\nsection verse 8 {\n  kick kick \"x\"\n}\n");
    assert_eq!(piece.throughout.len(), 1);
    assert_eq!(piece.throughout[0].name, "drone");
    assert!(piece.throughout[0].section.is_none());
}

#[test]
fn sections_do_not_nest() {
    assert_complains(
        "section a 8 {\nsection b 4 {\n}\n}\n",
        "sections don't nest",
    );
}

#[test]
fn a_section_is_closed() {
    assert_complains("section verse 8 {\n  kick kick \"x\"\n", "never closed");
}

#[test]
fn a_section_footer_takes_no_filters() {
    assert_complains(
        "section verse 8 {\n  kick kick \"x\"\n} | lpf 900\n",
        "takes no filters",
    );
}

#[test]
fn a_muted_section_still_takes_its_time() {
    let piece =
        piece("; section verse 8 {\n  kick kick \"x\"\n}\nsection b 4 {\n  hat hihat \"x\"\n}\n");
    assert!(piece.sections[0].muted);
    // Muted or not, the arrangement is twelve cycles long.
    assert_eq!(piece.total_cycles(), 12);
    assert_eq!(piece.sections[0].audible_on(1).count(), 0);
}

#[test]
fn a_section_declared_twice_is_refused() {
    assert_complains(
        "section verse 8 {\n}\nsection verse 4 {\n}\n",
        "declared twice",
    );
}

// --- Spans ---

#[test]
fn a_bare_span_is_one_cycle() {
    let piece = piece("section verse 16 {\n  fill snare \"x*16\" @ 16\n}\n");
    assert_eq!(piece.sections[0].patterns[0].span, Some(Span::at(16)));
    assert_eq!(piece.sections[0].audible_on(16).count(), 1);
    assert_eq!(piece.sections[0].audible_on(15).count(), 0);
}

#[test]
fn a_span_is_inclusive_at_both_ends() {
    let piece = piece("section verse 16 {\n  bass saw \"c2\" @ 3..8\n}\n");
    for cycle in [3, 4, 8] {
        assert_eq!(
            piece.sections[0].audible_on(cycle).count(),
            1,
            "cycle {cycle} should sound"
        );
    }
    for cycle in [1, 2, 9, 16] {
        assert_eq!(
            piece.sections[0].audible_on(cycle).count(),
            0,
            "cycle {cycle} should be silent"
        );
    }
}

#[test]
fn an_open_ended_span_runs_to_the_section_end() {
    let piece = piece("section verse 16 {\n  pad pad \"c3\" @ 9..\n}\n");
    let span = piece.sections[0].patterns[0].span.unwrap();
    assert_eq!(span.from, Some(9));
    assert_eq!(span.to, None);
    assert_eq!(span.end(16), 16);
    assert_eq!(piece.sections[0].audible_on(16).count(), 1);
    assert_eq!(piece.sections[0].audible_on(8).count(), 0);
}

#[test]
fn an_open_started_span_runs_from_the_section_start() {
    let piece = piece("section verse 16 {\n  hat hihat \"x*8\" @ ..8\n}\n");
    let span = piece.sections[0].patterns[0].span.unwrap();
    assert_eq!(span.from, None);
    assert_eq!(span.start(), 1);
    assert_eq!(piece.sections[0].audible_on(1).count(), 1);
    assert_eq!(piece.sections[0].audible_on(9).count(), 0);
}

#[test]
fn a_span_composes_with_a_pipeline() {
    let piece = piece("section verse 16 {\n  bass saw \"c2\" @ 3..8 | lpf 900 | gain 0.5\n}\n");
    let line = &piece.sections[0].patterns[0];
    assert_eq!(
        line.span,
        Some(super::super::ast::Span {
            from: Some(3),
            to: Some(8)
        })
    );
    assert_eq!(line.transforms.len(), 2);
}

#[test]
fn a_span_past_the_section_is_refused() {
    assert_complains(
        "section verse 8 {\n  fill snare \"x\" @ 12\n}\n",
        "past the section's 8",
    );
}

#[test]
fn a_backwards_span_is_refused() {
    assert_complains(
        "section verse 16 {\n  bass saw \"c2\" @ 8..3\n}\n",
        "starts at cycle 8 but ends at 3",
    );
}

#[test]
fn a_zero_cycle_span_is_refused() {
    assert_complains("section verse 8 {\n  x kick \"x\" @ 0\n}\n", "1-based");
}

#[test]
fn a_span_outside_a_section_is_refused() {
    assert_complains(
        "drone pad \"c2\" @ 3..8\nsection verse 8 {\n  kick kick \"x\"\n}\n",
        "a span needs a section",
    );
}

#[test]
fn an_empty_span_is_refused() {
    assert_complains(
        "section verse 8 {\n  x kick \"x\" @ ..\n}\n",
        "names no cycles",
    );
}

// --- Arrangement ---

#[test]
fn arrange_orders_and_repeats_sections() {
    let piece = piece(
        "section intro 4 {\n  a kick \"x\"\n}\n\
         section verse 8 {\n  b kick \"x\"\n}\n\
         arrange intro verse*2 intro\n",
    );
    assert_eq!(piece.timeline.len(), 4);
    let names: Vec<&str> = piece
        .timeline
        .iter()
        .map(|o| piece.sections[o.section].name.as_str())
        .collect();
    assert_eq!(names, ["intro", "verse", "verse", "intro"]);
    let starts: Vec<u64> = piece.timeline.iter().map(|o| o.start_cycle).collect();
    assert_eq!(starts, [0, 4, 12, 20]);
    assert_eq!(piece.total_cycles(), 24);
}

#[test]
fn without_arrange_the_sections_play_in_source_order() {
    let piece = piece("section a 4 {\n  x kick \"x\"\n}\nsection b 8 {\n  y kick \"x\"\n}\n");
    let names: Vec<&str> = piece
        .timeline
        .iter()
        .map(|o| piece.sections[o.section].name.as_str())
        .collect();
    assert_eq!(names, ["a", "b"]);
    assert_eq!(piece.total_cycles(), 12);
}

#[test]
fn an_unknown_section_in_arrange_is_refused() {
    assert_complains(
        "section verse 8 {\n  x kick \"x\"\n}\narrange verse chorus\n",
        "'chorus' is not a section",
    );
}

#[test]
fn a_second_arrange_is_refused() {
    assert_complains(
        "section a 4 {\n  x kick \"x\"\n}\narrange a\narrange a a\n",
        "already has an arrangement",
    );
}

#[test]
fn a_zero_repeat_is_refused() {
    assert_complains("section a 4 {\n}\narrange a*0\n", "plays nothing");
}

#[test]
fn an_unplayed_section_is_reported_but_legal() {
    let piece = piece(
        "section a 4 {\n  x kick \"x\"\n}\nsection sketch 4 {\n  y kick \"x\"\n}\narrange a\n",
    );
    assert_eq!(piece.unused, ["sketch"]);
}

// --- Section-scoped directives (§8.5) ---

#[test]
fn a_directive_inside_a_section_is_scoped_to_it() {
    let piece = piece(
        "bpm 96\n\
         section verse 8 {\n  a kick \"x\"\n}\n\
         section chorus 8 {\n  bpm 104\n  b kick \"x\"\n}\n\
         section outro 8 {\n  c kick \"x\"\n}\n",
    );
    assert_eq!(piece.sections[0].bpm, 96);
    assert_eq!(piece.sections[1].bpm, 104);
    // The chorus's tempo does not leak into the outro — that is what makes a
    // named section sound the same wherever the arrangement puts it.
    assert_eq!(piece.sections[2].bpm, 96);
}

#[test]
fn a_section_inherits_the_scale_above_it() {
    let piece = piece("scale C minor\nsection verse 8 {\n  a saw \"0 2 4\"\n}\n");
    let (root, mode) = piece.sections[0].scale.unwrap();
    assert_eq!(root.name, NoteLetter::C);
    assert_eq!(root.accidental, Accidental::Natural);
    assert_eq!(mode, ScaleMode::Minor);
}

#[test]
fn a_section_scoped_sig_changes_its_length_in_time_not_in_cycles() {
    let piece = piece(
        "bpm 120\nsection a 4 {\n  x kick \"x\"\n}\n\
         section b 4 {\n  sig 3/4\n  y kick \"x\"\n}\n",
    );
    assert_eq!(piece.sections[0].cycles, piece.sections[1].cycles);
    assert_eq!(piece.sections[0].sig, (4, 4));
    assert_eq!(piece.sections[1].sig, (3, 4));
    // 4/4 at 120 is 2s a cycle; 3/4 is 1.5s.
    assert!((piece.sections[0].cycle_seconds() - 2.0).abs() < 1e-9);
    assert!((piece.sections[1].cycle_seconds() - 1.5).abs() < 1e-9);
    assert!((piece.seconds() - (8.0 + 6.0)).abs() < 1e-9);
}

#[test]
fn a_buffer_wide_directive_inside_a_section_is_refused() {
    for directive in ["load \"pads.trbl\"", "phrase 8", "tail 3.0", "arrange a"] {
        assert_complains(
            &format!("section a 4 {{\n  {directive}\n  x kick \"x\"\n}}\n"),
            "configures the whole piece",
        );
    }
}

#[test]
fn a_def_inside_a_section_is_refused() {
    assert_complains(
        "section a 4 {\n  def wobble {\n    voice mono\n  }\n}\n",
        "cannot live inside section",
    );
}

// --- Groups inside sections (§7, §8.2) ---

#[test]
fn a_group_nests_one_level_inside_a_section() {
    let piece = piece(
        "section verse 8 {\n\
         \x20 group drums {\n\
         \x20   kick kick \"x ~ x ~\"\n\
         \x20   hat hihat \"x*8\"\n\
         \x20 } | lpf 1800\n\
         }\n",
    );
    assert_eq!(piece.sections[0].groups.len(), 1);
    assert_eq!(piece.sections[0].groups[0].name, "drums");
    assert_eq!(piece.sections[0].groups[0].transforms.len(), 1);
    assert_eq!(piece.sections[0].patterns.len(), 2);
    assert!(
        piece.sections[0]
            .patterns
            .iter()
            .all(|p| p.group.as_deref() == Some("drums"))
    );
}

#[test]
fn a_section_cannot_open_inside_a_group() {
    assert_complains(
        "group drums {\nsection a 4 {\n}\n}\n",
        "a group nests inside a section",
    );
}

#[test]
fn one_bus_may_be_declared_in_several_sections() {
    let piece = piece(
        "section a 4 {\n  group drums {\n    kick kick \"x\"\n  } | lpf 1800\n}\n\
         section b 4 {\n  group drums {\n    hat hihat \"x*8\"\n  } | lpf 1800\n}\n",
    );
    assert_eq!(piece.sections.len(), 2);
    assert_eq!(piece.sections[0].groups[0].name, "drums");
    assert_eq!(piece.sections[1].groups[0].name, "drums");
}

#[test]
fn a_bus_cannot_change_its_chain_between_sections() {
    assert_complains(
        "section a 4 {\n  group drums {\n    kick kick \"x\"\n  } | lpf 1800\n}\n\
         section b 4 {\n  group drums {\n    hat hihat \"x*8\"\n  } | lpf 400\n}\n",
        "a bus is one bus for the whole piece",
    );
}

// --- tail and seed ---

#[test]
fn tail_defaults_and_is_settable() {
    let bare = piece("section a 4 {\n  x kick \"x\"\n}\n");
    assert_eq!(bare.tail, super::DEFAULT_TAIL);

    let tailed = piece("tail 3.5\nsection a 4 {\n  x kick \"x\"\n}\n");
    assert_eq!(tailed.tail, 3.5);
    assert!((tailed.render_seconds() - (tailed.seconds() + 3.5)).abs() < 1e-9);
}

#[test]
fn a_zero_tail_is_legal() {
    let piece = piece("tail 0\nsection a 4 {\n  x kick \"x\"\n}\n");
    assert_eq!(piece.tail, 0.0);
}

#[test]
fn a_negative_tail_is_refused() {
    assert_complains("tail -1\nsection a 4 {\n}\n", "zero or more seconds");
}

#[test]
fn seed_defaults_to_zero_and_is_settable() {
    assert_eq!(piece("section a 4 {\n  x kick \"x\"\n}\n").seed, 0);
    assert_eq!(piece("seed 7\nsection a 4 {\n  x kick \"x\"\n}\n").seed, 7);
}

#[test]
fn seed_is_accepted_in_a_live_buffer_but_arrange_and_tail_are_not() {
    let mut session = Session::new();
    let result = session.evaluate("seed 7\nkick kick \"x ~ x ~\"\n");
    assert!(result.errors.is_empty(), "{:?}", result.errors);

    for directive in ["arrange verse", "tail 2.0"] {
        let mut session = Session::new();
        let result = session.evaluate(&format!("{directive}\nkick kick \"x ~ x ~\"\n"));
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.message.contains("a live buffer has no sections")),
            "{directive} should be refused in a live buffer, got {:?}",
            result.errors
        );
    }
}

#[test]
fn phrase_is_refused_in_a_piece() {
    assert_complains(
        "phrase 8\nsection a 4 {\n  x kick \"x\"\n}\n",
        "no boundary to land a change on",
    );
}

// --- Naming ---

#[test]
fn the_same_pattern_name_in_two_sections_is_normal() {
    let piece = piece(
        "section a 4 {\n  kick kick \"x ~ x ~\"\n}\n\
         section b 4 {\n  kick kick \"x*4\"\n}\n",
    );
    assert_eq!(piece.sections[0].patterns[0].name, "kick");
    assert_eq!(piece.sections[1].patterns[0].name, "kick");
}

#[test]
fn the_same_pattern_name_twice_in_one_section_is_refused() {
    assert_complains(
        "section a 4 {\n  kick kick \"x\"\n  kick kick \"x*4\"\n}\n",
        "is defined twice in section 'a'",
    );
}

#[test]
fn a_section_and_a_pattern_cannot_share_a_name() {
    assert_complains(
        "section verse 4 {\n  verse kick \"x\"\n}\n",
        "names both a section and a pattern",
    );
}

#[test]
fn a_throughout_line_cannot_shadow_a_section_member() {
    assert_complains(
        "kick kick \"x\"\nsection a 4 {\n  kick kick \"x*4\"\n}\n",
        "sounds throughout the piece and a member of section",
    );
}

// --- The timeline ---

#[test]
fn walk_places_each_occurrence_in_cycles_and_seconds() {
    let piece = piece(
        "bpm 120\n\
         section a 4 {\n  x kick \"x\"\n}\n\
         section b 2 {\n  bpm 60\n  y kick \"x\"\n}\n\
         arrange a b a\n",
    );
    let placed: Vec<_> = piece.walk().collect();
    assert_eq!(placed.len(), 3);

    // 4/4 at 120 BPM is 2s a cycle; at 60 BPM it is 4s.
    assert_eq!(placed[0].start_cycle, 0);
    assert!((placed[0].start_seconds - 0.0).abs() < 1e-9);
    assert_eq!(placed[1].start_cycle, 4);
    assert!((placed[1].start_seconds - 8.0).abs() < 1e-9);
    assert_eq!(placed[2].start_cycle, 6);
    assert!((placed[2].start_seconds - 16.0).abs() < 1e-9);

    assert_eq!(piece.total_cycles(), 10);
    assert!((piece.seconds() - 24.0).abs() < 1e-9);
}

#[test]
fn the_spec_example_resolves() {
    let piece = piece(
        "bpm 128\n\
         sig 4/4\n\
         tail 2.5\n\
         \n\
         section intro 8 {\n\
         \x20 kick  kick   \"x ~ x ~\"\n\
         \x20 hats  hihat  \"x*8\"        @ 5..\n\
         }\n\
         \n\
         section main 16 {\n\
         \x20 kick  kick   \"x ~ x ~\"\n\
         \x20 snare snare  \"~ X ~ x\"\n\
         \x20 bass  saw    \"c2 _ eb2 _\" | lpf 300..9000 | ramp 16 exp\n\
         \x20 fill  snare  \"x*16\"       @ 16\n\
         }\n\
         \n\
         section break 8 {\n\
         \x20 pad   pad    \"[c3,eb3,g3] ~\"\n\
         \x20 hats  hihat  \"x*8\"        @ 7..\n\
         }\n\
         \n\
         arrange intro main break main*2 intro\n",
    );

    assert_eq!(piece.sections.len(), 3);
    assert_eq!(piece.timeline.len(), 6);
    assert_eq!(piece.total_cycles(), 8 + 16 + 8 + 16 + 16 + 8);
    assert!(piece.unused.is_empty());
    assert_eq!(piece.tail, 2.5);

    // The fill lands only on the last cycle of every `main`.
    let main = piece
        .sections
        .iter()
        .find(|section| section.name == "main")
        .unwrap();
    assert_eq!(main.audible_on(16).count(), 4);
    assert_eq!(main.audible_on(15).count(), 3);
}

// --- Metadata (§8.9) ---

#[test]
fn metadata_travels_with_the_piece() {
    let piece = piece(
        "meta title \"Drift\"\nmeta composer \"A. N. Other\"\nsection a 4 {\n  x kick \"x\"\n}\n",
    );
    assert_eq!(piece.meta("title"), Some("Drift"));
    assert_eq!(piece.meta("composer"), Some("A. N. Other"));
    assert_eq!(piece.metadata.len(), 2);
}

#[test]
fn a_repeated_key_replaces_rather_than_accumulates() {
    // Editing a title must not leave the previous one in the output.
    let piece =
        piece("meta title \"First\"\nmeta title \"Second\"\nsection a 4 {\n  x kick \"x\"\n}\n");
    assert_eq!(piece.meta("title"), Some("Second"));
    assert_eq!(piece.metadata.len(), 1);
}

#[test]
fn keys_are_case_insensitive() {
    let piece = piece("meta Title \"Drift\"\nsection a 4 {\n  x kick \"x\"\n}\n");
    assert_eq!(piece.meta("title"), Some("Drift"));
}

#[test]
fn an_unquoted_or_unterminated_value_is_refused() {
    assert_complains(
        "meta title Drift\nsection a 4 {\n}\n",
        "must be double-quoted",
    );
    assert_complains("meta title\nsection a 4 {\n}\n", "expected a value");
}

#[test]
fn metadata_is_inert_in_a_live_buffer() {
    let mut session = Session::new();
    let result = session.evaluate("meta title \"A performance\"\nkick kick \"x ~ x ~\"\n");
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    // It changes no sound, so it produces no delta beyond the pattern itself.
    assert_eq!(result.deltas.len(), 1);
}
