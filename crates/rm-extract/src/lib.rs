//! A turn of dialogue, turned into things a memory store can hold.
//!
//! # No crate touches the network
//!
//! This crate needs a language model and does not call one. [`Completer`] is a
//! port the host implements, so `rm-extract` depends on `rm_core` and `serde`
//! and nothing else — the whole workspace still builds, tests and audits with
//! no third-party dependencies at all.
//!
//! The architecture sketch called this "the only crate that touches the
//! network". A port is the stronger version: *no* crate does, and the binary
//! that wires an engine to a provider is the only thing that has to care. A
//! stub `Completer` returning a canned string is three lines, so everything
//! here is tested offline and deterministically.
//!
//! # This crate owns both the prompt and the schema
//!
//! [`prompt`] builds the question and [`extract`] parses the answer. Letting a
//! host write its own prompt against a schema this crate owns is the mistake
//! `rm_resolve` avoided by exposing `BlockingKey::keys_for`: two copies of a
//! contract drift, and the drift here is silent. A prompt that has fallen
//! behind its schema produces a thin extraction, not an error, and nothing in
//! the output says the model was asked the wrong question.
//!
//! `prompt` is public anyway, so a host can read it, log it, or build a
//! few-shot variant on it. Owning a contract does not require hiding it.
//!
//! # What this crate does not decide
//!
//! An [`Extraction`] addresses everything by local index, because the entity
//! ids do not exist yet — resolution happens inside `rm_engine`'s `remember`,
//! which has not run. An extraction describes a turn; it is not a list of store
//! operations.

mod prompt;

pub use prompt::prompt;

use rm_core::Timestamp;
use serde::{Deserialize, Serialize};

/// One line of dialogue to extract from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub text: String,
    /// The speaker's name, so first-person references resolve to a mention
    /// that entity resolution can match against what is already known.
    ///
    /// `None` when the turn has no identified speaker — the prompt says so
    /// explicitly rather than leaving a blank for the model to fill.
    pub speaker: Option<String>,
    pub observed_at: Timestamp,
    pub session: String,
}

/// Something the turn referred to.
///
/// Its position in [`Extraction::mentions`] is its local index, and every other
/// part of an extraction refers to it by that.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mention {
    pub kind: String,
    /// The identifying field, used for entity resolution.
    pub name: String,
    /// What to embed: the phrasing the turn actually used for it.
    pub text: String,
}

/// An attribute assertion about one mention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fact {
    pub subject: usize,
    pub attribute: String,
    /// `None` asserts the attribute has no value — a tombstone.
    pub value: Option<String>,
    /// What to embed for *this fact*, not for its subject.
    ///
    /// A fact and the thing it is about are different search targets. If every
    /// fact about Ben shared Ben's embedding, "where does he work" could only
    /// reach the employer by first reaching Ben, and the assertion itself would
    /// be unreachable.
    pub text: String,
    pub valid_from: Timestamp,
}

/// A relationship between two mentions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    pub subject: usize,
    pub predicate: String,
    pub object: usize,
    pub valid_from: Timestamp,
}

/// An inference that a subject's relationships of one kind have ended.
///
/// It names a *predicate*, not an edge, because it cannot name an edge: "I
/// started at Globex" never mentions Acme, and this crate has never seen the
/// store. Only `rm_engine::Engine::ingest` can resolve which edges it ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Closure {
    pub subject: usize,
    pub predicate: String,
    pub at: Timestamp,
    /// The model's stated reason, kept for the caller to log.
    pub because: String,
}

/// Everything one turn yielded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extraction {
    pub mentions: Vec<Mention>,
    pub facts: Vec<Fact>,
    pub relations: Vec<Relation>,
    pub closures: Vec<Closure>,
}

/// Whatever went wrong reaching the model. Opaque here: the host's transport,
/// the host's error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompleterError(pub String);

impl std::fmt::Display for CompleterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the completer failed: {}", self.0)
    }
}

impl std::error::Error for CompleterError {}

/// A language model, supplied by the host.
///
/// Deliberately one method taking and returning a string. Anything richer —
/// streaming, tool calls, token counts — would be this crate taking a position
/// on how a model is served, which is exactly what keeping it a port avoids.
pub trait Completer {
    fn complete(&self, prompt: &str) -> Result<String, CompleterError>;
}

/// A turn that could not be extracted, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtractError {
    /// The completer failed before a response existed.
    Completer(CompleterError),
    /// The response was not the JSON this crate asked for.
    Unparsable(String),
    /// The response parsed but described something impossible.
    Malformed(String),
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::Completer(e) => write!(f, "{e}"),
            ExtractError::Unparsable(why) => write!(
                f,
                "the model's response was not the JSON this crate asked for: {why}"
            ),
            ExtractError::Malformed(why) => {
                write!(f, "the model described something impossible: {why}")
            }
        }
    }
}

