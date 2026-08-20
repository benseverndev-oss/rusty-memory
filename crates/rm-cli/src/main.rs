//! `rmem`.
//!
//! Run, print, exit. Everything with a decision in it lives in the library,
//! where it is tested -- dispatch included, since which commands need a
//! provider is a decision and it was wrong here once.

use std::path::Path;
use std::process::ExitCode;

use rm_cli::format::render;
use rm_cli::run::{exit_code, run};

const CONFIG: &str = "rmem.toml";

fn main() -> ExitCode {
    // A wall clock reading, used for both time axes. Nothing below reads a
    // clock of its own, deliberately, so it is taken once here and passed in.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let result = run(std::env::args().skip(1), Path::new(CONFIG), now);
    match &result {
        Ok(outcome) => println!("{}", render(outcome)),
        // The library's own words. Every refusal in this workspace names
        // what was missing, and wrapping that in "error: failed" would
        // discard the one part that took effort to write.
        Err(e) => eprintln!("{e}"),
    }
    ExitCode::from(exit_code(&result))
}
