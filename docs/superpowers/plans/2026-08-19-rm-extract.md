# rm-extract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn a line of dialogue into mentions, facts, relations and closures, and apply them to an `Engine`.

**Architecture:** `rm-extract` depends on `rm-core` alone, defines a `Completer` port the host implements, builds its own prompt, and parses the response into a plain `Extraction` addressed by local index. `rm-engine` gains an `Embedder` port and `ingest`, which embeds, resolves mentions to entities, applies facts and relations, and closes edges a closure ends — recording each closure as `Source::AgentInference`.

**Tech Stack:** Rust 2021, `rust-version = 1.85`, `serde` + `serde_json`. No new third-party dependencies. `cargo-nextest` for tests.

**Spec:** `docs/superpowers/specs/2026-08-19-rm-extract-design.md`

## Global Constraints

- Edition 2021, `rust-version = 1.85`, license MIT — inherited via `edition.workspace = true` etc.
- New crates start at `version = "0.0.0"`.
- **No new third-party dependencies.** `serde` and `serde_json` come from `[workspace.dependencies]`. No HTTP client, no async runtime, no date library, no provider SDK — anywhere.
- **No crate touches the network.** `Completer` and `Embedder` are traits the host implements. Every test in this plan runs offline against a stub.
- `BTreeMap`/`BTreeSet`, never `HashMap`/`HashSet`, in anything serialised.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo nextest run`, `cargo test --doc`, `cargo doc --no-deps --all-features` and `typos .` must all pass before any commit.
- Test names are behavioural sentences stating the property. Doc comments explain *why*, including why a rejected alternative was rejected.
- Where the data cannot answer, refuse or report a gap. Never guess plausibly.
- Do not modify `rm-engine`'s shared `test_ruleset()` — its 4.0/8.0 thresholds are load-bearing for many existing tests. New tests get their own ruleset.
- **`cargo fmt` collapses `\` line-continuations inside string literals in this repo.** Keep multi-sentence error messages on one line.

---

## File Structure

**Created:**
- `crates/rm-extract/Cargo.toml`
- `crates/rm-extract/src/lib.rs` — `Turn`, `Mention`, `Fact`, `Relation`, `Closure`, `Extraction`, `Completer`, errors, `extract`
- `crates/rm-extract/src/prompt.rs` — `prompt(&Turn) -> String` and the schema it describes
- `crates/rm-engine/tests/extract.rs` — the end-to-end story

**Modified:**
- `Cargo.toml` — add `crates/rm-extract` to members and `[workspace.dependencies]`
- `crates/rm-engine/Cargo.toml` — depend on `rm-extract`
- `crates/rm-engine/src/lib.rs` — `Embedder`, `EngineError::Embed`, re-exports
- `crates/rm-engine/src/ingest.rs` — new module: `Ingested`, `Closed`, `Engine::ingest`
- `README.md` — move `rm-extract` from planned to in progress

## Two decisions the spec left to this plan

**The wire schema.** The spec fixes the Rust types but not the JSON. It is:

```json
{
  "mentions": [{"kind": "person", "name": "Ben Severn", "text": "Ben"}],
  "facts": [{"subject": 0, "attribute": "employer", "value": "Globex",
             "text": "Ben works at Globex", "days_ago": null}],
  "relations": [{"subject": 0, "predicate": "employed_by", "object": 1,
                 "days_ago": null}],
  "closures": [{"subject": 0, "predicate": "employed_by", "days_ago": null,
                "because": "starting a job ends the previous one"}]
}
```

**Timestamps are a relative day offset, not an absolute.** `days_ago: Option<i64>`, resolved against `turn.observed_at` as `observed_at - days * 86_400_000`; `null` means the turn's own moment.

Asking a model for epoch milliseconds invites it to invent a number that looks right, and there is no way to tell an invented timestamp from a correct one after the fact. "Sixty days ago" is something it can actually reason about from the text. It also needs no date library, which matters because this workspace has no third-party dependencies and a calendar is not where that record should break.

The cost, recorded honestly: a turn saying "I joined in March 2019" is expressed as a day count rather than a date, so precision degrades for distant past events. `Interval` accepts pre-epoch values, so nothing breaks; only the resolution suffers. A date parser is additive later.

---

### Task 1: The crate, its vocabulary, and the prompt

**Files:**
- Create: `crates/rm-extract/Cargo.toml`, `crates/rm-extract/src/lib.rs`, `crates/rm-extract/src/prompt.rs`
- Modify: `Cargo.toml`

**Interfaces:**
- Consumes: `rm_core::{Interval, Timestamp}`.
- Produces: `Turn { text: String, speaker: Option<String>, observed_at: Timestamp, session: String }`; `Mention { kind: String, name: String, text: String }`; `Fact { subject: usize, attribute: String, value: Option<String>, text: String, valid_from: Timestamp }`; `Relation { subject: usize, predicate: String, object: usize, valid_from: Timestamp }`; `Closure { subject: usize, predicate: String, at: Timestamp, because: String }`; `Extraction { mentions: Vec<Mention>, facts: Vec<Fact>, relations: Vec<Relation>, closures: Vec<Closure> }`; `trait Completer { fn complete(&self, prompt: &str) -> Result<String, CompleterError>; }`; `struct CompleterError(pub String)`; `enum ExtractError`; `pub fn prompt(turn: &Turn) -> String`.

- [ ] **Step 1: Add the crate to the workspace**

In the root `Cargo.toml`, add `"crates/rm-extract"` to `members`, and to `[workspace.dependencies]`:

```toml
rm-extract = { path = "crates/rm-extract" }
```

`crates/rm-extract/Cargo.toml`:

```toml
[package]
name = "rm-extract"
version = "0.0.0"
description = "Turn a line of dialogue into mentions, facts and relations"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[dependencies]
rm-core = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
```

No `rm-engine`, no `rm-store`. This crate never learns what a store is.

- [ ] **Step 2: Write the failing tests**

In `crates/rm-extract/src/prompt.rs`, at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn turn(text: &str, speaker: Option<&str>) -> Turn {
        Turn {
            text: text.to_string(),
            speaker: speaker.map(str::to_string),
            observed_at: 1_720_000_000_000,
            session: "session-1".to_string(),
        }
    }

    #[test]
    fn the_prompt_carries_the_turn_it_is_about() {
        let p = prompt(&turn("I started at Globex", None));
        assert!(p.contains("I started at Globex"));
    }

    #[test]
    fn the_prompt_names_the_speaker_so_first_person_resolves() {
        // Without it the model has no way to turn "I" into a mention anything
        // can be resolved against, and every turn invents a new person.
        let p = prompt(&turn("I started at Globex", Some("Ben Severn")));
        assert!(p.contains("Ben Severn"));
    }

    #[test]
    fn an_unattributed_turn_says_so_rather_than_leaving_a_gap() {
        // A prompt with an empty slot where the speaker should be reads as a
        // template that failed to render, and a model will fill it with
        // something. Saying the speaker is unknown is an instruction; leaving
        // a blank is an invitation.
        let p = prompt(&turn("Alice started at Globex", None));
        assert!(!p.contains("Ben Severn"));
        assert!(
            p.to_lowercase().contains("speaker is not known"),
            "the prompt must state the absence rather than omit the line"
        );
    }

    #[test]
    fn the_prompt_describes_every_field_the_parser_reads() {
        // The prompt and the schema have to agree, and nothing else checks
        // that they do. If a field is added to the wire format without being
        // described here, the model never emits it and the extraction is
        // silently thin.
        let p = prompt(&turn("anything", None));
        for field in [
            "mentions", "facts", "relations", "closures", "kind", "name",
            "text", "subject", "attribute", "value", "predicate", "object",
            "days_ago", "because",
        ] {
            assert!(p.contains(field), "the prompt never mentions {field:?}");
        }
    }

    #[test]
    fn the_prompt_tells_the_model_when_to_close_a_relationship() {
        // The whole closure mechanism depends on the model volunteering one.
        let p = prompt(&turn("I started at Globex", None));
        let lower = p.to_lowercase();
        assert!(lower.contains("closure") || lower.contains("closures"));
        assert!(
            lower.contains("ended") || lower.contains("no longer"),
            "the prompt has to say what a closure is for, not just name the field"
        );
    }
}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test -p rm-extract 2>&1 | head -20`
Expected: FAIL — the crate has no `prompt`.

