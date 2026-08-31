# Treble — Language Specification v0.2

## Overview

Treble is a music DSL with two modes in one language.

**Live**: every line either configures the session or defines a pattern that
loops forever, and a change takes effect at the next loop boundary (quantised
to the time signature). It prioritises brevity and immediacy — the whole point
is that a line can be retyped while it sounds.

**Piece** (§8): the same lines, gathered into named `section` blocks of a
stated length and put in an order by `arrange`. A piece therefore has an end,
which makes it renderable to a file rather than only performable. A live buffer
is the degenerate case — a piece with no structure — and there is one parser,
one mini-notation and one set of transforms behind both.

The language draws inspiration from TidalCycles' mini-notation while remaining
self-contained (no host language required).

---

## 1. Source Structure

A Treble source file (`.rt`) is a sequence of **lines**. Each line is one of:

| Line kind        | Syntax                                    |
|------------------|-------------------------------------------|
| Comment          | `-- <text>`                               |
| Directive        | `<keyword> <value>`                       |
| Pattern          | `<name> <instrument> "<mini-notation>" [@ <span>] [| <transform> ...]` |
| Muted pattern    | `; <name> <instrument> "<mini-notation>"` |
| Block header     | `def <name> {` · `[;] group <name> {` · `[;] section <name> <cycles> {` |
| Block footer     | `}` , optionally `} | <transform> ...` on a group |
| Blank line       | *(ignored)*                               |

Lines are separated by newlines. Indentation is never significant: the three
block forms are delimited by braces on their own header and footer lines, and
their members stay ordinary lines so that line-based editing — muting a line,
nudging it, reporting an error against it — keeps working inside a block. A
`group` nests one level inside a `section`; nothing else nests.

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

Numerator and denominator are positive integers. Any positive denominator is
accepted, including non-powers of two such as `sig 4/3` and `sig 5/6`.

### 2.3 `phrase <cycles>`

How many cycles a musical phrase spans. Consumers quantise changes to it, so a
drop lands on the top of the phrase rather than merely on the next cycle.

```
phrase 8
phrase 16
```

Defaults to `1`, which is "the next cycle" and matches the behaviour before
phrases existed.

### 2.4 `scale <root> <mode>`

Set the default scale for scale-degree patterns.

```
scale C minor
scale Eb dorian
scale F# phrygian
```

Root is a pitch name (see §3.1) without octave. Mode is one of:

    major minor dorian phrygian lydian mixolydian aeolian locrian
    chromatic pentatonic blues

### 2.5 `load "<filepath>"`

Load instrument definitions from an external file, so a performance can carry
its own instrument dependencies and a buffer stays portable between machines.

```
load "pads.trbl"
load "instruments/wobble.trbl"
```

**Resolution order.** The path is tried in exactly two places, in order:

1. relative to the directory of the buffer being evaluated;
2. the user's instrument library directory.

The first hit wins. An absolute path is used as written. Nothing else is
searched — no ambient path variable, no working directory — so a buffer that
loads on one machine loads on the next as long as the file travels with it or
lives in the library.

**What a loaded file may contain.** A loaded file (`.trbl` by convention) holds
`def` blocks (§6) and nothing that sounds: pattern lines, directives and
`group` blocks in it are an error rather than silently ignored, because a file
of instruments that quietly started a pattern would be impossible to reason
about mid-performance.

**Precedence.** A name defined in the buffer wins over the same name loaded
from a file, so a local `def` is always the definition you are editing. Between
two loaded files, the later `load` line wins.

**Loads do not recurse.** A `load` line inside a loaded file is an error. That
keeps evaluation bounded and makes import cycles impossible to write, at the
cost of having to list the files a performance needs in the performance itself
— which is also what makes the dependency list visible.

**A missing or invalid file is an evaluation error**, reported per-line like any
other, and the previous state keeps sounding (§9.3). A `load` never silences a
performance.

Resolving the path is the consumer's job — this crate does no I/O and records
the path exactly as written. The rules above are the contract it resolves
against.

### 2.6 Piece Directives

Three more directives belong to pieces and are specified with them in §8:
`arrange` (§8.4) orders the sections, `tail` (§8.7) sets how long a render
rings out past the last cycle, `seed` (§8.8) salts the generative constructs so
a passage can be rerolled, and `meta` (§8.9) carries the piece's own title and
credits.

`arrange` and `tail` are errors in a live buffer, and `phrase` (§2.3) is an
error in a piece — a piece is not being edited while it plays, so it has no
boundary to land a change on. `seed` and `meta` are accepted in both.

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

An uppercase `X` is an **accented** trigger — the same event at full velocity
(§3.17).

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

A tie extends whatever sounded last, including a rest, so `"x ~ _ x"` is a hit,
a half-cycle silence, then a hit. A group is self-contained: a leading `_`
inside `[...]` has nothing to hold and remains silent.

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

