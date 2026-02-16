# Rustic Live — Language Specification v0.1

## Overview

Rustic Live is a live-coding music DSL designed for real-time composition in a
terminal environment. It prioritises brevity and immediacy: every line either
configures the session or defines a looping pattern. Changes take effect at the
next loop boundary (quantised to the time signature).

The language draws inspiration from TidalCycles' mini-notation while remaining
self-contained (no host language required).

---

## 1. Source Structure

A Rustic Live source file (`.rt`) is a sequence of **lines**. Each line is one
of:

| Line kind        | Syntax                                    |
|------------------|-------------------------------------------|
| Comment          | `-- <text>`                               |
| Directive        | `<keyword> <value>`                       |
| Pattern          | `<name> <instrument> "<mini-notation>" [| <transform> ...]` |
| Muted pattern    | `; <name> <instrument> "<mini-notation>"` |
| Blank line       | *(ignored)*                               |

Lines are separated by newlines. There is no block structure, no braces, and no
indentation significance.

### 1.1 Comments

```
-- This is a comment. Everything after -- until end-of-line is ignored.
```

### 1.2 Blank Lines

Blank lines (empty or whitespace-only) are ignored. They can be used freely for
visual grouping.

---

## 2. Directives

Directives configure session-wide settings. They take effect immediately upon
evaluation (not quantised). A directive is a **keyword** followed by one or more
**arguments**, separated by whitespace.

### 2.1 `bpm <integer>`

Set the tempo in beats per minute.

```
bpm 120
bpm 140
```

Constraints: integer in range [20, 999].

### 2.2 `sig <numerator>/<denominator>`

Set the time signature. This determines the loop length.

```
sig 4/4
sig 3/4
sig 7/8
```

Numerator and denominator are positive integers. Denominator must be a power of
2 (1, 2, 4, 8, 16, 32).

### 2.3 `scale <root> <mode>`

Set the default scale for scale-degree patterns.

```
scale C minor
scale Eb dorian
scale F# phrygian
```

Root is a pitch name (see §3.1) without octave. Mode is one of:

    major minor dorian phrygian lydian mixolydian aeolian locrian
    chromatic pentatonic blues

### 2.4 `load "<filepath>"`

Load instrument definitions from an external file.

```
load "pads.rt"
load "instruments/wobble.rt"
```

The path is relative to the current file. Only `def` blocks (see §6) are
extracted from loaded files.

---

## 3. Mini-Notation

The mini-notation is a terse pattern language written inside **double quotes**
(`"`). It describes a sequence of musical events that loops over one cycle
(the length of one measure as defined by `sig`).

### 3.1 Pitch Notation

A pitch is a note name, optional accidental, and octave number:

```
c4      -- middle C
eb3     -- E-flat in octave 3
f#5     -- F-sharp in octave 5
a2      -- A in octave 2
```

| Component   | Syntax            | Notes                          |
|-------------|-------------------|--------------------------------|
| Note name   | `a` through `g`   | Always lowercase               |
| Accidental  | `b` = flat, `#` = sharp | Optional. `b` for flat not `f` |
| Octave      | `0` through `9`   | Required for pitched notes     |

**Important:** Flats use `b` (the letter b), not `f`. This avoids ambiguity
with the note name F. Examples: `bb3` = B-flat 3, `eb4` = E-flat 4.

Double accidentals: `##` for double-sharp, `bb` for double-flat. When
ambiguous with the note B, the parser consumes the longest valid pitch: `bb3`
is B-flat 3, never a double-flat on an implicit note.

### 3.2 Scale Degrees

Integer values (`0`, `1`, `2`, ...) represent scale degrees relative to the
active scale (set by the `scale` directive or the `| scale` transform).

```
"0 2 4 6 4 2"       -- play the 1st, 3rd, 5th, 7th, 5th, 3rd scale degrees
```

Scale degrees are 0-indexed. Negative degrees descend below the root octave.

### 3.3 Drum Trigger

The character `x` is a **trigger event** for percussion instruments. It tells
the instrument to fire once. Which sound is produced depends on the instrument.

```
"x ~ x ~"           -- trigger, rest, trigger, rest
"x x x x"           -- four triggers per cycle
```

### 3.4 Rest

The tilde `~` represents silence for the duration of its slot.

```
"c4 ~ e4 ~"         -- note, rest, note, rest
```

### 3.5 Hold / Tie

The underscore `_` extends the preceding event's duration into this slot.

