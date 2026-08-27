//! The store as its own record of what it has read.
//!
//! Document ingest puts a content hash in each assertion's `source_ref`, so
//! "have I read this text before" is answerable from the store itself. The
//! alternative -- a sidecar ledger file -- can desync: delete the store, keep
//! the ledger, and a re-run skips everything into an empty store, which is a
//! silent no-op that looks like success.

use rm_engine::{
    BlockingKey, Comparator, Engine, FieldRule, Interval, Metric, Observation, Policy, Provenance,
    Record, Ruleset, Source, Strategy, Supersession, VectorIndex,
};

fn engine() -> Engine {
    Engine::new(
        VectorIndex::new(3, Metric::Cosine),
        Ruleset::new(
            vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
            vec![BlockingKey::Prefix("name".to_string(), 3)],
            4.0,
            6.0,
        )
        .unwrap(),
        Policy::new(Strategy::MostRecent),
    )
}

fn told_from(e: &mut Engine, who: &str, source_ref: &str) {
    e.remember(Observation {
        kind: "person".into(),
        mention: Record::new().with("name", who),
        attribute: "role".into(),
        value: Some("owns something".into()),
        valid: Interval::since(100),
        provenance: Provenance::new(Source::UserAssertion, 100, source_ref),
        supersession: Supersession::Corrects,
        according_to: None,
        embedding: vec![1.0, 0.0, 0.0],
    })
    .unwrap();
}

/// The store can say what it has already read.
#[test]
fn a_store_knows_which_sources_it_has_seen() {
    let mut e = engine();
    told_from(&mut e, "Rosalind Okafor", "docs/a.md#Title@aaaa");
    told_from(&mut e, "Rosalind Okafor", "docs/a.md#Title@aaaa");
    told_from(&mut e, "Delia Marchetti", "docs/b.md#Other@bbbb");

    let seen = e.source_refs();
    assert_eq!(seen.len(), 2, "a repeated source counted twice: {seen:?}");
    assert!(seen.contains("docs/a.md#Title@aaaa"));
    assert!(seen.contains("docs/b.md#Other@bbbb"));
}

/// An empty store knows nothing, which is why it reads everything.
///
/// The property a sidecar ledger cannot have: delete the store and the record
/// of what was read goes with it, rather than surviving to skip a re-read into
/// an empty store.
#[test]
fn an_empty_store_has_read_nothing() {
    assert!(engine().source_refs().is_empty());
}

/// An edited chunk is a different source, because its hash is part of the name.
///
/// This is the whole idempotency mechanism, stated as a property of the store
/// rather than of the chunker.
#[test]
fn editing_a_chunk_makes_it_a_source_the_store_has_not_seen() {
    let mut e = engine();
    told_from(&mut e, "Rosalind Okafor", "docs/a.md#Title@aaaa");

    let seen = e.source_refs();
    assert!(seen.contains("docs/a.md#Title@aaaa"));
    assert!(
        !seen.contains("docs/a.md#Title@bbbb"),
        "the same heading with different text read as already seen"
    );
}
