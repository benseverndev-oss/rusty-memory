//! Whose view a fact is, and why that is not a contradiction.
//!
//! Two people differing is the third case of a shape this store has handled
//! twice: disagreement across time is kept and resolved at read, identities too
//! close to call are filed rather than merged, and holders used to be settled
//! by arrival order -- reporting a correction where nothing was corrected.

use rm_engine::{
    Believed, BlockingKey, Comparator, Engine, FieldRule, Interval, Metric, Observation, Policy,
    Provenance, Record, Remembered, Ruleset, Source, StableId, Strategy, Supersession, VectorIndex,
};

const NOW: i64 = 1_725_000_000_000;

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

fn write(
    e: &mut Engine,
    who: &str,
    attribute: &str,
    value: &str,
    at: i64,
    according_to: Option<StableId>,
    embedding: [f32; 3],
) -> StableId {
    let out = e
        .remember(Observation {
            kind: "person".into(),
            mention: Record::new().with("name", who),
            attribute: attribute.into(),
            value: Some(value.into()),
            valid: Interval::since(at),
            provenance: Provenance::new(Source::UserAssertion, at, "holders-test"),
            supersession: Supersession::Corrects,
            according_to,
            embedding: embedding.to_vec(),
        })
        .unwrap();
    match out {
        Remembered::Created { entity, .. } | Remembered::Merged { entity, .. } => entity,
        other => panic!("unexpected landing: {other:?}"),
    }
}

/// A subject, and two people who will hold views about them.
fn cast(e: &mut Engine) -> (StableId, StableId, StableId) {
    let jon = write(
        e,
        "Jonathan Merrick",
        "role",
        "leads circ",
        NOW,
        None,
        [1.0, 0.0, 0.0],
    );
    let divya = write(
        e,
        "Priyanka Vale",
        "role",
        "manages R&A",
        NOW,
        None,
        [0.0, 1.0, 0.0],
    );
    let subject = write(
        e,
        "Rosalind Okafor",
        "role",
        "engineer",
        NOW,
        None,
        [0.0, 0.0, 1.0],
    );
    (subject, jon, divya)
}

/// Two people differing is not one person correcting themselves.
///
/// This is the whole feature. Before it, these two assertions landed in one
/// slot and survivorship picked a winner by arrival -- reporting a correction
/// where nothing was corrected and a change where nothing changed.
#[test]
fn two_holders_differing_is_not_a_correction() {
    let mut e = engine();
    let (subject, jon, divya) = cast(&mut e);

    write(
        &mut e,
        "Rosalind Okafor",
        "team",
        "Circulation",
        NOW + 1,
        Some(jon),
        [0.0, 0.0, 1.0],
    );
    write(
        &mut e,
        "Rosalind Okafor",
        "team",
        "R&A",
        NOW + 2,
        Some(divya),
        [0.0, 0.0, 1.0],
    );

    assert_eq!(
        e.about_according_to(subject, "team", jon, i64::MAX, i64::MAX)
            .unwrap(),
        Believed::Value("Circulation".into()),
        "the later view overwrote the earlier one"
    );
    assert_eq!(
        e.about_according_to(subject, "team", divya, i64::MAX, i64::MAX)
            .unwrap(),
        Believed::Value("R&A".into())
    );
}

/// A holder correcting themselves still corrects.
///
/// The guard that partitioning did not simply disable survivorship.
#[test]
fn one_holder_correcting_themselves_is_still_a_correction() {
    let mut e = engine();
    let (subject, jon, _) = cast(&mut e);

    write(
        &mut e,
        "Rosalind Okafor",
        "team",
        "Circulation",
        NOW + 1,
        Some(jon),
        [0.0, 0.0, 1.0],
    );
    write(
        &mut e,
        "Rosalind Okafor",
        "team",
        "Circ Ops",
        NOW + 2,
        Some(jon),
        [0.0, 0.0, 1.0],
    );

    assert_eq!(
        e.about_according_to(subject, "team", jon, i64::MAX, i64::MAX)
            .unwrap(),
        Believed::Value("Circ Ops".into())
    );
}

/// A holder-less read never sees a view, and a holder's read never sees a fact.
///
/// The compatibility promise, in both directions. Without the second half the
/// entities already in a live store would start answering differently the
/// moment anybody recorded an opinion about them.
#[test]
fn facts_and_views_do_not_mix_in_either_direction() {
    let mut e = engine();
    let (subject, jon, _) = cast(&mut e);

    write(
        &mut e,
        "Rosalind Okafor",
        "team",
        "Circulation",
        NOW + 1,
        None,
        [0.0, 0.0, 1.0],
    );
    write(
        &mut e,
        "Rosalind Okafor",
        "team",
        "R&A",
        NOW + 2,
        Some(jon),
        [0.0, 0.0, 1.0],
    );

    assert_eq!(
        e.about(subject, "team", i64::MAX, i64::MAX).unwrap(),
        Believed::Value("Circulation".into()),
        "an opinion reached a holder-less read"
    );
    assert_eq!(
        e.about_according_to(subject, "team", jon, i64::MAX, i64::MAX)
            .unwrap(),
        Believed::Value("R&A".into()),
        "a fact reached a holder's read"
    );
}

/// An attribute nobody holds a view on reads `Unknown` for a holder, not the
/// fact.
///
/// The direction that is easy to get wrong: falling back to the store's own
/// assertion would put words in a person's mouth.
#[test]
fn a_holder_who_said_nothing_is_unknown_rather_than_the_fact() {
    let mut e = engine();
    let (subject, jon, _) = cast(&mut e);

    write(
        &mut e,
        "Rosalind Okafor",
        "team",
        "Circulation",
        NOW + 1,
        None,
        [0.0, 0.0, 1.0],
    );

    assert_eq!(
        e.about_according_to(subject, "team", jon, i64::MAX, i64::MAX)
            .unwrap(),
        Believed::Unknown,
        "a fact was attributed to somebody who never said it"
    );
}

/// Disagreement is recorded, and a caller who wants to see it asks.
///
/// Deliberately a separate call rather than a fourth `Believed` variant: a
/// `Contested` answer would change what every existing read can return.
#[test]
fn holders_of_names_everyone_with_a_view_and_nobody_else() {
    let mut e = engine();
    let (subject, jon, divya) = cast(&mut e);

    write(
        &mut e,
        "Rosalind Okafor",
        "team",
        "Circulation",
        NOW + 1,
        None,
        [0.0, 0.0, 1.0],
    );
    write(
        &mut e,
        "Rosalind Okafor",
        "team",
        "Circulation",
        NOW + 2,
        Some(jon),
        [0.0, 0.0, 1.0],
    );
    write(
        &mut e,
        "Rosalind Okafor",
        "team",
        "R&A",
        NOW + 3,
        Some(divya),
        [0.0, 0.0, 1.0],
    );
    write(
        &mut e,
        "Rosalind Okafor",
        "role",
        "peer",
        NOW + 4,
        Some(jon),
        [0.0, 0.0, 1.0],
    );

    let mut expected = vec![jon, divya];
    expected.sort_unstable();
    assert_eq!(e.holders_of(subject, "team"), expected);

    assert!(
        e.holders_of(subject, "employer").is_empty(),
        "an attribute nobody holds a view on has no holders"
    );
}