```
"c4 _ _ e4"         -- c4 held for 3/4 of the cycle, e4 for 1/4
"x _ ~ x"           -- trigger held 2 slots, rest, trigger
```

### 3.6 Sequential Steps

Events separated by **whitespace** divide the cycle into equal-length steps.

```
"c4 e4 g4"          -- 3 equal steps (triplet feel in 4/4)
"c4 e4 g4 c5"       -- 4 equal steps (straight quarter notes in 4/4)
```

### 3.7 Grouping / Subdivision: `[ ]`

Square brackets group events so they share the time of **one step** in the
parent sequence.

```
"[c4 e4] g4"        -- c4+e4 share the first half, g4 gets the second half
"c4 [e4 g4 b4]"     -- c4 = 1/2 cycle, e4+g4+b4 each = 1/6 cycle
```

Brackets nest arbitrarily:

```
"[c4 [e4 g4]] b4"   -- c4 = 1/4, e4 = 1/8, g4 = 1/8, b4 = 1/2
```

### 3.8 Chords (Simultaneous Events): `,` inside `[ ]`

A comma inside brackets means the events play **simultaneously** (a chord).

```
"[c3,e3,g3]"        -- C major triad, full cycle
"[c3,e3,g3] [f3,a3,c4]"  -- two chords, half cycle each
```

### 3.9 Repeat: `*N`

The `*` suffix repeats an event or group N times within its time slot.

```
"c4*2 e4"           -- c4 plays twice (two eighth notes), then e4 (quarter)
"[c4 e4]*3"         -- the pair c4-e4 repeats 3 times
"x*4"               -- four triggers evenly across the cycle
```

N must be a positive integer.

### 3.10 Slow: `/N`

The `/` suffix stretches a group over N cycles.

```
"[c4 e4 g4 b4]/2"   -- the 4-note sequence plays over 2 cycles
"[c4 d4 e4 f4 g4 a4 b4 c5]/4"  -- 8 notes over 4 cycles
```

N must be a positive integer.

### 3.11 Alternation: `< >`

Angle brackets cycle through alternatives, one per loop iteration.

```
"<c4 e4 g4>"        -- cycle 1: c4, cycle 2: e4, cycle 3: g4, then repeats
"c4 <e4 g4> c5"     -- middle note alternates between e4 and g4
```

### 3.12 Random Choice: `|` inside `[ ]`

A pipe inside brackets picks one option at random each cycle.

Note: This is the `|` character *inside* the mini-notation string, not the
transform pipe which appears *outside* the string.

```
"[c4|e4|g4]"        -- randomly plays c4, e4, or g4 each cycle
```

### 3.13 Replicate: `!N`

The `!` suffix creates N copies as separate sequential steps (unlike `*` which
subdivides within the existing time slot).

```
"c4!3 e4"           -- becomes "c4 c4 c4 e4" (4 equal steps)
```

### 3.14 Euclidean Rhythm: `(beats,steps)` or `(beats,steps,offset)`

Distributes beats evenly across steps using the Euclidean algorithm.

```
"c4(3,8)"           -- 3 hits in 8 steps: c4 ~ ~ c4 ~ ~ c4 ~
"x(5,8)"            -- 5 triggers in 8 steps
"c4(3,8,1)"         -- same as (3,8) but rotated left by 1 step
```

### 3.15 Random Drop: `?`

The `?` suffix gives the event a 50% chance of being replaced by silence.

```
"x*8?"              -- 8 fast triggers, each with 50% chance of playing
"c4? e4 g4?"        -- first and last notes are randomly dropped
```

### 3.16 Weight / Proportional Duration: `@N`

The `@` suffix makes a step N times longer than normal (default weight = 1).

```
"c4@3 e4"           -- c4 takes 3/4 of the cycle, e4 takes 1/4
"c4@2 e4 g4"        -- c4 takes 2/4, e4 and g4 each take 1/4
```

N must be a positive integer.

---

## 4. Pattern Lines

A pattern line defines a named, looping musical phrase. Syntax:

```
<name> <instrument> "<mini-notation>" [| <transform> ...]
```

### 4.1 Pattern Name

An identifier: ASCII letters, digits, and underscores. Must start with a
letter. Names are unique within a session — re-defining a name replaces the
previous pattern.

```
kick drums "x ~ x ~"
bass01 sine "c2 ~ eb2 ~"
my_arp piano "[c3,e3,g3]"
```

### 4.2 Instrument Name

An identifier referring to a built-in or user-defined instrument (see §5, §6).

