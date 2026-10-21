//! The playing half of the server: render the asked-for lines of a section
//! the way the piece plays them, then stream the samples to the sound card.
//!
//! Rendering rather than scheduling live keeps one code path. What a line
//! sounds like here is, by construction, what it sounds like in the render,
//! ramps, drops and group buses included. The renderer runs at tens of times
//! real time, so a section is ready a fraction of a second after it is asked
//! for. A live edit re-renders in the background and the new take replaces the
//! old one at the next cycle boundary, with a short crossfade.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde_json::json;

use super::rpc::Outgoing;
use super::workspace::Analysis;
use crate::ast::ArrangeItem;
use crate::piece::{Occurrence, Piece};
use crate::render::offline::section_cycle_frames;
use crate::render::{first_playing, render};

/// How long a stop takes to fade out, in seconds.
const STOP_FADE: f64 = 0.02;
/// How long the old and new takes overlap when an edit swaps in, in seconds.
const SWAP_FADE: f64 = 0.01;
/// How often the client hears where playback is.
const PLAYHEAD_EVERY: Duration = Duration::from_millis(33);

/// What the client asked to hear.
#[derive(Debug, Clone)]
pub struct Request {
    pub id: u64,
    pub path: PathBuf,
    pub section: String,
    /// `None` is the whole section as written, mutes and all.
    pub lines: Option<Vec<String>>,
    pub looping: bool,
}

/// One render of a request: the section and its tail.
#[derive(Clone)]
struct Take {
    /// Stereo-interleaved.
    audio: Arc<Vec<f32>>,
    /// Where the asked-for lines sound, in frames.
    bounds: Bounds,
}

/// The part of a section a request is about, in frames from its start.
///
/// Lines heard on their own are heard from the first bar any of them sounds
/// on to the last, so a line written `@ 25..28` starts at once and a loop of
/// it comes back after four bars rather than after the whole section.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Bounds {
    /// Frames in one cycle of the section.
    cycle: u64,
    /// Where playback starts, and a loop returns to.
    start: u64,
    /// The end of the last bar the lines sound on: where a loop wraps, and
    /// what follows is their ring-out.
    body: u64,
    /// Where playing once stops: `body` plus the piece's `tail`.
    end: u64,
}

impl Bounds {
    /// The frame `cycle` starts on, kept within the bars that play: from the
    /// section's top (a start marker may sit before the lines come in) to the
    /// last bar the lines sound on.
    fn clamp(&self, cycle: u32) -> u64 {
        (u64::from(cycle) * self.cycle).min(self.body.saturating_sub(self.cycle))
    }
}

impl Take {
    fn frames(&self) -> u64 {
        (self.audio.len() as u64 / 2).min(self.bounds.end)
    }

    fn frame(&self, index: u64) -> (f32, f32) {
        let at = index as usize * 2;
        match (self.audio.get(at), self.audio.get(at + 1)) {
            (Some(left), Some(right)) => (*left, *right),
            _ => (0.0, 0.0),
        }
    }

    /// The sound at `position` of lap `lap`. A loop's later laps also carry
    /// the previous lap's tail, so a reverb rings over the seam the way it
    /// would into the next section.
    fn at(&self, position: u64, lap: u64, looping: bool) -> (f32, f32) {
        let (mut left, mut right) = self.frame(position);
        if looping && lap > 0 {
            let into_lap = position.saturating_sub(self.bounds.start);
            let (tail_left, tail_right) = self.frame(self.bounds.body + into_lap);
            left += tail_left;
            right += tail_right;
        }
        (left, right)
    }
}

/// A take moving through time.
#[derive(Clone)]
struct Cursor {
    take: Take,
    position: u64,
    lap: u64,
    looping: bool,
}

impl Cursor {
    fn sample(&self) -> (f32, f32) {
        self.take.at(self.position, self.lap, self.looping)
    }

    /// Move one frame on. `false` once a take that does not loop has run out.
    fn advance(&mut self) -> bool {
        self.position += 1;
        if self.looping && self.position >= self.take.bounds.body {
            self.position = self.take.bounds.start;
            self.lap += 1;
        }
        self.looping || self.position < self.take.frames()
    }
}