- [ ] **Step 4: Write the types**

`crates/rm-extract/src/lib.rs`:

```rust
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
            ExtractError::Malformed(why) => write!(f, "the model described something impossible: {why}"),
        }
    }
}

impl std::error::Error for ExtractError {}

impl From<CompleterError> for ExtractError {
    fn from(e: CompleterError) -> Self {
        ExtractError::Completer(e)
    }
}
```

- [ ] **Step 5: Write the prompt**

`crates/rm-extract/src/prompt.rs`:

```rust
//! The question this crate asks, and the shape of answer it will accept.
//!
//! Kept beside the parser deliberately. The two have to agree, and nothing
//! outside this crate checks that they do — a prompt that has drifted from its
//! schema yields a thin extraction rather than an error.

use crate::Turn;

/// The prompt for one turn.
///
/// Public so a host can read it, log it, diff it across versions, or build a
/// few-shot variant on top of it. The crate owning the contract does not mean
/// the contract has to be a secret.
pub fn prompt(turn: &Turn) -> String {
    let speaker = match &turn.speaker {
        Some(name) => format!("The speaker is {name}. Resolve \"I\", \"me\" and \"my\" to them."),
        // Stated rather than omitted: a prompt with a blank where the speaker
        // should be reads as a template that failed to render, and a model will
        // fill it. Saying the speaker is unknown is an instruction.
        None => "The speaker is not known. Do not invent a name for them; leave first-person references unattributed.".to_string(),
    };

    format!(
        r#"Extract what this turn of dialogue says about people, organisations and places, as JSON.

{speaker}

Turn:
{text}

Reply with only a JSON object of this shape, and nothing else:

{{
  "mentions": [
    {{"kind": "person", "name": "Ben Severn", "text": "Ben"}}
  ],
  "facts": [
    {{"subject": 0, "attribute": "employer", "value": "Globex",
      "text": "Ben works at Globex", "days_ago": null}}
  ],
  "relations": [
    {{"subject": 0, "predicate": "employed_by", "object": 1, "days_ago": null}}
  ],
  "closures": [
    {{"subject": 0, "predicate": "employed_by", "days_ago": null,
      "because": "starting a new job ends the previous one"}}
  ]
}}

Rules:

- "mentions" lists every distinct thing the turn refers to. "subject" and
  "object" everywhere else are indices into it, starting at 0.
- "kind" is what sort of thing it is: person, organisation, place, or another
  word that fits.
- "name" is what to call it, used to recognise it again in later turns. Use the
  fullest form the turn gives.
- "text" on a mention is the phrasing the turn used. "text" on a fact is a short
  sentence stating that fact on its own, because it is searched for separately.
- "value" may be null to say an attribute has no value — "he is between jobs"
  is a fact with a null value, not a missing fact.
- "days_ago" is how long before now the thing began or ended, as a whole number
  of days, or null if it is happening now. Do not output dates or timestamps.
- "closures" is for relationships that have ended. If the turn says someone
  started a new job, their previous employment ended: emit a closure naming the
  subject and the predicate, and say why in "because". Do not name what it
  ended — that is not known here. Emit nothing if nothing ended.
- Every list may be empty. A turn that says nothing worth remembering is a valid
  answer with four empty lists.
"#,
        text = turn.text
    )
}
```