### 4.3 Transform Pipeline

After the closing quote, zero or more **transforms** can be chained with `|`:

```
lead saw "c4 eb4 g4 bb4" | rev | slow 2
```

Transforms are applied left-to-right.

Available transforms:

| Transform              | Description                                    |
|------------------------|------------------------------------------------|
| `rev`                  | Reverse the pattern                            |
| `fast <N>`             | Speed up by factor N (float)                   |
| `slow <N>`             | Slow down by factor N (float)                  |
| `every <N> <transform>`| Apply transform every Nth cycle                |
| `arp <mode>`           | Arpeggiate chords (up, down, updown, random)   |
| `scale <root> <mode>`  | Quantise to scale (overrides global)           |
| `oct <offset>`         | Shift octave by offset (signed integer)        |
| `gain <amount>`        | Set volume (float, 0.0–1.0)                    |
| `lpf <cutoff>`         | Low-pass filter, cutoff in Hz                  |
| `hpf <cutoff>`         | High-pass filter, cutoff in Hz                 |
| `delay <time> <fb>`    | Delay effect (time in seconds, feedback 0–1)   |
| `reverb <amount>`      | Reverb mix (float, 0.0–1.0)                    |

### 4.4 Muting

A semicolon `;` at the start of a line mutes the pattern. The pattern is
parsed and retained but does not produce audio. This allows quick toggling.

```
; bass sine "c2 ~ eb2 ~"     -- muted
bass sine "c2 ~ eb2 ~"       -- active
```

---

## 5. Built-in Instruments

The following instruments are available without any `def` or `load`:

### 5.1 Percussion (use with `x` trigger)

| Name     | Description              |
|----------|--------------------------|
| `kick`   | Bass drum                |
| `snare`  | Snare drum               |
| `hihat`  | Closed hi-hat            |
| `clap`   | Handclap                 |
| `rim`    | Rimshot                  |
| `tom`    | Tom-tom                  |

### 5.2 Pitched (use with note names)

| Name       | Description                          |
|------------|--------------------------------------|
| `sine`     | Pure sine wave oscillator            |
| `saw`      | Sawtooth wave oscillator             |
| `square`   | Square wave oscillator               |
| `triangle` | Triangle wave oscillator             |
| `piano`    | Piano-like (saw + ADSR + LP filter)  |
| `bass`     | Bass synth (square + sub-oscillator) |
| `pad`      | Pad sound (detuned saws + slow ADSR) |
| `pluck`    | Plucked string (short envelope)      |
| `bell`     | Bell tone (additive harmonics)       |

---

## 6. Instrument Definitions

Custom instruments can be defined with `def` blocks. This is planned for a
future version and is **not in scope for v0.1**.

Planned syntax:

```
def wobble {
    gen  saw + sine * 0.3
    env  adsr 0.01 0.1 0.7 0.3
    fx   lowpass 800 0.7
    fx   delay 0.25 0.4
}
```

For v0.1, only built-in instruments (§5) are available.

---

## 7. Evaluation Semantics

### 7.1 Quantised Application

When the user saves (`:w` or `Ctrl+S`), the source is parsed and diffed
against the current live state. Changes are **queued** and applied at the next
**loop boundary** — i.e., the start of the next full measure as defined by
`sig`.

| Change kind                  | Behaviour                           |
|------------------------------|-------------------------------------|
| New pattern                  | Starts at next loop boundary        |
| Modified pattern             | Swaps in at next loop boundary      |
| Removed pattern              | Stops at next loop boundary         |
| Muted pattern (`;` prefix)   | Silences at next loop boundary     |
| Unmuted pattern              | Resumes at next loop boundary       |
| Directive change             | Applies immediately (except `sig`)  |
| `sig` change                 | Applies at next loop boundary       |

### 7.2 Diffing Rules

The session tracks patterns by **name**. After parsing:

- If a name exists in the new source but not the old: **added**.
- If a name exists in the old source but not the new: **removed**.
- If a name exists in both but the pattern content changed: **modified**.
- If a name exists in both with identical content: **unchanged** (no action).

### 7.3 Error Handling

Parsing is **line-independent**: an error on one line does not prevent other
lines from being parsed and evaluated. The session keeps the last-good version
of any pattern that fails to parse.

Errors are reported per-line in the eval output panel:

```
[#0003] [ERR] Line 7: expected closing '"' in pattern
[#0003] [ERR] Line 12: unknown instrument 'wobbl'
[#0003] [ OK] 4/6 patterns updated successfully.
```

