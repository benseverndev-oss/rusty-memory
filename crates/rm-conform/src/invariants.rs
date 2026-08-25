//! Properties that hold whatever the right answer is.
//!
//! The reference model and the engine were written by the same author against
//! the same mental model, so they can agree enthusiastically on a shared
//! misunderstanding. These are derived from what bi-temporality *means* rather
//! than from either implementation, which is the only cover for that.
//!
//! They need no oracle, so they are cheap once a generator exists.

use crate::engine_harness::build;
use crate::history::Assertion;
use rm_core::Timestamp;
use rm_engine::Strategy;

/// Learning something today must not change what you believed last Tuesday.
///
/// The defining property of the transaction axis. `Version::ingested_at` is
/// `provenance.observed_at`, and the read filters on `ingested_at() <= tx_t`,
/// so an assertion observed after `tx_t` cannot participate in the answer at
/// `tx_t` -- however early the span it claims to describe.
///
/// Probed strictly below the earliest observation in the suffix, so that no
/// withheld assertion would have been visible anyway. Cutting at `max(prefix)`
/// instead would be wrong wherever the two share a timestamp.
pub fn monotonic_in_transaction_time(
    history: &[Assertion],
    cut: usize,
    probes: &[(Timestamp, Timestamp)],
    strategy: Strategy,
) -> bool {
    if cut == 0 || cut >= history.len() {
        return true;
    }
    let prefix = &history[..cut];
    let Some(suffix_first) = history[cut..]
        .iter()
        .map(|a| a.provenance.observed_at)
        .min()
    else {
        return true;
    };

    let (before, id_b) = build(prefix, "attr", strategy.clone());
    let (after, id_a) = build(history, "attr", strategy);

    probes
        .iter()
        .filter(|(_, tx)| *tx < suffix_first)
        .all(|(valid_t, tx_t)| {
            before.about(id_b, "attr", *valid_t, *tx_t).ok()
                == after.about(id_a, "attr", *valid_t, *tx_t).ok()
        })
}

/// Ingestion order must not change belief when observation times are fixed.
///
/// Stated only for the strategies where it is a property at all. It is *not*
/// universal and asserting it everywhere would be wrong:
///
/// - `MostComplete` / `LongestValue` / `MajorityVote` / `ConfidenceMajority`
///   break ties by "the first seen", which is input order by definition.
/// - `FirstNonNull` is input order, in its name.
///
/// Those are order-dependent on purpose. `MostRecent` and `ValidInterval` order
/// by time rather than by arrival, so for them arrival order must not matter --
/// and that is the property worth having, because it is the one an optimisation
/// is most likely to break.
pub fn order_independent(
    history: &[Assertion],
    probes: &[(Timestamp, Timestamp)],
    strategy: Strategy,
) -> bool {
    debug_assert!(
        matches!(strategy, Strategy::MostRecent | Strategy::ValidInterval),
        "order independence is not a property of the tie-broken strategies"
    );
    let mut reversed = history.to_vec();
    reversed.reverse();
    let (a, id_a) = build(history, "attr", strategy.clone());
    let (b, id_b) = build(&reversed, "attr", strategy);
    probes.iter().all(|(valid_t, tx_t)| {
        a.about(id_a, "attr", *valid_t, *tx_t).ok() == b.about(id_b, "attr", *valid_t, *tx_t).ok()
    })
}

/// The grid both properties are probed on.
///
/// `pub` so `report` uses this one rather than a second grid that could drift
/// away from it and make the README and the suite disagree about what was
/// measured.
pub fn probe_grid() -> Vec<(Timestamp, Timestamp)> {
    let mut out = Vec::new();
    for valid_t in [900, 1_050, 1_200, 1_600, 4_000] {
        for tx_t in [1_000, 1_100, 1_500, 6_000] {
            out.push((valid_t, tx_t));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::{generate, Params};

    #[test]
    fn later_arrivals_never_change_earlier_belief() {
        let params = Params::default();
        let probes = probe_grid();
        for strategy in [Strategy::MostRecent, Strategy::ValidInterval] {
            for seed in 0..60 {
                let history = generate(seed, &params);
                for cut in [1, 3, 6, 9] {
                    assert!(
                        monotonic_in_transaction_time(&history, cut, &probes, strategy.clone()),
                        "{strategy:?} seed {seed} cut {cut}"
                    );
                }
            }
        }
    }

    #[test]
    fn reversing_ingestion_order_changes_nothing() {
        let params = Params::default();
        let probes = probe_grid();
        for strategy in [Strategy::MostRecent, Strategy::ValidInterval] {
            for seed in 0..60 {
                let history = generate(seed, &params);
                assert!(
                    order_independent(&history, &probes, strategy.clone()),
                    "{strategy:?} seed {seed}"
                );
            }
        }
    }

    #[test]
    fn reversal_can_change_an_answer_where_the_rule_is_order_dependent() {
        // The companion to the test above. "Reversing changed nothing" is
        // exactly the shape of claim that holds vacuously if reversal never
        // reaches anything -- if `build` ignored input order, say, or the probe
        // grid always landed on Unknown.
        //
        // MostComplete breaks length ties by "the first seen", so it is
        // order-dependent by design. Seeing it move proves the comparison is
        // live, and makes the two green properties above mean something.
        let params = Params {
            alphabet: 3,
            ..Params::default()
        };
        let probes = probe_grid();
        let moved = (0..60).any(|seed| {
            let history = generate(seed, &params);
            let mut reversed = history.clone();
            reversed.reverse();
            let (a, id_a) = build(&history, "attr", Strategy::MostComplete);
            let (b, id_b) = build(&reversed, "attr", Strategy::MostComplete);
            probes.iter().any(|(valid_t, tx_t)| {
                a.about(id_a, "attr", *valid_t, *tx_t).ok()
                    != b.about(id_b, "attr", *valid_t, *tx_t).ok()
            })
        });
        assert!(
            moved,
            "reversing input order never changed an answer even under an \
             order-dependent strategy, so the order tests are vacuous"
        );
    }

    #[test]
    fn the_monotonicity_probe_actually_compares_something() {
        // If every probe were filtered out by `tx < suffix_first`, the property
        // would hold vacuously across every seed and assert nothing.
        let params = Params::default();
        let probes = probe_grid();
        let mut compared = 0usize;
        for seed in 0..60 {
            let history = generate(seed, &params);
            for cut in [1, 3, 6, 9] {
                if cut >= history.len() {
                    continue;
                }
                let suffix_first = history[cut..]
                    .iter()
                    .map(|a| a.provenance.observed_at)
                    .min()
                    .unwrap();
                compared += probes.iter().filter(|(_, tx)| *tx < suffix_first).count();
            }
        }
        assert!(
            compared > 100,
            "only {compared} probes survived the transaction-time filter, so the \
             property is close to vacuous"
        );
    }
}
