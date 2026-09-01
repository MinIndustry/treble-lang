use criterion::{Criterion, criterion_group, criterion_main};
use treble::instruments::prelude::InstrumentRegistry;
use treble_lang::{parser::parse_program, piece, render::render};

const LARGE_ARRANGEMENT: &str = r#"
bpm 110
sig 4/4
scale C minor
seed 41
tail 3

section movement 8 {
  pulse kick  "x ~ x ~"                    | gain 0.55
  back  snare "~ x ~ x"                    | gain 0.38 | reverb 0.16
  air   hihat "x*8"                        | vel 0.25 | hpf 4200 | pan sine 4 0.22
  root  bass  "c2@2 g1@2"                  | vel 0.48 | lpf 760 | gain 0.62
  bed   pad   "[c3,g3,d4]@2 [ab2,eb3,c4]@2" | vel 0.24 | lpf 1800 | reverb 0.32
  motif pluck "0 2 3 5 4 3 1 0"            | oct 1 | vel 0.38 | delay 0.25 0.20 0.12
}

arrange movement movement movement movement movement movement
"#;

fn piece() -> piece::Piece {
    let (program, parse_errors) = parse_program(LARGE_ARRANGEMENT);
    assert!(parse_errors.is_empty(), "{parse_errors:?}");
    let (piece, resolve_errors) = piece::resolve(&program, (120, (4, 4), None));
    assert!(resolve_errors.is_empty(), "{resolve_errors:?}");
    piece
}

fn offline_large_arrangement(c: &mut Criterion) {
    let piece = piece();
    let registry = InstrumentRegistry::built_in();
    c.bench_function("offline_large_arrangement", |b| {
        b.iter(|| render(&piece, &registry, 8_000).expect("benchmark piece renders"))
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = offline_large_arrangement
}
criterion_main!(benches);
