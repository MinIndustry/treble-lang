//! The Treble command line.
//!
//! `check` reads a buffer and reports what it found; `render` turns a piece
//! into a WAV. Rendering is behind the `render` feature, because the language
//! itself has no engine dependency and a checker should not need one.

// Only the renderer deals in paths; `check` and `info` take them as strings.
#[cfg(feature = "render")]
use std::path::{Path, PathBuf};

const USAGE: &str = "\
treble — the Treble language toolchain

USAGE:
    treble check <file.rt>
    treble info  <file.rt>
    treble render <file.rt> [-o <out.wav>] [options]

COMMANDS:
    check     Parse a buffer and report errors. Exit 1 if any.
    info      Print a piece's structure — sections, arrangement, metadata.
    render    Render a piece to a WAV file.

RENDER OPTIONS:
    -o, --out <path>     Output file. Defaults to the input with a .wav suffix.
    -r, --rate <hz>      Sample rate. Default 44100.
    -m, --meta <k=v>     Add or override a metadata tag. Repeatable.
    -f, --force          Overwrite the output if it exists.
    -q, --quiet          Only report errors.
        --json           Emit a machine-readable summary on stdout.

Every option has a long form; `--` stops option parsing.
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        print!("{USAGE}");
        std::process::exit(if args.is_empty() { 1 } else { 0 });
    }
    if args[0] == "-V" || args[0] == "--version" {
        println!("treble-lang {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let (command, rest) = (args[0].as_str(), &args[1..]);
    let result = match command {
        "check" => cmd_check(rest),
        "info" => cmd_info(rest),
        "render" => cmd_render(rest),
        // A bare path keeps the older `treble-lang <file>` spelling working.
        other if !other.starts_with('-') => cmd_check(&args),
        other => Err(format!("unknown command '{other}'\n\n{USAGE}")),
    };
    if let Err(message) = result {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| format!("could not read '{path}': {error}"))
}

/// Parse and report, without resolving anything into audio.
fn cmd_check(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("check: expected a file")?;
    let source = read(path)?;
    let mut session = treble_lang::Session::new();
    let result = session.evaluate(&source);
    for error in &result.errors {
        eprintln!("{path}:{}: {}", error.location.line, error.message);
    }
    if !result.errors.is_empty() {
        return Err(format!(
            "{} problem{} in {path}",
            result.errors.len(),
            if result.errors.len() == 1 { "" } else { "s" }
        ));
    }
    // `load` lines resolve beside the file being checked, so a broken path
    // or a loaded file that sounds is found now rather than at render time.
    let loaded = match loaded_definitions(&session, Path::new(path)) {
        Ok(definitions) => definitions,
        Err(problem) => {
            eprintln!("{path}: {problem}");
            return Err(format!("a load line would not resolve in {path}"));
        }
    };
    // A `def` that parses can still be rejected when it is lowered onto the
    // engine — "a tone takes either a `gain` level or its own envelope, not
    // both" is a parse-clean, render-fatal mistake. Checking it here is the
    // difference between finding it now and finding it at render time.
    let mut lowering = check_definitions(&session);
    lowering.extend(check_loaded(&loaded));
    lowering.sort();
    for problem in &lowering {
        eprintln!("{path}: {problem}");
    }
    if !lowering.is_empty() {
        return Err(format!(
            "{} instrument definition{} would not build",
            lowering.len(),
            if lowering.len() == 1 { "" } else { "s" }
        ));
    }

    match session.piece() {
        Some(piece) => println!(
            "{path}: piece — {} sections, {} occurrences, {} cycles, {}",
            piece.sections.len(),
            piece.timeline.len(),
            piece.total_cycles(),
            clock(piece.render_seconds())
        ),
        None => println!(
            "{path}: live buffer — {} active, {} muted",
            result.patterns_active, result.patterns_muted
        ),
    }
    Ok(())
}

/// Print the structure the score editor draws, for a terminal.
fn cmd_info(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("info: expected a file")?;
    let source = read(path)?;
    let mut session = treble_lang::Session::new();
    let result = session.evaluate(&source);
    for error in &result.errors {
        eprintln!("{path}:{}: {}", error.location.line, error.message);
    }
    let piece = session
        .piece()
        .ok_or("that buffer has no sections, so there is no piece to describe")?;

    if !piece.metadata.is_empty() {
        for (key, value) in &piece.metadata {
            println!("{key:>12}  {value}");
        }
        println!();
    }
    println!(
        "{:>12}  {} sections, {} played, {} cycles",
        "structure",
        piece.sections.len(),
        piece.timeline.len(),
        piece.total_cycles()
    );
    println!(
        "{:>12}  {} + {:.1}s tail = {}",
        "length",
        clock(piece.seconds()),
        piece.tail,
        clock(piece.render_seconds())
    );
    println!("{:>12}  {}", "seed", piece.seed);
    println!();

    for section in &piece.sections {
        println!(
            "  {:<12} {:>3} cycles  {:>3} bpm  {}/{}  {:>6.1}s  ×{}",
            section.name,
            section.cycles,
            section.bpm,
            section.sig.0,
            section.sig.1,
            section.seconds(),
            piece
                .timeline
                .iter()
                .filter(|o| piece.sections[o.section].name == section.name)
                .count()
        );
        for line in &section.patterns {
            let span = match line.span {
                Some(span) if span.start() == span.end(section.cycles) => {
                    format!("@ {}", span.start())
                }
                Some(span) => format!("@ {}..{}", span.start(), span.end(section.cycles)),
                None => String::new(),
            };
            println!("      {:<10} {:<10} {}", line.name, line.instrument, span);
        }
    }
    if !piece.unused.is_empty() {
        println!("\n  never played: {}", piece.unused.join(", "));
    }
    Ok(())
}

/// Resolve the buffer's `load` lines into instrument definitions (§2.5).
///
/// The spec leaves resolution to the consumer. This consumer is a command
/// line with a file argument, so a relative path resolves beside that file
/// and nowhere else; an absolute path is used as written. A loaded file
/// holds `def` blocks, comments and blank lines only. Registration order
/// carries the precedence: earlier `load` lines first, buffer definitions
/// last, each later registration overwriting the name before it.
#[cfg(feature = "render")]
fn loaded_definitions(
    session: &treble_lang::Session,
    piece_path: &Path,
) -> Result<Vec<treble_lang::ast::InstrumentDef>, String> {
    let base = piece_path.parent().unwrap_or(Path::new("."));
    let mut definitions = Vec::new();
    for load in session.loads() {
        let path = Path::new(load);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        };
        let source = std::fs::read_to_string(&resolved)
            .map_err(|error| format!("load \"{load}\": {error}"))?;
        let (program, errors) = treble_lang::parser::parse_program(&source);
        if let Some(error) = errors.first() {
            return Err(format!(
                "load \"{load}\": line {}: {}",
                error.location.line, error.message
            ));
        }
        for line in program.lines {
            match line {
                treble_lang::SourceLine::Def(definition) => definitions.push(*definition),
                treble_lang::SourceLine::Comment(_) | treble_lang::SourceLine::Blank => {}
                _ => {
                    return Err(format!(
                        "load \"{load}\": a loaded file holds `def` blocks only — \
                         a line that sounds or configures belongs in the buffer"
                    ));
                }
            }
        }
    }
    Ok(definitions)
}

