//! Generated histories.
//!
//! Difficulty is a knob rather than a rewrite: length, how many values compete,
//! how often a fact is backdated, how often two assertions share an instant.
//!
//! Timestamp ties get their own parameter because three strategies are
//! specified to refuse on them, and a generator that never produced one would
//! leave that code unreached while reporting a green suite.

use crate::history::Assertion;
use crate::rng::Rng;
use rm_core::{Interval, Provenance, Source, Supersession};

#[derive(Clone, Debug)]
pub struct Params {
    pub len: usize,
    /// How many distinct values compete. Small, so collisions are frequent
    /// rather than rare -- a corpus where nothing ever contradicts anything
    /// would exercise none of what is being measured.
    pub alphabet: u64,
    /// Percent of assertions whose valid time precedes their observation.
    pub backdate_pct: u64,
    /// Percent of assertions that reuse the previous observation timestamp.
    pub tie_pct: u64,
    /// Percent of assertions that are tombstones.
    pub tombstone_pct: u64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            len: 12,
            alphabet: 4,
            backdate_pct: 30,
            tie_pct: 15,
            tombstone_pct: 10,
        }
    }
}

/// A history of `params.len` assertions, reproducible from `seed`.
pub fn generate(seed: u64, params: &Params) -> Vec<Assertion> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(params.len);
    let mut clock: i64 = 1_000;

    for _ in 0..params.len {
        let tie = rng.chance(params.tie_pct) && !out.is_empty();
        if !tie {
            clock += 1 + rng.below(50) as i64;
        }
        let observed_at = clock;

        let valid_from = if rng.chance(params.backdate_pct) {
            observed_at - 1 - rng.below(500) as i64
        } else {
            observed_at
        };

        let value = if rng.chance(params.tombstone_pct) {
            None
        } else {
            Some(format!("v{}", rng.below(params.alphabet)))
        };

        let supersession = match rng.below(3) {
            0 => Supersession::Corrects,
            1 => Supersession::Joins,
            _ => Supersession::Unstated,
        };

        out.push(Assertion {
            value,
            valid: Interval::since(valid_from),
            provenance: Provenance::new(Source::UserAssertion, observed_at, "conform"),
            supersession,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_history() {
        let p = Params::default();
        assert_eq!(generate(99, &p), generate(99, &p));
    }

    #[test]
    fn different_seeds_give_different_histories() {
        let p = Params::default();
        assert_ne!(generate(1, &p), generate(2, &p));
    }

    #[test]
    fn it_produces_the_requested_length() {
        let p = Params {
            len: 25,
            ..Params::default()
        };
        assert_eq!(generate(5, &p).len(), 25);
    }

    #[test]
    fn ties_actually_occur_at_the_configured_rate() {
        // The refusal paths are unreachable without these, so assert they
        // exist rather than hoping.
        let p = Params {
            len: 200,
            tie_pct: 50,
            ..Params::default()
        };
        let h = generate(3, &p);
        assert!(
            h.windows(2)
                .any(|w| w[0].provenance.observed_at == w[1].provenance.observed_at),
            "no timestamp tie in 200 assertions at tie_pct=50"
        );
    }

    #[test]
    fn ties_do_not_occur_when_switched_off() {
        let p = Params {
            len: 200,
            tie_pct: 0,
            ..Params::default()
        };
        let h = generate(3, &p);
        assert!(h
            .windows(2)
            .all(|w| w[0].provenance.observed_at != w[1].provenance.observed_at));
    }

    #[test]
    fn backdating_actually_occurs() {
        let p = Params {
            len: 200,
            backdate_pct: 50,
            ..Params::default()
        };
        let h = generate(4, &p);
        assert!(h.iter().any(|a| a.valid.from < a.provenance.observed_at));
    }

    #[test]
    fn tombstones_actually_occur() {
        let p = Params {
            len: 200,
            tombstone_pct: 40,
            ..Params::default()
        };
        assert!(generate(6, &p).iter().any(|a| a.value.is_none()));
    }
}
