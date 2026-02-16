//! CLI entry point for rustic-lang.
//!
//! Provides a simple way to check .rt files from the command line.

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: rustic-lang <file.rt>");
        eprintln!("       rustic-lang --check <source>");
        std::process::exit(1);
    }

    if args[1] == "--check" {
        let source = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let mut session = rustic_lang::Session::new();
        let result = session.evaluate(source);
        for err in &result.errors {
            eprintln!("{}", err);
        }
        println!(
            "{} active, {} muted, {} changes",
            result.patterns_active,
            result.patterns_muted,
            result.deltas.len()
        );
        if !result.errors.is_empty() {
            std::process::exit(1);
        }
    } else {
        let path = &args[1];
        match std::fs::read_to_string(path) {
            Ok(source) => {
                let mut session = rustic_lang::Session::new();
                let result = session.evaluate(&source);
                for err in &result.errors {
                    eprintln!("{}", err);
                }
                println!(
                    "{} active, {} muted, {} changes",
                    result.patterns_active,
                    result.patterns_muted,
                    result.deltas.len()
                );
            }
            Err(e) => {
                eprintln!("Error reading {}: {}", path, e);
                std::process::exit(1);
            }
        }
    }
}
