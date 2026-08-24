//! The decision layer: chains, standing, and the temporal question it cannot
//! be asked.
//!
//! Two facts make this simpler than it looks, both verified against the source
//! rather than assumed:
//!
//! - `find_decision` is an exact string match on the identity record's `name`,
//!   not a vector search, so these tests cannot be retrieval-flaky and a
//!   three-dimension index is fine.
//! - `DecisionDetail` exposes `still_stands`, `supersedes` and `superseded_by`
//!   directly, so no `Standing` accessor is needed here.

use rm_embed::Hashed;
use rm_engine::{Engine, Policy, Strategy};
use rm_host::command::{self, DecisionDetail, Outcome};
use rm_index::{Metric, VectorIndex};
use rm_resolve::{BlockingKey, Comparator, FieldRule, Ruleset};

fn ruleset() -> Ruleset {
    Ruleset::new(
        vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
        vec![BlockingKey::Prefix("name".to_string(), 3)],
        4.0,
        8.0,
    )
    .expect("a one-field ruleset is valid")
}

fn engine() -> Engine {
    Engine::new(
        VectorIndex::new(3, Metric::Cosine),
        ruleset(),
        Policy::new(Strategy::MostRecent),
    )
}

/// Record `titles` as a chain, each superseding the one before it.
pub fn build_chain(titles: &[&str]) -> Engine {
    let mut e = engine();
    let embedder = Hashed::new(3);
    let mut observed_at = 1_000;
    let mut previous: Option<&str> = None;
    for title in titles {
        command::decide(
            &mut e,
            title,
            "the chosen option",
            None, // status: defaults to accepted
            Some("a stated reason"),
            None, // context
            previous,
            None, // decided_at: defaults to observed_at
            observed_at,
            "conform",
            &embedder,
        )
        .expect("a decision with a fresh title is recorded");
        previous = Some(title);
        observed_at += 100;
    }
    e
}

/// One decision in full, or a panic naming the title that did not resolve.
pub fn detail(e: &Engine, title: &str) -> DecisionDetail {
    match command::decision(e, title).expect("a recorded title resolves") {
        Outcome::Decision(Some(d)) => *d,
        _ => panic!("expected a decision for {title:?}"),
    }
}

/// The fraction of a bi-temporal probe set the decision API can answer.
///
/// Zero, and computed rather than described: `command::decisions` and
/// `command::decision` take no time parameters at all -- verified at
/// `crates/rm-host/src/command.rs:788` and `:968` -- so there is no probe they
/// can answer. `Engine::about` takes both clocks; the product surface built on
/// top of it takes neither.
///
/// Stated as a number so it changes visibly the day somebody adds `--valid-at`
/// or `--as-of` to the decision reads, rather than living in a paragraph
/// nobody re-reads.
pub fn time_coverage() -> f64 {
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    // Far apart under jaro_winkler: the decide-title fuzzy match is a separate
    // concern, and titles that merged would measure the resolver instead of
    // the chain.
    const TITLES: [&str; 3] = ["adopt sqlite", "prefer postgres", "switch to duckdb"];

    #[test]
    fn a_chain_of_three_is_recovered_in_order() {
        let e = build_chain(&TITLES);

        let first = detail(&e, TITLES[0]);
        let after: Vec<&str> = first
            .superseded_by
            .iter()
            .map(|(_, t)| t.as_str())
            .collect();
        assert_eq!(after, vec![TITLES[1], TITLES[2]]);

        let last = detail(&e, TITLES[2]);
        let before: Vec<&str> = last.supersedes.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(before, vec![TITLES[1], TITLES[0]]);
    }

    #[test]
    fn only_the_end_of_the_chain_still_stands() {
        let e = build_chain(&TITLES);
        assert!(!detail(&e, TITLES[0]).still_stands);
        assert!(!detail(&e, TITLES[1]).still_stands);
        assert!(detail(&e, TITLES[2]).still_stands);
    }

    #[test]
    fn the_ends_of_the_chain_have_nothing_beyond_them() {
        let e = build_chain(&TITLES);
        assert!(detail(&e, TITLES[0]).supersedes.is_empty());
        assert!(detail(&e, TITLES[2]).superseded_by.is_empty());
    }

    #[test]
    fn a_longer_chain_is_recovered_whole() {
        // Three could pass by accident on a one-step lookup that never
        // recurses. Six cannot.
        let titles = [
            "alpha option",
            "beta option",
            "gamma option",
            "delta option",
            "epsilon option",
            "zeta option",
        ];
        let e = build_chain(&titles);
        let first = detail(&e, titles[0]);
        let after: Vec<&str> = first
            .superseded_by
            .iter()
            .map(|(_, t)| t.as_str())
            .collect();
        assert_eq!(after, titles[1..].to_vec());
        assert!(detail(&e, titles[5]).still_stands);
    }

    #[test]
    fn a_lone_decision_stands_and_chains_to_nothing() {
        let e = build_chain(&["only decision"]);
        let d = detail(&e, "only decision");
        assert!(d.still_stands);
        assert!(d.supersedes.is_empty());
        assert!(d.superseded_by.is_empty());
    }

    #[test]
    fn the_decision_layer_answers_no_temporal_probe() {
        assert_eq!(time_coverage(), 0.0);
    }
}
