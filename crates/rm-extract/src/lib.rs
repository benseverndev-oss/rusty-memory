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

pub mod arity;
mod prompt;

pub use prompt::prompt;

use rm_core::{Supersession, Timestamp};
use serde::Deserialize;

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
///
/// `Deserialize` only. A mention is the one public type the wire format hands
/// back unchanged, so it is read from JSON; nothing in this workspace ever
/// writes one back out. `Serialize` was derived alongside it and used by
/// nothing -- and a derive kept for a caller who might would put this crate's
/// field names into someone else's persisted format, where renaming one here
/// becomes their breaking change rather than a rename.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
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
    /// Whether this fact replaces what the same attribute already held, or
    /// joins it.
    ///
    /// The model is the only party that ever knows: the store sees arrival
    /// order and nothing else, and arrival order says a second pet replaced the
    /// first.
    ///
    /// [`prompt`] does not currently ask. It did, as a `"replaces"` boolean per
    /// fact, and the measurement is in that function's docs -- it answered the
    /// question well and cost 19% of the facts, which is a worse trade than
    /// leaving them [`Supersession::Unstated`]. The field stays because the
    /// wire format should still accept an answer: `prompt` is public so a host
    /// can build its own, and a host that has found a way to ask without
    /// costing an extraction should not have to fork the parser to be heard.
    ///
    /// [`prompt`]: crate::prompt::prompt
    pub supersession: Supersession,
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
    /// What the response carried that this crate would not keep, and why.
    ///
    /// The reason [`extract`] can salvage at all. Its previous refusal to do so
    /// rested on a sound argument -- "a turn silently half-remembered, and
    /// nothing downstream can tell that apart from a turn that genuinely said
    /// less" -- whose force is entirely in the word *silently*. This field is
    /// the answer to it: a caller holding an `Extraction` can tell the two
    /// apart by looking, so keeping the good half stops being a silent loss and
    /// becomes a reported one.
    ///
    /// Empty on a clean response, which is the common case and the one worth
    /// checking against.
    pub dropped: Vec<Dropped>,
}

/// One thing the response described that was not kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dropped {
    /// Which list it came from: `"mention"`, `"fact"`, `"relation"` or
    /// `"closure"`.
    pub what: &'static str,
    /// Its position in that list, as the model wrote it. Positions are how the
    /// response refers to mentions, so this is the number that appears in the
    /// reasons below.
    pub index: usize,
    /// Why it was not kept, in the same voice as a refusal: what was wrong,
    /// not merely that something was.
    pub why: String,
}

impl std::fmt::Display for Dropped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {} dropped: {}", self.what, self.index, self.why)
    }
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
///
/// Two variants, not three. `Malformed` — "the response parsed but described
/// something impossible" — is gone, because after [`extract`] learned to drop
/// individual items nothing could construct it: everything it used to cover is
/// now a [`Dropped`] beside a successful extraction. A variant no code path can
/// produce is a claim about this crate that is not true, and keeping one
/// because removing it *would* be a breaking change in a published crate is a
/// reason that does not apply to one that is not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtractError {
    /// The completer failed before a response existed.
    Completer(CompleterError),
    /// The response was not the JSON this crate asked for. The only whole-
    /// response failure left: there is no parsed half to salvage from.
    Unparsable(String),
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::Completer(e) => write!(f, "{e}"),
            ExtractError::Unparsable(why) => write!(
                f,
                "the model's response was not the JSON this crate asked for: {why}"
            ),
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
    // Held as raw values and parsed one at a time. Typed straight through,
    // serde fails the whole document over a single field of the wrong type --
    // which is how 24 turns in 419 were lost to a model answering a yes/no
    // attribute with `true` instead of `"true"`, taking every correctly
    // extracted mention beside it.
    #[serde(default)]
    mentions: Vec<serde_json::Value>,
    #[serde(default)]
    facts: Vec<serde_json::Value>,
    #[serde(default)]
    relations: Vec<serde_json::Value>,
    #[serde(default)]
    closures: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct WireFact {
    subject: usize,
    attribute: String,
    value: Option<String>,
    text: String,
    days_ago: Option<i64>,
    /// Untyped on purpose, and the one field here that cannot fail the fact.
    ///
    /// A `bool` would let `"replaces": "true"` -- the same string-for-scalar
    /// slip that cost this crate 24 turns in 419 -- discard an otherwise
    /// perfect fact over a field that is allowed to be missing anyway. An
    /// answer nothing can read is indistinguishable from no answer, and no
    /// answer is already a legal one.
    #[serde(default)]
    replaces: Option<serde_json::Value>,
}

