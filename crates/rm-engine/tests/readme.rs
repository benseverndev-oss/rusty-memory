//! The project's central claim, through the public API only.
//!
//! One `use`, from one crate. That is the point of the file as much as the
//! assertions are: if this test needs a `pub(crate)` to compile the API is
//! missing something a caller will need, and if it needs a second crate in the
//! manifest then so does everyone who wants to call `remember`.

use rm_engine::{
    Believed, BlockingKey, Comparator, Engine, FieldRule, Interval, Metric, Observation, Policy,
    Provenance, Query, Record, Remembered, Ruleset, Source, Standing, Strategy, Supersession,
    VectorIndex, Version,
};

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
        // A person has one employer at a time, so a later one replaces the
        // last. Saying so is what lets the recall below report a correction
        // rather than the mere fact that something arrived afterwards.
        supersession: Supersession::Corrects,
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

    // Both remain recallable, and the corrected one says so.
    let hits = engine.recall(&Query::new(vec![1.0, 0.0, 0.0], 5)).unwrap();
    assert_eq!(hits.len(), 2);
    let acme = hits
        .iter()
        .find(|h| h.value.as_deref() == Some("Acme"))
        .unwrap();
    assert_eq!(acme.standing, Standing::Corrected);
    assert!(!acme.standing.still_stands());

    // And the raw audit trail underneath, whose element type a caller can name
    // without taking a dependency on the store crate.
    let history: &[Version] = engine.store_history(entity, "employer");
    assert_eq!(history.len(), 2, "two facts, neither overwritten");
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