impl std::error::Error for ExtractError {}

impl From<CompleterError> for ExtractError {
    fn from(e: CompleterError) -> Self {
        ExtractError::Completer(e)
    }
}

/// The JSON this crate asks for.
///
/// Separate from the public types on purpose. The wire format speaks in days
/// before the turn because that is what a model can reason about; the public
/// types speak in absolute timestamps because that is what a store needs. One
/// struct doing both would leak the prompt's convenience into the API.
#[derive(Deserialize)]
struct WireExtraction {
    #[serde(default)]
    mentions: Vec<Mention>,
    #[serde(default)]
    facts: Vec<WireFact>,
    #[serde(default)]
    relations: Vec<WireRelation>,
    #[serde(default)]
    closures: Vec<WireClosure>,
}

#[derive(Deserialize)]
struct WireFact {
    subject: usize,
    attribute: String,
    value: Option<String>,
    text: String,
    days_ago: Option<i64>,
}

#[derive(Deserialize)]
struct WireRelation {
    subject: usize,
    predicate: String,
    object: usize,
    days_ago: Option<i64>,
}

#[derive(Deserialize)]
struct WireClosure {
    subject: usize,
    predicate: String,
    days_ago: Option<i64>,
    because: String,
}

/// Milliseconds in a day, for turning "days before the turn" into a timestamp.
const DAY_MS: i64 = 86_400_000;

/// Resolve a relative day count against the turn.
///
/// `None` is the turn's own moment. The subtraction saturates rather than
/// wrapping: a model that answers with `i64::MAX` should produce a timestamp at
/// the far edge of representable time, not one in the future.
///
/// A negative count is refused rather than resolved, because it is the one
/// input that reaches the future by arithmetic rather than by saturation. On a
/// fact it is merely wrong -- a fact dated after the turn that produced it. On
/// a closure it is damaging: `rm_engine`'s `ingest` asks
/// `edges_from(subject, closure.at, Timestamp::MAX)`, so a future `at` reads a
/// state of the graph that has not happened and tombstones the edges it finds
/// effective from then. Nothing errors, no query notices, and a live edge
/// silently expires on a date nobody chose.
///
/// Refused for facts and relations too, not only for closures. Accepting the
/// same nonsense in two places and refusing it in a third would leave a caller
/// unable to read a timestamp after the turn as anything but ambiguous -- did
/// the model mean the future, or did it get the sign wrong? Neither is worth
/// storing, and `days_ago` has one meaning in the prompt.
fn resolve(
    days_ago: Option<i64>,
    observed_at: Timestamp,
    what: &str,
) -> Result<Timestamp, ExtractError> {
    match days_ago {
        None => Ok(observed_at),
        Some(days) if days < 0 => Err(ExtractError::Malformed(format!(
            "a {what} gives days_ago as {days}, which is a moment after the turn it came from -- days_ago counts backwards, and a future timestamp on a closure would end edges that have not been asserted yet"
        ))),
        Some(days) => Ok(observed_at.saturating_sub(days.saturating_mul(DAY_MS))),
    }
}

