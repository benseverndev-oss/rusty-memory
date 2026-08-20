//! A conversation, remembered.
//!
//! One `use`, from one crate. Everything here goes through `rm_engine`'s
//! exported surface: if this test needs a second crate in the manifest, then so
//! does everyone who wants to feed a turn to a memory store.

use rm_engine::{
    Believed, BlockingKey, Comparator, Completer, CompleterError, Embedder, EmbedderError, Engine,
    FieldRule, Metric, Policy, Ruleset, Source, Strategy, Turn, VectorIndex, Walk,
};

/// Two canned responses, handed out in order. The model is not under test; what
/// the crate does with its answers is.
struct Script(std::cell::RefCell<Vec<&'static str>>);

impl Completer for Script {
    fn complete(&self, _prompt: &str) -> Result<String, CompleterError> {
        Ok(self.0.borrow_mut().remove(0).to_string())
    }
}

struct Buckets;

impl Embedder for Buckets {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedderError> {
        let mut v = [0.0f32; 3];
        for (i, b) in text.bytes().enumerate() {
            v[i % 3] += f32::from(b);
        }
        if v.iter().all(|x| *x == 0.0) {
            v[0] = 1.0;
        }
        Ok(v.to_vec())
    }
}

/// `match_at` is deliberately 6.0 here, not the 8.0 that `test_ruleset` in
/// `rm-engine`'s own suite uses. Two identical "Ben Severn" mentions score
/// `log2(0.9/0.01) ~= 6.49` bits of evidence; at or above that score, the
/// second turn would create a new Ben instead of recognising the one from the
/// first, and the story this test tells would not hold together.
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
        Policy::new(Strategy::ValidInterval),
    )
}

fn turn(text: &str, at: i64) -> Turn {
    Turn {
        text: text.to_string(),
        speaker: Some("Ben Severn".to_string()),
        observed_at: at,
        session: "session-1".to_string(),
    }
}

#[test]
fn two_turns_months_apart_leave_one_person_two_jobs_and_a_departure() {
    let script = Script(std::cell::RefCell::new(vec![
        // March: Ben works at Acme.
        r#"{"mentions":[{"kind":"person","name":"Ben Severn","text":"Ben"},
                        {"kind":"organisation","name":"Acme","text":"Acme"}],
            "facts":[{"subject":0,"attribute":"employer","value":"Acme",
                      "text":"Ben works at Acme","days_ago":null}],
            "relations":[{"subject":0,"predicate":"employed_by","object":1,"days_ago":null}],
            "closures":[]}"#,
        // July: he started at Globex. Nothing says he left Acme.
        r#"{"mentions":[{"kind":"person","name":"Ben Severn","text":"Ben"},
                        {"kind":"organisation","name":"Globex","text":"Globex"}],
            "facts":[{"subject":0,"attribute":"employer","value":"Globex",
                      "text":"Ben works at Globex","days_ago":null}],
            "relations":[{"subject":0,"predicate":"employed_by","object":1,"days_ago":null}],
            "closures":[{"subject":0,"predicate":"employed_by","days_ago":null,
                         "because":"starting a new job ends the previous one"}]}"#,
    ]));

    let mut engine = engine();

    let march = turn("I work at Acme", 300);
    let first = rm_engine::extract(&march, &script).unwrap();
    let first_out = engine.ingest(&march, &first, &Buckets).unwrap();
    let (ben, acme) = (first_out.entities[0], first_out.entities[1]);

    let july = turn("I started at Globex", 700);
    let second = rm_engine::extract(&july, &script).unwrap();
    let out = engine.ingest(&july, &second, &Buckets).unwrap();

    // One person, recognised across two turns months apart.
    assert_eq!(ben, out.entities[0], "the same Ben, not a second one");
    assert_eq!(engine.entity_count(), 3, "Ben, Acme, Globex");

    // Both employers survive as facts, and the store answers by time.
    assert_eq!(
        engine.about(ben, "employer", 400, 1000).unwrap(),
        Believed::Value("Acme".into())
    );
    assert_eq!(
        engine.about(ben, "employer", 800, 1000).unwrap(),
        Believed::Value("Globex".into())
    );

    // The departure was inferred, not stated -- and says so.
    assert_eq!(out.closed.len(), 1);
    assert_eq!(out.closed[0].object, acme);
    let ended = engine
        .edge_history(ben, "employed_by", acme)
        .last()
        .unwrap()
        .clone();
    assert!(!ended.present);
    assert_eq!(
        ended.provenance.source,
        Source::AgentInference,
        "nobody said he left Acme; the agent worked it out, and the store records which"
    );

    // A walk in August reaches Globex and not Acme.
    let reached: Vec<_> = engine
        .neighborhood(&Walk::new(vec![ben], 1, 10, 800, 1000))
        .reached
        .iter()
        .map(|r| r.entity)
        .collect();
    assert!(reached.contains(&out.entities[1]));
    assert!(!reached.contains(&acme));
}
