//! Engine against oracle, and the smallest history that separates them.

use crate::generate::{generate, Params};
use crate::history::Assertion;
use crate::reference;
use rm_survivor::{merge as engine_merge, Strategy};

/// A history on which the two implementations differ.
#[derive(Clone, Debug)]
pub struct Disagreement {
    pub seed: u64,
    pub strategy: String,
    pub history: Vec<Assertion>,
    pub engine: String,
    pub reference: String,
}

/// Whether the two agree on this history.
///
/// Refusals compare as refusals and never by message: the property is that the
/// two refuse on the same inputs, not that they chose the same sentence.
pub fn agrees(history: &[Assertion], strategy: &Strategy) -> bool {
    let candidates: Vec<_> = history.iter().map(|a| a.candidate()).collect();
    match (
        engine_merge(&candidates, strategy),
        reference::merge(&candidates, strategy),
    ) {
        (Ok(a), Ok(b)) => a == b,
        (Err(_), Err(_)) => true,
        _ => false,
    }
}

/// The shortest history reachable by deleting assertions that still disagrees.
///
/// Shrinking is what makes a disagreement useful: a random twelve-assertion
/// history that fails tells you nothing, and the three assertions that still
/// fail tell you which rule is wrong.
pub fn shrink(history: &[Assertion], strategy: &Strategy) -> Vec<Assertion> {
    let mut best = history.to_vec();
    let mut improved = true;
    while improved {
        improved = false;
        for i in 0..best.len() {
            let mut candidate = best.clone();
            candidate.remove(i);
            if !candidate.is_empty() && !agrees(&candidate, strategy) {
                best = candidate;
                improved = true;
                break;
            }
        }
    }
    best
}

/// Every seed, every strategy. Empty means the two agree everywhere tried.
pub fn sweep(
    seeds: impl Iterator<Item = u64>,
    params: &Params,
    strategies: &[Strategy],
) -> Vec<Disagreement> {
    let mut found = Vec::new();
    for seed in seeds {
        let history = generate(seed, params);
        for strategy in strategies {
            if agrees(&history, strategy) {
                continue;
            }
            let minimal = shrink(&history, strategy);
            let candidates: Vec<_> = minimal.iter().map(|a| a.candidate()).collect();
            found.push(Disagreement {
                seed,
                strategy: format!("{strategy:?}"),
                engine: format!("{:?}", engine_merge(&candidates, strategy)),
                reference: format!("{:?}", reference::merge(&candidates, strategy)),
                history: minimal,
            });
        }
    }
    found
}

/// The strategies scored by default.
///
/// `SourcePriority` is excluded because it needs a priority list to mean
/// anything; it is covered separately with one supplied.
pub fn default_strategies() -> Vec<Strategy> {
    vec![
        Strategy::MostRecent,
        Strategy::ValidInterval,
        Strategy::MostComplete,
        Strategy::LongestValue,
        Strategy::MajorityVote,
        Strategy::ConfidenceMajority,
        Strategy::FirstNonNull,
        Strategy::UnanimousOrNull,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_implementations_agree_across_the_fixed_seed_set() {
        let params = Params::default();
        let found = sweep(0..200, &params, &default_strategies());
        assert!(
            found.is_empty(),
            "{} disagreement(s); first: {:#?}",
            found.len(),
            found.first()
        );
    }

    #[test]
    fn the_comparison_can_detect_a_difference_at_all() {
        // Without this, a green sweep is consistent with `agrees` returning
        // true unconditionally. Two rules that genuinely differ must be seen
        // to differ, or the suite above has measured nothing.
        let params = Params::default();
        let separated = (0..50).any(|seed| {
            let h = generate(seed, &params);
            let cs: Vec<_> = h.iter().map(|a| a.candidate()).collect();
            engine_merge(&cs, &Strategy::MostRecent).ok()
                != engine_merge(&cs, &Strategy::FirstNonNull).ok()
        });
        assert!(
            separated,
            "MostRecent and FirstNonNull agreed on all 50 seeds, which cannot be right"
        );
    }

    #[test]
    fn shrinking_reduces_a_disagreement_to_something_smaller() {
        // Shrink against a deliberately mismatched pair so there is something
        // to minimise, and check the machinery actually minimises it.
        let params = Params {
            len: 12,
            ..Params::default()
        };
        let wrong = |h: &[Assertion]| {
            let cs: Vec<_> = h.iter().map(|a| a.candidate()).collect();
            engine_merge(&cs, &Strategy::MostRecent).ok()
                == reference::merge(&cs, &Strategy::FirstNonNull).ok()
        };
        let seed = (0..50)
            .find(|s| !wrong(&generate(*s, &params)))
            .expect("some seed separates MostRecent from FirstNonNull");
        let history = generate(seed, &params);

        // Hand-rolled shrink against the mismatched pair, mirroring `shrink`.
        let mut best = history.clone();
        loop {
            let mut improved = false;
            for i in 0..best.len() {
                let mut c = best.clone();
                c.remove(i);
                if !c.is_empty() && !wrong(&c) {
                    best = c;
                    improved = true;
                    break;
                }
            }
            if !improved {
                break;
            }
        }
        assert!(
            best.len() < history.len(),
            "shrinking removed nothing from a {}-assertion history",
            history.len()
        );
    }
}