/// Extract one turn.
///
/// Refuses rather than salvages. A response this crate can only partly
/// understand is a turn silently half-remembered, and nothing downstream can
/// tell that apart from a turn that genuinely said less -- so a mention with no
/// name, an index naming a mention that is not there, or a `days_ago` that
/// counts forwards, fails the whole extraction and says which.
///
/// No retries. The host owns the [`Completer`], so backoff, retry and provider
/// failover are its business and it is better placed to do them.
pub fn extract(turn: &Turn, completer: &impl Completer) -> Result<Extraction, ExtractError> {
    let response = completer.complete(&prompt(turn))?;

    let wire: WireExtraction = serde_json::from_str(response.trim())
        .map_err(|e| ExtractError::Unparsable(e.to_string()))?;

    let n = wire.mentions.len();
    let names = |i: usize, what: &str| -> Result<(), ExtractError> {
        if i >= n {
            return Err(ExtractError::Malformed(format!(
                "a {what} names mention {i}, but the response listed {n}"
            )));
        }
        Ok(())
    };

    for (i, mention) in wire.mentions.iter().enumerate() {
        if mention.name.trim().is_empty() {
            return Err(ExtractError::Malformed(format!(
                "mention {i} has no name, and resolution matches on the name -- an entity without one can never be recognised again, so every later turn about it would create another"
            )));
        }
    }

    let mut out = Extraction {
        mentions: wire.mentions,
        ..Extraction::default()
    };

    for f in wire.facts {
        names(f.subject, "fact")?;
        out.facts.push(Fact {
            subject: f.subject,
            attribute: f.attribute,
            value: f.value,
            text: f.text,
            valid_from: resolve(f.days_ago, turn.observed_at, "fact")?,
        });
    }

    for r in wire.relations {
        names(r.subject, "relation")?;
        names(r.object, "relation")?;
        if r.subject == r.object {
            return Err(ExtractError::Malformed(format!(
                "a relation runs from mention {} to itself, which rm_store::relate refuses to create",
                r.subject
            )));
        }
        out.relations.push(Relation {
            subject: r.subject,
            predicate: r.predicate,
            object: r.object,
            valid_from: resolve(r.days_ago, turn.observed_at, "relation")?,
        });
    }

    for c in wire.closures {
        names(c.subject, "closure")?;
        out.closures.push(Closure {
            subject: c.subject,
            predicate: c.predicate,
            at: resolve(c.days_ago, turn.observed_at, "closure")?,
            because: c.because,
        });
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Timestamp = 1_720_000_000_000;
    const DAY: i64 = 86_400_000;

    /// A completer that returns whatever it was built with, ignoring the
    /// prompt. Everything this crate does to a response is under test; how the
    /// response was obtained is the host's business.
    struct Canned(&'static str);

    impl Completer for Canned {
        fn complete(&self, _prompt: &str) -> Result<String, CompleterError> {
            Ok(self.0.to_string())
        }
    }

    struct Broken;

    impl Completer for Broken {
        fn complete(&self, _prompt: &str) -> Result<String, CompleterError> {
            Err(CompleterError("no route to host".to_string()))
        }
    }

    fn turn() -> Turn {
        Turn {
            text: "I started at Globex".to_string(),
            speaker: Some("Ben Severn".to_string()),
            observed_at: NOW,
            session: "session-1".to_string(),
        }
    }

    #[test]
    fn a_turn_naming_two_things_and_a_relationship_extracts_all_three() {
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[
                     {"kind":"person","name":"Ben Severn","text":"Ben"},
                     {"kind":"organisation","name":"Globex","text":"Globex"}],
                   "facts":[
                     {"subject":0,"attribute":"employer","value":"Globex",
                      "text":"Ben works at Globex","days_ago":null}],
                   "relations":[
                     {"subject":0,"predicate":"employed_by","object":1,"days_ago":null}],
                   "closures":[]}"#,
            ),
        )
        .unwrap();

        assert_eq!(out.mentions.len(), 2);
        assert_eq!(out.mentions[1].name, "Globex");
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.facts[0].value.as_deref(), Some("Globex"));
        assert_eq!(out.relations.len(), 1);
        assert_eq!(out.relations[0].object, 1);
    }

    #[test]
    fn a_null_days_ago_means_the_turn_s_own_moment() {
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[{"subject":0,"attribute":"mood","value":"tired",
                              "text":"Ben is tired","days_ago":null}],
                    "relations":[],"closures":[]}"#,
            ),
        )
        .unwrap();
        assert_eq!(out.facts[0].valid_from, NOW);
    }

    #[test]
    fn days_ago_counts_backwards_from_the_turn() {
        // Sixty days before the turn, not before some wall clock the crate does
        // not have.
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[{"subject":0,"attribute":"employer","value":"Globex",
                              "text":"Ben joined Globex","days_ago":60}],
                    "relations":[],"closures":[]}"#,
            ),
        )
        .unwrap();
        assert_eq!(out.facts[0].valid_from, NOW - 60 * DAY);
    }

    #[test]
    fn a_null_value_is_a_tombstone_not_a_missing_fact() {
        // "He is between jobs" asserts the attribute has no value, which has to
        // survive as a positive claim rather than being dropped.
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[{"subject":0,"attribute":"employer","value":null,
                              "text":"Ben is between jobs","days_ago":null}],
                    "relations":[],"closures":[]}"#,
            ),
        )
        .unwrap();
        assert_eq!(out.facts.len(), 1);
        assert!(out.facts[0].value.is_none());
    }

    #[test]
    fn a_closure_carries_its_reason() {
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[],"relations":[],
                    "closures":[{"subject":0,"predicate":"employed_by",
                                 "days_ago":null,"because":"a new job ends the old one"}]}"#,
            ),
        )
        .unwrap();
        assert_eq!(out.closures.len(), 1);
        assert_eq!(out.closures[0].because, "a new job ends the old one");
        assert_eq!(out.closures[0].at, NOW);
    }

    #[test]
    fn an_extraction_with_nothing_in_it_is_not_an_error() {
        // Plenty of turns say nothing worth remembering, and treating that as a
        // failure would make a caller unable to tell "nothing here" from "the
        // model broke".
        let out = extract(
            &turn(),
            &Canned(r#"{"mentions":[],"facts":[],"relations":[],"closures":[]}"#),
        )
        .unwrap();
        assert_eq!(out, Extraction::default());
    }

    #[test]
    fn a_completer_failure_reaches_the_caller_intact() {
        let err = extract(&turn(), &Broken).unwrap_err();
        assert!(
            err.to_string().contains("no route to host"),
            "the host's own explanation must survive: {err}"
        );
    }

    #[test]
    fn a_response_that_is_not_json_is_refused() {
        let err = extract(&turn(), &Canned("Sure! Here's what I found:")).unwrap_err();
        assert!(matches!(err, ExtractError::Unparsable(_)), "{err:?}");
    }

    #[test]
    fn a_mention_with_no_name_is_refused() {
        // Resolution matches on the name. A mention without one becomes an
        // entity nothing can ever match against again, so every later turn
        // about it creates another.
        let err = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"   ","text":"Ben"}],
                    "facts":[],"relations":[],"closures":[]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, ExtractError::Malformed(_)), "{err:?}");
        assert!(err.to_string().contains("name"), "{err}");
    }

    #[test]
    fn a_fact_naming_a_mention_that_does_not_exist_is_refused() {
        let err = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[{"subject":7,"attribute":"employer","value":"Globex",
                              "text":"x","days_ago":null}],
                    "relations":[],"closures":[]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, ExtractError::Malformed(_)), "{err:?}");
        assert!(err.to_string().contains('7'), "{err}");
    }

    #[test]
    fn a_relation_naming_a_mention_that_does_not_exist_is_refused() {
        let err = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[],
                    "relations":[{"subject":0,"predicate":"knows","object":4,"days_ago":null}],
                    "closures":[]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, ExtractError::Malformed(_)), "{err:?}");
    }

    #[test]
    fn a_closure_naming_a_mention_that_does_not_exist_is_refused() {
        let err = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[],"relations":[],
                    "closures":[{"subject":9,"predicate":"employed_by",
                                 "days_ago":null,"because":"x"}]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, ExtractError::Malformed(_)), "{err:?}");
    }

    #[test]
    fn a_relation_from_a_mention_to_itself_is_refused() {
        // `rm_store::relate` refuses one, so accepting it here only moves the
        // error to a layer with less context about where it came from.
        let err = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[],
                    "relations":[{"subject":0,"predicate":"knows","object":0,"days_ago":null}],
                    "closures":[]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, ExtractError::Malformed(_)), "{err:?}");
        assert!(err.to_string().contains("itself"), "{err}");
    }

    #[test]
    fn a_days_ago_that_counts_forwards_is_refused() {
        // A negative count reaches the future by arithmetic, which saturation
        // cannot catch. On a closure that is not merely wrong: `ingest` asks
        // `edges_from(subject, at, Timestamp::MAX)`, so a future `at` reads a
        // graph that has not happened and tombstones what it finds -- a live
        // edge that quietly expires later, with nothing raising an error.
        let err = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[],"relations":[],
                    "closures":[{"subject":0,"predicate":"employed_by",
                                 "days_ago":-30,"because":"x"}]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, ExtractError::Malformed(_)), "{err:?}");
        assert!(err.to_string().contains("-30"), "{err}");
        assert!(err.to_string().contains("days_ago"), "{err}");
    }

    #[test]
    fn a_fact_or_relation_dated_after_its_turn_is_refused_too() {
        // Refused everywhere `days_ago` appears, not only where it does damage.
        // The word has one meaning in the prompt, and accepting it backwards in
        // two places while refusing it in a third would leave a caller unable
        // to read a timestamp after the turn as anything but ambiguous.
        for bad in [
            r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                "facts":[{"subject":0,"attribute":"employer","value":"Globex",
                          "text":"x","days_ago":-1}],
                "relations":[],"closures":[]}"#,
            r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"},
                            {"kind":"organisation","name":"Globex","text":"Globex"}],
                "facts":[],
                "relations":[{"subject":0,"predicate":"employed_by","object":1,"days_ago":-1}],
                "closures":[]}"#,
        ] {
            let err = extract(&turn(), &Canned(bad)).unwrap_err();
            assert!(matches!(err, ExtractError::Malformed(_)), "{err:?}");
        }
    }

    #[test]
    fn a_refusal_names_what_was_wrong_rather_than_that_something_was() {
        for bad in [
            r#"{"mentions":[{"kind":"person","name":"","text":"Ben"}],"facts":[],"relations":[],"closures":[]}"#,
            r#"{"mentions":[],"facts":[{"subject":0,"attribute":"a","value":"b","text":"c","days_ago":null}],"relations":[],"closures":[]}"#,
        ] {
            let err = extract(&turn(), &Canned(bad)).unwrap_err();
            assert!(
                err.to_string().len() > 40,
                "a refusal that does not say what was missing is not much better than a panic: {err}"
            );
        }
    }
}
