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
//! method. The tests below pin its two observable properties — a trailing
//! slash on the base URL doesn't double up, and a failure to connect never
//! carries the API key — by dialing a port nothing listens on, which is a
//! transport failure without a socket that leaves the machine.
//!
//! The alternative was a `TcpListener` in the test suite — a hand-rolled HTTP
//! server, port allocation, and a class of flakiness this workspace does not
//! have. What a real success response looks like is still uncovered here;
//! that surface is small, boring, and fails loudly.

mod wire;

use rm_engine::{Embedder, EmbedderError};
use rm_extract::{Completer, CompleterError};

use wire::{completion_body, embedding_body, parse_completion, parse_embedding};

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

/// A provider reached over HTTP.
///
/// Blocking, deliberately. A CLI makes one request at a time and gains nothing
/// from an async runtime but a dependency tree and a colour to every function.
pub struct HttpProvider {
    base_url: String,
    api_key: String,
    completion_model: String,
    embedding_model: String,
}

impl HttpProvider {
    pub fn new(
        base_url: String,
        api_key: String,
        completion_model: String,
        embedding_model: String,
    ) -> Self {
        HttpProvider {
            // Stored without the trailing slash so `url` can join with exactly
            // one. A config file will contain both spellings.
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            completion_model,
            embedding_model,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base_url)
    }

    /// POST `body` to `path` and return the response text.
    ///
    /// The error never carries the key. It goes to a terminal, a log, and
    /// possibly an issue tracker, and a key that reaches any of those has to be
    /// rotated.
    fn post(&self, path: &str, body: String) -> Result<String, ProviderError> {
        let response = ureq::post(&self.url(path))
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", "application/json")
            .send_string(&body);

        match response {
            Ok(r) => r
                .into_string()
                .map_err(|e| ProviderError::Transport(e.to_string())),
            // A non-2xx with a body is the provider explaining itself, and the
            // body says more than the status line does.
            Err(ureq::Error::Status(_, r)) => r
                .into_string()
                .map_err(|e| ProviderError::Transport(e.to_string())),
            Err(ureq::Error::Transport(t)) => Err(ProviderError::Transport(t.to_string())),
        }
    }

    /// The length of a vector this provider's embedding model produces.
    ///
    /// `VectorIndex::new` needs a dimension up front and it is a property of the
    /// model, so asking a human to write it down invites a config where the two
    /// disagree — which makes every distance meaningless without erroring.
    pub fn probe_dimension(&self) -> Result<usize, ProviderError> {
        Ok(self.embed_inner("dimension probe")?.len())
    }

    fn embed_inner(&self, text: &str) -> Result<Vec<f32>, ProviderError> {
        let body = self.post("embeddings", embedding_body(&self.embedding_model, text))?;
        parse_embedding(&body)
    }
}

impl Completer for HttpProvider {
    fn complete(&self, prompt: &str) -> Result<String, CompleterError> {
        let body = self
            .post(
                "chat/completions",
                completion_body(&self.completion_model, prompt),
            )
            .map_err(|e| CompleterError(e.to_string()))?;
        parse_completion(&body).map_err(|e| CompleterError(e.to_string()))
    }
}

impl Embedder for HttpProvider {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedderError> {
        self.embed_inner(text)
            .map_err(|e| EmbedderError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> HttpProvider {
        // Port 1 on the loopback address has nothing listening on it, so the
        // connection is refused immediately by the local kernel: no DNS
        // lookup, no packet leaving the machine, no dependence on how the
        // network's resolver handles an unregistered name. That refusal is
        // still a transport failure, which is all these tests need.
        HttpProvider::new(
            "http://127.0.0.1:1".to_string(),
            "sk-secret-do-not-print".to_string(),
            "gpt-4o-mini".to_string(),
            "text-embedding-3-small".to_string(),
        )
    }

    #[test]
    fn a_trailing_slash_on_the_base_url_does_not_double_up() {
        // "https://host/v1/" and "https://host/v1" must reach the same place; a
        // config file will contain both, and a doubled slash 404s with a message
        // about the URL rather than about the trailing slash.
        let with = HttpProvider::new("https://h/v1/".into(), "k".into(), "c".into(), "e".into());
        let without = HttpProvider::new("https://h/v1".into(), "k".into(), "c".into(), "e".into());
        assert_eq!(
            with.url("chat/completions"),
            without.url("chat/completions")
        );
        assert_eq!(
            with.url("chat/completions"),
            "https://h/v1/chat/completions"
        );
    }

    #[test]
    fn a_transport_failure_never_carries_the_api_key() {
        // The error goes to a terminal, a log, and quite possibly an issue
        // tracker. A key that reaches any of those is a key that must be
        // rotated.
        let err = provider().embed("anything").unwrap_err();
        assert!(
            !err.0.contains("sk-secret-do-not-print"),
            "an error must never carry the key: {}",
            err.0
        );
    }

    #[test]
    fn a_transport_failure_says_it_could_not_reach_the_provider() {
        // 127.0.0.1:1 refuses the connection locally: no listener owns that
        // port, so the OS answers immediately without a DNS lookup or a packet
        // leaving the machine. That keeps this test — the one here that
        // exercises the transport rather than the wire parsing — offline.
        let err = provider().embed("anything").unwrap_err();
        assert!(
            err.0.contains("could not reach the provider"),
            "expected a transport error, got: {}",
            err.0
        );
    }
}