/// What the model said about arity, read leniently.
///
/// Anything unrecognised is [`Supersession::Unstated`] rather than a drop or a
/// guess: this field is optional, so a garbled answer to it is exactly as
/// informative as omitting it, and neither is a reason to lose the fact.
pub(crate) fn claim(raw: Option<&serde_json::Value>) -> Supersession {
    let yes = match raw {
        None | Some(serde_json::Value::Null) => return Supersession::Unstated,
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" => true,
            "false" | "no" => false,
            _ => return Supersession::Unstated,
        },
        _ => return Supersession::Unstated,
    };
    if yes {
        Supersession::Corrects
    } else {
        Supersession::Joins
    }
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
fn resolve(days_ago: Option<i64>, observed_at: Timestamp) -> Result<Timestamp, String> {
    match days_ago {
        None => Ok(observed_at),
        Some(days) if days < 0 => Err(format!(
            "it gives days_ago as {days}, which is a moment after the turn it came from -- days_ago counts backwards, and a future timestamp on a closure would end edges that have not been asserted yet"
        )),
        Some(days) => Ok(observed_at.saturating_sub(days.saturating_mul(DAY_MS))),
    }
}

/// Extract one turn.
///
/// # Salvages, and says what it did not keep
///
/// This used to refuse whole. The argument for that was good: "a response this
/// crate can only partly understand is a turn silently half-remembered, and
/// nothing downstream can tell that apart from a turn that genuinely said
/// less". Every word of it holds except *silently*, and [`Extraction::dropped`]
/// is what removes that word — a caller can tell the two apart by looking.
///
/// What made the old behaviour worth changing was measuring it. Over 419 turns
/// of real dialogue, 135 were refused outright, and the shapes that caused it
/// were nearly always one bad field beside several good ones: a fact naming a
/// mention the model had decided not to list (76), an attribute answered `true`
/// where a string belongs (24), a relation from someone to themselves (5).
/// Discarding a correctly identified person and a well-formed fact about her
/// because a *second* fact carried a bad index is not caution. It is a third of
/// a conversation lost to protect against remembering slightly less of it.
///
/// Refusal is kept for the one case where nothing can be salvaged: a response
/// that is not the JSON object this crate asked for. There is no good half of
/// that to keep.
///
/// # Positions survive dropping
///
/// A response refers to mentions by position, so dropping one cannot be allowed
/// to renumber the rest — a fact naming mention 2 must not silently come to
/// mean a different mention. Dropped mentions leave a hole that references to
/// them fall into: anything naming one is itself dropped, saying so.
///
/// No retries. The host owns the [`Completer`], so backoff, retry and provider
/// failover are its business and it is better placed to do them.
/// The JSON inside a markdown code fence, or the whole string if there is none.
///
/// Measured, not anticipated, and twice. It first appeared in [`arity`], whose
/// opening run parsed none of its seven batches because every response came
/// back as ```` ```json\n{...}\n``` ````. Looking for the same shape in the
/// extraction caches already on disk found it there too: **386 of 7,974 cached
/// responses across the ten LoCoMo conversations — 4.8% — are fenced**, and
/// every one of them was being refused as "not the JSON this crate asked for".
/// A whole turn's mentions, facts and relations, thrown away over three
/// backticks.
///
/// `serde_json` fails such a string at line 1 column 1, which reads exactly
/// like a model that would not answer. It is not: the answer is right there,
/// wrapped.
///
/// The prompt asks for no fence and this strips one anyway. The instruction is
/// a request; the parser is the guarantee.
///
/// Measured across all ten conversations, on the same caches so the model's
/// responses are byte-identical and only the parse differs:
///
/// ```text
///                 turns refused    assertions      mean recall@10
///   before             269           19,071             0.668
///   after               15           20,032             0.709
/// ```
///
/// 254 more turns ingested, 961 more assertions, and recall improved in
/// **10 conversations out of 10** -- which is the number that makes it a result
/// rather than a fluke. Recall had not moved for any earlier change in this
/// project, because every one of those relabelled facts the store already had;
/// an embedding-search metric can only see facts that were not there before.
pub(crate) fn unfenced(response: &str) -> &str {
    let text = response.trim();
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    // ```json, ```JSON, or just ``` -- the language tag runs to the first
    // newline, and a fence with no newline at all has no body to find.
    let body = match rest.split_once('\n') {
        Some((_tag, body)) => body,
        None => return text,
    };
    body.trim().strip_suffix("```").unwrap_or(body).trim()
}