- [ ] **Step 6: Run to verify they pass**

Run: `cargo test -p rm-extract 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 7: Verify and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run
typos .
git add Cargo.toml crates/rm-extract
git commit -m "feat(rm-extract): the crate, its vocabulary, and the prompt

Completer is a port the host implements, so no crate in this workspace touches
the network and rm-extract depends on rm-core and serde alone. A stub completer
is three lines, so everything here tests offline.

The prompt lives beside the parser because the two have to agree and nothing
outside this crate checks that they do -- a prompt that has drifted from its
schema yields a thin extraction rather than an error. It is public anyway: a
host can read it, log it, or build on it.

Timestamps are a relative day count rather than an absolute. A model asked for
epoch milliseconds invents a plausible number and nothing downstream can tell
an invented one from a correct one; days before now is something it can reason
about from the text, and it needs no date library."
```

---

### Task 2: `extract` — parse, and refuse everything that does not add up

**Files:**
- Modify: `crates/rm-extract/src/lib.rs`
- Test: `crates/rm-extract/src/lib.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: Task 1.
- Produces: `pub fn extract(turn: &Turn, completer: &impl Completer) -> Result<Extraction, ExtractError>`.

- [ ] **Step 1: Write the failing tests**

```rust
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
```

**Note on the two `Canned` literals in the last test:** they are `&'static str`, and the loop needs them to be, which is why `Canned` holds `&'static str` rather than `String`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rm-extract extract 2>&1 | head -20`
Expected: FAIL — `cannot find function 'extract'`.

- [ ] **Step 3: Write the wire types and the parser**

Add to `crates/rm-extract/src/lib.rs`:

```rust
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
fn resolve(days_ago: Option<i64>, observed_at: Timestamp) -> Timestamp {
    match days_ago {
        None => observed_at,
        Some(days) => observed_at.saturating_sub(days.saturating_mul(DAY_MS)),
    }
}

/// Extract one turn.
///
/// Refuses rather than salvages. A response this crate can only partly
/// understand is a turn silently half-remembered, and nothing downstream can
/// tell that apart from a turn that genuinely said less — so a mention with no
/// name, or an index naming a mention that is not there, fails the whole
/// extraction and says which.
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

    let mut out = Extraction::default();
    out.mentions = wire.mentions;

    for f in wire.facts {
        names(f.subject, "fact")?;
        out.facts.push(Fact {
            subject: f.subject,
            attribute: f.attribute,
            value: f.value,
            text: f.text,
            valid_from: resolve(f.days_ago, turn.observed_at),
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
            valid_from: resolve(r.days_ago, turn.observed_at),
        });
    }

    for c in wire.closures {
        names(c.subject, "closure")?;
        out.closures.push(Closure {
            subject: c.subject,
            predicate: c.predicate,
            at: resolve(c.days_ago, turn.observed_at),
            because: c.because,
        });
    }

    Ok(out)
}
```

`Mention` already derives `Deserialize`, so the wire form reuses it directly — it has no relative timestamp to resolve.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p rm-extract 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Verify and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run
typos .
git add crates/rm-extract/src/lib.rs
git commit -m "feat(rm-extract): extract, refusing what does not add up

A mention with no name, an index naming a mention that is not there, a relation
from something to itself -- each fails the whole extraction and says which. A
response this crate can only partly understand is a turn silently
half-remembered, and nothing downstream can tell that apart from a turn that
genuinely said less.

An empty extraction is not an error. Plenty of turns say nothing worth
remembering, and conflating that with failure would leave a caller unable to
tell the two apart."
```

---

### Task 3: `rm-engine` — the `Embedder` port and the start of `ingest`

**Files:**
- Modify: `crates/rm-engine/Cargo.toml`, `crates/rm-engine/src/lib.rs`
- Create: `crates/rm-engine/src/ingest.rs`

**Interfaces:**
- Consumes: Tasks 1-2.
- Produces: `pub trait Embedder { fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedderError>; }`; `pub struct EmbedderError(pub String)`; `EngineError::Embed(EmbedderError)`; `pub struct Ingested { entities: Vec<StableId>, assertions: Vec<AssertionId>, reviews: Vec<ReviewId>, closed: Vec<Closed> }`; `pub struct Closed { subject: StableId, predicate: String, object: StableId, because: String }`; `Engine::ingest(&mut self, &Extraction, &impl Embedder) -> Result<Ingested, EngineError>` — mentions only at this stage.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/rm-engine/src/lib.rs`:

```rust
    /// An embedder that maps text to a vector by hashing its bytes into three
    /// buckets. Deterministic, offline, and different texts get different
    /// vectors -- which is all any test here needs.
    struct Buckets;

    impl Embedder for Buckets {
        fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedderError> {
            let mut v = [0.0f32; 3];
            for (i, b) in text.bytes().enumerate() {
                v[i % 3] += f32::from(b);
            }
            // A zero vector is refused under cosine, and an empty string would
            // produce one.
            if v.iter().all(|x| *x == 0.0) {
                v[0] = 1.0;
            }
            Ok(v.to_vec())
        }
    }

    struct NoEmbedder;

    impl Embedder for NoEmbedder {
        fn embed(&self, _text: &str) -> Result<Vec<f32>, EmbedderError> {
            Err(EmbedderError("the embedding service is down".to_string()))
        }
    }

    fn mention(kind: &str, name: &str) -> rm_extract::Mention {
        rm_extract::Mention {
            kind: kind.to_string(),
            name: name.to_string(),
            text: name.to_string(),
        }
    }

    #[test]
    fn every_mention_becomes_an_entity_even_with_no_facts_about_it() {
        // A place named only as the object of an edge still has to exist, or
        // the edge could not name it.
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn"), mention("place", "Bristol")],
            ..Default::default()
        };

        let out = e.ingest(&extraction, &Buckets).unwrap();
        assert_eq!(out.entities.len(), 2);
        assert_eq!(e.entity_count(), 2);
        assert_ne!(out.entities[0], out.entities[1]);
    }

    #[test]
    fn a_mention_is_recorded_with_its_kind_so_it_can_be_recalled_at_all() {
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("place", "Bristol")],
            ..Default::default()
        };
        let out = e.ingest(&extraction, &Buckets).unwrap();
        assert_eq!(
            e.about(out.entities[0], "kind", 100, 100).unwrap(),
            Believed::Value("place".into())
        );
    }

    #[test]
    fn a_failing_embedder_leaves_the_store_and_the_index_untouched() {
        // The same guarantee `remember` makes: a write that cannot complete
        // must cost nothing, because a fact with no vector to find it is
        // undetectable from outside.
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn")],
            ..Default::default()
        };

        let err = e.ingest(&extraction, &NoEmbedder).unwrap_err();
        assert!(matches!(err, EngineError::Embed(_)), "{err:?}");
        assert!(
            err.to_string().contains("embedding service is down"),
            "the host's own explanation must survive: {err}"
        );
        assert_eq!(e.entity_count(), 0);
        assert_eq!(e.index_len(), 0);
    }

    #[test]
    fn an_ambiguous_mention_comes_back_as_a_review_rather_than_a_merge() {
        // Resolution's middle band survives ingestion. A turn naming someone
        // who might be someone already known must not quietly merge them, and
        // the question has to reach the caller -- a review nobody can see is
        // the same as no review.
        let mut e = engine();
        let first = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn")],
            ..Default::default()
        };
        e.ingest(&first, &Buckets).unwrap();

        let second = rm_extract::Extraction {
            mentions: vec![rm_extract::Mention {
                kind: "person".to_string(),
                name: "Ben Severne".to_string(),
                text: "Ben Severne".to_string(),
            }],
            ..Default::default()
        };
        let out = e.ingest(&second, &Buckets).unwrap();

        assert_eq!(out.reviews.len(), 1, "the near-miss has to be asked about");
        assert_eq!(e.entity_count(), 2, "and they stay apart until someone answers");
        assert_eq!(e.pending_review().len(), 1);
    }

    #[test]
    fn ingesting_nothing_writes_nothing_and_is_not_an_error() {
        let mut e = engine();
        let out = e.ingest(&rm_extract::Extraction::default(), &Buckets).unwrap();
        assert!(out.entities.is_empty());
        assert_eq!(e.entity_count(), 0);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rm-engine every_mention_becomes 2>&1 | head -20`
Expected: FAIL — `rm_extract` is not a dependency and `Engine` has no `ingest`.

- [ ] **Step 3: Add the dependency and the port**

In `crates/rm-engine/Cargo.toml`, add to `[dependencies]`:

```toml
rm-extract = { workspace = true }
```

In `crates/rm-engine/src/lib.rs`, beside the other module declarations add `mod ingest;`, and beside the other re-exports:

```rust
pub use ingest::{Closed, Embedder, EmbedderError, Ingested};
pub use rm_extract::{
    extract, prompt, Closure, Completer, CompleterError, ExtractError, Extraction, Fact, Mention,
    Relation, Turn,
};
```

`extract` and `prompt` are re-exported alongside the types: a caller who has to
reach into `rm-extract` for the function while naming its types through
`rm-engine` has the worst of both, and `tests/extract.rs` calls
`rm_engine::extract` to prove it does not have to.

A caller ingesting a turn needs to name every type in `extract`'s signature and in `ingest`'s. Re-exporting them keeps the one-crate-in-the-manifest promise `tests/readme.rs` exists to prove.

Add to `EngineError`:

```rust
    /// The host's embedder failed. Carries its explanation.
    Embed(EmbedderError),
```

with the `Display` arm `EngineError::Embed(e) => write!(f, "{e}")` and a `From<EmbedderError>` impl, matching how the other wrapped errors are handled.

- [ ] **Step 4: Write the module**

`crates/rm-engine/src/ingest.rs`:

```rust
//! Applying an extraction to the store.
//!
//! `rm_extract` describes a turn and knows nothing about entities; this is
//! where a description becomes writes. The mapping from a mention's local index
//! to a `StableId` lives here because here is where the ids are born — inside
//! `remember`, which resolves the mention against everything already known.

use rm_core::{Interval, Provenance, Source};
use rm_extract::Extraction;
use rm_store::StableId;

use crate::{AssertionId, Engine, EngineError, Observation, Record, Remembered, ReviewId};

/// Whatever went wrong producing an embedding. Opaque here: the host's
/// service, the host's error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbedderError(pub String);

impl std::fmt::Display for EmbedderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the embedder failed: {}", self.0)
    }
}

impl std::error::Error for EmbedderError {}

/// A text embedding model, supplied by the host.
///
/// The counterpart of `rm_extract::Completer`, and a port for the same reason:
/// no crate in this workspace touches the network, so the one thing that needs
/// a remote service asks for it rather than reaching for it. A test
/// implementation is a few lines, which is what keeps the whole pipeline
/// testable offline.
pub trait Embedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedderError>;
}

/// One edge a closure ended.
///
/// A named struct rather than a tuple: `(StableId, String, StableId, String)`
/// has two same-typed ids and two same-typed strings, so every reader would
/// have to go and check which is which.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Closed {
    pub subject: StableId,
    pub predicate: String,
    pub object: StableId,
    /// The reason the model gave, from the closure that ended this edge.
    pub because: String,
}

/// What one turn did to the store.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ingested {
    /// Local mention index to the entity it resolved to. Same order and length
    /// as the extraction's `mentions`.
    pub entities: Vec<StableId>,
    pub assertions: Vec<AssertionId>,
    /// Open questions raised while resolving the mentions. A mention that
    /// scored in the review band created its own entity and filed a question
    /// rather than merging, exactly as `remember` does on its own.
    pub reviews: Vec<ReviewId>,
    /// Edges closed by inference, with the reason the model gave.
    pub closed: Vec<Closed>,
}

impl Engine {
    /// Apply an extracted turn.
    ///
    /// Every embedding is produced and validated before anything is written, so
    /// a failing embedder costs nothing. That is the guarantee `Engine::remember`
    /// already makes and for the same reason: a fact in the store with no vector
    /// to find it is undetectable from outside — no query reports it and no
    /// error names it.
    pub fn ingest(
        &mut self,
        extraction: &Extraction,
        embedder: &impl Embedder,
    ) -> Result<Ingested, EngineError> {
        // Every vector first, and every vector checked, before the first write.
        let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(extraction.mentions.len());
        for mention in &extraction.mentions {
            let v = embedder.embed(&mention.text)?;
            self.index.check(&v)?;
            vectors.push(v);
        }

        let mut out = Ingested::default();

        for (mention, embedding) in extraction.mentions.iter().zip(vectors) {
            // The kind is asserted as an attribute so a mention with no facts
            // still becomes an entity. Not a workaround: the kind is a genuine
            // fact about the thing, it gives the entity something to be
            // recalled by, and it means no entity can exist without an
            // assertion -- which is what keeps `Engine::open`'s
            // every-assertion-has-a-vector rule free of exceptions.
            let remembered = self.remember(Observation {
                kind: mention.kind.clone(),
                mention: Record::new().with("name", mention.name.clone()),
                attribute: "kind".to_string(),
                value: Some(mention.kind.clone()),
                valid: Interval::since(0),
                provenance: Provenance::new(Source::ToolOutput, 0, "extraction"),
                embedding,
            })?;
            record(&mut out, remembered);
        }

        Ok(out)
    }
}

/// Fold one `remember` result into the running record.
fn record(out: &mut Ingested, remembered: Remembered) {
    match remembered {
        Remembered::Merged { entity, assertion } | Remembered::Created { entity, assertion } => {
            out.entities.push(entity);
            out.assertions.push(assertion);
        }
        Remembered::CreatedPendingReview {
            entity,
            assertion,
            review,
        } => {
            out.entities.push(entity);
            out.assertions.push(assertion);
            out.reviews.extend(review);
        }
    }
}
```

**Note on the provenance and validity above:** both are placeholders that Task 4 replaces, because an `Extraction` does not carry the turn that produced it. Task 4 changes `ingest`'s signature to take the `Turn` as well. It is split this way so this task's deliverable — mentions becoming entities, with a failing embedder costing nothing — can be reviewed on its own; do not ship the placeholder past Task 4.

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -p rm-engine 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 6: Verify and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run
typos .
git add Cargo.toml crates/rm-engine
git commit -m "feat(rm-engine): the Embedder port, and mentions becoming entities

Embedder is the counterpart of rm_extract::Completer and a port for the same
reason: no crate here touches the network, so the thing that needs a remote
service asks for it rather than reaching for it.

Every embedding is produced and checked before the first write, so a failing
embedder costs nothing -- the guarantee remember() already makes, because a
fact with no vector to find it is undetectable from outside.

A mention is recorded with its kind as an attribute, so one named only as an
edge target still becomes an entity. The kind is a real fact about the thing,
and it means no entity exists without an assertion."
```

---

### Task 4: `ingest` — facts and relations, and the turn that produced them

**Files:**
- Modify: `crates/rm-engine/src/ingest.rs`
- Test: `crates/rm-engine/src/lib.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: Task 3.
- Produces: `Engine::ingest(&mut self, turn: &Turn, extraction: &Extraction, embedder: &impl Embedder) -> Result<Ingested, EngineError>` — signature now takes the `Turn`.

- [ ] **Step 1: Write the failing tests**

