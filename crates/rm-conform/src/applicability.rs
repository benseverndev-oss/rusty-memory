//! Whether a memory reaches where it was asked from, measured.
//!
//! One rule governs every decision read:
//!
//! > A memory applies where its scope is an **ancestor-or-self** of the asker's
//! > position.
//!
//! It decides what a session is shown, and the live store has hundreds of
//! records under it. The headline table had five rows and none of them was
//! this.
//!
//! # This module never imports `rm_host::scope`
//!
//! Not `applies_at`, not `validate`, not `UNIVERSAL`. An oracle derived from
//! the code it judges is not an oracle. Scopes reach the store through
//! `command::decide` and `command::plan_rescope` like any other caller, so the
//! store is exercised normally; only the *expectation* is computed here.

/// Whether a memory scoped `scope` reaches an asker standing at `position`.
///
/// The oracle. Derived from the rule as written rather than from
/// `rm_host`'s `applies_at`, and derived *differently*: this is
/// separator-anchored string work where the implementation zips segment
/// iterators. Two ways to the same claim is the whole value of a differential.
///
/// `"*"` is spelled out rather than imported. Importing the constant would make
/// this track a change to it silently; spelling it means a change surfaces as a
/// disagreement, which is the point.
pub fn reaches(scope: &str, position: &str) -> bool {
    scope == "*"
        || position == scope
        // The trailing separator is what stops `prod` reaching `production`.
        || position.starts_with(&format!("{scope}/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_reaches_its_own_position_and_everything_below() {
        assert!(reaches("work", "work"));
        assert!(reaches("work", "work/goldenmatch"));
        assert!(reaches("work/goldenmatch", "work/goldenmatch/fs"));
        assert!(reaches("*", "anything/at/all"));
        assert!(reaches("*", "*"));
    }

    #[test]
    fn a_scope_reaches_neither_sideways_nor_upwards() {
        assert!(!reaches("work/goldenmatch/fs", "work/goldenmatch/er"));
        assert!(!reaches("personal", "work"));
        assert!(
            !reaches("work/goldenmatch", "work"),
            "narrower than the asker"
        );
        assert!(!reaches("work", "*"), "the root, where only * reaches");
    }

    /// The mistake the whole rule exists to prevent, and therefore the one an
    /// oracle must not share by construction. A bare `starts_with` says true
    /// to every line here.
    #[test]
    fn a_segment_boundary_is_not_a_string_prefix() {
        assert!(!reaches("prod", "production"));
        assert!(!reaches("work", "workshop"));
        assert!(!reaches("work", "workshop/thing"));
        assert!(reaches("prod", "prod/deploy"));
    }

    /// The constraint the whole module rests on, asserted rather than trusted
    /// to review. `include_str!` reads this file at compile time, so an import
    /// added later fails the suite rather than quietly voiding the measurement.
    #[test]
    fn this_module_does_not_import_the_code_it_judges() {
        let me = include_str!("applicability.rs");
        for banned in ["rm_host::scope", "scope::applies_at", "scope::UNIVERSAL"] {
            let uses = me
                .lines()
                .filter(|l| l.trim_start().starts_with("use "))
                .filter(|l| l.contains(banned))
                .count();
            assert_eq!(
                uses, 0,
                "applicability imports {banned}, so it judges itself"
            );
        }
    }
}
