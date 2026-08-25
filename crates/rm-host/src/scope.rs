//! How far a memory reaches.
//!
//! A scope is not a label of origin. It is a declaration of reach, and the
//! difference decides the design: "never run scale benchmarks on the Windows
//! box" was written while working on one project and applies to every project
//! on the machine. Tagged with where it was written, it would vanish the moment
//! the next session was about something else.
//!
//! There is one rule, and everything here serves it:
//!
//! > A memory applies where its scope is an **ancestor-or-self** of the asker's
//! > position.
//!
//! The store does not interpret the segments. `work`, `personal/finance` and
//! `clients/acme/migration` are opaque strings that happen to contain a
//! separator; depth is unbounded and naming is the user's business.
//!
//! Nothing here touches the engine or the store, so the rule can be read and
//! tested without either.

/// The reach that covers every position.
///
/// The one value this module ascribes meaning to. `/` is a separator; the
/// segments between them stay opaque.
pub const UNIVERSAL: &str = "*";

const SEPARATOR: char = '/';

/// Whether a memory scoped `scope` applies to an asker standing at `position`.
///
/// Segment-wise, never a string prefix: `prod` must not match `production`,
/// and a string comparison would make that mistake silently on every read.
pub fn applies_at(scope: &str, position: &str) -> bool {
    if scope == UNIVERSAL {
        return true;
    }
    let mut here = position.split(SEPARATOR);
    // Every segment of the scope must be matched, in order, by the position.
    // Leftover position segments are fine -- that is what "or below" means.
    scope
        .split(SEPARATOR)
        .all(|segment| here.next() == Some(segment))
}

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

/// Whether `scope` is a scope at all.
///
/// The refusals exist so that two spellings cannot mean one thing. `work` and
/// `work/` would compare unequal and read identically, which is the sort of
/// difference nobody finds until a decision is missing.
pub fn validate(scope: &str) -> Result<(), String> {
    if scope == UNIVERSAL {
        return Ok(());
    }
    if scope.is_empty() {
        return Err(format!(
            "a scope says how far a decision reaches. It is {UNIVERSAL:?} for everywhere, or a path like \"work/goldenmatch\""
        ));
    }
    if scope.starts_with(SEPARATOR) || scope.ends_with(SEPARATOR) {
        return Err(format!(
            "{scope:?} has a leading or trailing {SEPARATOR:?}, which would make it a second spelling of the same scope"
        ));
    }
    for segment in scope.split(SEPARATOR) {
        if segment.trim().is_empty() {
            return Err(format!(
                "{scope:?} has an empty part. Every part between {SEPARATOR:?} has to name something"
            ));
        }
        if segment == UNIVERSAL {
            return Err(format!(
                "{scope:?} uses {UNIVERSAL:?} as a part, but it is a value rather than a wildcard: it means \"everywhere\" on its own and nothing inside a path"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_memory_applies_at_its_own_position_and_below() {
        assert!(applies_at("work", "work"), "self");
        assert!(applies_at("work", "work/goldenmatch"), "ancestor");
        assert!(applies_at("work/goldenmatch", "work/goldenmatch/fs"));
        assert!(applies_at(UNIVERSAL, "anything/at/all"));
        assert!(applies_at(UNIVERSAL, UNIVERSAL));
    }

    #[test]
    fn a_memory_does_not_apply_beside_or_above_itself() {
        assert!(
            !applies_at("work/goldenmatch/fs", "work/goldenmatch/er"),
            "sibling"
        );
        assert!(!applies_at("personal", "work"), "unrelated");
        // Narrower than the asker: a memory about one subsystem does not
        // apply to the whole project.
        assert!(!applies_at("work/goldenmatch", "work"), "descendant");
        // A position of `*` is the root, where only universal memories reach.
        assert!(!applies_at("work", UNIVERSAL));
    }

    /// The whole reason comparison is segment-wise. A string prefix would
    /// make every `prod` decision apply to `production`, silently.
    #[test]
    fn a_segment_is_not_a_string_prefix() {
        assert!(!applies_at("prod", "production"));
        assert!(!applies_at("work", "workshop/thing"));
        assert!(applies_at("prod", "prod/deploy"));
    }

    #[test]
    fn a_scope_that_could_mean_two_things_is_refused() {
        assert!(validate("work").is_ok());
        assert!(validate("work/goldenmatch/fs").is_ok());
        assert!(validate(UNIVERSAL).is_ok());

        for bad in [
            "",
            "  ",
            "/work",
            "work/",
            "work//fs",
            "work/ /fs",
            "work/*",
        ] {
            let e = validate(bad).unwrap_err();
            assert!(!e.is_empty(), "for {bad:?}");
        }
    }

    /// `*` is a value, not a wildcard. Accepting `work/*` would promise a
    /// pattern language the rule does not have.
    #[test]
    fn the_universal_scope_is_refused_as_a_segment() {
        let e = validate("work/*").unwrap_err();
        assert!(
            e.contains('*'),
            "the message should name the character: {e}"
        );
    }

    /// An empty value is not a position. It arrives that way from a shell that
    /// says `RMEM_SCOPE=` and from a JSON `env` block with an empty string,
    /// both of which read as "not configured" to whoever wrote them.
    #[test]
    fn an_empty_position_is_no_position_at_all() {
        assert_eq!(position(None), None);
        assert_eq!(position(Some(String::new())), None);
        assert_eq!(position(Some("   ".into())), None, "whitespace is a typo");
        assert_eq!(
            position(Some(
                "	
"
                .into()
            )),
            None
        );

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