/// Problems a `def` block would hit when lowered onto the engine.
///
/// Only possible with the renderer available; without it the checker says what
/// it can and stays quiet about the rest rather than pretending to have looked.
#[cfg(feature = "render")]
fn check_definitions(session: &treble_lang::Session) -> Vec<String> {
    let cycle = session
        .piece()
        .and_then(|piece| piece.sections.first().map(|s| s.cycle_seconds()))
        .unwrap_or(2.0);
    let mut problems: Vec<String> = session
        .definitions()
        .values()
        .filter_map(|definition| {
            treble_lang::render::compile::lower_instrument_def(definition, cycle)
                .err()
                .map(|error| format!("def {}: {error}", definition.name))
        })
        .collect();
    // Definitions live in a map, so sort for a stable report.
    problems.sort();
    problems
}

#[cfg(feature = "render")]
fn check_loaded(definitions: &[treble_lang::ast::InstrumentDef]) -> Vec<String> {
    definitions
        .iter()
        .filter_map(|definition| {
            treble_lang::render::compile::lower_instrument_def(definition, 2.0)
                .err()
                .map(|error| format!("def {}: {error}", definition.name))
        })
        .collect()
}

#[cfg(not(feature = "render"))]
fn check_loaded(_definitions: &[treble_lang::ast::InstrumentDef]) -> Vec<String> {
    Vec::new()
}

#[cfg(not(feature = "render"))]
fn check_definitions(_session: &treble_lang::Session) -> Vec<String> {
    Vec::new()
}

#[cfg(not(feature = "render"))]
fn loaded_definitions(
    _session: &treble_lang::Session,
    _piece_path: &Path,
) -> Result<Vec<treble_lang::ast::InstrumentDef>, String> {
    Ok(Vec::new())
}

