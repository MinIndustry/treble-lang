# treble-lang

The Treble language: parser, session engine, and — behind the `render` feature —
the lowering that turns a buffer into something the engine can play.

`LANGUAGE.md` is the specification and the authority. This file is just how to
run the thing.

## The library

```toml
treble-lang = "0.8.0"
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
- `render::offline` — drives that output through the engine to samples,
  through a master limiter unless `RenderOptions` turns it off.
- `render::wav` — writes the result, metadata and all.

## The command line

```
cargo install --path . --features render

treble check  piece.rt      # parse, report errors, exit 1 on any
treble info   piece.rt      # sections, arrangement, spans, metadata
treble info   piece.rt --json            # the same, for a script
treble render piece.rt      # → piece.wav
```

`render` takes `-o/--out`, `-r/--rate`, `-f/--force`, `-q/--quiet`, `--json`,
and `-m/--meta key=value` (repeatable, overrides the file's own tags — handy for
stamping a build date or take number without editing the piece).

`load "<file.trbl>"` lines resolve **beside the piece file** (an absolute path
is used as written; nothing else is searched), so an album of pieces can share
one instrument library. `check` validates the loads too — a broken path or a
loaded file that sounds is reported before any render.

Progress is reported on stderr and the summary on stdout, so `--json` composes:

```
treble render piece.rt --json | jq .renderedSeconds
treble info   piece.rt --json | jq '.sections[] | {name, seconds, firstCycle}'
```

`info --json` gives the metadata, `tail`, `seed`, the arrangement and timeline,
and for each section its cycles, tempo, signature, length, the cycle it first
plays on, how often it plays, and its lines with their instrument, group, mute
and span. `--bars` (needs `render`) adds, per line, which bars it strikes on.

### Rendering parts of a piece

`render` can cut a piece up without touching its source — for a game that plays
a song back as per-section, per-instrument loops, say:

```
treble render piece.rt --section verse --passes 2 --only bass,sub --no-limit -o verse-bass.wav
```

- `-s/--section <name>` renders one section, **starting on the cycle the
  arrangement first plays it on**. Random choices, drops and solos count from
  a section's place in the piece, so a section rendered as a fresh piece would
  be a different take from the one in the song.
- `-p/--passes <n>` plays the section n times back to back and keeps only the
  last pass and its tail, cut on the exact frame it starts on. The file then
  opens on the reverb and delay the previous pass left ringing, which is what a
  loop sounds like when it follows itself. How much carries in is the tail, so
  `--tail 0` carries nothing.
- `--only <instrument>[,…]` mutes every line whose instrument is not listed;
  what is left sounds exactly as it does in the mix, buses and all. An
  instrument that no line of the rendered section plays is an error, so a typo
  fails rather than writing silence.
- `-t/--tail <seconds>` overrides the piece's `tail`.
- `--no-limit` skips the master limiter. Limited renders of parts do not add
  up to the limited whole; unlimited ones do, so leave the limiting to whatever
  mixes them. The WAV is 32-bit float and peaks above 1.0 are kept, not
  clipped — convert to integer PCM only after mixing. `--json` reports
  `limited` and the `peak` of the samples written.

### Metadata

`meta` tags (LANGUAGE.md §8.9) travel into the WAV's `INFO` chunk, so a
rendered piece carries its own title rather than relying on the filename:

```
meta title    "Nocturne for a Slow Machine"
meta composer "Minigrim0"
meta comment  "One collection, three centres."
```

The key set is open. Well-known keys map to their standard tags; anything else
is filed alongside the comment rather than dropped.

## The language server

```
cargo install --path . --features serve
treble serve
```

`serve` speaks LSP on stdin and stdout for an editor: diagnostics (the same
checks as `check`, plus unknown instruments), completion, hover and
go-to-definition on instrument names, and document symbols. On top of LSP it
answers `treble/outline` and `treble/timeline` — the notes of chosen lines as
the renderer schedules them, each pointing back at the step that wrote it —
and plays through the default sound card with `treble/play` and
`treble/stop`. Playback renders the asked-for lines of one section, placed
where the piece first plays it, so a line heard alone sounds as it does in the
render; an edit to a playing piece is re-rendered and swapped in at the next
cycle. The protocol is written down in Quill's `PROTOCOL.md`, the editor it
was built for.

## An example piece

`pieces/nocturne.rt` is a complete five-and-a-half-minute piece — ten sections,
seven instruments defined in the buffer, a theme in three metres, and a
question-and-answer solo. It is the toolchain's end-to-end case:

```
treble check  pieces/nocturne.rt
treble info   pieces/nocturne.rt
treble render pieces/nocturne.rt
```

Renders are reproducible, so the WAV is not kept in the repository — the source
beside it is the artefact.

The analysis examples read its notes rather than its audio:

```
cargo run --features render --example notes   -- pieces/nocturne.rt
cargo run --features render --example clashes -- pieces/nocturne.rt
```
