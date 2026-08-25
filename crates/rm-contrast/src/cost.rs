//! What a read costs, in shape rather than in seconds.
//!
//! `rm-contrast`'s README tells a reader choosing between this store and the
//! flat control to weigh a cost it declines to quantify: *"The difference is
//! asymptotic rather than a constant factor, and it is not measured here."*
//! This file is the asymptotic half. `benches/read-cost` is the other half,
//! and is what checks that these predictions still track reality.
//!
//! # The units are arbitrary, and that is load-bearing
//!
//! A "unit" here is roughly one candidate touched. Ratios between strategies
//! and across depths are meaningful because both sides are counted the same
//! way. **Ratios against the control are not**, and this module deliberately
//! does not offer one: a store unit builds a `Candidate` and clones a `String`
//! where a control unit is a hash lookup, so dividing them would produce a
//! confident number that means nothing. The store-versus-control crossover is
//! measured in nanoseconds, in the bench, where the units really are the same.

use rm_engine::Strategy;

/// Versions per attribute slot in the live store, measured rather than
/// assumed.
///
/// `D:\memory\decisions.json`, 2026-08-25: 219 entities, 1,086 attribute
/// slots, and every one of them holding exactly one version. Nothing has been
/// revised.
///
/// It is an anchor for where *that* store sits, not a claim about workloads in
/// general -- it is two days old and was seeded once.
pub const LIVE_STORE_DEPTH: usize = 1;

/// Predicted work for one `about()` against a slot holding `v` versions.
///
/// # This models the path where the value changes
///
/// `merge` returns early when every assertion agrees
/// (`rm-survivor/src/lib.rs:424`), so a slot holding one value `v` times pays
/// the unanimity scan and nothing else -- no sort, no strategy. That path is
/// deliberately not modelled here, because it is deliberately not generated in
/// the bench: measuring it would measure the early-out rather than
/// survivorship. A model that ignored the early-out while the bench exercised
/// it would have two errors that cancel invisibly.
///
/// # Terms
///
/// | term | what it models |
/// |---|---|
/// | `v` | one `Candidate` per tx-visible version, `rm-engine/src/read.rs:284` |
/// | `v` | the unanimity scan before any strategy runs, `rm-survivor/src/lib.rs:424` |
/// | `2v` | `MostRecent`'s max-then-filter, `rm-survivor/src/lib.rs:531` |
/// | `v*log2(v) + 2v` | `ValidInterval`'s sort, grouping and span-closing passes, `rm-survivor/src/lib.rs:619` |
///
/// Strategies other than those two collapse a history in a single pass and are
/// modelled as one, which is enough for a shape.
pub fn predicted_work(v: usize, strategy: &Strategy) -> f64 {
    let v = v as f64;
    // Paid on every read whatever the strategy resolves to.
    let shared = 2.0 * v;
    let by_strategy = match strategy {
        Strategy::MostRecent => 2.0 * v,
        // `log2(1)` is 0, so a single-version slot pays no sort at all. That
        // is not a rounding convenience -- it is why the two strategies cost
        // the same at the depth the live store is at.
        Strategy::ValidInterval => v * v.log2() + 2.0 * v,
        _ => v,
    };
    shared + by_strategy
}

/// The shallowest depth at which `ValidInterval` costs `factor` times
/// `MostRecent`.
///
/// `None` when they never diverge that far within a depth any store would
/// plausibly reach, which is itself an answer rather than a failure.
pub fn depth_where_ratio_exceeds(factor: f64) -> Option<usize> {
    (1..100_000).find(|&v| {
        predicted_work(v, &Strategy::ValidInterval) / predicted_work(v, &Strategy::MostRecent)
            >= factor
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The headline derived answer, and the reason this file exists.
    ///
    /// At depth 1, `log2(1)` is 0, so `ValidInterval` has no sort to pay and
    /// the two strategies do identical work. That is the depth the live store
    /// is actually at, so on this model the shipped-default question is not a
    /// cost question at all -- which is a claim worth failing loudly if the
    /// model ever stops supporting it.
    #[test]
    fn at_the_live_store_depth_the_two_strategies_cost_the_same() {
        assert_eq!(LIVE_STORE_DEPTH, 1);
        assert_eq!(
            predicted_work(LIVE_STORE_DEPTH, &Strategy::MostRecent),
            predicted_work(LIVE_STORE_DEPTH, &Strategy::ValidInterval),
        );
    }

    /// And the companion, because the test above would also pass for a model
    /// that returned a constant. The two must diverge somewhere.
    #[test]
    fn the_strategies_diverge_once_there_is_a_history_to_sort() {
        let ratio = |v| {
            predicted_work(v, &Strategy::ValidInterval) / predicted_work(v, &Strategy::MostRecent)
        };
        assert_eq!(ratio(1), 1.0);
        assert!(ratio(1000) > ratio(10), "the sort term must show up");
        assert!(
            ratio(1000) < 4.0,
            "a log factor, not a catastrophe: {}",
            ratio(1000)
        );
    }

    /// The depth at which the sort starts to matter, computed rather than
    /// typed. `None` would mean the strategies never diverge that far, which
    /// is itself an answer.
    #[test]
    fn the_divergence_depth_is_a_number_not_an_opinion() {
        let d = depth_where_ratio_exceeds(1.5).expect("they do diverge");
        assert!(
            d > LIVE_STORE_DEPTH,
            "must be past where the real store sits"
        );
        assert!(
            d < 10_000,
            "found at a plausible depth, not off the end: {d}"
        );
    }

    /// Work grows with depth for the store, on every strategy. A model that
    /// flattened would make the whole bench meaningless.
    #[test]
    fn more_history_is_never_less_work() {
        for s in [Strategy::MostRecent, Strategy::ValidInterval] {
            for v in 1..200 {
                assert!(
                    predicted_work(v + 1, &s) >= predicted_work(v, &s),
                    "{s:?} went down from {v} to {}",
                    v + 1
                );
            }
        }
    }
}
