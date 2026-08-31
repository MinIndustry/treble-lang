//! Report overlapping semitone simultaneities in a piece, by part pair.
fn main() {
    let path = std::env::args().nth(1).expect("usage: clashes <file.rt>");
    let source = std::fs::read_to_string(&path).expect("read");
    let mut session = treble_lang::Session::new();
    session.evaluate(&source);
    let piece = session.piece().expect("a piece").clone();
    let mut registry = treble::instruments::prelude::InstrumentRegistry::built_in();
    let cycle = piece.sections.first().map_or(2.0, |s| s.cycle_seconds());
    for d in session.definitions().values() {
        registry
            .register(treble_lang::render::compile::lower_instrument_def(d, cycle).unwrap())
            .unwrap();
    }
    let notes = treble_lang::render::scheduled_notes(&piece, &registry, 44_100).unwrap();
    let pitched: Vec<_> = notes
        .iter()
        .filter(|n| {
            // Percussion pitch is a trigger, not a pitch.
            !["clock", "skin", "dust", "pulse"].contains(&n.line.as_str())
        })
        .collect();

    let mut pairs: std::collections::BTreeMap<(String, String, String), (usize, f64)> =
        Default::default();
    let mut total_overlap = 0.0f64;
    for (i, a) in pitched.iter().enumerate() {
        for b in pitched[i + 1..].iter() {
            if b.start >= a.end {
                continue;
            }
            if b.start > a.start + 44_100 * 2 {
                break;
            }
            if a.line == b.line || a.midi.abs_diff(b.midi) != 1 {
                continue;
            }
            // Real overlap in seconds: both gates open at once.
            let overlap = (a.end.min(b.end).saturating_sub(b.start.max(a.start))) as f64 / 44_100.0;
            if overlap <= 0.02 {
                continue;
            }
            total_overlap += overlap;
            let (x, y) = if a.line < b.line {
                (a.line.clone(), b.line.clone())
            } else {
                (b.line.clone(), a.line.clone())
            };
            let entry = pairs.entry((a.section.clone(), x, y)).or_insert((0, 0.0));
            entry.0 += 1;
            entry.1 += overlap;
        }
    }
    println!("{} pitched note events", pitched.len());
    println!(
        "overlapping semitones between different parts: {} occurrences, {:.1}s of total overlap\n",
        pairs.values().map(|(n, _)| n).sum::<usize>(),
        total_overlap
    );
    let mut rows: Vec<_> = pairs.into_iter().collect();
    rows.sort_by(|a, b| b.1.1.partial_cmp(&a.1.1).unwrap());
    for ((section, x, y), (count, secs)) in rows.iter().take(15) {
        println!("  {section:<8} {x:<8} vs {y:<8}  {count:>4}×  {secs:>6.2}s");
    }
}
