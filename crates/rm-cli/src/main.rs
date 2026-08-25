//! `rmem`.
//!
//! Run, print, exit. Everything with a decision in it lives in the library,
//! where it is tested -- dispatch included, since which commands need a
//! provider is a decision and it was wrong here once.

use std::process::ExitCode;

use rm_cli::format::render;
use rm_cli::run::{exit_code, run};

const CONFIG: &str = "rmem.toml";

/// An environment variable naming the config to use instead of `./rmem.toml`.
///
/// Several agents sharing one store is the point, and each of them runs in its
/// own directory. Without this every project would need its own `rmem.toml`
/// pointing at the same store, and one of them would eventually point somewhere
/// else -- a divergence nothing reports, because two stores are not an error.
const CONFIG_ENV: &str = "RMEM_CONFIG";

/// Where this session stands, for deciding what applies to it.
///
/// Read-side only. It is never a write default: reach varies per decision, and
/// the caller is the only one who knows it.
const SCOPE_ENV: &str = "RMEM_SCOPE";

/// The config this process should read.
fn config_path() -> std::path::PathBuf {
    std::env::var_os(CONFIG_ENV)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(CONFIG))
}

fn main() -> ExitCode {
    // A wall clock reading, used for both time axes. Nothing below reads a
    // clock of its own, deliberately, so it is taken once here and passed in.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let result = run(
        std::env::args().skip(1),
        config_path().as_path(),
        now,
        std::env::var(SCOPE_ENV).ok(),
    );
    match &result {
        Ok(outcome) => println!("{}", render(outcome)),
        // The library's own words. Every refusal in this workspace names
        // what was missing, and wrapping that in "error: failed" would
        // discard the one part that took effort to write.
        Err(e) => eprintln!("{e}"),
    }
    ExitCode::from(exit_code(&result))
}