N must be a positive integer. `*0` is a parse error rather than silence.

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
"[c4|e4|g4]"        -- plays c4, e4, or g4 each cycle
"[x x|x ~ x]"       -- layers may be full sequences with their own timing
```

A group carries commas (a chord, §3.8) **or** pipes (a random choice), never
both: `[c4,e4|g4]` is a parse error. This keeps the two separators from needing
precedence against each other.

The choice is **deterministic**: it is seeded on the pattern name, the step
position, and the cycle number, so it evolves from cycle to cycle but replays
identically from the same buffer. Nothing in the language uses a real RNG.

### 3.13 Replicate: `!N`

The `!` suffix creates N copies as separate sequential steps (unlike `*` which
subdivides within the existing time slot).

```
"c4!3 e4"           -- becomes "c4 c4 c4 e4" (4 equal steps)
```

N must be a positive integer. Compare with `*N`, which keeps the step's slice of
the cycle: `"c4*3 e4"` puts three fast notes in the first half, while
`"c4!3 e4"` is four equal quarters.

### 3.14 Euclidean Rhythm: `(onsets,positions)` or `(onsets,positions,offset)`

Distributes onsets as evenly as possible across positions using the Euclidean
algorithm.

```
"c4(3,8)"           -- 3 hits in 8 steps: c4 ~ ~ c4 ~ ~ c4 ~
"x(5,8)"            -- 5 triggers in 8 steps: x ~ x x ~ x x ~
"c4(3,8,1)"         -- same as (3,8) but rotated left by 1 step
```

The distribution is **Bjorklund's algorithm**, the canonical Euclidean spacing.
`positions` must be at least 1.

The first argument counts **onsets, not sounding positions**, so it may exceed
the second. Every position receives `onsets / positions` onsets, and Bjorklund
spreads the `onsets % positions` remainder over the positions that receive one
more. A position holding several onsets subdivides its own span, so one figure
can mix note values:

```
"x(9,8)"            -- 9 onsets over 8 positions: one splits in two
"x(12,8)"           -- alternating single and doubled positions
"x(16,8)"           -- every position doubled
"x(20,8)"           -- alternating triples and doubles
```

```text
x(3,8)    x . . x . . x .
x(9,8)    xx x  x  x  x  x  x  x
x(12,8)   xx x  xx x  xx x  xx x
x(16,8)   xx xx xx xx xx xx xx xx
```

The total span is unchanged in every case — only the subdivision of each
position varies. `offset` rotates whole positions, so `"x(9,8,1)"` moves the
split one place along. When `onsets <= positions` the quotient is zero and this
reduces to the classic one-or-nothing Euclidean rhythm.

### 3.15 Random Drop: `?` / `?p`

The `?` suffix gives the event a 50% chance of being replaced by silence. An
immediately following number sets the probability instead, in `0.0..=1.0`.

```
"x*8?"              -- 8 fast triggers, each with 50% chance of playing
"c4? e4 g4?"        -- first and last notes are randomly dropped
"x*16?0.25"         -- 16 slots, each dropped a quarter of the time
```

No whitespace may separate `?` from its probability: `"x? 0.5"` is a bare drop
followed by scale degree `0.5`'s own step.

Like `[a|b]`, the decision is deterministic — seeded on the pattern name, step
and cycle — so a performance replays exactly.

### 3.16 Weight / Proportional Duration: `@N`

The `@` suffix makes a step N times longer than normal (default weight = 1).

```
"c4@3 e4"           -- c4 takes 3/4 of the cycle, e4 takes 1/4
"c4@2 e4 g4"        -- c4 takes 2/4, e4 and g4 each take 1/4
```

N must be a positive integer.

### 3.17 Velocity and Accent: `X` and `:v`

How hard a step is struck is written on the step, not only on the line. Two
spellings, because accenting a backbeat and dialling in a ghost note are
different jobs:

```
"X ~ x ~"           -- accented downbeat, then a normal trigger
"x:0.6 x:0.3"       -- an explicit velocity per step
"c4:0.35 e4"        -- works on any sounding atom, not only triggers
"[c4 e4]:0.8"       -- one velocity for everything the group sounds
```

`X` is an accent: **full velocity, 1.0**. It is exactly `x:1.0` and is stored
as such, so a consumer that honours `:v` honours `X` for free. Lowercase `x`
keeps its old meaning — "normal", taking whatever the line's `vel` says. `X` is
unambiguous because note letters are `a`–`g`, so no note spelling collides with
it. Writing both (`X:0.6`) is an error: it sets two velocities on one step.

`:v` is an explicit velocity in `0.0..=1.0`, valid on any **sounding** atom —
a note, a degree, a trigger, a `solo(..)`, a group, or an alternation. A rest
(`~:0.5`) or a hold (`_:0.5`) carrying a velocity is meaningless and is
rejected rather than ignored: a hold sustains the event it extends, so it has
no strike of its own to weight.

On a group or an alternation, the velocity applies to every event the step
sounds; a step **inside** it that names its own velocity wins for that step, so
`"[c4 e4:0.4]:0.9"` is a loud C and a quiet E.

The velocity may travel like any other value (§4.6), which is how a per-step
swell is written:

```
sn snare "x:0.3..0.9"     | ramp 8    -- each hit grows over eight cycles
sn snare "x:0.3>0.6>0.9"  | ramp 12   -- three held stages instead
```

**Interaction with `| vel`.** `:v` and `X` are **absolute**: they set the
step's velocity and override the line's `vel` for that step. `| vel` supplies
the default for every step that specifies neither. So in

```
sn snare "X x x:0.4 x" | vel 0.7
```

the four steps are struck at 1.0, 0.7, 0.4 and 0.7. Nothing multiplies: a
performer nudging `vel` to balance a line must not have to recompute the
accents they already placed. (The one place velocity *does* multiply is a
group's shared `| vel`, §7.2, which is a bus level rather than a step's
strike.)

### 3.18 Stacking Modifiers

More than one modifier may follow an atom. They apply **left to right in the
order written**. The slot-generating modifiers (`*N`, `!N`, `(k,n)`) run first
and build the slots; `?` and `@N` then apply to every slot produced.

```
"x*8?"              -- 8 slots, each with its own drop chance
"x(5,8)?0.25"       -- a Euclidean figure, then thinned
"[c4 e4]*2@3"       -- the pair twice over, in a triple-weight step
"x(3,8)(2,3)"       -- the (3,8) figure fills 2 of 3 positions
```

Two Euclidean modifiers therefore **nest**: the later one takes whatever the
earlier one produced as its payload, rather than re-gridding it.

`/N` is the exception: it stretches the whole pattern line over N cycles rather
than only its own step.

A velocity suffix (§3.17) may be written anywhere in the run of modifiers and
means the same thing wherever it sits, because it is a property of the step
rather than a transformation of its slots: `"X*4"`, `"x:0.6?0.25"` and
`"x(3,8):0.9"` all give **every** slot the step generates that velocity. Only
one velocity per step, so `"x:0.6:0.8"` is an error.

### 3.19 Generated Solo: `solo(low..high, steps)`

A `solo` atom asks Treble to write the melody for you: a weighted random walk
over the scale degrees `low..high` (inclusive, resolved against the active
`scale` like plain degrees), playing `steps` evenly spaced notes per cycle.

```
"solo(0..7, 8)"          -- 8 notes per cycle, one octave of the scale
"solo(-7..7, 4)"         -- a slower walk across two octaves
"~ solo(0..7, 8)"        -- the solo occupies the second half of the cycle
```

The walk follows three musical rules rather than raw dice:

- **Movement over repetition** — steps of ±1 dominate, ±2 leaps happen
  sometimes, staying put is rare.
- **Edge gravity** — motion that would pin the walk against `low` or `high`
  is reflected back into the range.
- **Cadence** — the final note of each cycle pulls toward the middle of the
  range, so phrases resolve instead of trailing off.

Like every random construct in Treble, the walk is seeded from the pattern
name and cycle number: it evolves every cycle but replays identically from
the same buffer.

`steps` may be a range or a step chain, so a solo can densify across a build:

```
lead pluck "solo(0..12, 4..16)" | ramp 16
```

The range needs a `| ramp` span like any other travel. `high` must exceed
`low` (a walk needs somewhere to go), `steps` must be at least 1, and a solo
cannot be a chord layer (`[c4,solo(0..7,4)]` is an error) — though it composes
freely with sequencing, choices, rests, and the usual modifiers (`?`, `@N`).

---

## 4. Pattern Lines

A pattern line defines a named, looping musical phrase. Syntax:

```
<name> <instrument> "<mini-notation>" [@ <span>] [| <transform> ...]
```

The optional `@ <span>` narrows the line to part of the section it sits in and
is only legal there (§8.3). Everything else on the line means the same in a
live buffer and in a piece.

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
| `vel <amount>`         | Note velocity (float, 0.0–1.0)                 |
| `ramp <cycles> [lin\|exp]` | How long the line's ranges take to travel, and how (§4.6) |
| `gain <amount>`        | Output level (float, 0.0–2.0)                  |
| `pan <position>`       | Fixed stereo position (-1.0 left – 1.0 right)  |
| `pan <wave> <rate> [depth]` | Stereo position swept by an LFO (§4.4)    |
| `lpf <cutoff>`         | Low-pass filter, cutoff in Hz                  |
| `hpf <cutoff>`         | High-pass filter, cutoff in Hz                 |
| `delay <time> <fb> [mix]` | Delay (seconds, feedback 0–0.99, wet mix 0–1, default 0.35) |
| `reverb <amount>`      | Reverb mix (float, 0.0–1.0)                    |
| `fx <filter> <arg>...` | Any engine filter, by name (§4.5)              |

Every numeric argument in this table may be a **range** instead of a single
value, travelling across the line's `ramp` span (§4.6) — `| lpf 300..9000 |
ramp 16` opens a filter over sixteen cycles. The exceptions are the arities and
the non-numeric arguments: `every`'s cycle count, `ramp`'s own span, `arp`'s
mode, `scale`'s root and mode, and a swept pan's waveform, rate and depth.

Transforms split into two families:

- **Event transforms** — `rev`, `fast`, `slow`, `arp`, `scale`, `oct`, `vel` —
  reshape the scheduled notes. `every N <event transform>` works.
- **Audio transforms** — `gain`, `pan`, `lpf`, `hpf`, `delay`, `reverb` —
  describe an ordered DSP chain, so their position matters: `| gain 0.5 | lpf 900`
  attenuates before filtering. Because the chain is compiled, `every N <audio
  transform>` cannot take effect and consumers should diagnose it.

`vel` and `gain` are distinct. `vel` is how hard the note is struck and reaches
the instrument's envelopes and velocity sensitivity; `gain` is the level of the
signal leaving them.

`vel` is the line's **default** strike: a step that names its own velocity with
`X` or `:v` (§3.17) overrides it for that step.

A transform's arity is fixed. Trailing arguments are an error, so a typo such as
`| pan -1.0 0.5` is reported rather than half-read.

### 4.4 Swept Pan

`pan` reads a **number** as a fixed position and a **waveform name** as a sweep:

```
lead saw "0 3 5" | pan -0.4          -- fixed, hard-ish left
lead saw "0 3 5" | pan sine 4        -- one sweep every 4 cycles
lead saw "0 3 5" | pan sq 1 0.35     -- shallow ping-pong, once per cycle
lead saw "0 3 5" | pan tri 0.5hz     -- one sweep every two seconds
```

    wave  = "sine" | "tri" | "sq" | "saw" | "rand"

`sine` and `tri` start centred and move right, `sq` alternates hard between the
sides, `saw` ramps left to right and jumps back, and `rand` holds one position
for each period.

**The rate is a period, not a frequency.** A bare number counts **cycles per
sweep**, so a larger number is slower and the sweep tracks the tempo: consumers
resolve it against the current `bpm` and `sig` when the pattern is compiled, and
a tempo edit re-tunes it. An `hz` suffix means absolute frequency instead and
ignores the tempo. Long spellings (`sine`, `triangle`, `square`, `random`) and a
mixed-case suffix (`0.5Hz`) are accepted.

The optional third argument is **depth** in `0.0..=1.0`, defaulting to a full
sweep. Depth `0.0` is a centred position that never moves.

Panning is equal-power (cos/sin), so perceived loudness stays constant across
the sweep rather than dipping at the centre.

A swept pan is an audio transform (§4.3), so it cannot take effect inside
`every`, and `rand` is stepped deterministically rather than from an RNG — the
same graph replays identically.

### 4.5 Engine Filters: `fx`

The named transforms above cover a handful of the engine's filters. `fx` reaches
any of them:

```
pad pad "0 _" | fx Tremolo frequency=5 depth=0.6
pad pad "0 _" | fx Limiter threshold=0.8 release=0.3
pad pad "0 _" | fx Compressor 0.3 8
```

    fx_call = ( "fx" filter | alias ) { fx_arg } ;
    fx_arg  = value | name "=" value ;
    value   = range [ "hz" ] ;              (* range: §4.6 *)
    alias   = "trem" | "bpf" | "rbpf" | "avg" | "clip" | "comp" | "limit" ;

Positional arguments fill the filter's declared parameters in order; a named
argument sets one and leaves the rest at their defaults. A positional argument
after a named one is an error, since it would have no defined slot.

**This crate does not resolve the filter.** It has no dependency on the engine
and no knowledge of which filters exist, so it records the name as written and
the consumer looks it up in its own registry — reporting an unknown filter, an
unknown parameter, or a value outside the range that filter declares. That keeps
the limits in one place and means a filter added to the engine needs no change
here.

The aliases exist only as accepted spellings; what each maps to is likewise the
consumer's business.

An LFO rate takes a period in cycles by default and an absolute frequency with
an `hz` suffix, exactly as in §4.4. Which parameters are rates is decided by the
consumer; every other argument is a literal value in the filter's own units.

### 4.6 Ranges and `ramp`

A value may **travel** across the line's `ramp` span and then hold there. There
are two spellings, because a build is not always a smooth one:

```
sn snare "x(4..16,4)"    | vel 0.4..1.0      | ramp 8   -- sweeps
sn snare "x(2>4>8>16,4)" | vel 0.3>0.6>1.0   | ramp 16  -- steps
```

    range = signed_number
          | signed_number ".." signed_number         (* sweeps continuously *)
          | signed_number ( ">" signed_number )+ ;   (* holds each stage *)

`a..b` moves continuously and passes through every value between the ends.
`a>b>c` holds each stage for an **equal share** of the span and passes through
nothing else — `x(2>4>8>16,4) | ramp 16` is four stages of four cycles. This is
what a doubling build needs, since a sweep from 2 to 16 would run through 3, 6,
9 and the rest on its way.

Mixing the two spellings in one value is an error, as is giving `..` more than
two ends.

One `ramp` covers every range on the line, because a build usually moves several
things over the same span. Ranges are accepted on:

- the event transforms `vel`, `oct`, `fast` and `slow`;
- the audio transforms `gain`, `pan` (the fixed position), `lpf`, `hpf`,
  `reverb`, and all three numbers of `delay` — so `| lpf 300..9000 | ramp 16`
  is a filter opening over sixteen cycles;
- the numeric arguments of `fx` and its aliases, positional or named, with or
  without an `hz` suffix: `| trem 2..8hz 0.7`;
- the mini-notation modifiers `*N`, `(onsets,positions)` and `?p`, the `:v`
  velocity suffix (§3.17), and `solo`'s step count.

A range without a `ramp`, or a `ramp` with nothing to move, is an authoring
mistake and should be reported rather than guessed at.

Each end and each stage of a range must be a legal value for the thing it is
written on — the whole travel has to be playable, not only where it starts, so
`| reverb 0.5..1.4` is rejected on its far end.

**Holding, not looping.** A ramp arrives and stays until the line changes; that
is what makes it a crescendo rather than a repeating sweep. Consumers should
measure travel from the cycle the line was last added or modified, so editing one
line does not restart another line's build.

An audio transform's range still means rebuilding what the graph needs to sweep
that parameter; the language only states the intent.

#### 4.6.1 The curve: `ramp <cycles> [lin|exp]`

    ramp_span  = "ramp" integer [ ramp_curve ] ;
    ramp_curve = "lin" | "exp" ;

```
lead saw "0 3 5" | lpf 300..9000 | ramp 16        -- linear, the default
lead saw "0 3 5" | lpf 300..9000 | ramp 16 exp    -- even in perceived pitch
```

`lin` moves in equal value steps and is the default when the curve is omitted,
so every buffer written before curves existed keeps its meaning exactly.

`exp` moves in equal **ratio** steps — geometric interpolation. It exists
because linear travel is audibly wrong for anything perceived logarithmically:
a linear sweep from 300 Hz to 9 kHz is past 4.5 kHz at the halfway point and
spends most of the span sounding "already open", while a geometric one crosses
1.6 kHz there and sounds like a steady opening. The same goes for delay times
and, arguably, for gain.

One `ramp` governs every range on the line — there are no per-value curves.
A build moves several parameters together and wants them to arrive together;
per-value curves would also make the line unreadable at performance speed.

`exp` applies to the sweep spelling (`a..b`); a step chain (`a>b>c`) already
names each value it holds, so the curve does not change what it plays.

**Interpreting `exp` is the consumer's job.** This crate records the intent and
nothing more. Two things the consumer decides, because they depend on the
parameter rather than on the notation: what to do with a range that touches or
crosses zero (geometric interpolation is undefined there — falling back to
linear travel for that value is the sane reading), and whether a value it treats
as an integer count is rounded on the way.

### 4.7 Muting

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

A `def` block defines an instrument in the performance buffer. It is the only
multi-line construct in the language: everything between the opening `{` and its
matching `}` belongs to the block.

```
def wobble {
    voice     poly 4
    lifecycle gated
    tone      saw gain 0.7 identity
    tone      sine gain 0.8 identity
    mix       average
    env       adsr 0.01 0.05 0.7 0.1
    lpf       900
    trem      10hz 0.7
}

