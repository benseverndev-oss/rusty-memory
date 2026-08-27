//! What a host of rusty-memory has to decide, and nothing about how it is
//! driven.
//!
//! Three questions come due the moment anything wants to *run* this library
//! rather than link it: where a store lives and how it is written safely,
//! where an engine's configuration comes from, and who actually calls a model.
//! `rm-cli` answered all three, and every one of the answers turned out to be
//! about hosting rather than about a terminal.
//!
//! So they live here, and the binaries keep only what is theirs. `rmem` keeps
//! argument parsing and text rendering; `rmem-mcp` keeps a protocol. Both get
//! the same [`config`], [`store`] and [`command`], which is the difference
//! between one host and two that drift.
//!
//! # Data out, text elsewhere
//!
//! Every command returns an [`Outcome`](command::Outcome), never a string.
//! That was written to make the commands testable without scraping output, and
//! it is what lets a second consumer render the same result as JSON without
//! reaching for a parser.

pub mod attribution;
pub mod command;
pub mod config;
pub mod ingest;
pub mod scope;
pub mod store;
pub mod testing;
pub mod time;

/// Something the user has to fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostError {
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
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::Config(why) => write!(f, "{why}"),
            // Names the field, not its value. Interpolating the variable's
            // name printed a key whenever someone had written the key there
            // instead of a variable name -- the single likeliest mistake this
            // file invites, since the field is called `api_key_env` and a key
            // is what you have in your hand. Refusing to guess which values
            // are keys is what put six leaks on this branch; the rule that
            // replaced it is that an error names a field, a location or a key
            // of the config, never a value read out of it. A user who wrote
            // the value can read it back out of their own file.
            HostError::MissingKey => write!(
                f,
                "rmem.toml's api_key_env names an environment variable, and that variable is not set -- set it, or point api_key_env at the variable you use. The name is not repeated here: if what is written there is the key itself rather than a variable name, printing it would put it in your terminal and your scrollback."
            ),
            HostError::Store(why) => write!(f, "{why}"),
            HostError::Refused(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for HostError {}
