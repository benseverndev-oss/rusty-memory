//! `Completer` and `Embedder` over an OpenAI-compatible API.
//!
//! # The only crate here that opens a socket
//!
//! Every library crate in this workspace draws its third-party dependencies
//! from `serde` and `serde_json` alone — `rm-graph` takes neither — and none
//! of them can reach the network. That is deliberate: `Completer` and
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
//! `wire`, private to this crate and so deliberately not linked here, holds
//! the request bodies and the response parsing as pure functions, and those
//! carry the behaviour worth testing: what a prompt with quotes and newlines
//! in it becomes, what an error response means, what an empty one means. What
//! is left in this module is a few lines of transport per method. The tests
//! below pin its two observable properties — a trailing
//! slash on the base URL doesn't double up, and a failure to connect never
//! carries the API key — by dialing a port nothing listens on, which is a
//! transport failure without a socket that leaves the machine.
//!
//! The alternative was a `TcpListener` in the test suite — a hand-rolled HTTP
//! server, port allocation, and a class of flakiness this workspace does not
//! have. What a real success response looks like is still uncovered here;
//! that surface is small, boring, and fails loudly.

pub mod network;
mod wire;

use rm_engine::{Embedder, EmbedderError};
use rm_extract::{Completer, CompleterError};

use network::Network;
use wire::{completion_body, embedding_body, parse_completion, parse_embedding};

/// # One thing this cannot promise
///
/// `Api`, `Unparsable` and `Empty` carry the provider's own words, on purpose:
/// a remote service explaining why it refused is the most useful thing this
/// crate can pass on, and inventing a substitute would throw it away. `redact`
/// scrubs the API key out of that text.
///
/// It cannot scrub `base_url`. A provider that echoes the request path back in
/// its error body — a 404 naming the route is ordinary — would have that
/// relayed, and `base_url` comes out of `rmem.toml`, so a credential pasted
/// *there* could return that way. Every other route for `base_url` is closed
/// (see `transport_failure`), and this one is not closed because the only ways
/// to close it are to match a value we hold against a rendering someone else
/// produced, which is what six leaks have now shown cannot be made airtight, or
/// to stop relaying provider messages at all, which is a worse product.
///
/// Recorded as a limitation rather than patched. It needs a user to have pasted
/// a key into `base_url` *and* a provider that echoes the path; `api_key_env`
/// is the field that invites the mistake, and it no longer prints anything.
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
    /// Built once, from the environment, in [`HttpProvider::new`].
    ///
    /// A `Result` in a field rather than a fallible constructor. Building it
    /// reads a file that may not be there and parses a proxy URL that may not
    /// be one, and both are worth refusing over -- but `new` is called from
    /// four places including a config loader that has no better answer than
    /// passing the error along, and every request already returns
    /// `Result<_, ProviderError>` where a transport problem is exactly what a
    /// caller is prepared for. So the failure is kept and returned by whichever
    /// request comes first. In practice that is `rmem init`'s dimension probe,
    /// which runs before anything is written.
    agent: Result<ureq::Agent, ProviderError>,
}

