//! Where a memory reaches, and where a host stands.
//!
//! The rule itself lives in [`rm_core::scope`] and is re-exported here. It
//! moved down so `rm_engine::Query` could use it: `Query` lives in `rm-engine`,
//! which depends on `rm-core` and not on this crate, and a second
//! implementation of ancestor-or-self in the engine is exactly the drift this
//! project keeps finding.
//!
//! [`position`] did not move. It normalises a *configured* value, which is a
//! fact about how a host learns a position rather than about what a scope
//! means.

pub use rm_core::scope::{applies_at, validate, UNIVERSAL};

/// A position, from a source that can hand back an empty value.
///
/// An unset `RMEM_SCOPE` and one set to the empty string look identical in a
/// shell and in a JSON `env` block, and they used to behave nothing alike:
/// unset suspends the applicability rule, while empty was read as a position
/// and split into one empty segment -- the root, where only [`UNIVERSAL`]
/// reaches. Measured on a 219-decision store, `RMEM_SCOPE=` returned 32
/// records where unset returned all 219.
///
/// That is the worst shape a defect can take here: a configuration that looks
/// unconfigured, hiding most of the store, reporting nothing. Whitespace is
/// trimmed for the same reason -- `RMEM_SCOPE=" "` is a typo, not a position.
pub fn position(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty value is not a position. It arrives that way from a shell that
    /// says `RMEM_SCOPE=` and from a JSON `env` block with an empty string,
    /// both of which read as "not configured" to whoever wrote them.
    #[test]
    fn an_empty_position_is_no_position_at_all() {
        assert_eq!(position(None), None);
        assert_eq!(position(Some(String::new())), None);
        assert_eq!(position(Some("   ".into())), None, "whitespace is a typo");
        assert_eq!(position(Some("\t\n".into())), None);

        assert_eq!(position(Some("work".into())), Some("work".into()));
        assert_eq!(
            position(Some("  work/goldenmatch  ".into())),
            Some("work/goldenmatch".into()),
            "trimmed, because a stray space is never meant"
        );
        // `*` is a real position -- the root -- and must survive.
        assert_eq!(position(Some(UNIVERSAL.into())), Some(UNIVERSAL.into()));
    }

    /// The bug this exists to prevent, stated as the two behaviours it kept
    /// apart. Without the filter above, `""` splits into one empty segment and
    /// nothing but `*` reaches it.
    #[test]
    fn an_empty_string_would_otherwise_be_the_root_position() {
        assert!(!applies_at("work", ""), "this is what made it dangerous");
        assert!(applies_at(UNIVERSAL, ""));
        // ...so the normalisation, not the rule, is what has to catch it.
        assert_eq!(position(Some(String::new())), None);
    }
}
