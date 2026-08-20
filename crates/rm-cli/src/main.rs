//! `rmem`.
//!
//! Parse, run, print, exit. Everything with a decision in it lives in the
//! library, where it is tested.

use std::path::Path;
use std::process::ExitCode;

use rm_cli::args::{parse, Command};
use rm_cli::command::{self, Outcome};
use rm_cli::config::Config;
use rm_cli::format::render;
use rm_cli::{store, CliError};

const CONFIG: &str = "rmem.toml";

fn main() -> ExitCode {
    match run() {
        Ok(outcome) => {
            println!("{}", render(&outcome));
            ExitCode::SUCCESS
        }
        Err(e) => {
            // The library's own words. Every refusal in this workspace names
            // what was missing, and wrapping that in "error: failed" would
            // discard the one part that took effort to write.
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<Outcome, CliError> {
    let command = parse(std::env::args().skip(1))?;
    let path = Path::new(CONFIG);

    if let Command::Init { force } = command {
        // Loaded here rather than by `init`, because init is what a user runs
        // when there is no config -- so only the provider block has to be
        // readable, and it comes from the template if the file is absent.
        // `?` here matters: a file that exists and fails to parse must
        // surface that, not be treated as if it were absent and silently
        // replaced by the template's defaults.
        let config = Config::load_or_template(path)?;
        let provider = config.provider()?;
        return command::init(path, force, &|| {
            provider.probe_dimension().map_err(|e| e.to_string())
        });
    }

    let config = Config::load(path)?;
    let provider = config.provider()?;
    let mut engine = store::load(
        &config.store.path,
        config.ruleset()?,
        config.policy_for_engine()?,
        config.provider.dimension,
        config.metric()?,
    )?;

    // A wall clock reading, used for both time axes. The engine takes no clock
    // of its own, deliberately, so the caller supplies one.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let (outcome, mutated) = match command {
        Command::Init { .. } => unreachable!("handled above"),
        Command::Remember { text } => (
            command::remember(&mut engine, &text, now, "cli", &provider, &provider)?,
            true,
        ),
        Command::Recall { query, k } => (command::recall(&engine, &query, k, &provider)?, false),
        Command::About { entity, attribute } => (
            command::about(&engine, entity, &attribute, now, now)?,
            false,
        ),
        Command::ReviewList => (command::review_list(&engine)?, false),
        Command::ReviewConfirm(id) => (command::review_confirm(&mut engine, id)?, true),
        Command::ReviewReject(id) => (command::review_reject(&mut engine, id)?, true),
    };

    if mutated {
        store::save(&config.store.path, &engine)?;
    }
    Ok(outcome)
}