```rust
    fn a_turn() -> rm_extract::Turn {
        rm_extract::Turn {
            text: "Ben works at Globex in Bristol".to_string(),
            speaker: Some("Ben Severn".to_string()),
            observed_at: 100,
            session: "session-1".to_string(),
        }
    }

    #[test]
    fn a_fact_lands_on_the_entity_its_mention_resolved_to() {
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn")],
            facts: vec![rm_extract::Fact {
                subject: 0,
                attribute: "employer".to_string(),
                value: Some("Globex".to_string()),
                text: "Ben works at Globex".to_string(),
                valid_from: 100,
            }],
            ..Default::default()
        };

        let out = e.ingest(&a_turn(), &extraction, &Buckets).unwrap();
        assert_eq!(
            e.about(out.entities[0], "employer", 100, 100).unwrap(),
            Believed::Value("Globex".into())
        );
    }

    #[test]
    fn a_fact_is_embedded_by_its_own_text_not_its_subject_s() {
        // "Where does he work" has to be able to reach the assertion without
        // first reaching Ben. Sharing the mention's embedding would make the
        // fact unreachable except through its subject.
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn")],
            facts: vec![rm_extract::Fact {
                subject: 0,
                attribute: "employer".to_string(),
                value: Some("Globex".to_string()),
                text: "Ben works at Globex".to_string(),
                valid_from: 100,
            }],
            ..Default::default()
        };
        e.ingest(&a_turn(), &extraction, &Buckets).unwrap();

        let by_fact = Buckets.embed("Ben works at Globex").unwrap();
        let hits = e.recall(&Query::new(by_fact, 1)).unwrap();
        assert_eq!(
            hits[0].value.as_deref(),
            Some("Globex"),
            "the nearest thing to the fact's own text must be the fact"
        );
    }

    #[test]
    fn a_relation_lands_between_the_entities_its_mentions_resolved_to() {
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn"), mention("organisation", "Globex")],
            relations: vec![rm_extract::Relation {
                subject: 0,
                predicate: "employed_by".to_string(),
                object: 1,
                valid_from: 100,
            }],
            ..Default::default()
        };

        let out = e.ingest(&a_turn(), &extraction, &Buckets).unwrap();
        let walk = Walk::new(vec![out.entities[0]], 1, 10, 100, 200);
        let reached = e.neighborhood(&walk);
        assert!(
            reached.reached.iter().any(|r| r.entity == out.entities[1]),
            "the walk should reach Globex from Ben"
        );
    }

    #[test]
    fn everything_a_turn_produced_carries_that_turn_s_session_and_moment() {
        // Provenance is what lets a later reader ask where a memory came from,
        // and an extraction that stamped its own writes with a placeholder
        // would make every one of them untraceable.
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn")],
            facts: vec![rm_extract::Fact {
                subject: 0,
                attribute: "employer".to_string(),
                value: Some("Globex".to_string()),
                text: "Ben works at Globex".to_string(),
                valid_from: 100,
            }],
            ..Default::default()
        };
        let out = e.ingest(&a_turn(), &extraction, &Buckets).unwrap();

        let history = e.store_history(out.entities[0], "employer");
        assert_eq!(history[0].provenance.source_ref, "session-1");
        assert_eq!(history[0].provenance.observed_at, 100);
        assert_eq!(history[0].provenance.source, Source::ToolOutput);
    }

    #[test]
    fn a_fact_keeps_the_valid_time_the_extraction_gave_it() {
        // "I joined sixty days ago", said now, is valid from sixty days ago --
        // not from the moment the turn was heard.
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn")],
            facts: vec![rm_extract::Fact {
                subject: 0,
                attribute: "employer".to_string(),
                value: Some("Globex".to_string()),
                text: "Ben joined Globex".to_string(),
                valid_from: 40,
            }],
            ..Default::default()
        };
        let out = e.ingest(&a_turn(), &extraction, &Buckets).unwrap();
        assert_eq!(
            e.about(out.entities[0], "employer", 50, 200).unwrap(),
            Believed::Value("Globex".into()),
            "it was already true at 50, before the turn was heard at 100"
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rm-engine a_fact_lands_on 2>&1 | head -20`
Expected: FAIL — `ingest` takes two arguments, not three.

- [ ] **Step 3: Take the turn, and apply facts and relations**

In `crates/rm-engine/src/ingest.rs`, change the signature and replace the placeholder provenance:

```rust
    pub fn ingest(
        &mut self,
        turn: &Turn,
        extraction: &Extraction,
        embedder: &impl Embedder,
    ) -> Result<Ingested, EngineError> {
```

Add near the top of the body, after the vectors are built:

```rust
        // Every write this turn produces carries the turn's own provenance.
        // `ToolOutput` because an extraction is what a tool returned, not what
        // the user said in so many words -- the user said the sentence, and
        // this is a model's reading of it.
        let prov = Provenance::new(Source::ToolOutput, turn.observed_at, turn.session.clone());
```

Replace the mention loop's `Observation` fields `valid` and `provenance` with `Interval::since(turn.observed_at)` and `prov.clone()`, then add after that loop:

```rust
        // Facts, each embedded by its own text. A fact and its subject are
        // different search targets: sharing an embedding would make "where does
        // he work" reachable only by first reaching Ben.
        for fact in &extraction.facts {
            let mention = &extraction.mentions[fact.subject];
            let embedding = embedder.embed(&fact.text)?;
            self.index.check(&embedding)?;
            let remembered = self.remember(Observation {
                kind: mention.kind.clone(),
                mention: Record::new().with("name", mention.name.clone()),
                attribute: fact.attribute.clone(),
                value: fact.value.clone(),
                valid: Interval::since(fact.valid_from),
                provenance: prov.clone(),
                embedding,
            })?;
            // A fact resolves to the same entity its mention already did, so
            // only the assertion and any review are new.
            match remembered {
                Remembered::Merged { assertion, .. } | Remembered::Created { assertion, .. } => {
                    out.assertions.push(assertion);
                }
                Remembered::CreatedPendingReview {
                    assertion, review, ..
                } => {
                    out.assertions.push(assertion);
                    out.reviews.extend(review);
                }
            }
        }

        for relation in &extraction.relations {
            self.relate(
                out.entities[relation.subject],
                relation.predicate.clone(),
                out.entities[relation.object],
                Interval::since(relation.valid_from),
                prov.clone(),
            )?;
        }
```