### 7.4 State Model

The session maintains:

| State field          | Type                    | Description              |
|----------------------|-------------------------|--------------------------|
| `bpm`                | `u32`                   | Current tempo            |
| `sig`                | `(u8, u8)`              | Time signature           |
| `scale`              | `Option<(Root, Mode)>`  | Default scale            |
| `patterns`           | `Map<Name, Pattern>`    | Active patterns          |
| `pending`            | `Vec<Delta>`            | Queued changes           |
| `beat_position`      | `f64`                   | Current beat in cycle    |

---

## 8. Grammar (Formal)

```ebnf
program       = { line } ;
line          = comment | directive | pattern_line | muted_line | blank ;
blank         = { whitespace } ;
comment       = "--" { any_char } ;

directive     = bpm_dir | sig_dir | scale_dir | load_dir ;
bpm_dir       = "bpm" integer ;
sig_dir       = "sig" integer "/" integer ;
scale_dir     = "scale" pitch_root scale_mode ;
load_dir      = "load" string_literal ;

pattern_line  = name instrument string_literal { "|" transform } ;
muted_line    = ";" name instrument string_literal { "|" transform } ;

name          = identifier ;
instrument    = identifier ;
identifier    = letter { letter | digit | "_" } ;

transform     = "rev"
              | "fast" number
              | "slow" number
              | "every" integer transform
              | "arp" arp_mode
              | "scale" pitch_root scale_mode
              | "oct" signed_integer
              | "gain" number
              | "lpf" number
              | "hpf" number
              | "delay" number number
              | "reverb" number ;

arp_mode      = "up" | "down" | "updown" | "random" ;

(* Mini-notation grammar — contents of string_literal *)
mini          = sequence ;
sequence      = step { whitespace step } ;
step          = atom [ modifier ] ;
atom          = note | degree | trigger | rest | hold
              | group | alternation ;
group         = "[" sequence_or_chord "]" ;
alternation   = "<" sequence ">" ;
sequence_or_chord = sequence { "," sequence } ;
note          = note_name [ accidental ] octave ;
degree        = integer ;
trigger       = "x" ;
rest          = "~" ;
hold          = "_" ;
note_name     = "a" | "b" | "c" | "d" | "e" | "f" | "g" ;
accidental    = "#" | "b" | "##" | "bb" ;
octave        = digit ;
modifier      = repeat | slow_mod | replicate | euclidean | drop | weight ;
repeat        = "*" integer ;
slow_mod      = "/" integer ;
replicate     = "!" integer ;
euclidean     = "(" integer "," integer [ "," integer ] ")" ;
drop          = "?" ;
weight        = "@" integer ;

pitch_root    = upper_note_name [ accidental ] ;
upper_note_name = "A" | "B" | "C" | "D" | "E" | "F" | "G" ;
scale_mode    = "major" | "minor" | "dorian" | "phrygian" | "lydian"
              | "mixolydian" | "aeolian" | "locrian"
              | "chromatic" | "pentatonic" | "blues" ;

number        = integer | float ;
integer       = digit { digit } ;
signed_integer = [ "-" ] integer ;
float         = digit { digit } "." digit { digit } ;
string_literal = '"' { mini_char } '"' ;
```

---

## 9. Example Session

```
-- Minimal techno loop
bpm 128
sig 4/4

kick  kick   "x ~ x ~"
snare snare  "~ x ~ x"
hats  hihat  "x*8"

bass  saw    "c2 _ eb2 _ g1 _ f2 _"
lead  piano  "c4 eb4 g4 bb4" | slow 2

; pad  pad   "[c3,eb3,g3] ~ [f3,ab3,c4] ~"
```

This defines:
- Three drum loops: 4-on-the-floor kick, backbeat snare, 8th-note hats
- A bass line stepping through C, Eb, G, F (each held 2 slots)
- A piano lead playing a Cm7 arpeggio over 2 cycles
- A muted pad pattern (ready to unmute by removing `;`)

---

## 10. Future Extensions (Not in v0.1)

- `def` blocks for custom instrument synthesis (§6)
- `fn` blocks for reusable pattern fragments
- Per-pattern `bpm` / `sig` overrides (polymetric)
- MIDI input/output
- OSC integration
- `import` for sharing patterns between files
- Conditional patterns (`if cycle > 16 then ...`)
- Probability weights on random choice (`[c4@3|e4@1]`)
