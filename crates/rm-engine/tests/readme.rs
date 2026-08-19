//! The project's central claim, through the public API only.
//!
//! Everything here uses `rm_engine`'s exported surface. If this test needs a
//! `pub(crate)` to compile, the API is missing something a caller will need.

use rm_core::{Interval, Provenance, Source};
use rm_engine::{Believed, Engine, Observation, Policy, Query, Remembered};
use rm_index::{Metric, VectorIndex};
use rm_resolve::{BlockingKey, Comparator, FieldRule, Record, Ruleset};
use rm_survivor::Strategy;

const MARCH: i64 = 1_710_000_000_000;
const MAY: i64 = 1_715_000_000_000;
const JULY: i64 = 1_720_000_000_000;
const AUGUST: i64 = 1_725_000_000_000;

// Two identical "Ben Severn" name records score log2(0.9/0.01) ≈ 6.49 bits
// under this rule. `match_at` has to sit below that or the second `remember`
// below lands in the review band instead of merging outright.
fn ruleset() -> Ruleset {
    Ruleset::new(
        vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
        vec![BlockingKey::Prefix("name".to_string(), 3)],
        4.0,
        6.0,
    )
    .unwrap()
}

fn told(employer: &str, at: i64, session: &str, embedding: [f32; 3]) -> Observation {
    Observation {
        kind: "person".to_string(),
        mention: Record::new().with("name", "Ben Severn"),
        attribute: "employer".to_string(),
        value: Some(employer.to_string()),
        valid: Interval::since(at),
        provenance: Provenance::new(Source::UserAssertion, at, session),
        embedding: embedding.to_vec(),
    }
}

#[test]
fn a_change_of_employer_is_two_facts_not_a_contradiction() {
    let mut engine = Engine::new(
        VectorIndex::new(3, Metric::Cosine),
        ruleset(),
        Policy::new(Strategy::ValidInterval),
    );

    let Remembered::Created { entity, .. } = engine
        .remember(told("Acme", MARCH, "session-1", [1.0, 0.0, 0.0]))
        .unwrap()
    else {
        panic!("the first thing we hear about someone is a new entity");
    };

    // Same person, months later. Resolution recognises them.
    let out = engine
        .remember(told("Globex", JULY, "session-9", [0.9, 0.1, 0.0]))
        .unwrap();
    assert!(matches!(out, Remembered::Merged { .. }), "got {out:?}");
    assert_eq!(engine.entity_count(), 1);

    // Neither fact was discarded. The store answers by time.
    assert_eq!(
        engine.about(entity, "employer", MAY, AUGUST).unwrap(),
        Believed::Value("Acme".into())
    );
    assert_eq!(
        engine.about(entity, "employer", AUGUST, AUGUST).unwrap(),
        Believed::Value("Globex".into())
    );

    // Both remain recallable, and the superseded one says so.
    let hits = engine.recall(&Query::new(vec![1.0, 0.0, 0.0], 5)).unwrap();
    assert_eq!(hits.len(), 2);
    let acme = hits
        .iter()
        .find(|h| h.value.as_deref() == Some("Acme"))
        .unwrap();
    assert!(acme.superseded);
}

#[test]
fn what_the_agent_knew_in_may_is_not_rewritten_by_what_it_learns_in_july() {
    let mut engine = Engine::new(
        VectorIndex::new(3, Metric::Cosine),
        ruleset(),
        Policy::new(Strategy::ValidInterval),
    );
    let Remembered::Created { entity, .. } = engine
        .remember(told("Acme", MARCH, "session-1", [1.0, 0.0, 0.0]))
        .unwrap()
    else {
        panic!("setup")
    };
    engine
        .remember(told("Globex", JULY, "session-9", [0.9, 0.1, 0.0]))
        .unwrap();

    // Asked as of May, the July conversation has not happened yet.
    assert_eq!(
        engine.about(entity, "employer", MAY, MAY).unwrap(),
        Believed::Value("Acme".into())
    );
}
