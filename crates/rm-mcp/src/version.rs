//! Which protocol this server speaks, and how it tells.
//!
//! MCP changed shape in `2026-07-28`. Before it, a connection opened with an
//! `initialize` handshake that negotiated a version once and held it for the
//! session. From it, there is no handshake at all: every request declares its
//! own version in `_meta`, the server accepts or rejects each request on its
//! own, and `server/discover` replaces the handshake as the way to ask what a
//! server speaks. The specification calls those two worlds **modern** and
//! **legacy**.
//!
//! # Why both
//!
//! A modern-only server is less code and it is also, by the specification's own
//! compatibility matrix, unreachable: *legacy client, modern server — **Fails***,
//! and legacy clients have no fall-forward mechanism to recover with. The
//! revision that removed the handshake is weeks old. So this server is
//! dual-era, which the specification permits and describes, and the routing
//! rule below is taken from it rather than invented here.

use serde_json::{json, Value};

use crate::jsonrpc::{Request, UNSUPPORTED_PROTOCOL_VERSION};

/// The modern revision, and the only version that may appear in a `_meta`
/// envelope here.
pub const MODERN: &str = "2026-07-28";

/// The handshake revisions this server can serve, newest first.
///
/// Not generosity: these are four revisions whose `tools/list` and
/// `tools/call` shapes are the ones this server already emits. The single
/// difference that reaches the wire is `structuredContent`, handled below.
pub const LEGACY: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The first revision with `structuredContent` on a tool result.
const STRUCTURED_FROM: &str = "2025-06-18";

/// Which world a request belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Era {
    /// Stateless, `_meta`-carrying, `2026-07-28`.
    Modern,
    /// A handshake happened, or is happening, at this version.
    Legacy(String),
}

impl Era {
    /// Whether a tool result may carry `structuredContent`.
    ///
    /// Revisions are `YYYY-MM-DD` and were chosen so that they sort, so this is
    /// a string comparison and not a date parse. That is a property of the
    /// naming scheme rather than a coincidence — the specification says the
    /// string "indicate\[s\] the last date backwards incompatible changes were
    /// made" — but it is load-bearing here, so a test states it.
    pub fn structured_content(&self) -> bool {
        match self {
            Era::Modern => true,
            Era::Legacy(v) => v.as_str() >= STRUCTURED_FROM,
        }
    }

    /// Whether a result must carry `resultType`.
    ///
    /// Modern results **MUST**. Legacy results have no such field, and while an
    /// unknown member is harmless to most readers, emitting one is claiming to
    /// speak a revision that did not have it.
    pub fn result_type(&self) -> bool {
        matches!(self, Era::Modern)
    }
}

/// Which era a request belongs to, or the error to answer it with.
///
/// Three rules, in this order, and each is the specification's:
///
/// 1. `initialize` is legacy. Nothing modern sends it, and a dual-era server
///    "selects its behavior from how the client opens".
/// 2. A request whose `_meta` carries a protocol version is modern, and is then
///    held to modern requirements: the version must be one this server serves,
///    and `clientCapabilities` must be present.
/// 3. Anything else is a legacy client that has already handshaked.
///
/// `negotiated` is the version a previous `initialize` settled on, which is why
/// this takes it. The legacy era really is stateful — the specification scopes
/// it "to the stdio process" — and the alternative is to guess a version for
/// every legacy `tools/call`, which would mean guessing whether the client can
/// read `structuredContent`.
pub fn era_of(request: &Request, negotiated: Option<&str>) -> Result<Era, Value> {
    if request.method == "initialize" {
        // The version is not settled yet; `negotiate` below does that, and the
        // era carried here is only used to shape the reply, which for
        // `initialize` is legacy by construction.
        return Ok(Era::Legacy(LEGACY[0].to_string()));
    }

    let Some(Value::String(requested)) = request.meta().get(super::PROTOCOL_VERSION_KEY) else {
        // No modern envelope. A legacy client that never handshaked still gets
        // served, at the newest handshake revision -- refusing it for
        // paperwork would fail the one client that is easiest to help, and
        // there is no session here for the paperwork to have established.
        return Ok(Era::Legacy(negotiated.unwrap_or(LEGACY[0]).to_string()));
    };

    if requested != MODERN {
        return Err(unsupported(request, requested));
    }

    // Required on every modern request, and a request missing a required
    // `_meta` field is malformed. Checked here rather than per method because
    // it is a property of the envelope, not of what was asked.
    if request.meta().get(super::CLIENT_CAPABILITIES_KEY).is_none() {
        return Err(crate::jsonrpc::error(
            request.id.as_ref(),
            crate::jsonrpc::INVALID_PARAMS,
            &format!(
                "Invalid params: _meta[\"{}\"] is required on every {MODERN} request",
                super::CLIENT_CAPABILITIES_KEY
            ),
            None,
        ));
    }

    Ok(Era::Modern)
}