/// The request playing now.
struct Track {
    id: u64,
    looping: bool,
    /// `None` until the first render is ready.
    cursor: Option<Cursor>,
    /// A newer render waiting for the next cycle boundary.
    pending: Option<Take>,
    /// Frames of fade-in left after a swap, and the fade's length.
    fade_in: (u32, u32),
    /// Where to start (a cycle within the section) once the first take is
    /// ready, when the client asked for somewhere other than the lines' start.
    start_at: Option<u32>,
}

/// A take on its way out — stopped, replaced or swapped — fading to silence.
struct Leaving {
    cursor: Cursor,
    remaining: u32,
    length: u32,
}

/// What the sound card reads, shared with the request loop and the renders.
#[derive(Default)]
struct Deck {
    track: Option<Track>,
    leaving: Vec<Leaving>,
    /// A request that ran out, for the monitor to report.
    finished: Option<u64>,
}

impl Deck {
    /// The next stereo frame.
    fn next(&mut self, rate: u32) -> (f32, f32) {
        let mut left = 0.0;
        let mut right = 0.0;
        for leaving in &mut self.leaving {
            let gain = leaving.remaining as f32 / leaving.length as f32;
            let (l, r) = leaving.cursor.sample();
            left += l * gain;
            right += r * gain;
            leaving.remaining = leaving.remaining.saturating_sub(1);
            leaving.cursor.advance();
        }
        self.leaving.retain(|leaving| leaving.remaining > 0);

        let Some(track) = &mut self.track else {
            return (left, right);
        };
        // An edit swaps in on a cycle boundary, at the same bar of the new take.
        if let Some(cursor) = &mut track.cursor
            && track.pending.is_some()
            && cursor.position % cursor.take.bounds.cycle == 0
        {
            let take = track.pending.take().expect("checked above");
            let bar = cursor.position / cursor.take.bounds.cycle;
            let (mut position, mut lap) = (bar * take.bounds.cycle, cursor.lap);
            if position >= take.bounds.body {
                position = take.bounds.start;
                lap += 1;
            } else if position < take.bounds.start {
                // The edit moved where the lines start past the playhead.
                position = take.bounds.start;
            }
            let incoming = Cursor {
                take,
                position,
                lap,
                looping: track.looping,
            };
            let outgoing = std::mem::replace(cursor, incoming);
            let length = frames_of(SWAP_FADE, rate);
            self.leaving.push(Leaving {
                cursor: outgoing,
                remaining: length,
                length,
            });
            track.fade_in = (length, length);
        }
        let Some(cursor) = &mut track.cursor else {
            return (left, right);
        };
        let (l, r) = cursor.sample();
        let gain = match track.fade_in {
            (0, _) => 1.0,
            (remaining, length) => {
                track.fade_in.0 -= 1;
                1.0 - remaining as f32 / length as f32
            }
        };
        left += l * gain;
        right += r * gain;
        if !cursor.advance() {
            self.finished = Some(track.id);
            self.track = None;
        }
        (left, right)
    }

    /// Move the playhead to the start of `cycle` (0-based within the section),
    /// kept inside the bars the take plays. The old position fades out under
    /// the new one, as an edit's swap does. Returns the cycle it landed on.
    fn seek(&mut self, cycle: u32, rate: u32) -> Option<u32> {
        let track = self.track.as_mut()?;
        let Some(cursor) = &mut track.cursor else {
            // Still rendering: start there once the take is ready.
            track.start_at = Some(cycle);
            return Some(cycle);
        };
        let position = cursor.take.bounds.clamp(cycle);
        let length = frames_of(SWAP_FADE, rate);
        self.leaving.push(Leaving {
            cursor: cursor.clone(),
            remaining: length,
            length,
        });
        cursor.position = position;
        track.fade_in = (length, length);
        Some((position / cursor.take.bounds.cycle.max(1)) as u32)
    }

