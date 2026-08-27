//! Proves the graph re-exports are sufficient, the same way `readme.rs` proves
//! it for `remember`/`recall`/`forget` — and tells the story the whole crate
//! exists for: a two-hop walk answers a question about an entity that names
//! neither of the entities it actually depends on, which semantic recall
//! alone cannot reach.
//!
//! One `use`, from one crate. A caller composing a [`Walk`] and reading back a
//! [`Neighborhood`] of [`Reached`] entities, choosing a [`Direction`], reading
//! an [`Edge`] or the [`EdgeVersion`]s behind it, and matching on
//! [`EngineError::UnknownEntity`] must be able to do all of that by naming
//! only `rm_engine` types — never `rm-graph` or `rm-store` in this crate's own
//! manifest. If this file needed a second dependency to compile, task 6's
//! re-export list would be incomplete.

use rm_engine::{
    BlockingKey, Comparator, Direction, Edge, EdgeVersion, Engine, EngineError, FieldRule,
    Interval, Metric, Neighborhood, Observation, Policy, Provenance, Reached, Record, Remembered,
    Ruleset, Source, Strategy, Supersession, VectorIndex, Walk,
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
        supersession: Supersession::Unstated,
        according_to: None,
        embedding: vec![1.0, 0.0, 0.0],
    }
}

