//! Does the store return the right one of three answers?
//!
//! The project's claim is that `Value`, `Absent` and `Unknown` are three
//! different answers, and that "they have no employer" is not "nobody has ever
//! said". This scores that against a corpus where all three are labelled by
//! hand.
//!
//! Deliberately measured through [`Engine::about`] and nothing else. No
//! `recall`, no vector threshold, no `weak_below`: the claim under test is
//! structural — an assertion exists or it does not — and measuring it through
//! a probabilistic path would reintroduce the mechanism `benches/locomo`
//! already tried and rejected at J = 0.494.

use std::collections::BTreeMap;

use rm_engine::{
    Believed, BlockingKey, Comparator, Engine, FieldRule, Interval, Metric, Observation, Policy,
    Provenance, Record, Remembered, Ruleset, Source, Strategy, Supersession, VectorIndex,
};

#[derive(serde::Deserialize)]
struct Corpus {
    cases: Vec<Case>,
}

/// One subject, what the conversation established about them, and the correct
/// answer for every attribute the case asks about.
#[derive(serde::Deserialize)]
struct Case {
    subject: String,
    /// Attribute to value. `None` is an asserted absence -- somebody said
    /// there is none -- which is what `rmem note --absent` writes.
    states: BTreeMap<String, Option<String>>,
    /// Attribute to `"value"`, `"absent"` or `"unknown"`. Written by hand, and
    /// never derived from a run: a test that supplies the thing it checks
    /// cannot fail. Attributes here but absent from `states` were never
    /// mentioned, and must read `unknown`.
    truth: BTreeMap<String, String>,
    /// What acting on a wrong answer would cost.
    why: String,
}

fn load() -> Corpus {
    serde_json::from_str(include_str!("absence/cases.json")).expect("the corpus parses")
}

/// The corpus cannot drift into only the easy half.
///
/// A corpus of values and silences is one **any two-state system scores
/// perfectly on**: without a stated absence there is nothing to confuse an
/// asserted "there is none" with. The whole claim lives in those cases, so
/// their presence is asserted rather than assumed.
#[test]
fn every_case_labels_all_three_outcomes() {
    let corpus = load();
    assert!(corpus.cases.len() >= 8, "the corpus shrank");

    for case in &corpus.cases {
        let kinds: std::collections::BTreeSet<&str> =
            case.truth.values().map(String::as_str).collect();
        for want in ["value", "absent", "unknown"] {
            assert!(
                kinds.contains(want),
                "{} has no {want} case, so it cannot exercise the distinction",
                case.subject
            );
        }
        assert!(
            !case.why.trim().is_empty(),
            "{} does not say what a wrong answer would cost",
            case.subject
        );
    }
}

