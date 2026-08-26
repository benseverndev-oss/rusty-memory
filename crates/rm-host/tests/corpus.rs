//! What the shipped resolution configuration does, on data with known truth.
//!
//! Every calibration decision in this project has been made by argument. The
//! thresholds cite a corpus "measured across four stores from a real corpus"
//! that is not in this repository, so `u = 0.38` cannot be re-measured or
//! checked, and answering one question — should `email` be a resolution field —
//! took four configurations and a throwaway corpus, reversed the
//! recommendation twice, and ended in a negative result no test here could have
//! caught.
//!
//! This lives in `rm-host` rather than `rm-resolve`, which the plan named,
//! because the point is to score the configuration that **ships**. `TEMPLATE`
//! and `Config::ruleset` are here, and `rm-resolve` cannot see them without a
//! dependency inversion. Hand-writing the numbers instead would measure a
//! configuration nobody runs.

use rm_engine::{Decision, Record, Ruleset};
use rm_host::config::Config;

#[derive(serde::Deserialize)]
struct Corpus {
    people: Vec<Person>,
    mentions: Vec<Mention>,
}

#[derive(serde::Deserialize)]
struct Person {
    id: String,
    name: String,
    kind: String,
    email: Option<String>,
}

/// `is` names the person this mention is really about, or is absent when the
/// mention is somebody the corpus has not seen before.
#[derive(serde::Deserialize)]
struct Mention {
    name: String,
    kind: String,
    email: Option<String>,
    is: Option<String>,
    shape: String,
}

fn load() -> Corpus {
    serde_json::from_str(include_str!("corpus/people.json")).expect("the corpus parses")
}

fn record(name: &str, kind: &str, email: Option<&str>) -> Record {
    let mut r = Record::new().with("name", name).with("kind", kind);
    if let Some(e) = email {
        r = r.with("email", e);
    }
    r
}

/// The configuration this crate ships, not one written for the test.
fn shipped() -> Ruleset {
    Config::from_template()
        .ruleset()
        .expect("the shipped template describes a valid ruleset")
}

/// The corpus cannot be trimmed to the easy cases and still pass.
///
/// A corpus of true matches measures nothing: a configuration that merges
/// everything scores perfectly on it. The negative shapes are the ones that
/// decided every real comparison, so their presence is asserted rather than
/// assumed.
#[test]
fn the_corpus_still_contains_every_shape_that_has_ever_decided_anything() {
    let corpus = load();
    let shapes: std::collections::BTreeSet<&str> =
        corpus.mentions.iter().map(|m| m.shape.as_str()).collect();

    for required in [
        "exact-repeat",
        "nickname",
        "given-name-alone",
        "surname-alone",
        "changed-surname-stable-local-part",
        "shared-surname-different-people",
        "shared-given-name-shared-domain",
        "kind-disagreement-guard",
    ] {
        assert!(
            shapes.contains(required),
            "the corpus lost its {required} case"
        );
    }
    assert!(
        corpus.mentions.iter().any(|m| m.is.is_none()),
        "a corpus with no strangers in it cannot detect a wrong merge"
    );
}

