//! `rmem`, a command line over rusty-memory.
//!
//! # What is here and what is not
//!
//! Only what is about a terminal: parsing a command line, rendering an
//! [`Outcome`](rm_host::command::Outcome) as text, and the dispatch between
//! them. Everything a *host* has to decide — the config file, the store file,
//! and the operations over them — is [`rm_host`], which `rmem-mcp` hosts the
//! same library through.
//!
//! # Data out, text elsewhere
//!
//! Every command returns an `Outcome`, never a string. Rendering lives in
//! [`mod@format`], so a command can be tested as an ordinary function against a
//! stub provider — no process to spawn, no output to scrape, and no network.

pub mod args;
pub mod format;
pub mod run;

use rm_host::HostError;

/// Something the user has to fix.
///
/// Four fifths of this was never about a command line, and now is not: a
/// missing config, an unset key, an unreadable store and a refusal from the
/// library are all things `rmem-mcp` meets too, so they live in [`HostError`]
/// and arrive here through [`From`]. What is left is the one failure mode a
/// server does not have, because a server has a schema instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliError {
    /// The config, the store, a provider or the library itself.
    Host(HostError),
    /// The command line did not parse. Carries the usage text.
    Usage(String),
}

impl From<HostError> for CliError {
    fn from(e: HostError) -> Self {
        CliError::Host(e)
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The host's own words, unwrapped. Every refusal in this workspace
            // names what was missing, and prefixing that with a layer's name
            // would push the part that took effort to write one indent to the
            // right of where a reader starts.
            CliError::Host(e) => write!(f, "{e}"),
            CliError::Usage(usage) => write!(f, "{usage}"),
        }
    }
}

impl std::error::Error for CliError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_key_still_names_the_field_and_not_the_variable() {
        // The message travelled to another crate and arrives here through two
        // `Display` impls. What it must not have picked up on the way is a
        // wrapper: `MissingKey` carries nothing precisely so that nothing it
        // prints can be a value out of `rmem.toml`, and a prefix naming the
        // layer would be the first thing between a reader and the sentence
        // that tells them what to do.
        let wrapped = CliError::from(HostError::MissingKey).to_string();
        assert_eq!(wrapped, HostError::MissingKey.to_string());
        assert!(wrapped.contains("api_key_env"), "{wrapped}");
    }
}