/// Every attribute labelled `unknown` really is one the corpus never states.
///
/// The guard above checks the labels are present; this checks they are honest.
/// A case that both states an attribute and labels it `unknown` would make the
/// store look wrong for answering correctly, and the error would be in the
/// fixture rather than the code -- the hardest kind to find later.
#[test]
fn an_unknown_label_is_never_contradicted_by_the_case_that_carries_it() {
    for case in &load().cases {
        for (attribute, truth) in &case.truth {
            let stated = case.states.get(attribute);
            match truth.as_str() {
                "unknown" => assert!(
                    stated.is_none(),
                    "{} labels {attribute} unknown but states it",
                    case.subject
                ),
                "absent" => assert_eq!(
                    stated,
                    Some(&None),
                    "{} labels {attribute} absent, so the case must state it as null",
                    case.subject
                ),
                "value" => assert!(
                    matches!(stated, Some(Some(_))),
                    "{} labels {attribute} a value, so the case must state one",
                    case.subject
                ),
                other => panic!("{} has an unrecognised truth {other:?}", case.subject),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

/// Rows are truth, columns are what the store answered, keyed `"truth->answered"`.
///
/// Kept as named cells rather than a 3x3 array so a change reads
/// `unknown->absent: 0 -> 1` instead of a number moving in a grid. Two of the
/// nine cells are fabrications and they are asserted individually: a single
/// accuracy figure lets one grow while another shrinks and reports no change,
/// which is the shape of number this project keeps having to catch.
#[derive(Debug, Default, PartialEq, Eq)]
struct Matrix {
    cells: BTreeMap<String, usize>,
}

impl Matrix {
    fn get(&self, cell: &str) -> usize {
        self.cells.get(cell).copied().unwrap_or(0)
    }

    fn record(&mut self, truth: &str, answered: &str) {
        *self
            .cells
            .entry(format!("{truth}->{answered}"))
            .or_default() += 1;
    }
}

const NOW: i64 = 1_725_000_000_000;

/// One field, one blocking key. Resolution is not what this file measures, and
/// each case is a distinct subject with a distinct name, so the resolver's job
/// here is only to keep them apart.
fn ruleset() -> Ruleset {
    Ruleset::new(
        vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
        vec![BlockingKey::Prefix("name".to_string(), 3)],
        4.0,
        6.0,
    )
    .unwrap()
}

/// Write what each case states, and nothing else.
///
/// The `unknown` attributes are never written, which is the entire mechanism
/// under test: the store answers `Unknown` because no assertion exists, not
/// because a score fell below a threshold.
fn seeded(corpus: &Corpus) -> (Engine, Vec<(usize, usize)>) {
    let mut engine = Engine::new(
        VectorIndex::new(3, Metric::Cosine),
        ruleset(),
        Policy::new(Strategy::MostRecent),
    );
    let mut entities = Vec::new();

    for (i, case) in corpus.cases.iter().enumerate() {
        let mut entity = None;
        for (attribute, value) in &case.states {
            // A distinct vector per subject: recall is not measured here, and
            // identical embeddings would make the index the thing under test.
            let embedding = vec![i as f32 + 1.0, attribute.len() as f32, 1.0];
            let observation = Observation {
                kind: "person".to_string(),
                mention: Record::new().with("name", case.subject.as_str()),
                attribute: attribute.clone(),
                value: value.clone(),
                valid: Interval::since(NOW),
                provenance: Provenance::new(Source::UserAssertion, NOW, "absence-corpus"),
                supersession: Supersession::Corrects,
                according_to: None,
                embedding,
            };
            let id = match engine.remember(observation).unwrap() {
                Remembered::Created { entity, .. } | Remembered::Merged { entity, .. } => entity,
                other => panic!("{} landed unexpectedly: {other:?}", case.subject),
            };
            entity = Some(id);
        }
        entities.push((i, entity.expect("every case states something") as usize));
    }

    (engine, entities)
}

fn score(engine: &Engine, corpus: &Corpus, entities: &[(usize, usize)]) -> Matrix {
    let mut matrix = Matrix::default();
    for (i, entity) in entities {
        let case = &corpus.cases[*i];
        for (attribute, truth) in &case.truth {
            let answered = match engine
                .about(*entity as u64, attribute, i64::MAX, i64::MAX)
                .unwrap()
            {
                Believed::Value(_) => "value",
                Believed::Absent => "absent",
                Believed::Unknown => "unknown",
            };
            matrix.record(truth, answered);
        }
    }
    matrix
}

/// The store answers all three, and fabricates nothing.
///
/// `unknown->absent` is the cell this project exists to keep at zero: it is
/// stating as fact that someone has no employer because nobody mentioned their
/// job. It is invisible in any two-state system, which has no way to represent
/// the difference between the two.
#[test]
fn the_store_distinguishes_all_three_and_fabricates_nothing() {
    let corpus = load();
    let (engine, entities) = seeded(&corpus);
    let matrix = score(&engine, &corpus, &entities);

    assert_eq!(
        matrix.get("unknown->absent"),
        0,
        "said there is none, where nobody had said anything: {matrix:?}"
    );
    assert_eq!(
        matrix.get("unknown->value"),
        0,
        "invented a value out of silence: {matrix:?}"
    );
    assert_eq!(
        matrix.get("absent->value"),
        0,
        "invented a value over a stated absence: {matrix:?}"
    );

    // ...and the distinction is being exercised rather than vacuously passed
    // by a store that answers `unknown` to everything.
    assert!(matrix.get("value->value") > 0, "{matrix:?}");
    assert!(matrix.get("absent->absent") > 0, "{matrix:?}");
    assert!(matrix.get("unknown->unknown") > 0, "{matrix:?}");
}

/// The matrix is printable, because the number is the deliverable.
///
/// `cargo test -p rm-engine --test absence -- --ignored --nocapture` prints it
/// for `docs/absence-benchmark.md`. Ignored rather than a normal test: it
/// asserts nothing, and a test that cannot fail should not be counted among
/// ones that can.
#[test]
#[ignore]
fn print_the_matrix() {
    let corpus = load();
    let (engine, entities) = seeded(&corpus);
    let matrix = score(&engine, &corpus, &entities);

    println!("\n            answered value  answered absent  answered unknown");
    for truth in ["value", "absent", "unknown"] {
        print!("truth {truth:<7}");
        for answered in ["value", "absent", "unknown"] {
            print!("{:>15}", matrix.get(&format!("{truth}->{answered}")));
        }
        println!();
    }
    println!("\ncases: {}", corpus.cases.len());
}
