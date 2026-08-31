//! Print the pitch-class census of a piece: `cargo run --example notes -- file.rt`
fn main() {
    let path = std::env::args().nth(1).expect("usage: notes <file.rt>");
    let source = std::fs::read_to_string(&path).expect("read");
    let mut session = treble_lang::Session::new();
    let result = session.evaluate(&source);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let piece = session.piece().expect("a piece").clone();

    let mut registry = treble::instruments::prelude::InstrumentRegistry::built_in();
    let cycle = piece.sections.first().map_or(2.0, |s| s.cycle_seconds());
    for definition in session.definitions().values() {
        let spec = treble_lang::render::compile::lower_instrument_def(definition, cycle)
            .unwrap_or_else(|e| panic!("def {}: {e}", definition.name));
        registry.register(spec).expect("register");
    }
    // Percussion is unpitched: its "note" is a trigger, not a pitch.
    let percussive: Vec<String> = registry
        .specs()
        .filter(|s| registry.is_percussion(&s.name))
        .map(|s| s.name.clone())
        .collect();
    eprintln!("percussion (excluded from the census): {percussive:?}");

    let notes = treble_lang::render::scheduled_notes(&piece, &registry, 44_100).expect("notes");
    let mut census = [0usize; 12];
    let (mut lo, mut hi) = (127u8, 0u8);
    for note in &notes {
        census[(note.midi % 12) as usize] += 1;
        lo = lo.min(note.midi);
        hi = hi.max(note.midi);
    }
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    println!("\n{} note events, midi {lo}..{hi}\n", notes.len());
    let total: usize = census.iter().sum();
    for (pc, count) in census.iter().enumerate() {
        let share = 100.0 * *count as f64 / total.max(1) as f64;
        println!(
            "  {:<3} {:>7}  {:>5.1}%  {}",
            NAMES[pc],
            count,
            share,
            "#".repeat((share / 2.0) as usize)
        );
    }
}