`extraction.mentions[fact.subject]` and `out.entities[relation.subject]` cannot panic: `rm_extract::extract` refuses any index outside `mentions`, and `out.entities` has one entry per mention. Say so in a comment where each indexing happens, naming the check that guarantees it.

**Note on embedding facts inside the loop:** the mentions' vectors are all produced up front, but a fact's vector is produced as its turn comes. That weakens the all-or-nothing guarantee to *mentions* only. Fix it by producing every fact vector in the same up-front pass as the mentions' — collect them into a second `Vec` before the first write, and zip in the loop. Do that rather than leaving the guarantee half-kept, and make sure `a_failing_embedder_leaves_the_store_and_the_index_untouched` also covers a failure on a *fact*'s text rather than a mention's.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p rm-engine 2>&1 | tail -10`
Expected: PASS, including the Task 3 tests updated for the new signature.

- [ ] **Step 5: Verify and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run
typos .
git add crates/rm-engine
git commit -m "feat(rm-engine): ingest applies facts and relations

ingest takes the Turn, so every write carries that turn's session and moment
rather than a placeholder -- provenance is what lets a later reader ask where a
memory came from.

Source::ToolOutput, not UserAssertion: the user said the sentence, and an
extraction is a model's reading of it. Facts are embedded by their own text,
because a fact and its subject are different search targets."
```

---

### Task 5: `ingest` — closures

**Files:**
- Modify: `crates/rm-engine/src/ingest.rs`
- Test: `crates/rm-engine/src/lib.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: Task 4.
- Produces: `Ingested::closed` populated.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn a_closure_ends_the_prior_edge_and_records_that_an_agent_inferred_it() {
        // "I started at Globex" does not say Ben left Acme. Closing that edge
        // is an inference, and it is recorded as one -- traceable in
        // edge_history, and outrankable by anything the user says directly.
        let mut e = engine();
        let first = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn"), mention("organisation", "Acme")],
            relations: vec![rm_extract::Relation {
                subject: 0,
                predicate: "employed_by".to_string(),
                object: 1,
                valid_from: 10,
            }],
            ..Default::default()
        };
        let first_out = e.ingest(&a_turn(), &first, &Buckets).unwrap();
        let (ben, acme) = (first_out.entities[0], first_out.entities[1]);

        let second = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn"), mention("organisation", "Globex")],
            relations: vec![rm_extract::Relation {
                subject: 0,
                predicate: "employed_by".to_string(),
                object: 1,
                valid_from: 100,
            }],
            closures: vec![rm_extract::Closure {
                subject: 0,
                predicate: "employed_by".to_string(),
                at: 100,
                because: "starting a new job ends the previous one".to_string(),
            }],
            ..Default::default()
        };
        let out = e.ingest(&a_turn(), &second, &Buckets).unwrap();

        assert_eq!(out.closed.len(), 1);
        assert_eq!(out.closed[0].object, acme);
        assert_eq!(out.closed[0].because, "starting a new job ends the previous one");

        // The walk no longer crosses to Acme, but does reach Globex.
        let now = Walk::new(vec![ben], 1, 10, 150, 300);
        let reached: Vec<_> = e.neighborhood(&now).reached.iter().map(|r| r.entity).collect();
        assert!(!reached.contains(&acme), "Acme should be behind us");
        assert!(reached.contains(&out.entities[1]), "Globex should not be");

        // And the tombstone says who concluded it.
        let history = e.edge_history(ben, "employed_by", acme);
        assert_eq!(history.last().unwrap().provenance.source, Source::AgentInference);
        assert!(!history.last().unwrap().present);
    }

    #[test]
    fn a_closure_does_not_end_an_edge_asserted_in_the_same_turn() {
        // Otherwise "I moved from Acme to Globex" would close Globex as fast as
        // it opened it.
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn"), mention("organisation", "Globex")],
            relations: vec![rm_extract::Relation {
                subject: 0,
                predicate: "employed_by".to_string(),
                object: 1,
                valid_from: 100,
            }],
            closures: vec![rm_extract::Closure {
                subject: 0,
                predicate: "employed_by".to_string(),
                at: 100,
                because: "a new job ends the old one".to_string(),
            }],
            ..Default::default()
        };
        let out = e.ingest(&a_turn(), &extraction, &Buckets).unwrap();

        assert!(out.closed.is_empty(), "there was nothing prior to close");
        let walk = Walk::new(vec![out.entities[0]], 1, 10, 150, 300);
        assert!(e
            .neighborhood(&walk)
            .reached
            .iter()
            .any(|r| r.entity == out.entities[1]));
    }

    #[test]
    fn a_closure_leaves_other_predicates_alone() {
        let mut e = engine();
        let first = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn"), mention("place", "Bristol")],
            relations: vec![rm_extract::Relation {
                subject: 0,
                predicate: "lives_in".to_string(),
                object: 1,
                valid_from: 10,
            }],
            ..Default::default()
        };
        let first_out = e.ingest(&a_turn(), &first, &Buckets).unwrap();
        let bristol = first_out.entities[1];

        let second = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn")],
            closures: vec![rm_extract::Closure {
                subject: 0,
                predicate: "employed_by".to_string(),
                at: 100,
                because: "a new job".to_string(),
            }],
            ..Default::default()
        };
        let out = e.ingest(&a_turn(), &second, &Buckets).unwrap();

        assert!(out.closed.is_empty());
        let walk = Walk::new(vec![out.entities[0]], 1, 10, 150, 300);
        assert!(
            e.neighborhood(&walk).reached.iter().any(|r| r.entity == bristol),
            "where he lives has nothing to do with where he works"
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rm-engine a_closure_ends 2>&1 | head -20`
Expected: FAIL — `out.closed` is empty; nothing resolves closures.

- [ ] **Step 3: Resolve the closures**

Add at the end of `ingest`, after the relations loop and before `Ok(out)`:

```rust
        // Closures last, so an edge this same turn asserted is already in place
        // and can be excluded. "I moved from Acme to Globex" must not close
        // Globex as fast as it opened it.
        for closure in &extraction.closures {
            let subject = out.entities[closure.subject];

            // Edges this extraction asserted for the same subject and
            // predicate. Anything else in force is what the closure ends.
            let spared: Vec<StableId> = extraction
                .relations
                .iter()
                .filter(|r| r.subject == closure.subject && r.predicate == closure.predicate)
                .map(|r| out.entities[r.object])
                .collect();

            // `Timestamp::MAX` on the transaction axis: the question is which
            // edges the store holds *now*, not what it believed earlier. The
            // engine has no clock, and reusing `closure.at` -- a valid-time
            // value -- would hide any edge learned after the moment the closure
            // speaks about, leaving it open forever.
            let doomed: Vec<(String, StableId)> = self
                .edges_from(subject, closure.at, Timestamp::MAX)
                .into_iter()
                .filter(|e| e.predicate == closure.predicate && !spared.contains(&e.object))
                .map(|e| (e.predicate.to_string(), e.object))
                .collect();

            // `AgentInference`, which `rm_core` documents as the weakest
            // source: "inferences are derived from the others and re-deriving
            // one does not make it more true". That is what makes this safe to
            // do at all -- the closure is traceable in `edge_history`, it can
            // never be mistaken for something the user said, and
            // `Strategy::SourcePriority` can rank it below a user assertion so
            // a later correction wins with no special handling.
            let inferred = Provenance::new(
                Source::AgentInference,
                turn.observed_at,
                turn.session.clone(),
            );

            for (predicate, object) in doomed {
                self.unrelate(subject, &predicate, object, closure.at, inferred.clone())?;
                out.closed.push(Closed {
                    subject,
                    predicate,
                    object,
                    because: closure.because.clone(),
                });
            }
        }
```

Add `rm_core::Timestamp` to the module's imports.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p rm-engine 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Verify and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run
typos .
git add crates/rm-engine/src/ingest.rs
git commit -m "feat(rm-engine): a closure ends the edges it means to

The store refuses to close an edge when a new one arrives, because arrival does
not entail departure. This is the layer that decides, and it records the
decision as Source::AgentInference -- the weakest source rm-core names, so the
inference is traceable in edge_history, never mistakable for testimony, and
outrankable by SourcePriority when the user says otherwise.

Closures run last so an edge this same turn asserted is already in place and
excluded: 'I moved from Acme to Globex' must not close Globex as fast as it
opened it."
```

---

### Task 6: The story end to end, and the docs

**Files:**
- Create: `crates/rm-engine/tests/extract.rs`
- Modify: `README.md`

**Interfaces:**
- Consumes: all previous tasks.
- Produces: nothing new; proves the pipeline works through the public API only.

- [ ] **Step 1: Write the end-to-end test**

`crates/rm-engine/tests/extract.rs`:

```rust
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
    let ended = engine.edge_history(ben, "employed_by", acme).last().unwrap().clone();
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
```

`rm_engine::extract` must be re-exported for this to compile — add `pub use rm_extract::extract;` beside the other `rm_extract` re-exports if Task 3 did not already.

- [ ] **Step 2: Run to verify it passes**

Run: `cargo test -p rm-engine --test extract 2>&1 | tail -10`
Expected: PASS. If the two turns produce two different Bens, the ruleset's `match_at` is above the score two identical names produce — check it against `log2(0.9/0.01) ≈ 6.49` and lower it, without touching `test_ruleset()` in `src/lib.rs`.

- [ ] **Step 3: Update `README.md`**

Change the `rm-extract` row from `planned` to `in progress`.

- [ ] **Step 4: Run the whole verification suite**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run
cargo test --doc
cargo doc --no-deps --all-features
typos .
```

Expected: all clean, including no rustdoc warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-engine/tests/extract.rs crates/rm-engine/src/lib.rs README.md
git commit -m "test(rm-engine): a conversation, remembered

Two turns months apart: one person recognised across both, both employers kept
as facts with disjoint validity, and a departure nobody stated inferred and
recorded as an inference. A walk in August reaches Globex and not Acme.

One use, from one crate -- if this needed a second crate in the manifest, so
would everyone feeding a turn to a memory store."
```

---

## Notes for the implementer

**On the placeholder in Task 3.** It stamps every write with `observed_at: 0` and `"extraction"` as the session, because an `Extraction` does not carry the turn that produced it. Task 4 changes the signature to take the `Turn` and replaces it. The split exists so mentions-become-entities can be reviewed on its own; it is not a shape to keep.

**On the all-or-nothing guarantee.** Task 3 embeds every mention before the first write, which is the property `remember` already promises. Task 4 adds facts, and the obvious way to write it — embedding each fact inside the apply loop — quietly weakens that guarantee to mentions only. Produce every vector, mentions and facts alike, in one pass before anything is written.

**On what `extract` refuses, and what `ingest` therefore need not check.** `extract` rejects any index outside `mentions`, so `ingest` may index `extraction.mentions` and `out.entities` directly. Put a comment at each such site naming the check that makes it safe — a future change to `extract` that relaxed a rule would otherwise turn these into panics with nothing to point at.

**On what is *not* in this plan.** Retries, batching, streaming, conversation history beyond one turn, a date parser, and any provider-specific code. Each is recorded as out of scope in the spec.
