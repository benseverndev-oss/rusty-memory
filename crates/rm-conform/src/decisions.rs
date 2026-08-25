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

use rm_core::Timestamp;
use rm_embed::Hashed;
use rm_engine::{Engine, Policy, Strategy};
use rm_host::command::{self, DecisionDetail, Found, Outcome};
use rm_host::time::At;
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
            "conform",
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
    match command::decision(e, title, At::latest(), None).expect("a recorded title resolves") {
        Outcome::Decision(Found::Decision(d)) => *d,
        _ => panic!("expected a decision for {title:?}"),
    }
}

/// The grid the decision layer is probed on.
///
/// `pub` so the report and the vacuity test read the same grid rather than two
/// that could drift apart. Chosen against `build_chain`'s clock: it records at
/// 1000, 1100, 1200, so 900 predates the chain and 1500 follows it.
pub fn coverage_probes() -> Vec<(Timestamp, Timestamp)> {
    let mut out = Vec::new();
    for valid_t in [900, 1_050, 1_150, 1_500] {
        for tx_t in [900, 1_050, 1_150, 1_500] {
            out.push((valid_t, tx_t));
        }
    }
    out
}

/// The fraction of a bi-temporal probe set the decision API answers correctly.
///
/// Was a hardcoded `0.0`: `command::decisions` and `command::decision` took no
/// time parameters at all, so there was no probe they could answer. They take
/// an `At` now, and this measures whether the answers are *right* rather than
/// whether an answer came back.
///
/// The expectation is computed here from what `build_chain` wrote, not from
/// what the command returns. An oracle derived from the code it judges is not
/// an oracle, which is the rule the rest of this crate is built on.
pub fn time_coverage() -> f64 {
    const TITLES: [&str; 3] = ["adopt sqlite", "prefer postgres", "switch to duckdb"];
    let recorded_at: [Timestamp; 3] = [1_000, 1_100, 1_200];
    let e = build_chain(&TITLES);

    let probes = coverage_probes();
    let mut right = 0usize;
    for (valid_t, tx_t) in &probes {
        let at = At {
            valid: *valid_t,
            tx: *tx_t,
        };
        // What should be true of the first link here, worked out from the
        // timestamps `build_chain` used. `decided_at` defaults to `observed_at`,
        // so each link enters both axes at the same instant.
        let known = recorded_at[0] <= *tx_t && recorded_at[0] <= *valid_t;
        // It is retired once the second link exists on both axes: that is the
        // one carrying the `supersedes` edge into it.
        let retired = recorded_at[1] <= *tx_t && recorded_at[1] <= *valid_t;

        let got = command::decision(&e, TITLES[0], at, None).expect("a recorded title resolves");
        let ok = match got {
            Outcome::Decision(Found::NotYetRecorded { .. }) => !known,
            Outcome::Decision(Found::Decision(d)) => known && d.still_stands == !retired,
            _ => false,
        };
        if ok {
            right += 1;
        }
    }
    right as f64 / probes.len() as f64
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
    fn every_temporal_probe_is_answered_correctly() {
        assert_eq!(
            time_coverage(),
            1.0,
            "the decision layer disagreed with the expectation on some probe"
        );
    }

    /// The companion. A coverage figure measured over a grid where every probe
    /// is trivially "now" would read 1.000 having tested nothing -- the same
    /// vacuity the differential suite guards against.
    #[test]
    fn the_probe_grid_straddles_the_chain_rather_than_sitting_after_it() {
        let probes = coverage_probes();
        let before = probes.iter().filter(|(_, tx)| *tx < 1_000).count();
        let inside = probes
            .iter()
            .filter(|(_, tx)| (1_000..1_300).contains(tx))
            .count();
        let after = probes.iter().filter(|(_, tx)| *tx >= 1_300).count();
        assert!(before > 0, "no probe predates the chain");
        assert!(inside > 0, "no probe lands mid-chain");
        assert!(after > 0, "no probe sees the whole chain");
    }

    /// Guards the figure above against a subtler vacuity than the grid one:
    /// a coverage of 1.000 in which every probe took the same branch would
    /// measure one behaviour and report it as three.
    #[test]
    fn the_probes_reach_all_three_answers() {
        const TITLES: [&str; 3] = ["adopt sqlite", "prefer postgres", "switch to duckdb"];
        let e = build_chain(&TITLES);
        let (mut not_yet, mut standing, mut retired) = (0, 0, 0);
        for (valid, tx) in coverage_probes() {
            match command::decision(&e, TITLES[0], At { valid, tx }, None).unwrap() {
                Outcome::Decision(Found::NotYetRecorded { .. }) => not_yet += 1,
                Outcome::Decision(Found::Decision(d)) if d.still_stands => standing += 1,
                Outcome::Decision(Found::Decision(_)) => retired += 1,
                other => panic!("unexpected outcome: {other:?}"),
            }
        }
        assert!(not_yet > 0, "no probe found it unrecorded");
        assert!(standing > 0, "no probe found it standing");
        assert!(retired > 0, "no probe found it retired");
    }
}