fn clock(seconds: f64) -> String {
    let whole = seconds.max(0.0) as u64;
    format!("{}:{:02}", whole / 60, whole % 60)
}

#[cfg(not(feature = "render"))]
fn cmd_render(_args: &[String]) -> Result<(), String> {
    Err("this build has no renderer — rebuild with `--features render`".into())
}

#[cfg(feature = "render")]
fn cmd_render(args: &[String]) -> Result<(), String> {
    use std::io::Write;
    use treble_lang::render::{Progress, render_with_progress, wav};

    let mut input: Option<String> = None;
    let mut out: Option<PathBuf> = None;
    let mut rate: u32 = 44_100;
    let mut extra: Vec<(String, String)> = Vec::new();
    let (mut force, mut quiet, mut json) = (false, false, false);

    let mut i = 0;
    let mut only_positional = false;
    while i < args.len() {
        let arg = args[i].as_str();
        let mut take = |name: &str| -> Result<String, String> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{name}: expected a value"))
        };
        match arg {
            _ if only_positional => input = Some(arg.to_string()),
            "--" => only_positional = true,
            "-o" | "--out" => out = Some(PathBuf::from(take("--out")?)),
            "-r" | "--rate" => {
                let value = take("--rate")?;
                rate = value
                    .parse()
                    .map_err(|_| format!("--rate: expected a sample rate, got '{value}'"))?;
            }
            "-m" | "--meta" => {
                let value = take("--meta")?;
                let (key, text) = value
                    .split_once('=')
                    .ok_or_else(|| format!("--meta: expected key=value, got '{value}'"))?;
                extra.push((key.to_ascii_lowercase(), text.to_string()));
            }
            "-f" | "--force" => force = true,
            "-q" | "--quiet" => quiet = true,
            "--json" => json = true,
            other if other.starts_with('-') => {
                return Err(format!("render: unknown option '{other}'"));
            }
            other => input = Some(other.to_string()),
        }
        i += 1;
    }

    let input = input.ok_or("render: expected a file")?;
    let source = read(&input)?;
    let out = out.unwrap_or_else(|| Path::new(&input).with_extension("wav"));

    let mut session = treble_lang::Session::new();
    let result = session.evaluate(&source);
    for error in &result.errors {
        eprintln!("{input}:{}: {}", error.location.line, error.message);
    }
    if !result.errors.is_empty() {
        return Err(format!("{} problem(s) in {input}", result.errors.len()));
    }
    let mut piece = session
        .piece()
        .ok_or(
            "that buffer has no sections, so there is nothing to render — add a `section` block",
        )?
        .clone();
    // Command-line tags win over the file's own, so a build can stamp a date
    // or a take number without editing the piece.
    for (key, value) in extra {
        piece.metadata.retain(|(name, _)| name != &key);
        piece.metadata.push((key, value));
    }

    // `def` blocks in the buffer are lowered into the registry the renderer
    // reads, so a piece can carry its own instruments.
    let mut registry = treble::instruments::prelude::InstrumentRegistry::built_in();
    // A definition's own cycle-relative rates resolve against the opening
    // tempo. Sections may differ, but an instrument is defined once for the
    // whole piece, so there is one cycle length to read it against.
    let cycle = piece
        .sections
        .first()
        .map_or(2.0, |section| section.cycle_seconds());
    for definition in &loaded_definitions(&session, Path::new(&input))? {
        let spec = treble_lang::render::compile::lower_instrument_def(definition, cycle)
            .map_err(|error| format!("def {}: {error}", definition.name))?;
        registry
            .register(spec)
            .map_err(|error| format!("def {}: {error}", definition.name))?;
    }
    for definition in session.definitions().values() {
        let spec = treble_lang::render::compile::lower_instrument_def(definition, cycle)
            .map_err(|error| format!("def {}: {error}", definition.name))?;
        registry
            .register(spec)
            .map_err(|error| format!("def {}: {error}", definition.name))?;
    }

    if !quiet {
        eprintln!(
            "{}  {} sections, {} occurrences, {} cycles, {}",
            piece.meta("title").unwrap_or(&input),
            piece.sections.len(),
            piece.timeline.len(),
            piece.total_cycles(),
            clock(piece.render_seconds())
        );
    }

    let started = std::time::Instant::now();
    let mut last_line = String::new();
    let mut report = |progress: Progress| {
        if quiet {
            return;
        }
        let line = match progress {
            Progress::Compiling { done, total } => {
                format!("compiling  {}", bar(done as f64 / total.max(1) as f64))
            }
            Progress::Building { slots } => format!("building   {slots} voices"),
            Progress::Rendering {
                frames,
                total_frames,
            } => format!(
                "rendering  {}  {}",
                bar(frames as f64 / total_frames.max(1) as f64),
                clock(frames as f64 / f64::from(rate))
            ),
            Progress::Done { .. } => "rendering  done".to_string(),
        };
        if line != last_line {
            eprint!("\r\x1b[2K{line}");
            let _ = std::io::stderr().flush();
            last_line = line;
        }
    };

    let rendered = render_with_progress(&piece, &registry, rate, &mut report)?;
    if !quiet {
        eprint!("\r\x1b[2K");
    }
    wav::write(&out, &rendered, &piece, force)?;
    let elapsed = started.elapsed().as_secs_f64();

    if json {
        println!(
            "{{\"path\":{:?},\"seconds\":{:.3},\"renderedSeconds\":{:.3},\"sampleRate\":{},\
             \"sections\":{},\"occurrences\":{},\"cycles\":{},\"notes\":{},\"elapsed\":{:.3},\
             \"islands\":{},\"slots\":{},\"workers\":{},\"preLimiterPeak\":{:.6},\
             \"postLimiterPeak\":{:.6},\"maxGainReductionDb\":{:.3},\"limitedSamples\":{}}}",
            out.display().to_string(),
            rendered.seconds,
            rendered.rendered_seconds,
            rendered.sample_rate,
            piece.sections.len(),
            rendered.occurrences,
            piece.total_cycles(),
            rendered.notes,
            elapsed,
            rendered.telemetry.islands,
            rendered.telemetry.slots,
            rendered.telemetry.worker_threads,
            rendered.telemetry.pre_limiter_peak,
            rendered.telemetry.post_limiter_peak,
            rendered.telemetry.max_gain_reduction_db,
            rendered.telemetry.limited_samples
        );
    } else if !quiet {
        eprintln!(
            "wrote {}  ({}, {} notes, {:.1}× realtime, {} islands on {} workers)",
            out.display(),
            clock(rendered.rendered_seconds),
            rendered.notes,
            rendered.rendered_seconds / elapsed.max(1e-9),
            rendered.telemetry.islands,
            rendered.telemetry.worker_threads
        );
        if rendered.telemetry.limited_samples > 0 {
            eprintln!(
                "  master: pre-limit {:.3}, peak {:.3}, max reduction {:.1} dB, {} limited samples",
                rendered.telemetry.pre_limiter_peak,
                rendered.telemetry.post_limiter_peak,
                rendered.telemetry.max_gain_reduction_db,
                rendered.telemetry.limited_samples
            );
        }
        for name in &rendered.unused {
            eprintln!("  note: section '{name}' is never played");
        }
    }
    Ok(())
}

