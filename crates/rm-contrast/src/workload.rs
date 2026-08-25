//! A generated history, the questions to ask of it, and what the answers are.
//!
//! Its own generator rather than `rm-conform`'s, and its own copy of SplitMix64
//! rather than an import. `rm-conform` gives the reason for its own fixtures: a
//! generator two measurements can reconfigure is coupling neither can see. That
//! matters more here, because a change to `rm-conform`'s generator made for a
//! correctness reason could silently move this crate's headline number.

use rm_core::{Interval, Provenance, Source, Supersession, Timestamp};

/// The one attribute every generated write is about.
///
/// One attribute, because the axes under test are time and arrival order.
/// Several would multiply the workload without varying anything measured.
pub const ATTRIBUTE: &str = "employer";

/// SplitMix64, copied deliberately -- see the module comment.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "below(0) has no answer");
        self.next_u64() % n
    }
}

/// How much world to build, and what to ask of it.
#[derive(Clone, Debug)]
pub struct Params {
    /// How many writes.
    pub len: usize,
    /// How many distinct values compete. Small, so values actually change.
    pub alphabet: u64,
    /// Percent of writes whose valid time precedes their arrival. The x-axis.
    pub backdate_pct: u64,
    /// How many entities the writes are spread across.
    pub entities: usize,
    /// How many questions to ask.
    pub queries: usize,
    /// Percent of questions about a past instant. The y-axis.
    pub retrospective_pct: u64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            len: 24,
            alphabet: 4,
            backdate_pct: 0,
            entities: 3,
            queries: 40,
            retrospective_pct: 0,
        }
    }
}

/// One write: a value that began to hold at `valid_from`, heard at `observed_at`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Write {
    pub entity: usize,
    /// `None` is a tombstone: someone said there is none.
    pub value: Option<String>,
    pub valid_from: Timestamp,
    pub observed_at: Timestamp,
}

/// One question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub entity: usize,
    pub valid_t: Timestamp,
    pub tx_t: Timestamp,
}

/// A generated history and the questions to ask of it.
pub struct Workload {
    pub writes: Vec<Write>,
    pub queries: Vec<Query>,
}

/// What the answer to a question actually is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Truth {
    /// One value had begun to hold. `None` inside is a tombstone.
    Value(Option<String>),
    /// Two different values began at the same instant and were heard at the
    /// same instant, so nothing orders them. There is no answer to get right,
    /// which is exactly where refusal lives.
    Ambiguous,
    /// Nothing had been heard, or nothing had begun to hold.
    Nothing,
}

/// The clock the generated history runs on.
const FIRST: Timestamp = 1_000;
const STEP: Timestamp = 100;

/// Build one workload from a seed.
pub fn workload(seed: u64, params: &Params) -> Workload {
    let mut rng = Rng::new(seed);
    let mut writes = Vec::with_capacity(params.len);

    for i in 0..params.len {
        let observed_at = FIRST + (i as Timestamp) * STEP;
        // Backdated writes claim to have begun somewhere earlier in the
        // history: heard in September, true since July.
        let valid_from = if rng.below(100) < params.backdate_pct && i > 0 {
            FIRST + (rng.below(i as u64) as Timestamp) * STEP
        } else {
            observed_at
        };
        let value = {
            let n = rng.below(params.alphabet + 1);
            if n == params.alphabet {
                None // a tombstone
            } else {
                Some(format!("value {n}"))
            }
        };
        writes.push(Write {
            entity: rng.below(params.entities as u64) as usize,
            value,
            valid_from,
            observed_at,
        });
    }

    let last = FIRST + (params.len as Timestamp) * STEP;
    let mut queries = Vec::with_capacity(params.queries);
    for _ in 0..params.queries {
        let entity = rng.below(params.entities as u64) as usize;
        if rng.below(100) < params.retrospective_pct {
            // Inside the history, on both axes independently.
            let valid_t = FIRST + (rng.below(params.len as u64) as Timestamp) * STEP;
            let tx_t = FIRST + (rng.below(params.len as u64) as Timestamp) * STEP;
            queries.push(Query {
                entity,
                valid_t,
                tx_t,
            });
        } else {
            queries.push(Query {
                entity,
                valid_t: last,
                tx_t: last,
            });
        }
    }

    Workload { writes, queries }
}

/// What the true answer to `q` is, computed from what was written.
///
/// Only writes the store could have heard by `q.tx_t` count, and among those
/// only the ones that had begun to hold by `q.valid_t`. The winner is the one
/// with the greatest `valid_from`, ties broken by `observed_at` -- and when
/// *both* collide on two different values, nothing orders them.
pub fn truth(w: &Workload, q: &Query) -> Truth {
    let mut visible: Vec<&Write> = w
        .writes
        .iter()
        .filter(|x| x.entity == q.entity && x.observed_at <= q.tx_t && x.valid_from <= q.valid_t)
        .collect();
    if visible.is_empty() {
        return Truth::Nothing;
    }
    visible.sort_by_key(|x| (x.valid_from, x.observed_at));
    let winner = visible[visible.len() - 1];
    // Anything sharing both clocks with the winner and disagreeing with it
    // leaves the question unanswerable.
    if visible.iter().any(|x| {
        x.valid_from == winner.valid_from
            && x.observed_at == winner.observed_at
            && x.value != winner.value
    }) {
        return Truth::Ambiguous;
    }
    Truth::Value(winner.value.clone())
}