bass wobble "0 _ 3 _" | pan -0.4
```

A defined name is available to pattern lines without an `include`; the
definition *is* the import. Redefining a name replaces it.

Lines inside a block are order-independent except for `tone`, whose order is the
tone order, and the envelope stage lines of §6.5. Blank lines and `--` comments
are allowed. Each field may appear once, apart from `tone` and `fx`.

### 6.1 `voice`

```
voice mono                      -- tracks pitch, replaces on retrigger
voice mono notrack drop
voice poly 8
voice poly 4 replaceoldest
```

    voice_line = "voice" ( "mono" [ "notrack" ] [ mono_alloc ]
                         | "poly" integer [ poly_alloc ] ) ;
    mono_alloc = "replace" | "drop" ;
    poly_alloc = "replaceoldest" | "replaceyoungest" | "replaceloudest"
               | "replacequietest" | "replacerandom" | "drop" ;

`notrack` turns off pitch tracking for a mono voice. Defaults: `poly 8` with
`replaceoldest`.

### 6.2 `lifecycle`

```
lifecycle oneshot    -- ignore note-off, run the envelope out (drums)
lifecycle gated      -- sustain while held, then release (keys, pads)
lifecycle cutoff     -- silence the voice immediately on note-off
```

Defaults to `gated`.

### 6.3 `tone`

One line per oscillator, in mix order.

```
tone saw
tone sine gain 0.3 ratio 2.01
tone sine freq 6713 gain 0.14
tone noise gain 0.82
tone sine harmonic 3 gain 0.2 env adsr 0.001 0.2 0.0 0.05
```

    tone_line   = "tone" waveform { tone_option } [ tone_block ] ;
    waveform    = "sine" | "square" | "saw" | "triangle"
                | "squareraw" | "sawraw" | "triangleraw"
                | "noise" | "pinknoise" | "blank" ;
    tone_option = "gain" number
                | "freq" number
                | relation
                | "env" inline_envelope ;
    relation    = "identity" | "harmonic" integer | "ratio" number
                | "offset" number | "semitones" signed_integer
                | "const" number ;

`gain` is the tone's mix level. It is shorthand for a constant amplitude
envelope, which is how the built-in instruments express partial levels.

`freq` pins the tone to a fixed frequency in hertz, ignoring the played note —
this is how inharmonic percussion partials are built. A `relation` instead ties
it to the note: `harmonic 3` is three times the note's frequency, `ratio 2.01`
a slightly detuned octave, `semitones -12` an octave down. Giving both `freq`
and a relation is an error, since only one can win.

A tone with **neither** follows the played note: consumers should treat it as
`identity` rather than leaving the frequency unset, which would leave the tone
droning at whatever its generator defaulted to.

A tone needing a multi-stage envelope of its own takes a block:

```
tone sine freq 6713 {
    env attack  linear 0 1 0.0008
    env decay   bezier 1 0 0.09 0.16 0.015
    env release constant 0
}
```

### 6.4 `mix`

    mix_line = "mix" ( "sum" | "multiply" | "max" | "average" ) ;

How the tones combine. Defaults to `sum`.

### 6.5 `env` and `pitchenv`

`env` is the amplitude envelope, `pitchenv` an optional pitch envelope. Both
take the same three forms:

```
env adsr 0.01 0.1 0.7 0.3        -- terse linear ADSR peaking at 1.0
env segment constant 0.8         -- one segment used as the whole envelope
```

or explicit stages, one line each:

```
env attack  bezier 0 1 0.0008 0 1
env decay   bezier 1 0 0.09 0.16 0.015
env sustain constant 0
env release constant 0
```

    env_line   = ( "env" | "pitchenv" )
                 ( "adsr" number number number number
                 | "segment" segment
                 | stage segment ) ;
    stage      = "attack" | "decay" | "sustain" | "release" ;
    segment    = "linear" from to duration
               | "bezier" from to duration control_x control_y
               | "constant" value [ duration ] ;

Stage lines accumulate into one envelope; `attack`, `decay` and `release` are
required if any stage is given, while `sustain` may be omitted to take the
builder default. Mixing `adsr` or `segment` with stage lines on the same
envelope is an error.

### 6.6 `sample`

```
sample "field.wav"
sample "kick.wav" root 36 start 0.01 end 0.4 loop
```

    sample_line = "sample" string { "root" integer | "start" number
                                  | "end" number | "loop" } ;

The path is relative to the file. `root` is the MIDI note the recording sounds
at, defaulting to 60.

### 6.7 `fx`

    fx_line = "fx" identifier { fx_arg } | fx_alias { fx_arg } ;

Exactly the filter grammar of §4.5, including the aliases, so a definition and a
pattern transform describe an effect the same way. Repeat the line for a chain;
the order is the chain order.

### 6.8 `gain`, `velsens` and `base`

```
gain    0.8     -- final instrument gain
velsens 1.0     -- how strongly pattern velocity affects loudness
base    440     -- base frequency for tones with no relation
```

### 6.9 Evaluation

A `def` is diffed by name like a pattern. Adding, changing or removing one
produces a quantised delta and takes effect at the next loop boundary, so a
definition being edited never interrupts the cycle that is playing. An invalid
definition is reported per-line and the previous version keeps sounding.

Blocks and pattern lines may be interleaved freely; a pattern may reference a
`def` that appears later in the buffer.

### 6.10 Relationship to the JSON form

A `def` block lowers to exactly the `InstrumentSpec` that the application's
JSON and visual editors produce, so the two are interchangeable and a definition
written either way behaves identically.

---

## 7. Instrument Groups

A `group` block routes several pattern lines through one **shared filter
chain** — a true bus in the audio graph. One `reverb` on the group is one
reverb tail for the whole kit; one compressor pumps against the summed drums.

```
group drums {
  kick kick "x ~ x ~"
  hat hihat "x*8"
  sn snare "x(4,4)"
} | lpf 1800 | gain 0.9
```

### 7.1 Structure

- The header is `group <name> {` with nothing after the brace; members are
  ordinary pattern lines on their own lines, so line-based editing (muting,
  nudging, per-line diagnostics) keeps working inside the block.
- The shared chain goes after the closing `}`, piped like any pattern line.
  A bare `}` closes a group with no shared filters — then the group is only
  a mute/solo and mixer unit.
- Groups do not nest, `def` blocks and directives cannot appear inside one,
  and `group` is a reserved word. A group name must not collide with a
  pattern name (they share the mixer namespace).

### 7.2 What the chain accepts

The group pipe takes **audio transforms** — `gain`, `pan` (fixed or swept),
`lpf`, `hpf`, `delay`, `reverb`, and `fx`/aliases — plus `vel`, which is not
a filter: it multiplies into every member's velocity. Event transforms
(`rev`, `fast`, `slow`, `every`, `arp`, `oct`, `scale`) shape individual
patterns and are rejected on a group; write them on the member lines.
Ranges are also rejected on a group, even though a pattern line accepts them
(§4.6): a bus serves several lines that were added and edited at different
cycles, so there is no one cycle to measure the travel from. `| ramp` on a
group is an error for now.

### 7.3 Muting and mixing

`;` before `group` mutes every member in one keystroke without touching the
members' own mute flags:

```
; group drums {
  kick kick "x ~ x ~"
}
```

The mixer shows a strip per group with its own mute/solo. Soloing a group
solos all of its members; a member's own mute still silences just that line.
External control targets `mixer:<group>:mute` and `mixer:<group>:solo` work
like their pattern counterparts.

---

## 8. Pieces

Everything up to here loops forever. A **piece** is the same language with a
structure: named sections of a stated length, an order to play them in, and
therefore an end. A piece is written, rendered and played back rather than
performed, and it is the same file format, the same parser and the same
mini-notation — a live buffer is simply a piece with no structure.

```
-- nocturne.rt
bpm 96
sig 4/4
scale C minor
seed 7
tail 3.0

