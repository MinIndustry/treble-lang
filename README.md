# treble-lang

The Treble language: parser, session engine, and — behind the `render` feature —
the lowering that turns a buffer into something the engine can play.

`LANGUAGE.md` is the specification and the authority. This file is just how to
run the thing.

## The library

```toml
treble-lang = { git = "https://github.com/MinIndustry/treble-lang.git", tag = "0.4.0" }
```

Parsing has no dependencies beyond `nom`. The engine is *not* pulled in: a tool
that only wants to read a buffer should not pay for a DSP stack.

```rust
let mut session = treble_lang::Session::new();
let result = session.evaluate(source);
if let Some(piece) = session.piece() {
    println!("{} cycles", piece.total_cycles());
}
```

## Rendering

The `render` feature adds `treble_lang::render`, which depends on `treble-core`:

```toml
treble-lang = { …, features = ["render"] }
```

- `render::compile` — mini-notation to events, `def` blocks to instrument
  specs, `fx` names resolved against the engine's registry. Shared with the
  live front end, so a rendered piece and a played performance compile through
  the same code.
- `render::offline` — drives that output through the engine to samples.
- `render::wav` — writes the result, metadata and all.

## The command line

```
cargo install --path . --features render

treble check  piece.rt      # parse, report errors, exit 1 on any
treble info   piece.rt      # sections, arrangement, spans, metadata
treble render piece.rt      # → piece.wav
```

`render` takes `-o/--out`, `-r/--rate`, `-f/--force`, `-q/--quiet`, `--json`,
and `-m/--meta key=value` (repeatable, overrides the file's own tags — handy for
stamping a build date or take number without editing the piece).

Progress is reported on stderr and the summary on stdout, so `--json` composes:

```
treble render piece.rt --json | jq .renderedSeconds
```

### Metadata

`meta` tags (LANGUAGE.md §8.9) travel into the WAV's `INFO` chunk, so a
rendered piece carries its own title rather than relying on the filename:

```
meta title    "Nocturne for a Slow Machine"
meta composer "F. Grimau"
meta comment  "One collection, three centres."
```

The key set is open. Well-known keys map to their standard tags; anything else
is filed alongside the comment rather than dropped.