/// The pieces `rm_engine::Observation` needs that do not vary here.
///
/// `pub(crate)` so `score.rs` builds the engine the same way every time.
#[allow(dead_code)] // callers arrive in Task 3
pub(crate) fn provenance(observed_at: Timestamp) -> Provenance {
    Provenance {
        source: Source::UserAssertion,
        observed_at,
        // A `String`, not an `Option<String>` -- verified against
        // `rm-core/src/lib.rs:87`, not assumed.
        source_ref: "contrast".to_string(),
    }
}

/// The interval a write claims, open-ended from when it began to hold.
#[allow(dead_code)] // callers arrive in Task 3
pub(crate) fn interval(valid_from: Timestamp) -> Interval {
    Interval {
        from: valid_from,
        to: None,
    }
}

/// Every write states nothing about what the slot already held.
///
/// `Unstated`, which is what `rm-conform`'s generator uses for the same reason:
/// the store does not read it during survivorship, and claiming `Corrects` or
/// `Joins` would assert something the generator has no basis for. There is no
/// `Extends` variant -- the enum is `Corrects`, `Joins`, `Unstated`.
#[allow(dead_code)] // callers arrive in Task 3
pub(crate) fn supersession() -> Supersession {
    Supersession::Unstated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workload_is_deterministic_and_shaped_by_its_params() {
        let params = Params::default();
        let a = workload(3, &params);
        let b = workload(3, &params);
        assert_eq!(a.writes.len(), b.writes.len(), "same seed, same workload");
        assert_eq!(a.queries.len(), params.queries);
        assert_eq!(a.writes.len(), params.len);
        assert!(a.writes.iter().all(|w| w.entity < params.entities));

        let c = workload(4, &params);
        assert!(
            a.writes
                .iter()
                .zip(&c.writes)
                .any(|(x, y)| x.value != y.value || x.valid_from != y.valid_from),
            "different seed, different workload"
        );
    }

    /// The x-axis has to move something. At 0% every write's valid time is its
    /// arrival time; above it, some arrive out of order.
    #[test]
    fn the_backdate_rate_actually_produces_out_of_order_arrival() {
        let none = Params {
            backdate_pct: 0,
            ..Params::default()
        };
        for seed in 0..20 {
            let w = workload(seed, &none);
            assert!(
                w.writes.iter().all(|x| x.valid_from == x.observed_at),
                "no backdating means valid time is arrival time"
            );
        }

        let lots = Params {
            backdate_pct: 60,
            ..Params::default()
        };
        let out_of_order = (0..20).any(|seed| {
            let w = workload(seed, &lots);
            w.writes.iter().any(|x| x.valid_from < x.observed_at)
        });
        assert!(out_of_order, "backdating produced no out-of-order arrival");
    }

    /// The y-axis likewise. At 0% every query asks about the end of time; at
    /// 100% every one asks about a past instant.
    #[test]
    fn the_retrospective_share_actually_moves_the_queries() {
        let now = Params {
            retrospective_pct: 0,
            ..Params::default()
        };
        let w = workload(1, &now);
        let latest = w.writes.iter().map(|x| x.observed_at).max().unwrap();
        assert!(
            w.queries
                .iter()
                .all(|q| q.valid_t >= latest && q.tx_t >= latest),
            "a present-tense query asks about after everything happened"
        );

        let past = Params {
            retrospective_pct: 100,
            ..Params::default()
        };
        let w = workload(1, &past);
        assert!(
            w.queries
                .iter()
                .all(|q| q.valid_t < latest || q.tx_t < latest),
            "a retrospective query asks about an instant inside the history"
        );
    }

    /// Truth is computed from what was written, and it has three answers --
    /// the third is where refusal lives.
    #[test]
    fn truth_is_the_latest_value_that_had_begun_to_hold() {
        let w = Workload {
            writes: vec![
                Write {
                    entity: 0,
                    value: Some("Acme".into()),
                    valid_from: 100,
                    observed_at: 100,
                },
                Write {
                    entity: 0,
                    value: Some("Globex".into()),
                    valid_from: 300,
                    observed_at: 300,
                },
            ],
            queries: vec![],
        };
        let at = |valid_t, tx_t| {
            truth(
                &w,
                &Query {
                    entity: 0,
                    valid_t,
                    tx_t,
                },
            )
        };

        assert_eq!(at(50, 1_000), Truth::Nothing, "before anything held");
        assert_eq!(at(200, 1_000), Truth::Value(Some("Acme".into())));
        assert_eq!(at(400, 1_000), Truth::Value(Some("Globex".into())));
        // Transaction time: the store had not been told the second one yet.
        assert_eq!(at(400, 200), Truth::Value(Some("Acme".into())));
        assert_eq!(at(400, 50), Truth::Nothing, "before it was told anything");
    }

    #[test]
    fn two_values_beginning_at_the_same_instant_are_ambiguous() {
        let w = Workload {
            writes: vec![
                Write {
                    entity: 0,
                    value: Some("Acme".into()),
                    valid_from: 100,
                    observed_at: 100,
                },
                Write {
                    entity: 0,
                    value: Some("Globex".into()),
                    valid_from: 100,
                    observed_at: 100,
                },
            ],
            queries: vec![],
        };
        assert_eq!(
            truth(
                &w,
                &Query {
                    entity: 0,
                    valid_t: 200,
                    tx_t: 1_000
                }
            ),
            Truth::Ambiguous,
            "nothing orders them, so there is no true answer to get right"
        );
    }
}
