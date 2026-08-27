//! Three depths over one stored assertion, and nothing lost between them.
//!
//! The property that separates this from summarising: a deeper level is a
//! superset of a shallower one, byte for byte, not a re-rendering. No level
//! calls a model and no level compresses anything.

use rm_engine::{
    BlockingKey, Comparator, Engine, FieldRule, Interval, Metric, Observation, Policy, Provenance,
    Query, Record, Ruleset, Source, Strategy, Supersession, VectorIndex,
};

const NOW: i64 = 1_725_000_000_000;

fn ruleset() -> Ruleset {
    Ruleset::new(
        vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
        vec![BlockingKey::Prefix("name".to_string(), 3)],
        4.0,
        6.0,
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

fn told(e: &mut Engine, value: &str, at: i64, session: &str) {
    e.remember(Observation {
        kind: "person".into(),
        mention: Record::new().with("name", "Rosalind Okafor"),
        attribute: "role".into(),
        value: Some(value.into()),
        valid: Interval::since(at),
        provenance: Provenance::new(Source::UserAssertion, at, session),
        supersession: Supersession::Corrects,
        embedding: vec![1.0, 0.0, 0.0],
    })
    .unwrap();
}

fn seeded() -> Engine {
    let mut e = engine();
    told(&mut e, "owns the Okta setup", NOW, "tiering-test");
    e
}

/// The cheapest level says what was found and whether it stands, and carries
/// no assertion text at all.
///
/// It is a distinct type rather than `Recalled` with fields blanked, because
/// `Recalled::value` is already `Option` and `None` there means *asserted
/// absent*. Reusing it for *not asked for* would make a tombstone
/// indistinguishable from an omission, which is the confusion this store
/// exists to prevent -- in the store's own return type.
#[test]
fn located_carries_the_locator_and_nothing_the_caller_did_not_ask_for() {
    let e = seeded();
    let hits = e
        .recall_located(&Query::new(vec![1.0, 0.0, 0.0], 5))
        .unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].attribute, "role");
    assert_eq!(hits[0].name.as_deref(), Some("Rosalind Okafor"));
    assert!(hits[0].score > 0.9, "{}", hits[0].score);

    // The type is the guarantee: there is no field to read the value from, so
    // no caller can come to depend on text at this depth.
    let rendered = format!("{:?}", hits[0]);
    assert!(
        !rendered.contains("owns the Okta setup"),
        "the value reached a locator-only hit: {rendered}"
    );
}

/// The deepest level answers what an answer rests on.
///
/// Not why the vector matched -- that is a cosine score and this does not
/// pretend otherwise. What it gives is the part a caller can act on: who
/// asserted it, and what it stands against.
#[test]
fn traced_carries_what_the_answer_rests_on() {
    let mut e = seeded();
    told(
        &mut e,
        "owns Okta and the SSO rollout",
        NOW + 1,
        "tiering-test-2",
    );

    let hits = e
        .recall_traced(&Query::new(vec![1.0, 0.0, 0.0], 5))
        .unwrap();
    let hit = hits.first().expect("something came back");

    assert_eq!(hit.history.len(), 2, "both versions, neither overwritten");
    assert!(
        hits.iter()
            .any(|h| h.recalled.provenance.source_ref == "tiering-test-2"),
        "the later assertion's provenance is missing"
    );
}

/// Nothing is lost between levels.
///
/// The guarantee that separates this from summarisation: a deeper level is a
/// superset, byte for byte, rather than a re-rendering of the same content.
#[test]
fn a_deeper_level_is_a_superset_and_not_a_rewrite() {
    let e = seeded();
    let q = Query::new(vec![1.0, 0.0, 0.0], 5);

    let located = e.recall_located(&q).unwrap();
    let stated = e.recall(&q).unwrap();
    let traced = e.recall_traced(&q).unwrap();

    assert_eq!(located.len(), stated.len());
    assert_eq!(stated.len(), traced.len());
    for ((l, s), t) in located.iter().zip(&stated).zip(&traced) {
        assert_eq!(l.assertion, s.assertion);
        assert_eq!(l.entity, s.entity);
        assert_eq!(l.standing, s.standing);
        assert_eq!(&t.recalled, s, "Traced re-rendered the assertion");
    }
}

/// `Located` has to be meaningfully cheaper, or it is not worth a second call.
///
/// A floor rather than a target. The spec's decision rule is that a small
/// saving means this level should not ship, and this is what makes that
/// decision from a number instead of an impression: if it fails, the rule is
/// firing and the level should be dropped, not the assertion loosened.
#[test]
fn located_is_at_least_a_third_cheaper_than_stated() {
    let e = seeded_with(20);
    let q = Query::new(vec![1.0, 0.0, 0.0], 20);
    let located = format!("{:?}", e.recall_located(&q).unwrap()).len();
    let stated = format!("{:?}", e.recall(&q).unwrap()).len();
    assert!(
        (located as f64) < 0.67 * stated as f64,
        "located {located} against stated {stated} -- a small saving means this \
         level should not ship"
    );
}

/// What each depth costs, for `docs/tiering-cost.md`.
///
/// Ignored: it asserts nothing. `Debug` length is a proxy for wire bytes and
/// overstates every level similarly, which is fine for a ratio and wrong for
/// an absolute -- the doc says so.
#[test]
#[ignore]
fn report_bytes_per_hit_at_each_depth() {
    let e = seeded_with(20);
    let q = Query::new(vec![1.0, 0.0, 0.0], 20);

    let located = format!("{:?}", e.recall_located(&q).unwrap()).len();
    let stated = format!("{:?}", e.recall(&q).unwrap()).len();
    let traced = format!("{:?}", e.recall_traced(&q).unwrap()).len();

    println!("hits: {}", e.recall(&q).unwrap().len());
    println!("located {located} chars, stated {stated}, traced {traced}");
    println!(
        "located saves {:.0}% against stated; traced costs {:.0}% more",
        100.0 * (1.0 - located as f64 / stated as f64),
        100.0 * (traced as f64 / stated as f64 - 1.0)
    );
}

/// Twenty distinct subjects, so the ratio is measured over a realistic result
/// set rather than a single hit.
fn seeded_with(n: usize) -> Engine {
    let mut e = engine();
    for i in 0..n {
        e.remember(Observation {
            kind: "person".into(),
            mention: Record::new().with("name", format!("Subject {i:02}")),
            attribute: "role".into(),
            value: Some(format!("owns the {i:02} surface and its rollout")),
            valid: Interval::since(NOW),
            provenance: Provenance::new(Source::UserAssertion, NOW, "cost"),
            supersession: Supersession::Corrects,
            embedding: vec![1.0, i as f32 / 100.0, 0.0],
        })
        .unwrap();
    }
    e
}
