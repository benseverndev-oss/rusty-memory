//! `rmem`, a command line over rusty-memory.
//!
//! # Data out, text elsewhere
//!
//! Every command returns an [`Outcome`](command::Outcome), never a string.
//! Rendering lives in [`mod@format`], so a command can be tested as an ordinary
//! function against a stub provider — no process to spawn, no output to
//! scrape, and no network.

pub mod args;
pub mod command;
pub mod config;
pub mod format;
pub mod store;
#[cfg(test)]
pub mod testing;

/// Something the user has to fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliError {
    /// The config file could not be read or made sense of.
    Config(String),
    /// The config names an environment variable that is not set.
    MissingKey(String),
    /// A store file could not be read or written.
    Store(String),
    /// The engine, the extractor or a provider refused. Carries their words.
    Refused(String),
    /// The command line did not parse. Carries the usage text.
    Usage(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Config(why) => write!(f, "{why}"),
            CliError::MissingKey(var) => write!(
                f,
                "the environment variable {var} is not set, and rmem.toml names it as where the API key lives -- set it, or point api_key_env at the variable you use"
            ),
            CliError::Store(why) => write!(f, "{why}"),
            CliError::Refused(why) => write!(f, "{why}"),
            CliError::Usage(usage) => write!(f, "{usage}"),
        }
    }
}

impl std::error::Error for CliError {}
