//! The read path, not just the merge underneath it.
//!
//! Everything up to here scored `rm_survivor` alone. This adds what sits on top
//! of it: history assembly and the `as_of` filtering on both axes.
//!
//! Entities are pinned with `remember_as` rather than resolved. Resolution is
//! out of scope by design -- generated names would measure the generator's name
//! distribution and call it a resolver score -- and pinning is how it stays out.
//! Embeddings are irrelevant to survivorship, so every observation carries the
//! same fixed vector.

use crate::history::Assertion;
use rm_core::Timestamp;
use rm_engine::{Believed, Engine, Observation, Policy, StableId, Strategy};
use rm_index::{Metric, VectorIndex};
use rm_resolve::{BlockingKey, Comparator, FieldRule, Record, Ruleset};

/// A ruleset that resolves nothing interesting. Entities are pinned by id, so
/// this exists only because `Engine::new` requires one.
fn ruleset() -> Ruleset {
    Ruleset::new(
        vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
        vec![BlockingKey::Prefix("name".to_string(), 3)],
        4.0,
        8.0,
    )
    .expect("a one-field ruleset is valid")
}

/// An engine holding `history` on one attribute of one entity.
pub fn build(history: &[Assertion], attribute: &str, strategy: Strategy) -> (Engine, StableId) {
    let mut engine = Engine::new(
        VectorIndex::new(3, Metric::Cosine),
        ruleset(),
        Policy::new(strategy),
    );
    let mut entity = None;
    for a in history {
        let obs = Observation {
            kind: "thing".to_string(),
            mention: Record::new().with("name", "subject"),
            attribute: attribute.to_string(),
            value: a.value.clone(),
            valid: a.valid,
            provenance: a.provenance.clone(),
            supersession: a.supersession,
            according_to: None,
            embedding: vec![1.0, 0.0, 0.0],
        };
        let (id, _) = engine.remember_as(entity, obs).expect("pinned write");
        entity = Some(id);
    }
    (engine, entity.expect("history is non-empty"))
}