/// Every `is` names somebody the corpus actually defines.
///
/// A typo here would silently reclassify a true match as a stranger, and the
/// score would move for a reason that is not in the code.
#[test]
fn ground_truth_only_names_people_the_corpus_defines() {
    let corpus = load();
    let ids: std::collections::BTreeSet<&str> =
        corpus.people.iter().map(|p| p.id.as_str()).collect();
    for m in &corpus.mentions {
        if let Some(who) = &m.is {
            assert!(ids.contains(who.as_str()), "{} names unknown {who}", m.name);
        }
    }
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

/// The three outcomes, kept apart and never summed.
///
/// One wrong merge is worse than any number of questions, and a weighted total
/// would let one hide behind the other. Pairs rather than counts, so a change
/// says *which* pair moved.
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Score {
    questions: Vec<String>,
    silent_misses: Vec<String>,
    wrong_merges: Vec<String>,
}

/// Classified against the person the mention is *really* about, not merely
/// against whichever scores highest.
///
/// The first version of this scored the best candidate only, and reported four
/// questions. One of them was a question about the wrong person: a mention
/// whose true match is `p4` scored highest against `p5`, so a reviewer
/// answering it correctly would say "no" and the true match would never be
/// raised at all. Counting that as a question flattered the configuration.
/// What matters is what happens to the *true* pair, and separately whether any
/// other pair merges.
fn score(rules: &Ruleset, corpus: &Corpus) -> Score {
    let mut out = Score::default();

    for m in &corpus.mentions {
        let mention = record(&m.name, &m.kind, m.email.as_deref());
        let decide = |p: &Person| {
            let s = rules.score(&mention, &record(&p.name, &p.kind, p.email.as_deref()));
            (rules.decide(s), s)
        };

        // A merge onto anybody who is not the true match. Silent and
        // permanent, and the outcome the whole design exists to avoid --
        // checked first because no other classification outweighs it.
        for p in &corpus.people {
            if m.is.as_deref() == Some(p.id.as_str()) {
                continue;
            }
            if let (Decision::Match, s) = decide(p) {
                out.wrong_merges
                    .push(format!("{} -> {} ({s:.2})", m.name, p.id));
            }
        }

        // Then what became of the true pair, if there is one. A mention of a
        // stranger has no true pair, and is correct precisely when the loop
        // above found nothing.
        let Some(who) = m.is.as_deref() else { continue };
        let truth = corpus
            .people
            .iter()
            .find(|p| p.id == who)
            .expect("ground truth names a person the corpus defines");
        let (decision, s) = decide(truth);
        let pair = format!("{} -> {who} ({s:.2})", m.name);
        match decision {
            Decision::Match => {}
            Decision::Review => out.questions.push(pair),
            Decision::NonMatch => out.silent_misses.push(pair),
        }
    }

    out.questions.sort();
    out.silent_misses.sort();
    out.wrong_merges.sort();
    out
}

/// The shipped configuration never merges two different people.
///
/// Asserted on its own rather than as part of a total. A wrong merge is the
/// one outcome that cannot be recovered from: nobody is told, and the two
/// records are one from then on.
#[test]
fn the_shipped_configuration_merges_nobody_it_should_not() {
    let corpus = load();
    let s = score(&shipped(), &corpus);
    assert!(
        s.wrong_merges.is_empty(),
        "a stranger was absorbed: {:?}",
        s.wrong_merges
    );
}

/// ...and the corpus is exercising the review band rather than sailing past it.
///
/// A configuration that merged nothing at all would satisfy the test above.
#[test]
fn the_corpus_actually_reaches_the_review_band() {
    let corpus = load();
    let s = score(&shipped(), &corpus);
    assert!(
        !s.questions.is_empty(),
        "no question was raised, so the band is not being tested: {s:?}"
    );
}

/// What the shipped configuration does today, pinned.
///
/// A change that turns a caught match into a silent duplicate, or a stranger
/// into a merge, fails here. Updating `baseline.json` is how you say a change
/// was meant: a deliberate act with a diff a reviewer can read, rather than a
/// number nobody watches.
///
/// The pairs carry their scores, so the diff says which pair moved and by how
/// much rather than that a count changed.
#[test]
fn the_shipped_configuration_still_scores_what_the_baseline_says() {
    let expected: Score =
        serde_json::from_str(include_str!("corpus/baseline.json")).expect("the baseline parses");
    let actual = score(&shipped(), &load());
    assert_eq!(
        actual, expected,
        "resolution behaviour moved -- read the diff"
    );
}

/// A baseline of nothing at all would pass forever and mean nothing.
///
/// It is what an empty corpus, a broken loader, or a ruleset comparing nothing
/// all look like, and each of those is silent.
#[test]
fn the_baseline_is_not_a_configuration_that_does_nothing() {
    let expected: Score =
        serde_json::from_str(include_str!("corpus/baseline.json")).expect("the baseline parses");
    assert!(
        !expected.questions.is_empty(),
        "a baseline with no questions is not exercising the review band"
    );
}
/// The three counts, for `docs/`. Ignored: it asserts nothing, and a test that
/// cannot fail should not be counted among ones that can.
#[test]
#[ignore]
fn print_the_score() {
    let corpus = load();
    let s = score(&shipped(), &corpus);
    println!("\nmentions:      {}", corpus.mentions.len());
    println!("questions:     {}", s.questions.len());
    for q in &s.questions {
        println!("    {q}");
    }
    println!("silent misses: {}", s.silent_misses.len());
    for q in &s.silent_misses {
        println!("    {q}");
    }
    println!("wrong merges:  {}", s.wrong_merges.len());
    for q in &s.wrong_merges {
        println!("    {q}");
    }
}

/// A stricter floor turns questions into silent misses, and this proves the
/// scorer can see that happen.
///
/// Without it the assertions above might be counting nothing: a scorer that
/// classified everything as correct would pass them both. Perturbing the
/// shipped thresholds and watching the numbers move is the cheapest evidence
/// that the measurement is live.
#[test]
fn raising_the_floor_converts_questions_into_silent_misses() {
    let corpus = load();
    let relaxed = score(&shipped(), &corpus);

    // The shipped configuration with only its thresholds moved, so the fields
    // and blocking key are unchanged and the floor is the single variable.
    let mut config = Config::from_template();
    config.resolution.review_at = 20.0;
    config.resolution.match_at = 21.0;
    let strict = score(&config.ruleset().unwrap(), &corpus);

    assert!(
        strict.silent_misses.len() > relaxed.silent_misses.len(),
        "raising the floor to 20 bits changed nothing, so the scorer is not \
         reading the thresholds: {relaxed:?} then {strict:?}"
    );
    assert!(strict.questions.is_empty(), "{strict:?}");
}