/// An observation of `attribute: value` for `name`, modelling one of the
/// distinct things a two-hop question runs through — a person, a company, a
/// city — each with its own kind, attribute, value and embedding.
///
/// The embedding is a parameter because the entities are different things and
/// giving them all the same vector would say otherwise; it is *not* what keeps
/// them apart under resolution. Blocking here is name-prefix only and the
/// comparator scores `name`, so embeddings play no part in matching at all —
/// the distinct names are what stop these three colliding.
fn seen(name: &str, attribute: &str, value: &str, at: i64, v: [f32; 3]) -> Observation {
    Observation {
        kind: "thing".to_string(),
        mention: Record::new().with("name", name),
        attribute: attribute.to_string(),
        value: Some(value.to_string()),
        valid: Interval::since(at),
        provenance: Provenance::new(Source::UserAssertion, at, "session-1"),
        supersession: Supersession::Unstated,
        according_to: None,
        embedding: v.to_vec(),
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

#[test]
fn an_unrelated_edge_still_shows_that_it_held_and_who_said_so() {
    // `unrelate` appends a tombstone rather than deleting, and justifies that
    // by saying the record still shows the relationship held. That is a promise
    // about what a caller can read, so it has to be provable from the engine's
    // own surface -- and it is the reason `Edge` and `EdgeVersion` are
    // re-exported rather than left behind the engine with `MemoryStore`.
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
        Provenance::new(Source::UserAssertion, 1, "the-introduction"),
    )
    .unwrap();

    let live: Vec<Edge<'_>> = e.edges_from(alice, 2, 2);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].object, bob);
    assert_eq!(live[0].predicate, "knows");
    assert_eq!(
        live[0].provenance.source_ref, "the-introduction",
        "an edge carries who said so, which a Reached cannot"
    );
    assert_eq!(e.edges_into(bob, 2, 2).len(), 1, "and the reverse question");

    e.unrelate(
        alice,
        "knows",
        bob,
        5,
        Provenance::new(Source::UserAssertion, 5, "the-falling-out"),
    )
    .unwrap();

    assert!(
        e.edges_from(alice, 9, 9).is_empty(),
        "the relationship has stopped holding"
    );
    assert_eq!(
        e.edges_from(alice, 2, 9).len(),
        1,
        "but it still held in February, which is the point of a tombstone"
    );

    let history: &[EdgeVersion] = e.edge_history(alice, "knows", bob);
    assert_eq!(history.len(), 2, "two assertions, neither one overwritten");
    assert!(history[0].present);
    assert_eq!(history[0].provenance.source_ref, "the-introduction");
    assert!(!history[1].present, "the tombstone");
    assert_eq!(history[1].provenance.source_ref, "the-falling-out");

    assert!(
        e.edge_history(bob, "knows", alice).is_empty(),
        "a triple nobody asserted is empty, not an error"
    );
}

#[test]
fn a_two_hop_walk_answers_what_recall_alone_cannot() {
    // "Where is my employer based" is a question about Alice that names
    // neither Acme nor Bristol. Semantic recall cannot get there; the graph
    // can.
    let mut e = engine();
    let Remembered::Created { entity: alice, .. } = e
        .remember(seen("Ben Severn", "role", "engineer", 1, [1.0, 0.0, 0.0]))
        .unwrap()
    else {
        panic!("setup")
    };
    let Remembered::Created { entity: acme, .. } = e
        .remember(seen("Acme Corp", "kind", "company", 2, [0.0, 1.0, 0.0]))
        .unwrap()
    else {
        panic!("setup")
    };
    let Remembered::Created {
        entity: bristol, ..
    } = e
        .remember(seen("Bristol", "kind", "city", 3, [0.0, 0.0, 1.0]))
        .unwrap()
    else {
        panic!("setup")
    };

    let prov = Provenance::new(Source::UserAssertion, 4, "session-2");
    e.relate(alice, "employed_by", acme, Interval::since(1), prov.clone())
        .unwrap();
    e.relate(acme, "based_in", bristol, Interval::since(1), prov)
        .unwrap();

    let n = e.neighborhood(&Walk::new(vec![alice], 2, 10, 5, 9));
    let hit = n.reached.iter().find(|r| r.entity == bristol).unwrap();
    assert_eq!(hit.distance, 2);
    assert!(!n.truncated);
}

#[test]
fn a_walk_answers_as_of_a_past_moment_the_way_the_rest_of_the_store_does() {
    let mut e = engine();
    let Remembered::Created { entity: alice, .. } = e
        .remember(seen("Ben Severn", "role", "engineer", 1, [1.0, 0.0, 0.0]))
        .unwrap()
    else {
        panic!("setup")
    };
    let Remembered::Created { entity: acme, .. } = e
        .remember(seen("Acme Corp", "kind", "company", 2, [0.0, 1.0, 0.0]))
        .unwrap()
    else {
        panic!("setup")
    };

    // Told in September that it started in July.
    e.relate(
        alice,
        "employed_by",
        acme,
        Interval::since(7),
        Provenance::new(Source::UserAssertion, 9, "s"),
    )
    .unwrap();

    assert_eq!(
        e.neighborhood(&Walk::new(vec![alice], 1, 10, 8, 8))
            .reached
            .len(),
        1
    );
    assert_eq!(
        e.neighborhood(&Walk::new(vec![alice], 1, 10, 8, 10))
            .reached
            .len(),
        2
    );
}

#[test]
fn edges_survive_a_snapshot_round_trip() {
    let mut e = engine();
    let Remembered::Created { entity: alice, .. } = e
        .remember(seen("Ben Severn", "role", "engineer", 1, [1.0, 0.0, 0.0]))
        .unwrap()
    else {
        panic!("setup")
    };
    let Remembered::Created { entity: acme, .. } = e
        .remember(seen("Acme Corp", "kind", "company", 2, [0.0, 1.0, 0.0]))
        .unwrap()
    else {
        panic!("setup")
    };
    e.relate(
        alice,
        "employed_by",
        acme,
        Interval::since(1),
        Provenance::new(Source::UserAssertion, 3, "s"),
    )
    .unwrap();

    let restored =
        Engine::open(&e.snapshot(), ruleset(), Policy::new(Strategy::MostRecent)).unwrap();

    let out = restored.neighborhood(&Walk::new(vec![alice], 1, 10, 5, 9));
    assert_eq!(out.reached.len(), 2);

    // The reverse direction proves the derived map was rebuilt, not persisted.
    let into = restored.neighborhood(&Walk::new(vec![acme], 1, 10, 5, 9).direction(Direction::In));
    assert_eq!(into.reached.len(), 2);

    assert_eq!(
        restored.snapshot(),
        e.snapshot(),
        "still byte-stable with edges"
    );
}