/// `Believed` compared with what the reference says, mirroring the read path's
/// own order of operations.
///
/// The order matters and this got it wrong first time. `Engine::about_under`
/// filters by **transaction time only**, merges everything known by `tx_t`, and
/// applies `held_at(valid_t)` to the *outcome*. It does not pre-filter
/// candidates by valid time. Pre-filtering makes the store look as though it
/// answers "what was true then" under every strategy, when in fact only
/// `ValidInterval` does.
pub fn probe_agreement(history: &[Assertion], valid_t: Timestamp, tx_t: Timestamp) -> bool {
    let (engine, id) = build(history, "attr", Strategy::MostRecent);
    let answered = engine.about(id, "attr", valid_t, tx_t);

    // Transaction time only: later knowledge does not leak backwards.
    let visible: Vec<Assertion> = history
        .iter()
        .filter(|a| a.provenance.observed_at <= tx_t)
        .cloned()
        .collect();
    if visible.is_empty() {
        return answered.ok() == Some(Believed::Unknown);
    }
    let candidates: Vec<_> = visible.iter().map(|a| a.candidate()).collect();

    // A refusal is a defined answer here as everywhere else, and it propagates
    // out of `about_under` rather than falling back to a looser rule. Compare
    // refusal-to-refusal, never by message.
    let outcome = match (
        answered,
        crate::reference::merge(&candidates, &Strategy::MostRecent),
    ) {
        (Err(_), Err(_)) => return true,
        (Ok(_), Err(_)) | (Err(_), Ok(_)) => return false,
        (Ok(_), Ok(o)) => o,
    };
    let believed = engine
        .about(id, "attr", valid_t, tx_t)
        .expect("just checked it answers");
    let expected = match crate::reference::held_at(&outcome, valid_t)
        .expect("MostRecent yields a Survivor, which never refuses at an instant")
    {
        Some(rm_survivor::Held::Value(v)) => Believed::Value(v.clone()),
        Some(rm_survivor::Held::Absent) => Believed::Absent,
        None => Believed::Unknown,
    };
    believed == expected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::{generate, Params};

    #[test]
    fn a_backdated_fact_is_true_from_when_it_happened() {
        let history = vec![
            Assertion::new("fly.io", 100, 100),
            Assertion::new("render", 200, 900),
        ];
        let (engine, id) = build(&history, "attr", Strategy::MostRecent);

        // Knowing everything, asked about t=250: the backdated correction holds.
        assert_eq!(
            engine.about(id, "attr", 250, 1000).unwrap(),
            Believed::Value("render".to_string())
        );
        // Knowing only what was said by t=500: not yet heard, so the old value.
        assert_eq!(
            engine.about(id, "attr", 250, 500).unwrap(),
            Believed::Value("fly.io".to_string())
        );
    }

    #[test]
    fn the_read_path_agrees_with_the_reference_across_probes() {
        let params = Params::default();
        for seed in 0..50 {
            let history = generate(seed, &params);
            for valid_t in [900, 1_050, 1_200, 1_500, 3_000] {
                for tx_t in [1_000, 1_100, 1_400, 5_000] {
                    assert!(
                        probe_agreement(&history, valid_t, tx_t),
                        "seed {seed} valid_t {valid_t} tx_t {tx_t}"
                    );
                }
            }
        }
    }

    #[test]
    fn valid_time_is_inert_under_the_default_strategy() {
        // Found by modelling the read path faithfully, and worth pinning
        // because it is invisible from the outside.
        //
        // `about` takes `valid_t`, and under `Strategy::MostRecent` the outcome
        // is a `Survivor`, which has no time dimension -- so `held_at` returns
        // the same value for every `valid_t` ever passed. That is coherent
        // (`MostRecent` answers "which value survives", not "what was true
        // when") but it has a sharp edge: `rmem.toml`'s template ships
        // `[policy] default = "most_recent"` with only `employer` set to
        // `valid_interval`, and `rmem about` advertises `--valid-at` as
        // "asks what was true then".
        //
        // So on every attribute but one, that flag is accepted and does
        // nothing, and nothing says so.
        let history = vec![
            Assertion::new("fly.io", 100, 100),
            Assertion::new("render", 200, 200),
        ];
        let (engine, id) = build(&history, "attr", Strategy::MostRecent);

        let answers: Vec<Believed> = [0, 150, 250, 10_000]
            .iter()
            .map(|valid_t| engine.about(id, "attr", *valid_t, 10_000).unwrap())
            .collect();

        assert!(
            answers.iter().all(|a| *a == answers[0]),
            "valid_t changed the answer under MostRecent, which would mean this \
             note is out of date: {answers:?}"
        );
        assert_eq!(answers[0], Believed::Value("render".to_string()));
    }

    #[test]
    fn valid_time_does_bite_under_valid_interval() {
        // The mirror, so the test above is read as "this strategy ignores it"
        // rather than "the store ignores it".
        let history = vec![
            Assertion::new("fly.io", 100, 100),
            Assertion::new("render", 200, 200),
        ];
        let (engine, id) = build(&history, "attr", Strategy::ValidInterval);

        assert_eq!(
            engine.about(id, "attr", 150, 10_000).unwrap(),
            Believed::Value("fly.io".to_string())
        );
        assert_eq!(
            engine.about(id, "attr", 250, 10_000).unwrap(),
            Believed::Value("render".to_string())
        );
    }

    #[test]
    fn the_probe_grid_reaches_more_than_one_answer() {
        // A grid on which the store always says Unknown would pass the test
        // above while comparing nothing.
        let params = Params::default();
        let mut seen: Vec<Believed> = Vec::new();
        for seed in 0..50 {
            let history = generate(seed, &params);
            let (engine, id) = build(&history, "attr", Strategy::MostRecent);
            for valid_t in [900, 1_050, 1_200, 1_500, 3_000] {
                for tx_t in [1_000, 1_100, 1_400, 5_000] {
                    if let Ok(b) = engine.about(id, "attr", valid_t, tx_t) {
                        if !seen.contains(&b) {
                            seen.push(b);
                        }
                    }
                }
            }
        }
        assert!(
            seen.len() > 1,
            "every probe returned the same answer: {seen:?}"
        );
    }
}