/// Is this string the prompt's worked example rather than something the turn said?
///
/// Applied to a mention's name and to a fact's value, because the example
/// offers both and a model copies whichever it reaches for.
///
/// True only when the name is one the prompt shows *and* the turn does not
/// contain it. A model given a chunk with nothing extractable in it tends to
/// answer with the example it was shown, and the resulting entity is a person
/// who does not exist -- measured at 16 of 213 facts across arrow's API
/// reference, from six chunks whose whole text was a line like "Null type".
///
/// The second half of the condition is what makes this safe. Someone who
/// really does work at Globex is untouched, because their turn says so.
fn echoes_the_example(name: &str, turn: &str) -> bool {
    let name = name.trim();
    crate::prompt::EXAMPLE_NAMES
        .iter()
        .any(|e| e.eq_ignore_ascii_case(name))
        && !turn.to_lowercase().contains(&name.to_lowercase())
}

pub fn extract(turn: &Turn, completer: &impl Completer) -> Result<Extraction, ExtractError> {
    let response = completer.complete(&prompt(turn))?;

    let wire: WireExtraction = serde_json::from_str(unfenced(&response))
        .map_err(|e| ExtractError::Unparsable(e.to_string()))?;

    let mut out = Extraction::default();

    // Mentions first, because everything else indexes into them. `slot[i]` is
    // where the model's mention `i` ended up, or `None` if it was dropped.
    let mut slot: Vec<Option<usize>> = Vec::with_capacity(wire.mentions.len());
    for (i, raw) in wire.mentions.iter().enumerate() {
        match serde_json::from_value::<Mention>(raw.clone()) {
            Err(e) => {
                slot.push(None);
                out.dropped.push(Dropped {
                    what: "mention",
                    index: i,
                    why: e.to_string(),
                });
            }
            Ok(m) if m.name.trim().is_empty() => {
                slot.push(None);
                out.dropped.push(Dropped {
                    what: "mention",
                    index: i,
                    why: "it has no name, and resolution matches on the name -- an entity without one can never be recognised again, so every later turn about it would create another".to_string(),
                });
            }
            Ok(m) if echoes_the_example(&m.name, &turn.text) => {
                slot.push(None);
                out.dropped.push(Dropped {
                    what: "mention",
                    index: i,
                    why: format!(
                        "{:?} is a name from the prompt's own example and the turn does not contain it -- a turn with nothing in it draws the example back rather than an empty answer, and recording it would invent a person",
                        m.name
                    ),
                });
            }
            Ok(m) => {
                slot.push(Some(out.mentions.len()));
                out.mentions.push(m);
            }
        }
    }

    // Where a reference lands, or why it does not. Separates "there was never a
    // mention there" from "there was one and it was dropped": the first is the
    // model miscounting, the second is this function's own doing, and a reader
    // of `dropped` should not have to guess which.
    let n = wire.mentions.len();
    let landing = |i: usize| -> Result<usize, String> {
        match slot.get(i) {
            Some(Some(at)) => Ok(*at),
            Some(None) => Err(format!("it names mention {i}, which was itself dropped")),
            None => Err(format!("it names mention {i}, but the response listed {n}")),
        }
    };

    for (i, raw) in wire.facts.iter().enumerate() {
        let drop = |why: String| Dropped {
            what: "fact",
            index: i,
            why,
        };
        let f: WireFact = match serde_json::from_value(raw.clone()) {
            Ok(f) => f,
            Err(e) => {
                out.dropped.push(drop(e.to_string()));
                continue;
            }
        };
        let (subject, valid_from) =
            match (landing(f.subject), resolve(f.days_ago, turn.observed_at)) {
                (Ok(s), Ok(t)) => (s, t),
                (Err(why), _) | (_, Err(why)) => {
                    out.dropped.push(drop(why));
                    continue;
                }
            };
        // The value, as well as the mention's name. The example offers both,
        // and a model reaching for one is as likely to reach for the other:
        // one fact survived a 322-chunk run as `employer = "Globex"` hung on a
        // mention this guard had let through under a different spelling.
        if let Some(v) = f.value.as_deref() {
            if echoes_the_example(v, &turn.text) {
                out.dropped.push(drop(format!(
                    "its value {v:?} is from the prompt's own example and the turn does not contain it"
                )));
                continue;
            }
        }
        out.facts.push(Fact {
            subject,
            attribute: f.attribute,
            value: f.value,
            text: f.text,
            valid_from,
            supersession: claim(f.replaces.as_ref()),
        });
    }

    for (i, raw) in wire.relations.iter().enumerate() {
        let drop = |why: String| Dropped {
            what: "relation",
            index: i,
            why,
        };
        let r: WireRelation = match serde_json::from_value(raw.clone()) {
            Ok(r) => r,
            Err(e) => {
                out.dropped.push(drop(e.to_string()));
                continue;
            }
        };
        if r.subject == r.object {
            out.dropped.push(drop(format!(
                "it runs from mention {} to itself, which rm_store::relate refuses to create",
                r.subject
            )));
            continue;
        }
        let (subject, object, valid_from) = match (
            landing(r.subject),
            landing(r.object),
            resolve(r.days_ago, turn.observed_at),
        ) {
            (Ok(s), Ok(o), Ok(t)) => (s, o, t),
            (Err(why), _, _) | (_, Err(why), _) | (_, _, Err(why)) => {
                out.dropped.push(drop(why));
                continue;
            }
        };
        out.relations.push(Relation {
            subject,
            predicate: r.predicate,
            object,
            valid_from,
        });
    }

    for (i, raw) in wire.closures.iter().enumerate() {
        let drop = |why: String| Dropped {
            what: "closure",
            index: i,
            why,
        };
        let c: WireClosure = match serde_json::from_value(raw.clone()) {
            Ok(c) => c,
            Err(e) => {
                out.dropped.push(drop(e.to_string()));
                continue;
            }
        };
        let (subject, at) = match (landing(c.subject), resolve(c.days_ago, turn.observed_at)) {
            (Ok(s), Ok(t)) => (s, t),
            (Err(why), _) | (_, Err(why)) => {
                out.dropped.push(drop(why));
                continue;
            }
        };
        out.closures.push(Closure {
            subject,
            predicate: c.predicate,
            at,
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
    fn a_fact_carries_what_the_model_said_about_arity() {
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[{"subject":0,"attribute":"employer","value":"Globex",
                              "text":"Ben works at Globex","days_ago":null,"replaces":true},
                             {"subject":0,"attribute":"pet","value":"a cat",
                              "text":"Ben has a cat","days_ago":null,"replaces":false},
                             {"subject":0,"attribute":"mood","value":"tired",
                              "text":"Ben is tired","days_ago":null}],
                    "relations":[],"closures":[]}"#,
            ),
        )
        .unwrap();
        assert_eq!(out.facts[0].supersession, Supersession::Corrects);
        assert_eq!(out.facts[1].supersession, Supersession::Joins);
        assert_eq!(
            out.facts[2].supersession,
            Supersession::Unstated,
            "a fact that did not answer says so, rather than defaulting to a claim"
        );
    }

    #[test]
    fn a_garbled_answer_about_arity_costs_the_answer_and_not_the_fact() {
        // The lesson this crate already learned once, applied before it can be
        // learned again: a model that writes `"true"` for a boolean took 24
        // turns in 419 down with it. This field is optional, so an answer
        // nothing can read is worth exactly what an absent one is worth -- and
        // the fact beside it is worth keeping either way.
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[{"subject":0,"attribute":"employer","value":"Globex",
                              "text":"Ben works at Globex","days_ago":null,"replaces":"yes"},
                             {"subject":0,"attribute":"pet","value":"a cat",
                              "text":"Ben has a cat","days_ago":null,"replaces":"No"},
                             {"subject":0,"attribute":"mood","value":"tired",
                              "text":"Ben is tired","days_ago":null,"replaces":"sometimes"},
                             {"subject":0,"attribute":"age","value":"41",
                              "text":"Ben is 41","days_ago":null,"replaces":7}],
                    "relations":[],"closures":[]}"#,
            ),
        )
        .unwrap();
        assert_eq!(out.facts.len(), 4, "every fact survives");
        assert!(out.dropped.is_empty(), "and nothing is reported as lost");
        assert_eq!(out.facts[0].supersession, Supersession::Corrects);
        assert_eq!(out.facts[1].supersession, Supersession::Joins);
        assert_eq!(out.facts[2].supersession, Supersession::Unstated);
        assert_eq!(out.facts[3].supersession, Supersession::Unstated);
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
    fn a_mention_with_no_name_is_dropped() {
        // Resolution matches on the name. A mention without one becomes an
        // entity nothing can ever match against again, so every later turn
        // about it creates another -- which is why it is not kept. What
        // changed is the blast radius: the mention goes, not the turn.
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"   ","text":"Ben"}],
                    "facts":[],"relations":[],"closures":[]}"#,
            ),
        )
        .unwrap();
        assert!(out.mentions.is_empty());
        assert_eq!(out.dropped.len(), 1);
        assert!(out.dropped[0].why.contains("name"), "{}", out.dropped[0]);
    }

    // ---- what used to be a whole-turn refusal -----------------------------
    //
    // Each of these asserted that the entire extraction failed. They now
    // assert what replaced it: the offending item goes, everything else
    // survives, and `dropped` says which and why. The change is deliberate and
    // these are where it is visible -- see `extract`'s documentation for the
    // measurement that motivated it.

    #[test]
    fn a_fact_naming_a_mention_that_does_not_exist_is_dropped_and_the_rest_kept() {
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[{"subject":0,"attribute":"employer","value":"Globex","text":"Ben works at Globex","days_ago":null},
                             {"subject":9,"attribute":"a","value":"b","text":"c","days_ago":null}],
                    "relations":[],"closures":[]}"#,
            ),
        )
        .expect("one bad fact must not discard the turn");

        assert_eq!(out.mentions.len(), 1, "the named person survives");
        assert_eq!(out.facts.len(), 1, "and so does the fact that was fine");
        assert_eq!(out.facts[0].attribute, "employer");
        assert_eq!(out.dropped.len(), 1);
        assert_eq!(out.dropped[0].what, "fact");
        assert_eq!(out.dropped[0].index, 1);
        assert!(
            out.dropped[0].why.contains("names mention 9"),
            "{}",
            out.dropped[0]
        );
    }

    #[test]
    fn a_relation_naming_a_mention_that_does_not_exist_is_dropped_and_the_rest_kept() {
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[],
                    "relations":[{"subject":0,"predicate":"employed_by","object":9,"days_ago":null}],
                    "closures":[]}"#,
            ),
        )
        .unwrap();
        assert_eq!(out.mentions.len(), 1);
        assert!(out.relations.is_empty());
        assert_eq!(out.dropped[0].what, "relation");
    }

    #[test]
    fn a_closure_naming_a_mention_that_does_not_exist_is_dropped_and_the_rest_kept() {
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[],"relations":[],
                    "closures":[{"subject":9,"predicate":"employed_by",
                                 "days_ago":null,"because":"x"}]}"#,
            ),
        )
        .unwrap();
        assert_eq!(out.mentions.len(), 1);
        assert!(out.closures.is_empty());
        assert_eq!(out.dropped[0].what, "closure");
    }

    #[test]
    fn a_relation_from_a_mention_to_itself_is_dropped_and_the_rest_kept() {
        // `rm_store::relate` refuses one, so accepting it here only moves the
        // failure somewhere with less context. Dropping it keeps that property
        // while letting the turn's other content through.
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[],
                    "relations":[{"subject":0,"predicate":"knows","object":0,"days_ago":null}],
                    "closures":[]}"#,
            ),
        )
        .unwrap();
        assert!(out.relations.is_empty());
        assert!(
            out.dropped[0].why.contains("to itself"),
            "{}",
            out.dropped[0]
        );
    }

    #[test]
    fn a_mention_with_no_name_is_dropped_without_renumbering_the_others() {
        // The hazard dropping a mention creates: positions are how the response
        // refers to mentions, so removing one must not make a later reference
        // mean something different. Mention 1 goes; the fact naming mention 2
        // must still find Ada rather than silently landing on someone else.
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"},
                                {"kind":"person","name":"  ","text":"someone"},
                                {"kind":"person","name":"Ada","text":"Ada"}],
                    "facts":[{"subject":2,"attribute":"employer","value":"Globex","text":"Ada works at Globex","days_ago":null},
                             {"subject":1,"attribute":"employer","value":"Acme","text":"someone works at Acme","days_ago":null}],
                    "relations":[],"closures":[]}"#,
            ),
        )
        .unwrap();

        assert_eq!(out.mentions.len(), 2);
        assert_eq!(out.mentions[0].name, "Ben");
        assert_eq!(out.mentions[1].name, "Ada");
        assert_eq!(out.facts.len(), 1);
        assert_eq!(
            out.mentions[out.facts[0].subject].name, "Ada",
            "the surviving fact must still point at the mention the model meant"
        );
        // Two drops: the nameless mention, and the fact that referred to it.
        assert_eq!(out.dropped.len(), 2);
        assert!(out.dropped[0].why.contains("no name"), "{}", out.dropped[0]);
        assert!(
            out.dropped[1].why.contains("itself dropped"),
            "a reference to a dropped mention should say so, not claim the model miscounted: {}",
            out.dropped[1]
        );
    }

    #[test]
    fn a_days_ago_that_counts_forwards_drops_only_what_carried_it() {
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                    "facts":[{"subject":0,"attribute":"employer","value":"Globex","text":"x","days_ago":-3},
                             {"subject":0,"attribute":"city","value":"London","text":"y","days_ago":2}],
                    "relations":[],"closures":[]}"#,
            ),
        )
        .unwrap();
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.facts[0].attribute, "city");
        assert!(
            out.dropped[0].why.contains("days_ago"),
            "{}",
            out.dropped[0]
        );
    }

    #[test]
    fn a_fact_or_relation_dated_after_its_turn_is_dropped_too() {
        for bad in [
            r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"},{"kind":"organisation","name":"Globex","text":"Globex"}],
                "facts":[],"relations":[{"subject":0,"predicate":"employed_by","object":1,"days_ago":-1}],"closures":[]}"#,
            r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],
                "facts":[],"relations":[],"closures":[{"subject":0,"predicate":"employed_by","days_ago":-1,"because":"x"}]}"#,
        ] {
            let out = extract(&turn(), &Canned(bad)).unwrap();
            assert_eq!(out.dropped.len(), 1, "{bad}");
            assert!(
                out.dropped[0].why.contains("days_ago"),
                "{}",
                out.dropped[0]
            );
        }
    }

    #[test]
    fn a_response_that_is_not_the_json_asked_for_is_still_refused_whole() {
        // The one case with no good half to keep. Salvage needs something
        // parsed to salvage from.
        //
        // A fenced response used to be in this list. It is not any more, and
        // the test below is why -- refusing those was throwing away 4.8% of
        // every turn in the corpus over three backticks.
        for bad in ["Sure! Here you go: {}", "[1,2,3]", "```"] {
            let err = extract(&turn(), &Canned(bad)).unwrap_err();
            assert!(matches!(err, ExtractError::Unparsable(_)), "{err:?}");
        }
    }

    #[test]
    fn a_fenced_response_is_read_rather_than_refused() {
        // Found by looking for `arity`'s failure mode in the extraction caches
        // already on disk: 386 of 7,974 cached responses across the ten LoCoMo
        // conversations come back wrapped in a markdown fence, and every one
        // was refused as "not the JSON this crate asked for" -- a whole turn's
        // mentions, facts and relations discarded.
        //
        // `serde_json` fails such a string at line 1 column 1, which reads
        // exactly like a model that would not answer. It answered.
        let fenced = "```json\n{\"mentions\":[{\"kind\":\"person\",\"name\":\"Ben Severn\",\"text\":\"Ben\"}],\
                      \"facts\":[{\"subject\":0,\"attribute\":\"employer\",\"value\":\"Globex\",\
                      \"text\":\"Ben works at Globex\",\"days_ago\":null}],\
                      \"relations\":[],\"closures\":[]}\n```";
        let out = extract(&turn(), &Canned(fenced)).unwrap();
        assert_eq!(out.mentions.len(), 1);
        assert_eq!(out.mentions[0].name, "Ben Severn");
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.facts[0].value.as_deref(), Some("Globex"));
        assert!(out.dropped.is_empty(), "and nothing is reported as lost");
    }

    #[test]
    fn a_bare_fence_and_an_upper_case_tag_are_both_read() {
        for fenced in [
            "```\n{\"mentions\":[],\"facts\":[],\"relations\":[],\"closures\":[]}\n```",
            "```JSON\n{\"mentions\":[],\"facts\":[],\"relations\":[],\"closures\":[]}\n```",
        ] {
            assert!(extract(&turn(), &Canned(fenced)).is_ok(), "{fenced}");
        }
    }

    #[test]
    fn every_drop_names_what_was_wrong_rather_than_that_something_was() {
        for bad in [
            r#"{"mentions":[{"kind":"person","name":"","text":"Ben"}],"facts":[],"relations":[],"closures":[]}"#,
            r#"{"mentions":[],"facts":[{"subject":0,"attribute":"a","value":"b","text":"c","days_ago":null}],"relations":[],"closures":[]}"#,
            r#"{"mentions":[{"kind":"person","name":"Ben","text":"Ben"}],"facts":[{"subject":0,"attribute":"a","value":true,"text":"c","days_ago":null}],"relations":[],"closures":[]}"#,
        ] {
            let out = extract(&turn(), &Canned(bad)).unwrap();
            assert_eq!(out.dropped.len(), 1, "{bad}");
            assert!(
                out.dropped[0].why.len() > 20,
                "a drop that does not say what was wrong is not much better than losing it silently: {}",
                out.dropped[0]
            );
        }
    }
    /// The prompt's own worked example is not a fact about the turn.
    ///
    /// A model given a chunk with nothing in it does not answer "nothing" -- it
    /// answers with the example it was shown. Measured on arrow's API
    /// reference: 16 of 213 facts were Alex Chen working at Globex, extracted
    /// from six chunks whose whole text was a line like "Null type". For a
    /// store whose entire claim is that it can tell you what it does not know,
    /// inventing a person is the worst thing it can do.
    ///
    /// Guarded here rather than by rewording the prompt. Wording it away was
    /// tried first and measured: the leak went to zero and so did the yield,
    /// 37 facts to 0 on the same corpus, because the sentence that stops the
    /// model copying an example also stops it reading a definition. This is
    /// exact, costs nothing when the turn is real, and can be tested without a
    /// network.
    #[test]
    fn the_prompts_own_example_is_not_a_fact_about_the_turn() {
        let empty = Turn {
            text: "Null type".to_string(),
            speaker: None,
            observed_at: NOW,
            session: "session-1".to_string(),
        };
        let out = extract(
            &empty,
            &Canned(
                r#"{"mentions":[
                     {"kind":"person","name":"Alex Chen","text":"Alex"},
                     {"kind":"organisation","name":"Globex","text":"Globex"}],
                   "facts":[
                     {"subject":0,"attribute":"employer","value":"Globex",
                      "text":"Alex works at Globex","days_ago":null}],
                   "relations":[
                     {"subject":0,"predicate":"employed_by","object":1,"days_ago":null}],
                   "closures":[]}"#,
            ),
        )
        .unwrap();

        assert!(
            out.mentions.is_empty(),
            "the example was recorded as a mention: {:?}",
            out.mentions
        );
        assert!(out.facts.is_empty(), "{:?}", out.facts);
        assert!(out.relations.is_empty(), "{:?}", out.relations);
        // Two mentions, plus the fact and the relation that pointed at them:
        // nothing is discarded quietly, which is the whole point of `dropped`.
        let by_example: Vec<&Dropped> = out
            .dropped
            .iter()
            .filter(|d| d.why.contains("example"))
            .collect();
        assert_eq!(
            by_example.len(),
            2,
            "the example drop was not reported: {:?}",
            out.dropped
        );
        assert!(
            by_example.iter().all(|d| d.what == "mention"),
            "{by_example:?}"
        );
        assert_eq!(
            out.dropped.len(),
            4,
            "the fact and relation that pointed at them went unreported: {:?}",
            out.dropped
        );
    }

    /// A turn that really does name one of them keeps it.
    ///
    /// The guard is worth nothing if it costs real facts. It fires on the name
    /// being absent from the turn, never on the name itself -- so a person who
    /// genuinely works at Globex is unaffected, and this is the test that says
    /// so.
    #[test]
    fn an_example_name_the_turn_actually_uses_is_kept() {
        let out = extract(
            &turn(),
            &Canned(
                r#"{"mentions":[
                     {"kind":"person","name":"Alex Chen","text":"Alex"},
                     {"kind":"organisation","name":"Globex","text":"Globex"}],
                   "facts":[
                     {"subject":0,"attribute":"employer","value":"Globex",
                      "text":"Alex works at Globex","days_ago":null}],
                   "relations":[],"closures":[]}"#,
            ),
        )
        .unwrap();

        // "Globex" is in the turn, "Alex Chen" is not.
        assert_eq!(out.mentions.len(), 1, "{:?}", out.mentions);
        assert_eq!(out.mentions[0].name, "Globex");
        assert!(
            out.facts.is_empty(),
            "a fact about a dropped mention survived: {:?}",
            out.facts
        );
    }
    /// An example value hung on a real mention is dropped too.
    ///
    /// This is the one that got through. The guard covered a mention's name, so
    /// a 322-chunk run over arrow came back with one leaked fact instead of
    /// sixteen: a mention the guard let through, carrying `employer = "Globex"`
    /// from the example. A name-only guard turns a loud failure into a quiet
    /// one, which is worse than the failure.
    ///
    /// The mention here is legitimate and stays. Only the value is the example.
    #[test]
    fn an_example_value_on_a_real_mention_is_dropped() {
        let doc = Turn {
            text: "BatchCoalescer concatenates small batches into larger ones".to_string(),
            speaker: None,
            observed_at: NOW,
            session: "session-1".to_string(),
        };
        let out = extract(
            &doc,
            &Canned(
                r#"{"mentions":[
                     {"kind":"thing","name":"BatchCoalescer","text":"BatchCoalescer"}],
                   "facts":[
                     {"subject":0,"attribute":"employer","value":"Globex",
                      "text":"BatchCoalescer works at Globex","days_ago":null}],
                   "relations":[],"closures":[]}"#,
            ),
        )
        .unwrap();

        assert_eq!(out.mentions.len(), 1, "a real mention was thrown away");
        assert_eq!(out.mentions[0].name, "BatchCoalescer");
        assert!(
            out.facts.is_empty(),
            "the example's value was recorded as a fact: {:?}",
            out.facts
        );
        assert_eq!(out.dropped.len(), 1, "{:?}", out.dropped);
        assert_eq!(out.dropped[0].what, "fact");
        assert!(out.dropped[0].why.contains("Globex"), "{:?}", out.dropped);
    }
}