/// A 24-cell progress bar. Written with block characters so it reads at a
/// glance without needing a terminal library.
#[cfg(feature = "render")]
fn bar(fraction: f64) -> String {
    const WIDTH: usize = 24;
    let filled = ((fraction.clamp(0.0, 1.0)) * WIDTH as f64).round() as usize;
    format!(
        "[{}{}] {:>3.0}%",
        "█".repeat(filled),
        "·".repeat(WIDTH - filled),
        fraction.clamp(0.0, 1.0) * 100.0
    )
}

#[cfg(all(test, feature = "render"))]
mod load_resolution_tests {
    use super::*;

    /// `load` resolves beside the piece file, accepts a defs-only file, and
    /// refuses one that sounds — the CLI half of §2.5.
    #[test]
    fn load_lines_resolve_beside_the_piece_file() {
        let dir = std::env::temp_dir().join(format!("treble-load-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(
            dir.join("sounds.trbl"),
            "-- a comment\ndef lamp {\n    tone sine gain 0.5 identity\n    env adsr 0.01 0.1 0.5 0.1\n}\n",
        )
        .expect("write defs");
        let piece = dir.join("piece.rt");
        std::fs::write(
            &piece,
            "bpm 120\nload \"sounds.trbl\"\nsection s 1 {\n  l lamp \"0\"\n}\n",
        )
        .expect("write piece");

        let mut session = treble_lang::Session::new();
        session.evaluate(&std::fs::read_to_string(&piece).unwrap());
        let definitions = loaded_definitions(&session, &piece).expect("resolve");
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].name, "lamp");

        std::fs::write(dir.join("sounds.trbl"), "bpm 99\n").expect("rewrite");
        let error = loaded_definitions(&session, &piece).expect_err("a directive must be refused");
        assert!(error.contains("def` blocks only"), "{error}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