impl HttpProvider {
    pub fn new(
        base_url: String,
        api_key: String,
        completion_model: String,
        embedding_model: String,
    ) -> Self {
        // Stored without the trailing slash so `url` can join with exactly
        // one. A config file will contain both spellings.
        let base_url = base_url.trim_end_matches('/').to_string();
        let agent = Network::from_env(|name| std::env::var(name).ok()).agent(&base_url);

        HttpProvider {
            base_url,
            api_key,
            completion_model,
            embedding_model,
            agent,
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
        // Not `ureq::post`, which is a free function over a default agent that
        // reads no environment: no proxy, and the compiled-in Mozilla roots
        // whatever the machine is configured to trust.
        let agent = match &self.agent {
            Ok(agent) => agent,
            Err(e) => return Err(e.clone()),
        };

        let response = agent
            .post(&self.url(path))
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", "application/json")
            .send_string(&body);

        self.handle_response(response, path)
    }

    /// What went wrong reaching the provider, in this crate's own words.
    ///
    /// # Why `ureq`'s own message is not relayed
    ///
    /// It opens with the URL it was given, and that URL is built from
    /// `base_url` out of `rmem.toml` — a value a user can paste a credential
    /// into. The version before this one substituted the URL away, which was
    /// the same mistake in a different crate: it replaced the string this
    /// crate *built*, while `ureq` prints the string it *parsed*. Any
    /// normalisation and the match misses — `https://api.openai.com:443/v1`
    /// loses its default port, an uppercase scheme is lowercased, a missing
    /// path gains a slash. Seven of sixteen shapes a review tried leaked, and
    /// the test passed only because its own fixture was already normalised.
    ///
    /// Matching a value we hold against a rendering someone else produced
    /// cannot be made airtight, so nothing is matched. `ureq::ErrorKind` is a
    /// fieldless enum, so a value of it cannot carry a URL or anything else
    /// out of the file, and every arm below returns one of this function's own
    /// literals. `path` is this crate's own constant — `embeddings` or
    /// `chat/completions` — not anything read from the config.
    ///
    /// # What that costs
    ///
    /// `ureq`'s `message()` is dropped, and it sometimes carried the OS error
    /// behind a refused connection ("the target machine actively refused it").
    /// It cannot be kept selectively: it carries the hostname outright for
    /// `Dns` and the scheme for `UnknownScheme`. What a reader needs in order
    /// to act is the class of failure and which config field points at the
    /// endpoint, and both survive.
    fn transport_failure(kind: ureq::ErrorKind, path: &str) -> ProviderError {
        let why = match kind {
            ureq::ErrorKind::Dns => {
                "the host it names could not be resolved -- check the spelling, and that this machine has DNS"
            }
            // Not only a refused connection. `ureq` maps a failed TLS
            // handshake here too (`rtls.rs` turns `complete_io` failing into
            // `ConnectionFailed`), so an unknown CA, an expired certificate or
            // a corporate MITM proxy all arrive as this variant -- and those
            // are the common enterprise failures. Something did accept the
            // connection; the certificate was rejected. Saying only "check the
            // firewall" sends whoever hit that to the wrong place for an
            // afternoon, so the certificate case is named here rather than
            // guessed at from a message we do not read.
            ureq::ErrorKind::ConnectionFailed => {
                "the connection did not establish -- either nothing accepted it, so check the host, the port and any proxy or firewall in the way, or it was accepted and the TLS handshake failed, so check that the certificate is one this machine trusts (an unknown CA, an expired certificate, or a proxy substituting its own)"
            }
            ureq::ErrorKind::InvalidUrl | ureq::ErrorKind::UnknownScheme => {
                "it is not a URL this build can use -- it needs a scheme it knows, and a host"
            }
            ureq::ErrorKind::InsecureRequestHttpsOnly => {
                "it is an http:// URL and this build was configured to allow https:// only"
            }
            ureq::ErrorKind::TooManyRedirects => {
                "it redirected more times than this build will follow"
            }
            ureq::ErrorKind::BadStatus | ureq::ErrorKind::BadHeader => {
                "what answered did not speak HTTP this build understands -- check that it points at an OpenAI-compatible API rather than, say, a web page"
            }
            ureq::ErrorKind::InvalidProxyUrl => {
                "the proxy URL in this environment is not one this build can use"
            }
            ureq::ErrorKind::ProxyConnect => {
                "the proxy in this environment refused the connection"
            }
            ureq::ErrorKind::ProxyUnauthorized => {
                "the proxy in this environment rejected the credentials it was given"
            }
            // Reached only for a non-2xx, which `handle_response` takes
            // through its own arm before this is ever called.
            ureq::ErrorKind::HTTP => "the provider answered with an error",
            // Also not only what it sounds like: both the connect and read
            // timeouts land here, as does TLS session setup failing before the
            // handshake starts. "Failed part way through" is not what a
            // connect timeout is.
            ureq::ErrorKind::Io => {
                "the connection failed at the network layer -- it timed out, was reset, or TLS could not be set up at all"
            }
        };
        ProviderError::Transport(format!(
            "{path}, under the base_url named in rmem.toml: {why}"
        ))
    }

    /// Turn a `ureq` result into body text.
    ///
    /// The `Ok` body and the `Status` arm's body both reach `redact` below
    /// before this returns: those are response content, which is where a
    /// provider could echo the key back. The `Transport` arm never reaches it
    /// and does not need to — [`Self::transport_failure`] builds that message
    /// from a fieldless enum, so there is nothing in it to scrub.
    ///
    /// Both `into_string()` failure arms relay `std::io::Error`'s text. That
    /// is not a message built out of a value we hold: nothing from `rmem.toml`
    /// is passed to the reader, and the body itself is not in the error, only
    /// the reason it could not be decoded.
    ///
    /// Split out from `post` so the `Status` arm — reached only when a real
    /// server answers with a non-2xx — can be driven in a test with a
    /// response built in-process by `ureq::Response::new`, without a socket.
    fn handle_response(
        &self,
        response: Result<ureq::Response, ureq::Error>,
        path: &str,
    ) -> Result<String, ProviderError> {
        let undecodable = |e: std::io::Error| {
            ProviderError::Transport(format!(
                "the provider's response could not be read as text: {e}"
            ))
        };
        let text = match response {
            Ok(r) => r.into_string().map_err(undecodable),
            // A non-2xx with a body is the provider explaining itself, and the
            // body says more than the status line does.
            Err(ureq::Error::Status(_, r)) => r.into_string().map_err(undecodable),
            Err(ureq::Error::Transport(t)) => Err(Self::transport_failure(t.kind(), path)),
        }?;

        Ok(self.redact(&text))
    }

    /// Replace every occurrence of the API key in `text` with `[REDACTED]`,
    /// whether it is echoed verbatim or in a masked rendering.
    ///
    /// Two shapes, because two shapes are what providers actually send.
    ///
    /// **Verbatim.** Self-hosted OpenAI-compatible servers -- vLLM, LiteLLM,
    /// LM Studio -- echo the offending credential in full in a 401 body
    /// ("invalid api key: sk-..."), and `wire::api_error` relays a provider's
    /// message word-for-word by design. A plain substring replacement covers
    /// that.
    ///
    /// **Masked.** OpenAI itself -- the provider `rm-cli`'s own `TEMPLATE`
    /// ships with, so the default configuration -- does not echo the key
    /// whole. It sends the first 8 characters, asterisks padded out to the
    /// key's length, then the last 4:
    ///
    /// ```text
    /// Incorrect API key provided: sk-FAKE-********************6789. You can
    /// find your API key at https://platform.openai.com/account/api-keys
    /// ```
    ///
    /// No substring of that equals the key, so the verbatim pass alone is a
    /// no-op against the provider this crate is most likely to be pointed at.
    /// [`Self::redact_masked`] closes it, keyed entirely on the key already
    /// held rather than on anything that merely looks like a credential.
    ///
    /// **Still uncaught, and named rather than implied:** a rendering showing
    /// only a suffix (`...6789`) or only a prefix (`sk-FAKE-...`), a re-cased,
    /// URL-encoded, JSON-escaped or line-split rendering, and any mask whose
    /// visible head and tail are shorter than 8 and 4 characters. Chasing
    /// those would mean guessing at what looks like a secret, which trades a
    /// known false negative for a class of false positives -- mangled provider
    /// messages, and a redactor nobody can predict. Every rule here is still
    /// "match against the key we hold".
    fn redact(&self, text: &str) -> String {
        if self.api_key.is_empty() {
            // `str::replace` with an empty pattern inserts the replacement
            // between every character instead of doing nothing, so an empty
            // key -- invalid in practice, but not a type error -- must be
            // handled separately rather than falling into the branch below.
            return text.to_string();
        }
        let verbatim = text.replace(self.api_key.as_str(), "[REDACTED]");
        self.redact_masked(&verbatim)
    }

    /// Replace `<first 8 chars of the key> <anything> <last 4 chars of the
    /// key>` with `[REDACTED]`, where `<anything>` fits inside the key's own
    /// length plus a little slack.
    ///
    /// # It over-redacts, and by how much depends on the key
    ///
    /// The window scales with the key, so for a long key it is long. Current
    /// OpenAI project keys run to about 164 characters and their first 8
    /// characters are `sk-proj-` — a public constant, not a secret, and one
    /// that turns up in provider prose and documentation links. So a message
    /// mentioning `sk-proj-` and then, within ~172 characters, any four
    /// characters matching the key's tail collapses into `[REDACTED]`.
    /// Measured, with a 164-character key ending `TAIL`:
    ///
    /// ```text
    /// in : "You are using sk-proj- style keys now. See the migration guide, … then retry. Ref TAIL."
    /// out: "You are using [REDACTED]."
    /// ```
    ///
    /// 123 characters of the provider's explanation destroyed. That is worse
    /// than the earlier claim here — "short enough to be a mask of this key
    /// rather than unrelated prose" — which was simply not true for the
    /// default provider's own current key format.
    ///
    /// It is not fixed, and deliberately. The window has to be at least the
    /// key's length because the mask this exists to catch is padded to the
    /// key's length; anything shorter stops catching the real case. The
    /// failure direction is losing provider text, never leaking key material,
    /// and between a mangled error message and a printed credential this is
    /// the right way round. What is not acceptable is a doc comment that
    /// describes the tolerable failure as if it did not happen.
    fn redact_masked(&self, text: &str) -> String {
        // Below 16 bytes the head and the tail would overlap, and a 12-of-16
        // character match is loose enough to start hitting ordinary text.
        // Short keys keep the verbatim pass only.
        const HEAD: usize = 8;
        const TAIL: usize = 4;
        const SLACK: usize = 8;

        if self.api_key.len() < 16 {
            return text.to_string();
        }
        // On character boundaries: an API key is ASCII in practice, but
        // `api_key` is a `String` a config file supplies and slicing it
        // mid-character would panic.
        let head_end = self
            .api_key
            .char_indices()
            .nth(HEAD)
            .map_or(self.api_key.len(), |(i, _)| i);
        let tail_start = self
            .api_key
            .char_indices()
            .nth_back(TAIL - 1)
            .map_or(0, |(i, _)| i);
        if head_end >= tail_start {
            // Fewer than 12 characters despite 16-plus bytes: multibyte, so
            // not a key shape this handles.
            return text.to_string();
        }
        let head = &self.api_key[..head_end];
        let tail = &self.api_key[tail_start..];

        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find(head) {
            // Past the head either way, so `rest` always shrinks by at least
            // `head.len()` -- which is non-empty -- and this cannot spin on a
            // zero-width or overlapping match.
            let after = at + head.len();
            let mut window = (after + self.api_key.len() + SLACK).min(rest.len());
            while !rest.is_char_boundary(window) {
                window -= 1;
            }
            match rest[after..window].find(tail) {
                Some(found) => {
                    out.push_str(&rest[..at]);
                    out.push_str("[REDACTED]");
                    rest = &rest[after + found + tail.len()..];
                }
                None => {
                    out.push_str(&rest[..after]);
                    rest = &rest[after..];
                }
            }
        }
        out.push_str(rest);
        out
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
        // connection is refused by the local kernel: no DNS lookup, no packet
        // leaving the machine, no dependence on how the network's resolver
        // handles an unregistered name. That refusal is still a transport
        // failure, which is all these tests need. It is not instant — this
        // machine takes roughly two seconds to refuse it, deterministically,
        // for a cause not established (plausibly local firewall or AV
        // interception of connects to a reserved port) — but it stays within
        // what this task treats as offline.
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
        // port, so no DNS lookup happens and no packet leaves the machine.
        // That keeps this test — the one here that exercises the transport
        // rather than the wire parsing — offline. See `provider` above for
        // why this takes a couple of seconds rather than being instant.
        let err = provider().embed("anything").unwrap_err();
        assert!(
            err.0.contains("could not reach the provider"),
            "expected a transport error, got: {}",
            err.0
        );
    }

    #[test]
    fn a_transport_failure_never_carries_the_configured_base_url_in_any_url_shape() {
        // `base_url` is a value out of `rmem.toml`, and `ureq` opens its
        // transport message with the URL it was handed.
        //
        // The version of this test before it used one already-normalised
        // fixture and passed while seven of sixteen shapes leaked, because the
        // fix it guarded substituted away the string this crate *built* while
        // `ureq` prints the string it *parsed*. So the fixtures here are
        // deliberately awkward -- an uppercase scheme, a trailing slash, a
        // query, userinfo, an unknown scheme, no scheme at all -- and they
        // cover three different `ErrorKind`s. Nothing is matched any more, so
        // none of them can miss.
        //
        // Offline throughout. The shapes that reach the network reach
        // 127.0.0.1 port 1, where the local kernel refuses the connection and
        // no packet leaves the machine; the rest fail on the URL before any
        // socket exists.
        const CANARY: &str = "REALSECRETabc123DEF456";
        let bases = [
            // Awkward first, so a failure reports the informative shape: an
            // uppercase scheme is one `ureq` normalises, which is what
            // defeated the substitution this replaced.
            format!("HTTP://127.0.0.1:1/{CANARY}"),
            format!("http://127.0.0.1:1/{CANARY}"),
            format!("http://127.0.0.1:1/{CANARY}/"),
            format!("http://user:{CANARY}@127.0.0.1:1/v1"),
            // No socket for these two: the URL is refused before connecting.
            format!("gopher://{CANARY}/v1"),
            CANARY.to_string(),
        ];

        for base in bases {
            let provider = HttpProvider::new(
                base.clone(),
                "sk-key-not-under-test".into(),
                "c".into(),
                "e".into(),
            );
            let err = provider.embed("anything").unwrap_err().0;

            assert!(
                !err.contains(CANARY),
                "base_url {base:?} came back out: {err}"
            );
            assert!(
                !err.contains("127.0.0.1"),
                "the host is part of base_url too: {err}"
            );
            assert!(
                err.contains("embeddings"),
                "the endpoint is this crate's own word and has to survive: {err}"
            );
            assert!(
                err.contains("base_url named in rmem.toml"),
                "and the field to look at: {err}"
            );
        }
    }

    #[test]
    fn every_transport_failure_kind_gets_a_sentence_of_our_own() {
        // `transport_failure` maps a fieldless enum onto this crate's own
        // literals, which is what makes it airtight -- there is nothing in an
        // `ErrorKind` for a URL to hide in. This pins that every variant is
        // actually mapped and that none of them says nothing useful, so a new
        // variant cannot quietly fall through to a bare heading.
        let kinds = [
            ureq::ErrorKind::InvalidUrl,
            ureq::ErrorKind::UnknownScheme,
            ureq::ErrorKind::Dns,
            ureq::ErrorKind::InsecureRequestHttpsOnly,
            ureq::ErrorKind::ConnectionFailed,
            ureq::ErrorKind::TooManyRedirects,
            ureq::ErrorKind::BadStatus,
            ureq::ErrorKind::BadHeader,
            ureq::ErrorKind::Io,
            ureq::ErrorKind::InvalidProxyUrl,
            ureq::ErrorKind::ProxyConnect,
            ureq::ErrorKind::ProxyUnauthorized,
            ureq::ErrorKind::HTTP,
        ];
        for kind in kinds {
            let ProviderError::Transport(message) =
                HttpProvider::transport_failure(kind, "embeddings")
            else {
                panic!("a transport failure has to be one");
            };
            assert!(
                message.contains("embeddings") && message.contains("base_url"),
                "{kind:?}: {message}"
            );
            assert!(
                message.len() > 60,
                "{kind:?} says nothing a reader could act on: {message}"
            );
        }
    }

    /// The message `transport_failure` gives for `kind`.
    fn transport_message(kind: ureq::ErrorKind) -> String {
        let ProviderError::Transport(message) = HttpProvider::transport_failure(kind, "embeddings")
        else {
            panic!("a transport failure has to be one");
        };
        message
    }

    #[test]
    fn a_rejected_certificate_is_not_reported_as_nothing_listening() {
        // `ureq` maps a failed TLS handshake onto `ConnectionFailed`, the same
        // variant a refused connection uses -- so an unknown CA, an expired
        // certificate, or a corporate proxy substituting its own all used to
        // be reported as "nothing accepted a connection there -- check the
        // host, the port and the firewall". Something *did* accept the
        // connection. Sending someone to their firewall when the fix is a CA
        // bundle costs an afternoon, and behind a corporate proxy it is the
        // likeliest failure there is.
        let message = transport_message(ureq::ErrorKind::ConnectionFailed);
        assert!(message.contains("certificate"), "{message}");
        assert!(message.contains("TLS"), "{message}");
        // And still names the other cause, which is the common one outside a
        // corporate network.
        assert!(message.contains("port"), "{message}");
    }

    #[test]
    fn a_timeout_is_not_reported_as_a_connection_that_failed_part_way() {
        // Both the connect and the read timeout arrive as `Io`, as does TLS
        // session setup failing before a handshake begins. A connect timeout
        // is not a connection that failed part way through, and a reader
        // chasing the wrong one loses the same afternoon.
        let message = transport_message(ureq::ErrorKind::Io);
        assert!(message.contains("timed out"), "{message}");
    }

    #[test]
    fn a_completer_transport_failure_also_says_it_could_not_reach_the_provider() {
        // `complete` and `embed` share `post`; only `embed` was exercised
        // above. This pins the same conversion through the other port so a
        // future change to one path can't silently stop covering the other.
        let err = provider().complete("anything").unwrap_err();
        assert!(
            err.0.contains("could not reach the provider"),
            "expected a transport error, got: {}",
            err.0
        );
    }

    #[test]
    fn a_401_body_that_echoes_the_key_back_is_scrubbed_before_it_reaches_the_caller() {
        // The refused-connection tests above only ever reach the `Transport`
        // arm of `handle_response`'s match; nothing exercised `Status`. That
        // arm is where a real leak would live: some providers echo the
        // offending credential back in the error body ("invalid api key:
        // sk-..."), and `wire::api_error` relays a provider's message
        // verbatim by design. `ureq::Response::new` builds a `Response` by
        // parsing an in-memory string, the same as `ureq::Response::from_str`
        // does — no socket, no listener, nothing that leaves the machine.
        let key = "sk-secret-do-not-print";
        let body = format!(r#"{{"error":{{"message":"invalid api key: {key}"}}}}"#);
        let response = ureq::Response::new(401, "Unauthorized", &body).unwrap();

        // A `Status` response with a readable body is not itself a transport
        // failure — `handle_response` hands the text up for `wire` to
        // interpret, exactly as `complete` and `embed_inner` do in
        // production. The `ProviderError` the reviewer asked about is the
        // one `parse_completion` produces from it.
        let text = provider()
            .handle_response(Err(ureq::Error::Status(401, response)), "chat/completions")
            .expect("a readable body is not a transport failure");
        let err = parse_completion(&text).unwrap_err();

        assert!(
            !err.to_string().contains(key),
            "an error must never carry the key: {err}"
        );
    }

    #[test]
    fn the_masked_key_openai_echoes_back_is_scrubbed_before_it_reaches_the_caller() {
        // The fixture is the shape the default provider actually sends, not
        // one invented to suit the implementation. `TEMPLATE` ships
        // `base_url = "https://api.openai.com/v1"`, and OpenAI does not echo
        // a rejected key whole: it sends the first 8 characters, asterisks
        // padded out to the key's length, then the last 4. Nothing in that is
        // a substring of the key, so the verbatim replacement `redact`
        // started as was a no-op against the one provider this crate is
        // configured for out of the box -- 12 characters of a live
        // credential reaching stderr, scrollback and CI logs.
        let key = "sk-FAKE-0123456789abcdefghij6789";
        let masked = "sk-FAKE-********************6789";
        assert_eq!(key.len(), masked.len(), "the mask pads to the key's length");

        let body = format!(
            r#"{{"error":{{"message":"Incorrect API key provided: {masked}. You can find your API key at https://platform.openai.com/account/api-keys","type":"invalid_request_error","param":null,"code":"invalid_api_key"}}}}"#
        );
        // `TEMPLATE`'s own defaults, so the fixture says out loud which
        // provider this is about. Nothing here dials them: this test calls
        // `handle_response` with a `Response` built in memory, and no method
        // that reaches `ureq::post` is invoked. No socket, as everywhere else
        // in this suite.
        let provider = HttpProvider::new(
            "https://api.openai.com/v1".into(),
            key.into(),
            "gpt-4o-mini".into(),
            "text-embedding-3-small".into(),
        );
        let response = ureq::Response::new(401, "Unauthorized", &body).unwrap();
        let text = provider
            .handle_response(Err(ureq::Error::Status(401, response)), "chat/completions")
            .expect("a readable body is not a transport failure");
        let err = parse_completion(&text).unwrap_err().to_string();

        assert!(
            !err.contains("sk-FAKE-"),
            "the visible head of the key leaked: {err}"
        );
        assert!(
            !err.contains(masked),
            "the masked rendering leaked whole: {err}"
        );
        assert!(err.contains("[REDACTED]"), "{err}");
        assert!(
            err.contains("You can find your API key at"),
            "the rest of the provider's message has to survive: {err}"
        );
    }

    #[test]
    fn a_head_and_a_tail_too_far_apart_to_be_one_mask_are_left_alone() {
        // The masked match is bounded so a message that happens to open with
        // the key's first characters and close with its last does not
        // collapse into one redaction that swallows everything between. The
        // bound is the key's own length plus a little slack, which is what a
        // real mask fits inside and ordinary prose does not.
        let key = "sk-FAKE-0123456789abcdefghij6789";
        // Only `redact` is called; nothing here opens a connection.
        let provider = HttpProvider::new(
            "https://api.openai.com/v1".into(),
            key.into(),
            "c".into(),
            "e".into(),
        );
        let text = "sk-FAKE- appears here, and then a long stretch of the provider explaining itself at length before anything ends in 6789";
        assert_eq!(provider.redact(text), text);
    }

    #[test]
    fn a_long_key_makes_a_long_window_and_swallows_provider_prose_with_it() {
        // Recorded rather than required. The window scales with the key, and a
        // current OpenAI project key is ~164 characters whose first 8 are the
        // public constant `sk-proj-` -- which appears in provider prose. So a
        // message that mentions `sk-proj-` and then any four characters
        // matching the key's tail loses everything between.
        //
        // This pins the direction, which is what matters: text is lost, key
        // material is not. If it ever fails because the matcher was
        // tightened, the doc comment on `redact_masked` is what needs
        // updating -- it states this measurement, and a doc claim nothing
        // checks is how the previous version of it came to be wrong.
        let key = format!("sk-proj-{}TAIL", "F".repeat(152));
        assert_eq!(key.len(), 164, "a current OpenAI project key's length");
        let provider = HttpProvider::new("https://h".into(), key.clone(), "c".into(), "e".into());

        let message =
            "You are using sk-proj- style keys now. See the migration guide, then retry. Ref TAIL.";
        let out = provider.redact(message);

        assert!(!out.contains(&key), "key material survived: {out}");
        assert!(out.contains("[REDACTED]"), "{out}");
        assert!(
            !out.contains("migration guide"),
            "this is the over-redaction the doc comment describes; if it stopped happening, update that comment: {out}"
        );
        assert!(out.starts_with("You are using "), "{out}");
    }

    #[test]
    fn a_key_too_short_for_a_head_and_a_tail_still_gets_the_verbatim_pass() {
        // Below 16 bytes the 8-character head and the 4-character tail would
        // overlap, so the masked rule is off and exact matching is all there
        // is. That still has to work.
        let provider =
            HttpProvider::new("http://h".into(), "sk-short".into(), "c".into(), "e".into());
        assert_eq!(
            provider.redact("refused: sk-short is not a key"),
            "refused: [REDACTED] is not a key"
        );
        assert_eq!(
            provider.redact("refused: sk-sh***rt"),
            "refused: sk-sh***rt",
            "a short key must not start matching loosely instead"
        );
    }

    #[test]
    fn an_empty_key_leaves_the_response_text_unchanged_rather_than_corrupting_it() {
        // `str::replace` with an empty pattern does not no-op: it inserts the
        // replacement between every character, since an empty string matches
        // at every position. `redact` guards against that explicitly, and
        // this pins the reason the guard exists rather than just its
        // presence — see the mutation check in the task report, which
        // removed the guard and watched this test catch the corruption.
        let provider = HttpProvider::new(
            "http://127.0.0.1:1".into(),
            String::new(),
            "c".into(),
            "e".into(),
        );
        let text = r#"{"error":{"message":"no key was supplied"}}"#;
        assert_eq!(provider.redact(text), text);
    }
}
