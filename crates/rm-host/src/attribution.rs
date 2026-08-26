//! Who wrote a record.
//!
//! `Provenance::source_ref` is documented as "the session, turn, or document
//! this came from [...] the host decides its shape". This is where a host
//! decides it.
//!
//! The field was never missing. `rm-mcp` has always written the handshake
//! client name into it, and `rm-cli` has always written the literal string
//! `"cli"` -- and since everything in the live store was written through the
//! CLI, the store reads as though provenance had no author field at all. 256
//! records, one constant.
//!
//! # The shape
//!
//! `<agent>@<host>/<session>` -- `RM@bsev-002/b149f85e`.
//!
//! Three parts because three separate questions went unanswered: which agent
//! found it, which machine it happened on, and which run. A name alone
//! collides -- on the machine this was written for there were five sessions
//! called `Print` and four called `Circ`.

/// What to record as the author, given what the host said and where it ran.
///
/// Pure, and takes both values as parameters, so it can be tested without
/// touching the environment. `std::env::set_var` is process-global and Rust
/// runs tests in parallel, so two tests setting `RMEM_SESSION` would race and
/// fail each other intermittently -- the kind of flake that gets re-run rather
/// than read.
pub fn source_ref(session: Option<&str>, host: &str) -> String {
    let named = session.map(str::trim).filter(|s| !s.is_empty());
    match named {
        Some(s) => s.to_string(),
        None => {
            let host = host.trim();
            let host = if host.is_empty() {
                "unknown-host"
            } else {
                host
            };
            format!("cli@{host}")
        }
    }
}

/// What this machine calls itself.
///
/// From the environment rather than a crate: this project parses its own
/// arguments rather than take `clap`, and a hostname dependency is the same
/// trade. `COMPUTERNAME` on Windows, `HOSTNAME` elsewhere -- the latter is not
/// always exported, which is why the caller has a fallback rather than this
/// returning an `Option` nobody would handle.
pub fn host() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default()
}

/// The author string for a CLI invocation.
///
/// The one function here that reads the environment, and deliberately the one
/// with no unit test: everything it decides is decided in [`source_ref`].
pub fn cli() -> String {
    source_ref(std::env::var("RMEM_SESSION").ok().as_deref(), &host())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `RMEM_SESSION` is taken at its word. A host that says who it is knows
    /// better than this crate does.
    #[test]
    fn a_session_that_names_itself_is_used_verbatim() {
        assert_eq!(
            source_ref(Some("RM@bsev-002/b149f85e"), "ignored"),
            "RM@bsev-002/b149f85e"
        );
    }

    /// Unset degrades to something honest rather than to a constant.
    ///
    /// The machine is knowable and the session is not, so the answer says the
    /// first and omits the second. That is strictly more than `"cli"`, which
    /// is what every record in the live store carries and why none of them can
    /// be attributed.
    #[test]
    fn an_unnamed_session_still_records_the_machine() {
        assert_eq!(source_ref(None, "bsev-002"), "cli@bsev-002");
    }

    /// Empty and whitespace are how an env var looks when someone meant to
    /// unset it. Same rule as `RMEM_SCOPE`, for the same reason: a setting
    /// that looks unconfigured must behave as unconfigured.
    #[test]
    fn an_empty_session_is_no_session_at_all() {
        assert_eq!(source_ref(Some(""), "bsev-002"), "cli@bsev-002");
        assert_eq!(source_ref(Some("   "), "bsev-002"), "cli@bsev-002");
        assert_eq!(source_ref(Some("\t\n"), "bsev-002"), "cli@bsev-002");
    }

    /// Whitespace around a real value is a typo, not part of the name.
    #[test]
    fn a_named_session_is_trimmed() {
        assert_eq!(source_ref(Some("  RM@host/abc  "), "x"), "RM@host/abc");
    }

    /// An unknown host is named as unknown rather than left blank, so the
    /// field never reads as though the machine were part of the identity when
    /// it is not.
    #[test]
    fn an_unknown_host_says_so() {
        assert_eq!(source_ref(None, ""), "cli@unknown-host");
    }
}