/// `UnsupportedProtocolVersionError`, in the shape the specification gives it.
///
/// `supported` lists only `MODERN`, because that is the honest answer to the
/// question actually asked: which versions may appear in a `_meta` envelope.
/// The handshake revisions are reachable, but not this way, so the message says
/// how to reach them instead of putting them in a list a client would retry
/// through the wrong door.
fn unsupported(request: &Request, requested: &str) -> Value {
    let legacy = LEGACY.join(", ");
    crate::jsonrpc::error(
        request.id.as_ref(),
        UNSUPPORTED_PROTOCOL_VERSION,
        &format!(
            "Unsupported protocol version. This server serves {MODERN} in a _meta envelope, and {legacy} through an initialize handshake."
        ),
        Some(json!({"supported": [MODERN], "requested": requested})),
    )
}

/// The version to answer an `initialize` with.
///
/// "If the server supports the requested protocol version, it **MUST** respond
/// with the same version. Otherwise, the server **MUST** respond with another
/// protocol version it supports. This **SHOULD** be the *latest*." So a
/// version we serve comes straight back, and anything else -- including
/// `MODERN`, from a client that sends `initialize` for a revision that has no
/// `initialize` -- is answered with the newest handshake revision, and the
/// client decides whether it can speak it.
pub fn negotiate(requested: Option<&str>) -> String {
    match requested {
        Some(v) if LEGACY.contains(&v) => v.to_string(),
        _ => LEGACY[0].to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonrpc::parse;
    use crate::{CLIENT_CAPABILITIES_KEY, PROTOCOL_VERSION_KEY};

    fn modern_request(method: &str) -> Request {
        parse(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{{"_meta":{{
                "{PROTOCOL_VERSION_KEY}":"{MODERN}","{CLIENT_CAPABILITIES_KEY}":{{}}}}}}}}"#
        ))
        .unwrap()
    }

    fn bare(method: &str) -> Request {
        parse(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"{method}"}}"#
        ))
        .unwrap()
    }

    #[test]
    fn a_meta_envelope_at_the_current_version_is_modern() {
        assert_eq!(era_of(&modern_request("tools/list"), None), Ok(Era::Modern));
    }

    #[test]
    fn initialize_is_legacy_even_though_nothing_modern_sends_it() {
        // The dual-era routing rule's first clause. `initialize` is how a
        // legacy client opens, and it is the only signal available before
        // anything else has been said.
        assert_eq!(
            era_of(&bare("initialize"), None),
            Ok(Era::Legacy("2025-11-25".to_string()))
        );
    }

    #[test]
    fn a_request_with_no_meta_is_a_legacy_client_that_already_handshaked() {
        assert_eq!(
            era_of(&bare("tools/call"), Some("2024-11-05")),
            Ok(Era::Legacy("2024-11-05".to_string()))
        );
    }

    #[test]
    fn a_legacy_client_that_never_handshaked_is_served_anyway() {
        // Refusing it would fail the client easiest to help, for paperwork
        // there is no session here to have filed.
        assert_eq!(
            era_of(&bare("tools/call"), None),
            Ok(Era::Legacy(LEGACY[0].to_string()))
        );
    }

    #[test]
    fn an_unknown_version_in_a_meta_envelope_is_minus_32022_and_says_what_to_do() {
        let r = parse(&format!(
            r#"{{"jsonrpc":"2.0","id":9,"method":"tools/list","params":{{"_meta":{{
                "{PROTOCOL_VERSION_KEY}":"1900-01-01","{CLIENT_CAPABILITIES_KEY}":{{}}}}}}}}"#
        ))
        .unwrap();
        let e = era_of(&r, None).unwrap_err();
        assert_eq!(e["error"]["code"], json!(UNSUPPORTED_PROTOCOL_VERSION));
        assert_eq!(e["id"], json!(9));
        assert_eq!(e["error"]["data"]["requested"], json!("1900-01-01"));
        assert_eq!(e["error"]["data"]["supported"], json!([MODERN]));
        // The handshake revisions are not in `supported`, because a client
        // retrying with one of them in a `_meta` envelope would fail again.
        // They are in the message, with the door that does open them.
        let message = e["error"]["message"].as_str().unwrap();
        assert!(message.contains("initialize"), "{message}");
        assert!(message.contains("2025-11-25"), "{message}");
    }

    #[test]
    fn a_modern_request_without_client_capabilities_is_invalid_params() {
        // Required on every modern request, and the specification says a
        // request missing a required `_meta` field is malformed. The refusal
        // names the key, which is the only way a client can act on it.
        let r = parse(&format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{{"_meta":{{
                "{PROTOCOL_VERSION_KEY}":"{MODERN}"}}}}}}"#
        ))
        .unwrap();
        let e = era_of(&r, None).unwrap_err();
        assert_eq!(e["error"]["code"], json!(crate::jsonrpc::INVALID_PARAMS));
        let message = e["error"]["message"].as_str().unwrap();
        assert!(message.contains(CLIENT_CAPABILITIES_KEY), "{message}");
    }

    #[test]
    fn negotiation_echoes_a_version_we_serve_and_offers_the_newest_otherwise() {
        for v in LEGACY {
            assert_eq!(negotiate(Some(v)), v, "a version we serve comes back");
        }
        assert_eq!(negotiate(Some("1999-01-01")), LEGACY[0]);
        assert_eq!(negotiate(None), LEGACY[0]);
        // Including the modern revision: a client sending `initialize` for a
        // revision that has no `initialize` is asking for something that does
        // not exist, and the answer is the newest thing that does.
        assert_eq!(negotiate(Some(MODERN)), LEGACY[0]);
    }

    #[test]
    fn structured_content_is_withheld_from_the_revisions_that_predate_it() {
        // It arrived in 2025-06-18. Sending it to an older client is claiming
        // to speak a revision it did not have.
        assert!(Era::Modern.structured_content());
        assert!(Era::Legacy("2025-11-25".into()).structured_content());
        assert!(Era::Legacy("2025-06-18".into()).structured_content());
        assert!(!Era::Legacy("2025-03-26".into()).structured_content());
        assert!(!Era::Legacy("2024-11-05".into()).structured_content());
    }

    #[test]
    fn revision_strings_sort_in_release_order() {
        // `structured_content` compares them as strings rather than parsing
        // dates, which is only sound because YYYY-MM-DD sorts. That is a
        // property of the naming scheme, and it is load-bearing, so it is
        // stated here rather than left implied by the tests above.
        let mut sorted = LEGACY.to_vec();
        sorted.sort_unstable();
        sorted.reverse();
        assert_eq!(sorted, LEGACY.to_vec(), "LEGACY is newest-first");
        assert!(MODERN > LEGACY[0], "the modern revision is the newest");
    }

    #[test]
    fn result_type_belongs_to_the_modern_era_alone() {
        assert!(Era::Modern.result_type());
        assert!(!Era::Legacy(LEGACY[0].into()).result_type());
    }
}