    /// Fade the current track out and forget it, returning its id.
    fn stop(&mut self, rate: u32) -> Option<u64> {
        let track = self.track.take()?;
        if let Some(cursor) = track.cursor {
            let length = frames_of(STOP_FADE, rate);
            self.leaving.push(Leaving {
                cursor,
                remaining: length,
                length,
            });
        }
        Some(track.id)
    }
}

fn frames_of(seconds: f64, rate: u32) -> u32 {
    ((seconds * f64::from(rate)) as u32).max(1)
}

pub struct Player {
    deck: Arc<Mutex<Deck>>,
    out: Outgoing,
    /// The sound card's rate once it is open.
    rate: Option<u32>,
    next_id: u64,
    current: Option<Request>,
    /// Bumped by every render started, so a slow render cannot land on top of
    /// a newer one.
    generation: Arc<AtomicU64>,
}

impl Player {
    pub fn new(out: Outgoing) -> Self {
        Self {
            deck: Arc::new(Mutex::new(Deck::default())),
            out,
            rate: None,
            next_id: 0,
            current: None,
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Start playing a request, replacing whatever plays now. Returns its id;
    /// the sound follows when the render is ready.
    pub fn play(
        &mut self,
        path: PathBuf,
        section: String,
        lines: Option<Vec<String>>,
        looping: bool,
        from: Option<u32>,
        analysis: Arc<Analysis>,
    ) -> Result<u64, String> {
        self.next_id += 1;
        let request = Request {
            id: self.next_id,
            path,
            section,
            lines,
            looping,
        };
        // Refuse what cannot render now, so the error is the request's answer
        // rather than a notification after the fact.
        let rate = self.open_audio()?;
        sub_piece(&analysis, &request, rate)?;
        let replaced = {
            let mut deck = lock(&self.deck);
            let replaced = deck.stop(rate);
            deck.track = Some(Track {
                id: request.id,
                looping,
                cursor: None,
                pending: None,
                fade_in: (0, 0),
                start_at: from,
            });
            replaced
        };
        if let Some(id) = replaced {
            self.out
                .notify("treble/ended", json!({"playId": id, "reason": "stopped"}));
        }
        self.current = Some(request.clone());
        self.render(request, analysis, rate);
        Ok(self.next_id)
    }

    /// Jump the playing request to the start of `cycle`. `None` when nothing
    /// plays.
    pub fn seek(&mut self, cycle: u32) -> Option<u32> {
        let rate = self.rate?;
        lock(&self.deck).seek(cycle, rate)
    }

    pub fn stop(&mut self) {
        let Some(rate) = self.rate else {
            return;
        };
        let stopped = lock(&self.deck).stop(rate);
        self.current = None;
        if let Some(id) = stopped {
            self.out
                .notify("treble/ended", json!({"playId": id, "reason": "stopped"}));
        }
    }

    /// The piece at `analysis.path` changed. If it is playing and the change
    /// compiles, render it again; the new take swaps in at the next boundary.
    pub fn update(&mut self, analysis: Arc<Analysis>) {
        let (Some(request), Some(rate)) = (&self.current, self.rate) else {
            return;
        };
        if request.path != analysis.path || !analysis.clean {
            return;
        }
        let playing = lock(&self.deck)
            .track
            .as_ref()
            .is_some_and(|track| track.id == request.id);
        if playing {
            self.render(request.clone(), analysis, rate);
        } else {
            self.current = None;
        }
    }

    fn render(&self, request: Request, analysis: Arc<Analysis>, rate: u32) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let latest = self.generation.clone();
        let deck = self.deck.clone();
        let out = self.out.clone();
        thread::spawn(move || {
            let take = sub_piece(&analysis, &request, rate).and_then(|(piece, bounds)| {
                render(&piece, &analysis.registry, rate).map(|rendered| Take {
                    audio: Arc::new(rendered.samples),
                    bounds,
                })
            });
            if latest.load(Ordering::SeqCst) != generation {
                return;
            }
            let mut deck = lock(&deck);
            let Some(track) = deck.track.as_mut().filter(|track| track.id == request.id) else {
                return;
            };
            match take {
                Ok(take) if track.cursor.is_none() => {
                    let position = match track.start_at {
                        Some(cycle) => take.bounds.clamp(cycle),
                        None => take.bounds.start,
                    };
                    track.cursor = Some(Cursor {
                        position,
                        take,
                        lap: 0,
                        looping: request.looping,
                    });
                }
                Ok(take) => track.pending = Some(take),
                // A failed re-render leaves the old take playing; the edit's
                // diagnostics already say what is wrong.
                Err(_) if track.cursor.is_some() => {}
                Err(message) => {
                    deck.track = None;
                    drop(deck);
                    out.notify(
                        "treble/ended",
                        json!({"playId": request.id, "reason": "error", "message": message}),
                    );
                }
            }
        });
    }

    /// Open the default output device on first use, and start the monitor
    /// that reports the playhead.
    fn open_audio(&mut self) -> Result<u32, String> {
        if let Some(rate) = self.rate {
            return Ok(rate);
        }
        let (opened, result) = mpsc::channel();
        let deck = self.deck.clone();
        // A cpal stream cannot move between threads on every platform, so one
        // thread opens it and keeps it alive for the life of the process.
        thread::spawn(move || {
            let stream = (|| -> Result<(cpal::Stream, u32), String> {
                let device = cpal::default_host()
                    .default_output_device()
                    .ok_or("There's no audio output device.")?;
                let config = device
                    .default_output_config()
                    .map_err(|error| format!("The output device has no usable format: {error}"))?;
                if config.sample_format() != cpal::SampleFormat::F32 {
                    return Err(format!(
                        "The output device wants {:?} samples; only 32-bit float output is supported so far.",
                        config.sample_format()
                    ));
                }
                let rate = config.sample_rate().0;
                let channels = usize::from(config.channels());
                let stream = device
                    .build_output_stream(
                        &config.into(),
                        move |data: &mut [f32], _| {
                            let mut deck = lock(&deck);
                            for frame in data.chunks_mut(channels) {
                                let (left, right) = deck.next(rate);
                                let (left, right) = (left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0));
                                match frame.len() {
                                    1 => frame[0] = 0.5 * (left + right),
                                    _ => {
                                        frame[0] = left;
                                        frame[1] = right;
                                        frame[2..].fill(0.0);
                                    }
                                }
                            }
                        },
                        |error| eprintln!("audio output: {error}"),
                        None,
                    )
                    .map_err(|error| format!("Can't open the audio output: {error}"))?;
                stream
                    .play()
                    .map_err(|error| format!("Can't start the audio output: {error}"))?;
                Ok((stream, rate))
            })();
            match stream {
                Ok((stream, rate)) => {
                    let _ = opened.send(Ok(rate));
                    loop {
                        thread::park();
                        let _ = &stream;
                    }
                }
                Err(message) => {
                    let _ = opened.send(Err(message));
                }
            }
        });
        let rate = result
            .recv()
            .map_err(|_| "The audio thread stopped before opening the output.".to_string())??;
        self.rate = Some(rate);
        self.monitor(rate);
        Ok(rate)
    }

