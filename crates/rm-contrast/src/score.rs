//! Both stores answer the same questions, and are marked the same way.

use rm_engine::{
    Believed, Engine, Metric, Observation, Policy, Record, StableId, Strategy, VectorIndex,
};
use rm_resolve::{BlockingKey, Comparator, FieldRule, Ruleset};

use crate::flat::Flat;
use crate::workload::{interval, provenance, supersession, truth, Truth, Workload, ATTRIBUTE};

/// Right, wrong, and declined.
///
/// Three outcomes rather than two. A refusal is neither a right answer nor a
/// wrong one: it is the store saying nothing orders these candidates. Counting
/// it as an error punishes the most distinctive behaviour this project built;
/// counting it as a success rigs the result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Score {
    pub right: usize,
    pub wrong: usize,
    pub declined: usize,
}

impl Score {
    pub fn total(&self) -> usize {
        self.right + self.wrong + self.declined
    }

    /// `right / (right + wrong + declined)`.
    ///
    /// Declined stays in the denominator. Removing it would flatter the store
    /// on exactly the axis it is most distinctive, and a declined question is
    /// still one the caller did not get an answer to.
    pub fn accuracy(&self) -> f64 {
        if self.total() == 0 {
            return 1.0;
        }
        self.right as f64 / self.total() as f64
    }
}

fn ruleset() -> Ruleset {
    Ruleset::new(
        vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
        vec![BlockingKey::Prefix("name".to_string(), 3)],
        4.0,
        8.0,
    )
    .expect("a one-field ruleset is valid")
}

/// Load `w` into a real engine, one pinned entity per generated entity.
///
/// `remember_as` with a fixed vector: entities are pinned rather than resolved,
/// for the reason `rm-conform` gives -- generated names would measure the
/// generator's name distribution and call it a resolver score -- and embeddings
/// are irrelevant to survivorship, so every observation carries the same one.
/// No embedder, no network, no key.
fn load(w: &Workload) -> (Engine, Vec<StableId>) {
    let mut engine = Engine::new(
        VectorIndex::new(3, Metric::Cosine),
        ruleset(),
        // ValidInterval: the strategy under test. MostRecent collapses to a
        // winner and would answer every valid time the same, which is the
        // control's behaviour rather than this store's.
        Policy::new(Strategy::ValidInterval),
    );
    let widest = w.writes.iter().map(|x| x.entity).max().unwrap_or(0) + 1;
    let mut ids: Vec<Option<StableId>> = vec![None; widest];
    for write in &w.writes {
        let obs = Observation {
            kind: "thing".to_string(),
            mention: Record::new().with("name", format!("subject {}", write.entity).as_str()),
            attribute: ATTRIBUTE.to_string(),
            value: write.value.clone(),
            valid: interval(write.valid_from),
            provenance: provenance(write.observed_at),
            supersession: supersession(),
            embedding: vec![1.0, 0.0, 0.0],
        };
        let (id, _) = engine
            .remember_as(ids[write.entity], obs)
            .expect("a pinned write cannot fail");
        ids[write.entity] = Some(id);
    }
    (
        engine,
        ids.into_iter().map(|i| i.unwrap_or_default()).collect(),
    )
}

enum Outcome {
    Right,
    Wrong,
    Declined,
}

/// Mark one answer against the truth.
fn mark(answer: Option<Option<String>>, truth: &Truth) -> Outcome {
    match (answer, truth) {
        // Nothing to get right: the question has no answer.
        (_, Truth::Ambiguous) => Outcome::Declined,
        (Some(v), Truth::Value(t)) if &v == t => Outcome::Right,
        (None, Truth::Nothing) => Outcome::Right,
        _ => Outcome::Wrong,
    }
}

fn tally(outcomes: impl Iterator<Item = Outcome>) -> Score {
    let mut s = Score::default();
    for o in outcomes {
        match o {
            Outcome::Right => s.right += 1,
            Outcome::Wrong => s.wrong += 1,
            Outcome::Declined => s.declined += 1,
        }
    }
    s
}

/// This store, asked with both clocks.
pub fn score_store(w: &Workload) -> Score {
    let (engine, ids) = load(w);
    tally(w.queries.iter().map(|q| {
        match engine.about(ids[q.entity], ATTRIBUTE, q.valid_t, q.tx_t) {
            // A refusal from survivorship is a decline, whatever the truth is.
            Err(_) => Outcome::Declined,
            Ok(Believed::Unknown) => mark(None, &truth(w, q)),
            Ok(Believed::Absent) => mark(Some(None), &truth(w, q)),
            Ok(Believed::Value(v)) => mark(Some(Some(v)), &truth(w, q)),
        }
    }))
}

/// The control, asked the only way it can be.
///
/// It sees the writes in arrival order and is then asked every question,
/// including the retrospective ones, which it answers with what it holds.
pub fn score_flat(w: &Workload) -> Score {
    let mut flat = Flat::new();
    for write in &w.writes {
        flat.remember(write.entity as StableId, ATTRIBUTE, write.value.as_deref());
    }
    tally(w.queries.iter().map(|q| {
        let answer = flat.about(q.entity as StableId, ATTRIBUTE);
        mark(answer, &truth(w, q))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workload::{workload, Params};

    /// The calibration cell in miniature. With no backdating and no
    /// retrospective queries, a latest-wins store is exactly the right tool and
    /// must get everything right.
    #[test]
    fn both_stores_are_perfect_on_an_in_order_present_tense_workload() {
        let params = Params::default();
        for seed in 0..20 {
            let w = workload(seed, &params);
            let flat = score_flat(&w);
            let store = score_store(&w);
            assert_eq!(
                flat.accuracy(),
                1.0,
                "seed {seed}: the control failed a workload it is built for, \
                 so the benchmark is unfair rather than the control weak"
            );
            assert_eq!(store.accuracy(), 1.0, "seed {seed}");
        }
    }

    /// Accuracy counts declined against you. A declined question is one the
    /// caller did not get an answer to.
    #[test]
    fn accuracy_keeps_declined_in_the_denominator() {
        let s = Score {
            right: 3,
            wrong: 1,
            declined: 1,
        };
        assert_eq!(s.total(), 5);
        assert!((s.accuracy() - 0.6).abs() < 1e-9);
    }

    /// The control cannot decline. It has no way to.
    #[test]
    fn the_control_never_declines() {
        let params = Params {
            backdate_pct: 60,
            retrospective_pct: 100,
            ..Params::default()
        };
        for seed in 0..20 {
            assert_eq!(score_flat(&workload(seed, &params)).declined, 0);
        }
    }
}