section intro 8 {
  pad  pad   "[c3,eb3,g3] ~ ~ ~"
  bell piano "0 ~ ~ 4"           @ 5..
}

section verse 16 {
  kick kick  "x ~ x ~"
  bass saw   "c2 _ eb2 _ g1 _ f2 _" | lpf 400..4000 | ramp 16 exp
  fill snare "x*16"               @ 16
}

section chorus 16 {
  bpm 104
  kick kick  "x*4"
  lead saw   "0 3 5 7"            | vel 0.9
  bass saw   "c2*2 eb2*2"
}

arrange intro verse chorus verse*2 chorus intro
```

### 8.1 Piece Mode

A buffer is in **piece mode** if it contains at least one `section` block.
Nothing else switches the mode on, and a buffer with no `section` behaves
exactly as it did before pieces existed.

The distinction matters because the two modes answer a different question. A
live buffer asks *what is sounding now*; a piece asks *what sounds at cycle
N of the whole*. Piece-only constructs (`section`, `arrange`, `tail`, `@`) are
an error in a live buffer, and the live-only `phrase` is an error in a piece —
a piece has no "next boundary to land the drop on", because nothing is being
edited while it plays.

### 8.2 `section <name> <cycles> { … }`

```
section verse 16 {
  kick kick "x ~ x ~"
}
```

The header is `section <name> <cycles> {` with nothing after the brace, and the
closing `}` takes nothing after it. `<cycles>` is a positive integer: how many
cycles the section lasts, which is stated rather than inferred because the
length is the one thing about a section that cannot be read off its members —
a one-cycle pattern repeated for sixteen cycles and the same pattern played
once look identical inside the braces.

A section may contain:

- **pattern lines**, which loop for the section's length unless narrowed by a
  span (§8.3);
- **`group` blocks** (§7), which nest one level inside a section;
- the directives **`bpm`**, **`sig`** and **`scale`**, scoped to the section
  (§8.5).

A section may not contain `def`, `load`, `include`, `phrase`, `arrange`,
`tail`, `seed`, or another `section`. Sections do not nest. `section` is a
reserved word, so no pattern may be named it, and a section name must not
collide with a pattern or group name.

`;` before `section` mutes every member at once, exactly as it does on a group.
**The section's time still passes** — a muted section is a rest of its own
length in the arrangement, not a removal from it, so muting a chorus to hear
the transition around it does not shorten the piece.

### 8.3 Spans: `@ <cycles>`

A member line may narrow itself to part of its section. The span sits
immediately after the mini-notation, before any `|`:

```
section verse 16 {
  kick kick   "x ~ x ~"                    -- all 16 cycles
  fill snare  "x*16"           @ 16        -- the last cycle only
  bass saw    "c2 _ eb2 _"     @ 3..16     -- from cycle 3 to the end
  pad  pad    "[c3,eb3,g3]"    @ 9..       -- the same, written open
  hat  hihat  "x*8"            @ ..8       -- the first half
}
```

    span = "@" ( integer | [ integer ] ".." [ integer ] ) ;

Cycles are **1-based and inclusive at both ends** — `@ 3..16` sounds on cycles
3 and 16 and every cycle between them. Both readings follow the rest of the
language: `a..b` is an inclusive interval wherever it appears (§4.6), and a
musician counting bars starts at one.

Either end may be omitted. `@ 5..` runs from cycle 5 to the end of the section,
`@ ..4` from the start through cycle 4, and a bare `@ n` is the single cycle
`n`. A span whose start is after its end, or whose end is past the section's
length, is an error: a line that can never sound is a mistake rather than a
silence, and saying so at parse time is cheaper than hunting for it in a render.

The position is fixed rather than free within the pipeline. A span is not a
transform — it says *when the line exists*, not what happens to it — and
keeping it adjacent to the notation leaves the `|` chain contiguous and
scannable at a glance.

A span is only meaningful inside a section. A **top-level pattern line in a
piece** sounds for the whole arrangement, which is how a drone or a click track
is written; a span on such a line is an error, because its cycle numbers would
have to be piece-absolute and mean something different from every other span in
the file.

### 8.4 `arrange <name> [ <name> … ]`

```
arrange intro verse chorus verse*2 chorus intro
```

    arrange_dir  = "arrange" arrange_item { arrange_item } ;
    arrange_item = name [ "*" integer ] ;

The arrangement names sections in playing order. `name*N` plays a section N
times in a row, reusing `*N` from the mini-notation, where it also means
"repeat this".

A section may appear any number of times or not at all — an unused section is
a sketch, not an error, though a consumer is expected to say which sections it
never played. A name that is not a section, an `arrange` with no items, and a
second `arrange` line are all errors.

**`arrange` is optional.** With no `arrange` line, the arrangement is the
sections in source order, once each — which is what a file being written from
the top down already means, and it lets a piece grow one section at a time
before its structure is decided.

### 8.5 Directives Inside a Section

`bpm`, `sig` and `scale` inside a section are **scoped to that section**: they
take effect at its first cycle and the previous value resumes when it ends.

```
bpm 96

