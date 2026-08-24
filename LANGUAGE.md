# Treble Live — Language Specification v0.1

## Overview

Treble Live is a live-coding music DSL designed for real-time composition in a
terminal environment. It prioritises brevity and immediacy: every line either
configures the session or defines a looping pattern. Changes take effect at the
next loop boundary (quantised to the time signature).

The language draws inspiration from TidalCycles' mini-notation while remaining
self-contained (no host language required).

---

## 1. Source Structure

A Treble Live source file (`.rt`) is a sequence of **lines**. Each line is one
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

### 3.17 Stacking Modifiers

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

### 3.18 Generated Solo: `solo(low..high, steps)`

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
| `vel <amount>`         | Note velocity (float, 0.0–1.0)                 |
| `ramp <cycles>`        | How long the line's ranges take to travel (§4.6) |
| `gain <amount>`        | Output level (float, 0.0–2.0)                  |
| `pan <position>`       | Fixed stereo position (-1.0 left – 1.0 right)  |
| `pan <wave> <rate> [depth]` | Stereo position swept by an LFO (§4.4)    |
| `lpf <cutoff>`         | Low-pass filter, cutoff in Hz                  |
| `hpf <cutoff>`         | High-pass filter, cutoff in Hz                 |
| `delay <time> <fb> [mix]` | Delay (seconds, feedback 0–0.99, wet mix 0–1, default 0.35) |
| `reverb <amount>`      | Reverb mix (float, 0.0–1.0)                    |
| `fx <filter> <arg>...` | Any engine filter, by name (§4.5)              |

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
    value   = signed_number [ "hz" ] ;
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

    range = number
          | number ".." number          (* sweeps continuously *)
          | number ( ">" number )+ ;    (* holds each stage *)

`a..b` moves continuously and passes through every value between the ends.
`a>b>c` holds each stage for an **equal share** of the span and passes through
nothing else — `x(2>4>8>16,4) | ramp 16` is four stages of four cycles. This is
what a doubling build needs, since a sweep from 2 to 16 would run through 3, 6,
9 and the rest on its way.

Mixing the two spellings in one value is an error, as is giving `..` more than
two ends.

One `ramp` covers every range on the line, because a build usually moves several
things over the same span. Ranges are accepted on the `vel`, `oct`, `fast` and
`slow` transforms, and on the `*N`, `(onsets,positions)` and `?p` mini-notation
modifiers.

A range without a `ramp`, or a `ramp` with nothing to move, is an authoring
mistake and should be reported rather than guessed at.

**Holding, not looping.** A ramp arrives and stays until the line changes; that
is what makes it a crescendo rather than a repeating sweep. Consumers should
measure travel from the cycle the line was last added or modified, so editing one
line does not restart another line's build.

Audio transforms (§4.3) cannot take ranges: a filter parameter only changes when
the graph is rebuilt.

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
Ranges are also rejected: bus filter parameters only change at graph
rebuilds, so `| ramp` on a group is an error for now.

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

## 8. Evaluation Semantics

### 8.1 Quantised Application

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

### 8.2 Diffing Rules

The session tracks patterns by **name**. After parsing:

- If a name exists in the new source but not the old: **added**.
- If a name exists in the old source but not the new: **removed**.
- If a name exists in both but the pattern content changed: **modified**.
- If a name exists in both with identical content: **unchanged** (no action).

### 8.3 Error Handling

Parsing is **line-independent**: an error on one line does not prevent other
lines from being parsed and evaluated. The session keeps the last-good version
of any pattern that fails to parse.

Errors are reported per-line in the eval output panel:

```
[#0003] [ERR] Line 7: expected closing '"' in pattern
[#0003] [ERR] Line 12: unknown instrument 'wobbl'
[#0003] [ OK] 4/6 patterns updated successfully.
```

### 8.4 State Model

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

## 9. Grammar (Formal)

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

group_header  = [ ";" ] "group" name "{" ;
group_footer  = "}" { "|" transform } ;
(* member pattern lines sit between header and footer, one per line *)

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
              | "vel" number
              | "gain" number
              | "pan" signed_number
              | "pan" lfo_wave lfo_rate [ number ]
              | fx_call
              | "lpf" number
              | "hpf" number
              | "delay" number number [ number ]
              | "reverb" number ;

arp_mode      = "up" | "down" | "updown" | "random" ;

lfo_wave      = "sine" | "sin" | "tri" | "triangle"
              | "sq" | "square" | "saw" | "rand" | "random" ;
lfo_rate      = number [ "hz" ] ;   (* bare = cycles per sweep *)

fx_call       = ( "fx" identifier | fx_alias ) { fx_arg } ;
fx_arg        = fx_value | identifier "=" fx_value ;
fx_value      = signed_number [ "hz" ] ;
fx_alias      = "trem" | "bpf" | "rbpf" | "avg"
              | "clip" | "comp" | "limit" ;

(* Mini-notation grammar — contents of string_literal *)
mini          = sequence ;
sequence      = step { whitespace step } ;
step          = atom { modifier } ;
atom          = note | degree | trigger | rest | hold
              | group | alternation ;
group         = "[" ( chord | choice ) "]" ;
alternation   = "<" sequence ">" ;
chord         = sequence { "," sequence } ;
choice        = sequence { "|" sequence } ;
(* commas and pipes never mix inside one group *)
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
euclidean     = "(" onsets "," positions [ "," offset ] ")" ;
                (* onsets may exceed positions; positions >= 1 *)
onsets        = integer ;
positions     = integer ;
offset        = integer ;
drop          = "?" [ number ] ;   (* no whitespace before the number *)
weight        = "@" integer ;

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

## 10. Example Session

```
-- Minimal techno loop
bpm 128
sig 4/4

kick  kick   "x ~ x ~"
snare snare  "~ x ~ x"
hats  hihat  "x*8"

bass  saw    "c2 _ eb2 _ g1 _ f2 _" | pan -0.3
lead  piano  "c4 eb4 g4 bb4" | slow 2 | vel 0.8 | pan sine 4 0.6

; pad  pad   "[c3,eb3,g3] ~ [f3,ab3,c4] ~"
```

This defines:
- Three drum loops: 4-on-the-floor kick, backbeat snare, 8th-note hats
- A bass line stepping through C, Eb, G, F (each held 2 slots)
- A piano lead playing a Cm7 arpeggio over 2 cycles
- A muted pad pattern (ready to unmute by removing `;`)

---

## 11. Future Extensions (Not in v0.1)

- `def` blocks for custom instrument synthesis (§6)
- `fn` blocks for reusable pattern fragments
- Per-pattern `bpm` / `sig` overrides (polymetric)
- MIDI input/output
- OSC integration
- `import` for sharing patterns between files
- Conditional patterns (`if cycle > 16 then ...`)
- Probability weights on random choice (`[c4@3|e4@1]`)
- Structural transforms beyond `every`: `rot`, `palindrome`, `iter`, `ply`,
  `off`, `stut`, `jux`, `chunk`
- Per-step velocity/accent syntax
- LFO sweeps on parameters other than `pan` (`lpf`, `gain`, …), which need one
  modulated filter per parameter in the engine
- Phase offset on a sweep, so two lines can be counter-panned against each other
