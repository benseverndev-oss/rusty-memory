//! `Completer` and `Embedder` over an OpenAI-compatible API.
//!
//! # The only crate here that opens a socket
//!
//! Every library crate in this workspace depends on `serde` and nothing else,
//! and none of them can reach the network. That is deliberate: `Completer` and
//! `Embedder` are ports, so the thing that needs a remote service asks for one
//! rather than reaching for it.
//!
//! Something still has to make the request. This crate is that something, and
//! it is a crate rather than a module inside `rm-cli` because an MCP server
//! will need the same two implementations — putting them in a binary would
//! leave the server duplicating them or depending on a binary crate.
//!
//! # Why almost all of it is testable offline
//!
//! [`wire`] holds the request bodies and the response parsing as pure
//! functions, and those carry the behaviour worth testing: what a prompt with
//! quotes and newlines in it becomes, what an error response means, what an
//! empty one means. What is left in this module is a few lines of transport per
//! method, which no test covers.
//!
//! The alternative was a `TcpListener` in the test suite — a hand-rolled HTTP
//! server, port allocation, and a class of flakiness this workspace does not
//! have. The uncovered surface is small, boring, and fails loudly.

mod wire;

/// Something went wrong reaching a provider, or in what it sent back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    /// The request never completed. Connection refused, DNS, TLS, timeout.
    Transport(String),
    /// The provider answered with an error, carrying its own message.
    Api(String),
    /// The response was not the JSON this crate expects.
    Unparsable(String),
    /// The response parsed and contained nothing to use. Names which part.
    Empty(&'static str),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::Transport(why) => write!(f, "could not reach the provider: {why}"),
            ProviderError::Api(why) => write!(f, "the provider refused the request: {why}"),
            ProviderError::Unparsable(why) => write!(
                f,
                "the provider's response was not the JSON this crate expects: {why}"
            ),
            ProviderError::Empty(what) => write!(
                f,
                "the provider's response parsed but {what}, so there is nothing to use"
            ),
        }
    }
}

impl std::error::Error for ProviderError {}