section verse 16 { … }            -- 96
section chorus 16 { bpm 104 … }   -- 104
section outro 8 { … }             -- 96 again
```

This is the one place a piece deliberately departs from how a score is usually
notated, where a tempo marking persists until the next one. The reason is
reuse: a section is *named* so it can be played from more than one place in the
arrangement, and a directive that leaked forward would make the same name sound
different depending on what preceded it. `arrange intro verse chorus` and
`arrange chorus verse intro` would then play different `verse`s, and the
arrangement line — the one line that is supposed to show the shape of the piece
— would stop being readable on its own.

A tempo that should hold across several sections is therefore written in each
of them. That is more typing and it is honest: each section states the tempo it
plays at, and reading any one of them tells you how it sounds.

A `sig` change alters the length of a cycle and so the length of the section in
real time, but not in cycles: `section bridge 8` is eight cycles of whatever
`sig` is in force inside it.

### 8.6 Time Inside a Section

`ramp` (§4.6) and `every` (§4.3) count from the **start of the line's span in
the current occurrence of the section**, not from the start of the piece.

A section is self-contained for the same reason its directives are: `arrange
verse*4` plays four identical verses, and a filter opening over the verse opens
once per verse. The consequence is that a build cannot straddle the seam
between two sections — a sweep that should run across the whole piece is
written as one long section rather than as a repeat. That is the trade for
being able to read a section on its own, and it is the more common case: most
builds are a section-length gesture.

### 8.7 `tail <seconds>`

```
tail 3.0
```

How long to keep rendering after the last cycle of the arrangement, so note
releases, delay repeats and reverb tails ring out instead of being cut at the
final barline. Defaults to `2.0`. A piece whose last chord is a long pad wants
more; a piece that ends on a closed hi-hat wants none, and `tail 0` is legal.

This is the length of the *rendered* tail, not a musical instruction — it adds
no cycles to the arrangement and nothing new is triggered during it.

### 8.8 `seed <integer>`

```
seed 7
```

The generative constructs — random choice (§3.12), random drop (§3.15),
`solo` (§3.19) and a `rand` pan sweep (§4.4) — are resolved by hashing, not
from a running RNG, so the same source already renders identically every time.
`seed` salts that hash. It exists so a composer can **reroll** a generative
passage without rewriting it: change the seed, get a different solo, keep the
piece byte-identical to itself thereafter.

Defaults to `0`, so every piece written before seeds existed keeps the phrase
it had.

A live buffer accepts `seed` too — the reroll is just as useful mid-session —
but the value takes effect at the next boundary like any other directive.

### 8.9 `meta <key> "<value>"`

```
meta title    "Nocturne for a Slow Machine"
meta composer "F. Grimau"
meta comment  "One collection, three centres."
meta tuning   "twelve-tone equal"
```

    meta_line = "meta" identifier string_literal ;

A free-form tag that travels with the piece. The key set is deliberately open:
a piece knows things about itself that a music language has no business
enumerating, and a renderer can pass whatever it finds straight through to the
file it writes.

Keys are lower-cased, so `Title` and `title` are one tag. Writing a key twice
replaces it rather than accumulating, so editing a title does not leave the
previous one behind in the output.

Nothing in the language reads these — they change no sound and produce no
delta. A consumer decides what to do with them; the reference renderer writes
the well-known ones (`title`, `composer`, `artist`, `album`, `genre`, `year`,
`comment`, `copyright`) into the WAV's `INFO` chunk and files the rest
alongside the comment, since dropping what the author wrote would be worse than
filing it loosely.

`meta` is accepted in a live buffer too. It is inert there, but a performance
that gets captured to disk carries its own name that way.

---

### 8.10 Playback and Rendering

A piece has a definite length: the sum of its arranged sections' cycles, plus
`tail`. That is what makes it renderable — a consumer can compile the whole
timeline up front, schedule every event at an absolute frame, and write the
result to a file, rather than staying one boundary ahead of a performer.

Diffing and quantised application (§9) do not apply while a piece plays. A
piece is re-evaluated from the top: an edit produces a new timeline and the
consumer decides where to resume from, which is a transport question rather
than a language one.

This crate resolves the arrangement into a flat timeline — a list of section
occurrences with their absolute starting cycle, the state in force in each,
and the lines audible in each — and does nothing else with it. Turning that
into audio, and choosing what to do with an edit mid-playback, is the
consumer's half exactly as it is for the live language.

---

## 9. Evaluation Semantics

### 9.1 Quantised Application

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
| New or changed `load`        | Definitions swap at next loop boundary |
| Removed `load`               | Its definitions leave at next loop boundary |

### 9.2 Diffing Rules

The session tracks patterns by **name**. After parsing:

- If a name exists in the new source but not the old: **added**.
- If a name exists in the old source but not the new: **removed**.
- If a name exists in both but the pattern content changed: **modified**.
- If a name exists in both with identical content: **unchanged** (no action).

`def` blocks and `group` blocks diff by name the same way. `load` lines diff by
**path**: a path new to the buffer, or one whose file has changed on disk since
it was resolved, is a load to (re)resolve; a path no longer in the buffer
unloads its definitions. Only the consumer can see a file change, since this
crate does no I/O — the session raises the load when the buffer's set of paths
changes and the consumer raises it again when it notices the file itself move.

### 9.3 Error Handling

Parsing is **line-independent**: an error on one line does not prevent other
lines from being parsed and evaluated. The session keeps the last-good version
of any pattern that fails to parse.

Errors are reported per-line in the eval output panel:

```
[#0003] [ERR] Line 7: expected closing '"' in pattern
[#0003] [ERR] Line 12: unknown instrument 'wobbl'
[#0003] [ OK] 4/6 patterns updated successfully.
```

### 9.4 State Model

The session maintains:

| State field          | Type                    | Description              |
|----------------------|-------------------------|--------------------------|
| `bpm`                | `u32`                   | Current tempo            |
| `sig`                | `(u8, u8)`              | Time signature           |
| `scale`              | `Option<(Root, Mode)>`  | Default scale            |
| `patterns`           | `Map<Name, Pattern>`    | Active patterns          |
| `loads`              | `Set<Path>`             | `load` paths in the buffer |
| `pending`            | `Vec<Delta>`            | Queued changes           |
| `beat_position`      | `f64`                   | Current beat in cycle    |

---

## 10. Grammar (Formal)

```ebnf
program       = { line } ;
line          = comment | directive | pattern_line | muted_line | blank ;
blank         = { whitespace } ;
comment       = "--" { any_char } ;

directive     = bpm_dir | sig_dir | phrase_dir | scale_dir | load_dir
              | include_dir | arrange_dir | tail_dir | seed_dir | meta_dir ;
bpm_dir       = "bpm" integer ;
sig_dir       = "sig" integer "/" integer ;
phrase_dir    = "phrase" integer ;          (* live only *)
scale_dir     = "scale" pitch_root scale_mode ;
load_dir      = "load" string_literal ;
include_dir   = ( "include" | "use" ) name ;
arrange_dir   = "arrange" arrange_item { arrange_item } ;   (* piece only, §8.4 *)
arrange_item  = name [ "*" integer ] ;
tail_dir      = "tail" number ;             (* piece only, §8.7 *)
seed_dir      = "seed" integer ;            (* §8.8 *)
meta_dir      = "meta" name string_literal ;  (* §8.9 *)

pattern_line  = name instrument string_literal [ span ] { "|" transform } ;
muted_line    = ";" name instrument string_literal [ span ] { "|" transform } ;

(* Where the line sounds inside its section — 1-based, both ends inclusive,
   either end omittable. Section members only (§8.3). *)
span          = "@" ( integer | [ integer ] ".." [ integer ] ) ;

group_header  = [ ";" ] "group" name "{" ;
group_footer  = "}" { "|" transform } ;
(* member pattern lines sit between header and footer, one per line *)

section_header = [ ";" ] "section" name integer "{" ;   (* §8.2 *)
section_footer = "}" ;                                  (* takes no pipeline *)
(* members are pattern lines, group blocks, and bpm/sig/scale directives *)

name          = identifier ;
instrument    = identifier ;
identifier    = letter { letter | digit | "_" } ;

transform     = "rev"
              | "fast" range
              | "slow" range
              | "every" integer transform
              | "arp" arp_mode
              | "scale" pitch_root scale_mode
              | "oct" range
              | "vel" range
              | "ramp" integer [ ramp_curve ]
              | "gain" range
              | "pan" range
              | "pan" lfo_wave lfo_rate [ number ]
              | fx_call
              | "lpf" range
              | "hpf" range
              | "delay" range range [ range ]
              | "reverb" range ;

arp_mode      = "up" | "down" | "updown" | "random" ;

(* A value that travels across the line's ramp span — §4.6. *)
range         = signed_number
              | signed_number ".." signed_number
              | signed_number { ">" signed_number } ;
ramp_curve    = "lin" | "exp" ;   (* omitted = "lin" *)

lfo_wave      = "sine" | "sin" | "tri" | "triangle"
              | "sq" | "square" | "saw" | "rand" | "random" ;
lfo_rate      = number [ "hz" ] ;   (* bare = cycles per sweep *)

fx_call       = ( "fx" identifier | fx_alias ) { fx_arg } ;
fx_arg        = fx_value | identifier "=" fx_value ;
fx_value      = range [ "hz" ] ;
fx_alias      = "trem" | "bpf" | "rbpf" | "avg"
              | "clip" | "comp" | "limit" ;

(* Mini-notation grammar — contents of string_literal *)
mini          = sequence ;
sequence      = step { whitespace step } ;
(* At most one velocity per step, written anywhere among the modifiers — §3.17.
   `accent` is shorthand for velocity 1.0, so `X` and `x:1.0` are one thing. *)
step          = atom { modifier | velocity } ;
atom          = note | degree | trigger | accent | rest | hold
              | group | alternation | solo ;
group         = "[" ( chord | choice ) "]" ;
alternation   = "<" sequence ">" ;
chord         = sequence { "," sequence } ;
choice        = sequence { "|" sequence } ;
(* commas and pipes never mix inside one group *)
note          = note_name [ accidental ] octave ;
degree        = integer ;
trigger       = "x" ;
accent        = "X" ;                (* = trigger at velocity 1.0 *)
rest          = "~" ;
hold          = "_" ;
solo          = "solo" "(" signed_integer ".." signed_integer "," range ")" ;
note_name     = "a" | "b" | "c" | "d" | "e" | "f" | "g" ;
accidental    = "#" | "b" | "##" | "bb" ;
octave        = digit ;
modifier      = repeat | slow_mod | replicate | euclidean | drop | weight ;
repeat        = "*" range ;
slow_mod      = "/" integer ;
replicate     = "!" integer ;
euclidean     = "(" onsets "," positions [ "," offset ] ")" ;
                (* onsets may exceed positions; positions >= 1 *)
onsets        = range ;
positions     = range ;
offset        = integer ;
drop          = "?" [ range ] ;    (* no whitespace before the number *)
weight        = "@" integer ;
velocity      = ":" range ;        (* 0.0..=1.0; not on a rest or a hold *)

pitch_root    = upper_note_name [ accidental ] ;
upper_note_name = "A" | "B" | "C" | "D" | "E" | "F" | "G" ;
scale_mode    = "major" | "minor" | "dorian" | "phrygian" | "lydian"
              | "mixolydian" | "aeolian" | "locrian"
              | "chromatic" | "pentatonic" | "blues" ;

number        = integer | float ;
signed_number = [ "-" ] number ;
integer       = digit { digit } ;
signed_integer = [ "-" ] integer ;
float         = digit { digit } "." digit { digit } ;
string_literal = '"' { mini_char } '"' ;
```

---

## 11. Example Session

```
-- Minimal techno loop
bpm 128
sig 4/4

kick  kick   "x ~ x ~"
snare snare  "~ X ~ x"
hats  hihat  "x*8 x:0.4*8"

bass  saw    "c2 _ eb2 _ g1 _ f2 _" | pan -0.3 | lpf 300..9000 | ramp 16 exp
lead  piano  "c4 eb4 g4 bb4" | slow 2 | vel 0.8 | pan sine 4 0.6

; pad  pad   "[c3,eb3,g3] ~ [f3,ab3,c4] ~"
```

This defines:
- Three drum loops: 4-on-the-floor kick, a backbeat snare accenting beat 2,
  and hats that drop to a ghost velocity for the second half of the cycle
- A bass line stepping through C, Eb, G, F (each held 2 slots), under a filter
  that opens from 300 Hz to 9 kHz over sixteen cycles, geometrically
- A piano lead playing a Cm7 arpeggio over 2 cycles
- A muted pad pattern (ready to unmute by removing `;`)

### 11.1 The Same Material as a Piece

```
-- Same techno, given a structure and an end
bpm 128
sig 4/4
tail 2.5

section intro 8 {
  kick  kick   "x ~ x ~"
  hats  hihat  "x*8"        @ 5..
}

section main 16 {
  kick  kick   "x ~ x ~"
  snare snare  "~ X ~ x"
  hats  hihat  "x*8 x:0.4*8"
  bass  saw    "c2 _ eb2 _ g1 _ f2 _" | pan -0.3 | lpf 300..9000 | ramp 16 exp
  fill  snare  "x*16"       @ 16
}

section break 8 {
  bpm 128
  pad   pad    "[c3,eb3,g3] ~ [f3,ab3,c4] ~"
  hats  hihat  "x*8"        @ 7..
}

arrange intro main break main*2 intro
```

Sixty-four cycles at 128 BPM plus a 2.5-second tail, playing the same lines the
live buffer above does. `main` appears three times and sounds the same each
time — its `lpf` build restarts with it (§8.6) — and the `fill` lands only on
the last cycle of each `main`.

---

## 12. Future Extensions

- `fn` blocks for reusable pattern fragments
- Per-pattern `bpm` / `sig` overrides (polymetric)
- MIDI input/output
- OSC integration
- `import` for sharing patterns between files
- Conditional patterns (`if cycle > 16 then ...`)
- Probability weights on random choice (`[c4@3|e4@1]`)
- Structural transforms beyond `every`: `rot`, `palindrome`, `iter`, `ply`,
  `off`, `stut`, `jux`, `chunk`
- Ranges on a group's shared chain (§7.2)
- Curves beyond `lin` and `exp` (`log`, an eased `s`), and per-value curves
- LFO sweeps on parameters other than `pan` (`lpf`, `gain`, …), which need one
  modulated filter per parameter in the engine
- Phase offset on a sweep, so two lines can be counter-panned against each other
- Piece-absolute spans on a top-level line (§8.3), once there is a spelling
  that cannot be confused with a section-relative one
- A build that straddles a section seam (§8.6) — needs a way to name an origin
  outside the current section without giving up a section's self-containment
- Nested sections, so a piece can have movements as well as sections
