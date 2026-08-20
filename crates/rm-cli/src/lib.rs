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
pub mod run;
pub mod store;
#[cfg(test)]
pub mod testing;

/// Something the user has to fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliError {
    /// The config file could not be read or made sense of.
    Config(String),
    /// The config names an environment variable that is not set.
    ///
    /// Carries nothing, deliberately. The variable's name comes out of
    /// `rmem.toml`, and the likeliest way to get that file wrong is to write
    /// the key where the variable's name belongs — so the name is exactly the
    /// thing that may be a secret. A payload would leak through this enum's
    /// `Debug` even if `Display` were careful.
    MissingKey,
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
            // Names the field, not its value. Interpolating the variable's
            // name printed a key whenever someone had written the key there
            // instead of a variable name -- the single likeliest mistake this
            // file invites, since the field is called `api_key_env` and a key
            // is what you have in your hand. Refusing to guess which values
            // are keys is what put six leaks on this branch; the rule that
            // replaced it is that an error names a field, a location or a key
            // of the config, never a value read out of it. A user who wrote
            // the value can read it back out of their own file.
            CliError::MissingKey => write!(
                f,
                "rmem.toml's api_key_env names an environment variable, and that variable is not set -- set it, or point api_key_env at the variable you use. The name is not repeated here: if what is written there is the key itself rather than a variable name, printing it would put it in your terminal and your scrollback."
            ),
            CliError::Store(why) => write!(f, "{why}"),
            CliError::Refused(why) => write!(f, "{why}"),
            CliError::Usage(usage) => write!(f, "{usage}"),
        }
    }
}

impl std::error::Error for CliError {}
