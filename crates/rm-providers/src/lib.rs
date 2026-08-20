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
//! the request bodies and the response parsing as pure
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

        self.handle_response(response)
    }

    /// Turn a `ureq` result into body text.
    ///
    /// The `Ok` body and the `Status` arm's body both reach `redact` below
    /// before this returns: those are response content, which is where a
    /// provider could echo the key back. The `Transport` arm and both
    /// `into_string()` failure arms return via `?` first and never reach it —
    /// their strings come from the transport and IO layers (a refused
    /// connection, a body that failed to decode as UTF-8), not from response
    /// content or the `Authorization` header, so there is nothing in them for
    /// `redact` to catch. `a_transport_failure_never_carries_the_api_key`
    /// pins the `Transport` arm specifically, end to end.
    ///
    /// Split out from `post` so the `Status` arm — reached only when a real
    /// server answers with a non-2xx — can be driven in a test with a
    /// response built in-process by `ureq::Response::new`, without a socket.
    fn handle_response(
        &self,
        response: Result<ureq::Response, ureq::Error>,
    ) -> Result<String, ProviderError> {
        let text = match response {
            Ok(r) => r
                .into_string()
                .map_err(|e| ProviderError::Transport(e.to_string())),
            // A non-2xx with a body is the provider explaining itself, and the
            // body says more than the status line does.
            Err(ureq::Error::Status(_, r)) => r
                .into_string()
                .map_err(|e| ProviderError::Transport(e.to_string())),
            Err(ureq::Error::Transport(t)) => Err(ProviderError::Transport(t.to_string())),
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
    /// key>` with `[REDACTED]`, where `<anything>` is short enough to be a
    /// mask of this key rather than unrelated prose.
    ///
    /// The window is the key's own length plus a little slack, so a mask that
    /// pads to a slightly different width still matches while a prefix
    /// appearing in one sentence and a suffix in the next does not join up
    /// into one enormous redaction that swallows the provider's message.
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
            .handle_response(Err(ureq::Error::Status(401, response)))
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
        let provider = HttpProvider::new(
            "https://api.openai.com/v1".into(),
            key.into(),
            "gpt-4o-mini".into(),
            "text-embedding-3-small".into(),
        );
        let response = ureq::Response::new(401, "Unauthorized", &body).unwrap();
        let text = provider
            .handle_response(Err(ureq::Error::Status(401, response)))
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
