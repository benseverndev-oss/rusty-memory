//! Proves the graph re-exports are sufficient, the same way `readme.rs` proves
//! it for `remember`/`recall`/`forget`.
//!
//! One `use`, from one crate. A caller composing a [`Walk`] and reading back a
//! [`Neighborhood`] of [`Reached`] entities, choosing a [`Direction`], and
//! matching on [`EngineError::UnknownEntity`] must be able to do all of that
//! by naming only `rm_engine` types — never `rm-graph` or `rm-store` in this
//! crate's own manifest. If this file needed a second dependency to compile,
//! task 6's re-export list would be incomplete.

use rm_engine::{
    BlockingKey, Comparator, Direction, Engine, EngineError, FieldRule, Interval, Metric,
    Neighborhood, Observation, Policy, Provenance, Reached, Record, Remembered, Ruleset, Source,
    Strategy, VectorIndex, Walk,
};

fn ruleset() -> Ruleset {
    Ruleset::new(
        vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
        vec![BlockingKey::Prefix("name".to_string(), 3)],
        4.0,
        8.0,
    )
    .unwrap()
}

fn engine() -> Engine {
    Engine::new(
        VectorIndex::new(3, Metric::Cosine),
        ruleset(),
        Policy::new(Strategy::MostRecent),
    )
}

fn person(name: &str, at: i64) -> Observation {
    Observation {
        kind: "person".to_string(),
        mention: Record::new().with("name", name),
        attribute: "kind".to_string(),
        value: Some("person".to_string()),
        valid: Interval::since(at),
        provenance: Provenance::new(Source::UserAssertion, at, "s"),
        embedding: vec![1.0, 0.0, 0.0],
    }
}

#[test]
fn a_caller_can_compose_a_walk_from_rm_engine_alone() {
    let mut e = engine();
    let Remembered::Created { entity: alice, .. } = e.remember(person("Alice", 1)).unwrap() else {
        panic!("setup")
    };
    let Remembered::Created { entity: bob, .. } = e.remember(person("Bob", 1)).unwrap() else {
        panic!("setup")
    };

    e.relate(
        alice,
        "knows",
        bob,
        Interval::since(1),
        Provenance::new(Source::UserAssertion, 1, "s"),
    )
    .unwrap();

    let walk = Walk::new(vec![alice], 1, 10, 5, 5).direction(Direction::Both);
    let found: Neighborhood = e.neighborhood(&walk);
    let hit: &Reached = found
        .reached
        .iter()
        .find(|r| r.entity == bob)
        .expect("bob is one hop out");
    assert_eq!(hit.distance, 1);
    assert!(!found.truncated);

    e.unrelate(
        alice,
        "knows",
        bob,
        5,
        Provenance::new(Source::UserAssertion, 5, "s2"),
    )
    .unwrap();
    assert_eq!(
        e.neighborhood(&Walk::new(vec![alice], 1, 10, 9, 9))
            .reached
            .len(),
        1
    );

    e.relate(
        alice,
        "knows",
        bob,
        Interval::since(1),
        Provenance::new(Source::UserAssertion, 1, "s"),
    )
    .unwrap();
    assert_eq!(e.erase_edges(alice).unwrap(), 1);
    assert_eq!(
        e.erase_edges(9999),
        Err(EngineError::UnknownEntity(9999)),
        "the error variant is nameable without a second crate too"
    );
}