    /// Report the playhead while something plays, and the end when it runs out.
    fn monitor(&self, rate: u32) {
        let deck = self.deck.clone();
        let out = self.out.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(PLAYHEAD_EVERY);
                let (playhead, finished) = {
                    let mut deck = lock(&deck);
                    let playhead = deck.track.as_ref().map(|track| match &track.cursor {
                        Some(cursor) => json!({
                            "playId": track.id,
                            "state": "playing",
                            "cycle": cursor.position / cursor.take.bounds.cycle.max(1),
                            "seconds": cursor.position as f64 / f64::from(rate),
                            "lap": cursor.lap,
                        }),
                        None => json!({
                            "playId": track.id,
                            "state": "rendering",
                            "cycle": 0,
                            "seconds": 0.0,
                            "lap": 0,
                        }),
                    });
                    (playhead, deck.finished.take())
                };
                if let Some(playhead) = playhead {
                    out.notify("treble/playhead", playhead);
                }
                if let Some(id) = finished {
                    out.notify("treble/ended", json!({"playId": id, "reason": "finished"}));
                }
            }
        });
    }
}

fn lock(deck: &Mutex<Deck>) -> std::sync::MutexGuard<'_, Deck> {
    deck.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The piece a request renders: its one section, played where the piece
/// first plays it so travels, drops and choices land as they do in the song,
/// holding only the lines asked for, with where they sound.
fn sub_piece(analysis: &Analysis, request: &Request, rate: u32) -> Result<(Piece, Bounds), String> {
    let piece = analysis
        .piece
        .as_ref()
        .ok_or("This piece doesn't parse yet, so there's nothing to play.")?;
    let index = piece
        .sections
        .iter()
        .position(|section| section.name == request.section)
        .ok_or_else(|| format!("There's no section named {}.", request.section))?;
    let mut section = piece.sections[index].clone();
    // Asking for something by name is asking to hear it, mute or not.
    section.muted = false;
    if let Some(names) = &request.lines {
        section.patterns.retain(|line| names.contains(&line.name));
        if section.patterns.is_empty() {
            return Err(format!(
                "{} has no line named {}.",
                section.name,
                names.join(" or ")
            ));
        }
        for line in &mut section.patterns {
            line.muted = false;
        }
    }
    if let Some(line) = section
        .patterns
        .iter()
        .find(|line| analysis.registry.get(&line.instrument).is_none())
    {
        return Err(format!(
            "{} plays '{}', which isn't defined.",
            line.name, line.instrument
        ));
    }
    // A section heard whole runs top to bottom; lines heard on their own run
    // from where the earliest comes in to where the latest leaves.
    let cycles = section.cycles;
    let (first_bar, last_bar) = match request.lines {
        Some(_) => section
            .patterns
            .iter()
            .fold((cycles, 1), |(first, last), line| {
                let (from, to) = line
                    .span
                    .map_or((1, cycles), |span| (span.start(), span.end(cycles)));
                (first.min(from), last.max(to))
            }),
        None => (1, cycles),
    };
    let cycle = section_cycle_frames(&section, rate);
    let body = cycle * u64::from(last_bar.clamp(1, cycles));
    let bounds = Bounds {
        cycle,
        start: (cycle * u64::from(first_bar.clamp(1, cycles) - 1)).min(body - cycle),
        body,
        end: body + (piece.tail.max(0.0) * f64::from(rate)).round() as u64,
    };
    let name = section.name.clone();
    let sub = Piece {
        sections: vec![section],
        arrangement: vec![ArrangeItem {
            section: name,
            repeat: 1,
        }],
        timeline: vec![Occurrence {
            section: 0,
            start_cycle: first_playing(piece, index),
        }],
        // Top-level lines sound under the whole piece, so they belong in a
        // section heard whole and not under a line heard on its own.
        throughout: if request.lines.is_none() {
            piece.throughout.clone()
        } else {
            Vec::new()
        },
        tail: piece.tail,
        seed: piece.seed,
        unused: Vec::new(),
        metadata: Vec::new(),
    };
    Ok((sub, bounds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(frames: usize, body: u64, cycle: u64, value: impl Fn(usize) -> f32) -> Take {
        let audio = (0..frames).flat_map(|i| [value(i), value(i)]).collect();
        Take {
            audio: Arc::new(audio),
            bounds: Bounds {
                cycle,
                start: 0,
                body,
                end: frames as u64,
            },
        }
    }

    #[test]
    fn a_loop_carries_the_previous_laps_tail() {
        // Four frames of section, two of tail.
        let take = take(6, 4, 2, |i| if i >= 4 { 0.5 } else { 0.1 });
        assert_eq!(take.at(0, 0, true), (0.1, 0.1));
        assert_eq!(take.at(0, 1, true), (0.6, 0.6));
        assert_eq!(take.at(2, 1, true), (0.1, 0.1), "past the tail");
    }

    #[test]
    fn a_pending_take_swaps_in_on_a_cycle_boundary() {
        let rate = 1000;
        let mut deck = Deck {
            track: Some(Track {
                id: 1,
                looping: true,
                cursor: Some(Cursor {
                    take: take(8, 8, 4, |_| 0.0),
                    position: 1,
                    lap: 0,
                    looping: true,
                }),
                pending: Some(take(8, 8, 4, |_| 1.0)),
                fade_in: (0, 0),
                start_at: None,
            }),
            ..Deck::default()
        };
        for _ in 0..3 {
            assert_eq!(deck.next(rate), (0.0, 0.0), "not yet on a boundary");
        }
        deck.next(rate);
        assert!(
            deck.track.as_ref().unwrap().pending.is_none(),
            "swapped at frame 4"
        );
        for _ in 0..frames_of(SWAP_FADE, rate) {
            deck.next(rate);
        }
        assert_eq!(deck.next(rate), (1.0, 1.0), "the new take, fully faded in");
    }

    #[test]
    fn a_loop_returns_to_where_its_lines_start() {
        // Two bars of two frames; the line comes in on bar 2.
        let mut late = take(6, 4, 2, |i| i as f32);
        late.bounds.start = 2;
        let mut cursor = Cursor {
            position: late.bounds.start,
            take: late,
            lap: 0,
            looping: true,
        };
        assert_eq!(cursor.sample(), (2.0, 2.0), "starts on bar 2, not bar 1");
        cursor.advance();
        cursor.advance();
        assert_eq!(cursor.position, 2, "loops back to bar 2");
        assert_eq!(
            cursor.sample(),
            (2.0 + 4.0, 2.0 + 4.0),
            "with the first tail frame"
        );
    }

    #[test]
    fn playing_once_stops_after_the_lines_ring_out() {
        // The lines end at frame 4; one frame of tail, then silence to 10.
        let mut early = take(10, 4, 2, |_| 0.3);
        early.bounds.end = 5;
        let mut cursor = Cursor {
            take: early,
            position: 0,
            lap: 0,
            looping: false,
        };
        let heard = std::iter::from_fn(|| cursor.advance().then_some(())).count();
        assert_eq!(
            heard, 4,
            "frames 1 to 4 follow frame 0; frame 5 is the stop"
        );
    }

    #[test]
    fn seeking_lands_on_a_bar_and_stays_inside_the_lines() {
        let rate = 1000;
        let mut deck = Deck {
            track: Some(Track {
                id: 3,
                looping: true,
                cursor: Some(Cursor {
                    take: take(40, 16, 4, |i| i as f32),
                    position: 1,
                    lap: 0,
                    looping: true,
                }),
                pending: None,
                fade_in: (0, 0),
                start_at: None,
            }),
            ..Deck::default()
        };
        assert_eq!(deck.seek(2, rate), Some(2));
        assert_eq!(
            deck.track
                .as_ref()
                .unwrap()
                .cursor
                .as_ref()
                .unwrap()
                .position,
            8
        );
        assert_eq!(deck.seek(9, rate), Some(3), "clamped to the last bar");
        assert_eq!(
            deck.leaving.len(),
            2,
            "each jump fades the old position out"
        );
        deck.track.as_mut().unwrap().cursor = None;
        assert_eq!(deck.seek(1, rate), Some(1));
        assert_eq!(
            deck.track.as_ref().unwrap().start_at,
            Some(1),
            "kept for when the take is ready"
        );
    }

    #[test]
    fn a_take_that_does_not_loop_finishes() {
        let mut deck = Deck {
            track: Some(Track {
                id: 7,
                looping: false,
                cursor: Some(Cursor {
                    take: take(3, 2, 1, |_| 0.2),
                    position: 0,
                    lap: 0,
                    looping: false,
                }),
                pending: None,
                fade_in: (0, 0),
                start_at: None,
            }),
            ..Deck::default()
        };
        for _ in 0..3 {
            deck.next(1000);
        }
        assert!(deck.track.is_none());
        assert_eq!(deck.finished, Some(7));
    }
}
