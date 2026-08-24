//! The tools this server offers, and reading the arguments of a call.
//!
//! # Five, and the same five `rmem` has
//!
//! `rm_host::command` moves across from the CLI untouched, so a bug in this
//! crate is a bug in the protocol and cannot be a bug in what remembering
//! means. The engine surface neither binary reaches yet — `forget`, `erase`,
//! `neighborhood`, `store_history`, and `Query`'s `entity` / `session` /
//! `source` / `as_of` filters — stays out for the same reason: adding it here
//! would mean designing a second, wider API inside the change that introduces
//! the protocol.
//!
//! `about` is the one place this server offers more than the command line, and
//! it is not new surface: `command::about` already takes both time axes and
//! `rmem` simply passes `now` twice. Handing them to an agent is what makes
//! this project's central claim reachable by one.
//!
//! # Schemas are written by hand, so each one is called by hand
//!
//! There is no derive here and no schema generator, which is the same trade
//! this workspace made for its argument parser. The cost is that
//! [`definitions`] and [`Call::read`] can drift, so every tool has a test that
//! calls it with exactly what its own schema advertises, and one test walks the
//! whole table to check that what is `required` there is what is refused here.

use serde_json::{json, Value};

use rm_engine::{ReviewId, StableId, Timestamp};

/// The tool table, in a fixed order.
///
/// Fixed because the specification asks for it: a deterministic `tools/list`
/// lets a client cache the list, and keeps the tool block stable in a model's
/// prompt from one call to the next.
/// The environment variable naming which tools to expose.
///
/// A comma-separated list of names; unset means all of them.
pub const TOOLS_ENV: &str = "RMEM_TOOLS";

/// The tools this server offers, honouring [`TOOLS_ENV`].
///
/// # Why a session should be able to ask for fewer
///
/// The tool table is sent on every turn of every session that has this server
/// configured, whether or not it is used. Measured: eight tools are about 1,700
/// tokens, which is roughly what a thirty-decision log costs to read in full --
/// so a project that only ever consults decisions pays a log's worth of context
/// per turn to advertise five tools it will never call.
///
/// Names that match nothing are ignored rather than refused. This is read at
/// startup on a path with no good way to report, and a server that will not
/// start because a list has a typo in it is worse than one that offers fewer
/// tools than expected -- which `tools/list` shows plainly.
pub fn definitions() -> Vec<Value> {
    let all = all_definitions();
    let Ok(wanted) = std::env::var(TOOLS_ENV) else {
        return all;
    };
    let wanted: Vec<&str> = wanted
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if wanted.is_empty() {
        return all;
    }
    all.into_iter()
        .filter(|t| wanted.contains(&t["name"].as_str().unwrap_or_default()))
        .collect()
}

fn all_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "remember",
            "title": "Remember a turn",
            "description": "Extract entities, facts and relationships from a turn of dialogue and append them to memory. Nothing is overwritten: a fact that contradicts an earlier one is stored beside it with its own validity, and which one is believed is decided when you ask, not now.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "What was said, in full. One turn."
                    },
                    "speaker": {
                        "type": "string",
                        "description": "Who said it. Omit only when the turn genuinely has none -- a log line, a document. Without it a first-person turn names nobody and most of its facts are lost."
                    },
                    "session": {
                        "type": "string",
                        "description": "A name for the conversation this turn belongs to."
                    }
                },
                "required": ["text"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "recall",
            "title": "Recall assertions",
            "description": "Search memory for the assertions nearest a query, by embedding distance. Returns the entity behind every hit, so what comes back can be asked about. Every hit carries a \"standing\": \"latest\" (nothing later under that attribute), \"joined\" (later assertions exist and each said it was one more of the same thing, so this is still true), \"corrected\" (a later assertion said it replaces this) or \"unsettled\" (something later exists and nobody said whether it replaces this). Only \"corrected\" means stale; \"still_stands\" is the same judgement as a boolean. A corrected hit is returned rather than hidden, because what was believed is part of the record.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What to search for, in words."
                    },
                    "k": {
                        "type": "integer",
                        "description": "How many hits to return. Defaults to 5.",
                        "minimum": 1
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "about",
            "title": "What memory believes",
            "description": "What memory believes an entity's attribute held. Answers one of three ways, and the difference matters: a value; absent, meaning someone said there is none; or unknown, meaning it has never been discussed. Both time axes can be moved independently -- valid_at asks what was true then, as_of asks what was known then.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity": {
                        "type": "integer",
                        "description": "The entity id, as returned by remember or recall.",
                        "minimum": 0
                    },
                    "attribute": {
                        "type": "string",
                        "description": "Which attribute, for example \"employer\"."
                    },
                    "valid_at": {
                        "type": "integer",
                        "description": "Epoch milliseconds. What was true then. Defaults to now."
                    },
                    "as_of": {
                        "type": "integer",
                        "description": "Epoch milliseconds. What was known then. Defaults to now."
                    }
                },
                "required": ["entity", "attribute"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "reviews",
            "title": "Open questions",
            "description": "The pairs of entities that scored too close to call. Nothing here has been merged: a match in the middle band creates a separate entity and files the pair, because fusing two people on a score that could not be called corrupts memory permanently and silently.",
            "inputSchema": {"type": "object", "additionalProperties": false}
        }),
        json!({
            "name": "decide",
            "title": "Record a decision",
            "description": "Record a decision so it can be found and cited later. Use this for choices with reasons behind them -- an approach taken, a library picked, a convention agreed -- not for ordinary facts, which belong in remember. Unlike remember this never guesses a shape: the title, choice, reason and context are stored under those exact names, so a decision stays findable and a later decision can retire it by title. Give a title you would search for. Record options you considered and turned down too, with status rejected and the reason in because -- a rejected option with its reason is what stops the same question being reopened later. If this replaces an earlier decision, name it in supersedes and the old one is marked retired.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "How this decision is found again. Short and specific, like \"Store snapshots as one file\"."
                    },
                    "choice": {
                        "type": "string",
                        "description": "What was decided."
                    },
                    "status": {
                        "type": "string",
                        "enum": ["proposed", "accepted", "rejected", "deprecated"],
                        "description": "Where this decision stands. Defaults to accepted. Record an option you turned down as rejected, with the reason in because."
                    },
                    "because": {
                        "type": "string",
                        "description": "Why, including what it was chosen over, and for a rejected option the reason it lost."
                    },
                    "context": {
                        "type": "string",
                        "description": "What prompted the decision -- the problem or constraint in play at the time."
                    },
                    "supersedes": {
                        "type": "string",
                        "description": "The exact title of a decision this replaces."
                    },
                    "decided_at": {
                        "type": "string",
                        "description": "The day it was made, as YYYY-MM-DD, if that is not today. Sets when it held from, not when the store heard it."
                    }
                },
                "required": ["title", "choice"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "decisions",
            "title": "Decisions on record",
            "description": "Every decision recorded, newest first, with whether it still stands. A decision that was re-decided under the same title, or superseded by a later one, is marked as replaced. Read this before proposing an approach that may already have been settled -- and pass status=rejected to see what was tried and turned down, which is what stops a settled question being reopened.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["proposed", "accepted", "rejected", "deprecated", "superseded"],
                        "description": "Show only decisions with this status. Omit for all of them."
                    }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "decision",
            "title": "One decision, and what replaced it",
            "description": "Read one decision in full by its exact title: what was chosen, why, what it replaced, and — when it has been superseded — the chain forward to the decision that stands now. Use this when `decisions` or `recall` surfaces a decision you are about to rely on, because a decision marked replaced tells you the answer is out of date and this is what tells you the current one.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "The decision's exact title, as `decisions` lists it. Titles are matched exactly, not approximately.",
                        "minLength": 1
                    }
                },
                "required": ["title"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "resolve_review",
            "title": "Answer an open question",
            "description": "Answer one open question. same=true merges the pair; same=false records that they are different and stops the pair being asked about again.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "integer",
                        "description": "The review id, as listed by reviews.",
                        "minimum": 0
                    },
                    "same": {
                        "type": "boolean",
                        "description": "Whether the two entities are the same thing."
                    }
                },
                "required": ["id", "same"],
                "additionalProperties": false
            }
        }),
    ]
}

/// A `tools/call` this server understood well enough to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    Remember {
        text: String,
        session: String,
        /// Who said it. `None` when the caller did not say, which
        /// `rm_extract`'s prompt states explicitly rather than leaving blank.
        speaker: Option<String>,
    },
    Recall {
        query: String,
        k: usize,
    },
    About {
        entity: StableId,
        attribute: String,
        valid_at: Timestamp,
        as_of: Timestamp,
    },
    Reviews,
    ResolveReview {
        id: ReviewId,
        same: bool,
    },
    Decide {
        title: String,
        choice: String,
        status: Option<String>,
        decided_at: Option<rm_engine::Timestamp>,
        because: Option<String>,
        context: Option<String>,
        supersedes: Option<String>,
        session: String,
    },
    Decisions {
        status: Option<String>,
    },
    /// Read one decision in full, by exact title.
    Decision {
        title: String,
    },
}

/// Why a call could not be read.
///
/// A `String` and not a variant per shape, because every one of these goes to
/// the same place: a tool result with `isError: true`, whose text a model reads
/// and corrects itself from. The specification is explicit that input
/// validation belongs there rather than in a JSON-RPC error, so the words are
/// the whole payload.
pub type Unreadable = String;

/// The one thing that is *not* a tool execution error.
///
/// An unknown tool is a protocol error by name in the specification, alongside
/// malformed requests, and the reasoning is stated there: these are "issues
/// with the request structure itself that models are less likely to be able to
/// fix". A model cannot invent a tool this server does not have.
pub fn is_known(name: &str) -> bool {
    definitions()
        .iter()
        .any(|t| t["name"].as_str() == Some(name))
}

impl Call {
    /// Read a call's arguments.
    ///
    /// `now` is passed in for the same reason the engine takes no clock: a
    /// caller that cannot control the time cannot test anything depending on
    /// it, and here it is the default for both of `about`'s axes.
    /// Who to record a write against.
    ///
    /// The client names itself once, in the handshake, and that is the half
    /// worth trusting: a `session` argument is chosen per call, and an agent
    /// that forgets it leaves the write anonymous. In a store one agent uses
    /// that is a cosmetic gap; in one several share it is the difference
    /// between a log and a pile.
    ///
    /// So the client's own name is always present, and a session the caller
    /// supplies is appended to it rather than replacing it. Neither can hide
    /// the other.
    fn attributed(arguments: &Value, client: Option<&str>) -> Result<String, Unreadable> {
        let session = optional_string(arguments, "session")?;
        Ok(match (client, session) {
            (Some(c), Some(s)) => format!("{c}/{s}"),
            (Some(c), None) => c.to_string(),
            // No handshake identity: a client that did not name itself, which
            // the specification allows. The old default, so nothing that worked
            // before now records less.
            (None, Some(s)) => s,
            (None, None) => "mcp".to_string(),
        })
    }

    pub fn read(
        name: &str,
        arguments: &Value,
        now: Timestamp,
        client: Option<&str>,
    ) -> Result<Call, Unreadable> {
        match name {
            "remember" => Ok(Call::Remember {
                text: string(arguments, "text")?,
                // The CLI passes "cli" here, so the default names this server
                // rather than inheriting a label that would be wrong.
                session: Call::attributed(arguments, client)?,
                // Optional, and deliberately not defaulted to anything. A
                // guessed speaker is worse than none: the prompt resolves "I"
                // to whoever is named, so a wrong name attributes the turn to
                // the wrong person rather than leaving it unattributed.
                speaker: optional_string(arguments, "speaker")?,
            }),
            "recall" => Ok(Call::Recall {
                query: string(arguments, "query")?,
                k: match optional_integer(arguments, "k")? {
                    None => 5,
                    // Refused rather than clamped to 1. A caller asking for
                    // zero hits has a bug, and answering "here are no results"
                    // would look like an empty store.
                    Some(k) if k < 1 => {
                        return Err("k must be at least 1".to_string());
                    }
                    Some(k) => k as usize,
                },
            }),
            "about" => Ok(Call::About {
                entity: non_negative(arguments, "entity")? as StableId,
                attribute: string(arguments, "attribute")?,
                valid_at: optional_integer(arguments, "valid_at")?.unwrap_or(now),
                as_of: optional_integer(arguments, "as_of")?.unwrap_or(now),
            }),
            "reviews" => Ok(Call::Reviews),
            "decisions" => Ok(Call::Decisions {
                status: optional_string(arguments, "status")?,
            }),
            "decision" => Ok(Call::Decision {
                title: string(arguments, "title")?,
            }),
            "decide" => Ok(Call::Decide {
                title: string(arguments, "title")?,
                choice: string(arguments, "choice")?,
                status: optional_string(arguments, "status")?,
                decided_at: optional_string(arguments, "decided_at")?
                    .map(|d| rm_host::time::parse_day(&d))
                    .transpose()?,
                because: optional_string(arguments, "because")?,
                context: optional_string(arguments, "context")?,
                supersedes: optional_string(arguments, "supersedes")?,
                session: Call::attributed(arguments, client)?,
            }),
            "resolve_review" => Ok(Call::ResolveReview {
                id: non_negative(arguments, "id")? as ReviewId,
                same: boolean(arguments, "same")?,
            }),
            // `is_known` guards this, and it is checked before `read` is
            // reached so that an unknown tool becomes a protocol error rather
            // than an `isError` result.
            other => Err(format!("no tool named {other}")),
        }
    }

    /// Whether running this call can change the store.
    ///
    /// What decides whether the snapshot is written back. A read that rewrote
    /// the file would turn every `about` into a rewrite of something it had no
    /// business touching.
    pub fn mutates(&self) -> bool {
        match self {
            Call::Remember { .. } | Call::ResolveReview { .. } | Call::Decide { .. } => true,
            Call::Recall { .. }
            | Call::About { .. }
            | Call::Reviews
            | Call::Decisions { .. }
            | Call::Decision { .. } => false,
        }
    }
}

/// A required string.
fn string(args: &Value, field: &str) -> Result<String, Unreadable> {
    match args.get(field) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        // Named separately from "missing": a caller that sent the field and
        // sent it empty has a different bug from one that forgot it, and
        // "required" would send them looking in the wrong place.
        Some(Value::String(_)) => Err(format!("{field} was empty")),
        Some(_) => Err(format!("{field} must be a string")),
        None => Err(format!("{field} is required")),
    }
}

fn optional_string(args: &Value, field: &str) -> Result<Option<String>, Unreadable> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
        Some(Value::String(_)) => Err(format!("{field} was empty")),
        Some(_) => Err(format!("{field} must be a string")),
    }
}

fn boolean(args: &Value, field: &str) -> Result<bool, Unreadable> {
    match args.get(field) {
        Some(Value::Bool(b)) => Ok(*b),
        // Refused rather than read as truthy. `same` decides whether two
        // entities are merged, which is the one operation here that cannot be
        // undone, and a caller that sent "yes" or 1 has not said which.
        Some(_) => Err(format!("{field} must be true or false")),
        None => Err(format!("{field} is required")),
    }
}

fn optional_integer(args: &Value, field: &str) -> Result<Option<i64>, Unreadable> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        // Split, because the two failures send a caller to different places.
        // A float is a caller who computed the value; a number past `i64` is a
        // caller who is not addressing anything in this store. Truncating
        // either would answer a question that was not asked.
        Some(Value::Number(n)) => match n.as_i64() {
            Some(i) => Ok(Some(i)),
            None if n.is_f64() => Err(format!("{field} must be a whole number")),
            None => Err(format!("{field} is larger than any id this store holds")),
        },
        Some(_) => Err(format!("{field} must be a number")),
    }
}

/// A required integer that cannot be negative.
///
/// Ids are `u64`, so every non-negative `i64` is one and the upper bound is
/// `optional_integer`'s. The lower bound is the one that matters: `-1 as u64`
/// is `u64::MAX` and wraps in silence, and an entity that came back from that
/// would be a real one the caller never asked about.
fn non_negative(args: &Value, field: &str) -> Result<i64, Unreadable> {
    match optional_integer(args, field)? {
        None => Err(format!("{field} is required")),
        Some(n) if n < 0 => Err(format!("{field} cannot be negative")),
        Some(n) => Ok(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Timestamp = 1_000;

    fn read(name: &str, args: Value) -> Result<Call, Unreadable> {
        Call::read(name, &args, NOW, None)
    }

    #[test]
    fn every_tool_can_be_called_with_exactly_what_its_own_schema_advertises() {
        // The whole reason hand-written schemas are affordable. A schema that
        // names a property `question` while `read` looks for `query` is
        // invisible until an agent tries it, and this is the test that sees
        // it: build the minimal argument object from each schema's own
        // `required` list and check the call reads.
        let examples: Vec<(&str, Value)> = vec![
            ("remember", json!({"text": "I moved to Globex"})),
            ("recall", json!({"query": "where do I work"})),
            ("about", json!({"entity": 0, "attribute": "employer"})),
            ("reviews", json!({})),
            (
                "decide",
                json!({"title": "Use one file", "choice": "One snapshot per store"}),
            ),
            ("decisions", json!({})),
            ("decision", json!({"title": "Use one file"})),
            ("resolve_review", json!({"id": 0, "same": true})),
        ];

        for tool in definitions() {
            let name = tool["name"].as_str().unwrap();
            let (_, args) = examples
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap_or_else(|| panic!("{name} is in the table with no example"));

            // Every property the example sets must exist in the schema...
            let properties = &tool["inputSchema"]["properties"];
            for key in args.as_object().unwrap().keys() {
                assert!(
                    properties.get(key).is_some(),
                    "{name}'s schema has no property {key}"
                );
            }
            // ...and every property the schema requires must be in the example.
            let required = tool["inputSchema"]["required"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for field in &required {
                let field = field.as_str().unwrap();
                assert!(
                    args.get(field).is_some(),
                    "{name} requires {field} and the example omits it"
                );
            }
            read(name, args.clone())
                .unwrap_or_else(|e| panic!("{name} could not read its own schema's example: {e}"));
        }
    }

    /// A write carries who made it, whether or not the caller remembered to say.
    ///
    /// In a store one agent uses this is cosmetic. In one several share it is
    /// the difference between a log and a pile, and asking each agent to pass a
    /// `session` argument is asking for the writes where it was forgotten.
    #[test]
    fn a_write_is_attributed_to_the_client_that_made_it() {
        let with =
            |client: Option<&str>, args: Value| match Call::read("decide", &args, NOW, client)
                .unwrap()
            {
                Call::Decide { session, .. } => session,
                other => panic!("{other:?}"),
            };
        let bare = json!({"title": "T", "choice": "C"});

        // The handshake name alone.
        assert_eq!(
            with(Some("claude-code 2.1"), bare.clone()),
            "claude-code 2.1"
        );

        // Both, and neither hides the other: an agent can name its conversation
        // without erasing which agent it was.
        assert_eq!(
            with(
                Some("claude-code 2.1"),
                json!({"title":"T","choice":"C","session":"refactor"})
            ),
            "claude-code 2.1/refactor"
        );

        // A client that named itself in the handshake and nothing else still
        // gets attribution, which is the case this exists for.
        assert_eq!(with(Some("agent-B"), bare.clone()), "agent-B");

        // No handshake identity: the specification allows it, and nothing that
        // worked before now records less.
        assert_eq!(with(None, bare), "mcp");
        assert_eq!(
            with(None, json!({"title":"T","choice":"C","session":"s"})),
            "s"
        );
    }

    #[test]
    fn every_required_field_is_refused_when_it_is_missing() {
        // The other direction of the same drift. A field listed as `required`
        // that `read` happily defaults is a schema making a promise the code
        // does not keep, and an agent finds out by getting a silent wrong
        // answer rather than an error.
        let full: Vec<(&str, Value)> = vec![
            ("remember", json!({"text": "x", "session": "s"})),
            ("recall", json!({"query": "x", "k": 3})),
            (
                "about",
                json!({"entity": 1, "attribute": "employer", "valid_at": 5, "as_of": 6}),
            ),
            ("resolve_review", json!({"id": 2, "same": false})),
        ];

        for tool in definitions() {
            let name = tool["name"].as_str().unwrap();
            let Some((_, args)) = full.iter().find(|(n, _)| *n == name) else {
                continue; // `reviews` takes nothing.
            };
            for field in tool["inputSchema"]["required"].as_array().unwrap() {
                let field = field.as_str().unwrap();
                let mut without = args.clone();
                without.as_object_mut().unwrap().remove(field);
                let err = read(name, without)
                    .unwrap_err_or_panic(&format!("{name} accepted a call with no {field}"));
                assert!(
                    err.contains(field),
                    "{name}'s refusal has to name {field}: {err}"
                );
            }
        }
    }

    /// `Result::unwrap_err` with a message, since `Call` is not `Debug`-free.
    trait UnwrapErrOr {
        fn unwrap_err_or_panic(self, message: &str) -> Unreadable;
    }
    impl UnwrapErrOr for Result<Call, Unreadable> {
        fn unwrap_err_or_panic(self, message: &str) -> Unreadable {
            match self {
                Ok(_) => panic!("{message}"),
                Err(e) => e,
            }
        }
    }

    #[test]
    fn a_speaker_is_carried_when_the_caller_gives_one() {
        // The gap this closed: `remember` had no way to say who was speaking,
        // so every turn reached a prompt built for dialogue with the speaker
        // unknown. Measured on a real corpus, supplying it took responses
        // listing no mentions at all from 45% to 1%.
        assert_eq!(
            read(
                "remember",
                json!({"text": "I moved to Chicago", "speaker": "Melanie"})
            )
            .unwrap(),
            Call::Remember {
                text: "I moved to Chicago".into(),
                session: "mcp".into(),
                speaker: Some("Melanie".into()),
            }
        );
    }

    #[test]
    fn the_defaults_are_the_ones_the_descriptions_promise() {
        // Each of these is written into a tool description an agent reads, so
        // a disagreement is a lie told to the caller rather than an internal
        // detail.
        assert_eq!(
            read("remember", json!({"text": "x"})).unwrap(),
            Call::Remember {
                text: "x".into(),
                session: "mcp".into(),
                // No default, and none promised: the description says to omit
                // it only when the turn has no identified speaker. Guessing
                // one would be worse than leaving it out, because the prompt
                // resolves "I" to whoever is named.
                speaker: None,
            }
        );
        assert_eq!(
            read("recall", json!({"query": "x"})).unwrap(),
            Call::Recall {
                query: "x".into(),
                k: 5
            }
        );
        assert_eq!(
            read("about", json!({"entity": 2, "attribute": "employer"})).unwrap(),
            Call::About {
                entity: 2,
                attribute: "employer".into(),
                valid_at: NOW,
                as_of: NOW
            }
        );
    }

    #[test]
    fn the_two_time_axes_move_independently() {
        // The point of exposing them at all. "What did I believe last Tuesday
        // about what was true in May" is a different question from either
        // half, and a server that moved them together could not ask it.
        let call = read(
            "about",
            json!({"entity": 1, "attribute": "employer", "valid_at": 500, "as_of": 900}),
        )
        .unwrap();
        assert_eq!(
            call,
            Call::About {
                entity: 1,
                attribute: "employer".into(),
                valid_at: 500,
                as_of: 900
            }
        );
    }

    #[test]
    fn a_blank_string_is_refused_as_blank_rather_than_as_missing() {
        // A caller that sent the field and sent it empty has a different bug
        // from one that forgot it, and "required" sends them to the wrong
        // place. `remember` in particular: whitespace would reach the model,
        // cost a completion and come back as nothing.
        let err = read("remember", json!({"text": "   "})).unwrap_err();
        assert!(err.contains("empty"), "{err}");
        assert!(!err.contains("required"), "{err}");
    }

    #[test]
    fn same_must_be_a_boolean_and_not_something_boolean_shaped() {
        // `same` decides a merge, which is the one operation here that cannot
        // be undone. "yes", 1 and "true" have not said which way.
        for shape in [json!("yes"), json!(1), json!("true"), json!(null)] {
            let err = read("resolve_review", json!({"id": 0, "same": shape})).unwrap_err();
            assert!(err.contains("same"), "{err}");
        }
        assert!(read("resolve_review", json!({"id": 0, "same": false})).is_ok());
    }

    #[test]
    fn a_negative_id_is_refused_rather_than_wrapped() {
        // Ids are `u64`. `-1 as u64` is `u64::MAX` and wraps in silence, so
        // the entity that came back would be a real one the caller never asked
        // about -- and `about` answering confidently about the wrong entity is
        // the worst failure this server has.
        assert!(read("about", json!({"entity": -1, "attribute": "a"}))
            .unwrap_err()
            .contains("negative"));
        assert!(read("resolve_review", json!({"id": -1, "same": true}))
            .unwrap_err()
            .contains("negative"));
    }

    #[test]
    fn a_number_past_i64_is_refused_as_too_large_rather_than_as_fractional() {
        // The two failures send a caller to different places, and "must be a
        // whole number" about an integer would send them to the wrong one.
        let err = read(
            "about",
            json!({"entity": 18_446_744_073_709_551_615u64, "attribute": "a"}),
        )
        .unwrap_err();
        assert!(err.contains("larger"), "{err}");
    }

    #[test]
    fn a_fractional_number_is_refused_rather_than_truncated() {
        let err = read("recall", json!({"query": "x", "k": 2.5})).unwrap_err();
        assert!(err.contains("whole number"), "{err}");
    }

    #[test]
    fn zero_hits_is_refused_because_an_empty_answer_would_look_like_an_empty_store() {
        let err = read("recall", json!({"query": "x", "k": 0})).unwrap_err();
        assert!(err.contains("at least 1"), "{err}");
    }

    #[test]
    fn only_the_two_writing_tools_write() {
        // What decides whether the snapshot is written back.
        assert!(read("remember", json!({"text": "x"})).unwrap().mutates());
        assert!(read("resolve_review", json!({"id": 0, "same": true}))
            .unwrap()
            .mutates());
        assert!(!read("recall", json!({"query": "x"})).unwrap().mutates());
        assert!(!read("about", json!({"entity": 0, "attribute": "a"}))
            .unwrap()
            .mutates());
        assert!(!read("reviews", json!({})).unwrap().mutates());
        assert!(!read("decisions", json!({})).unwrap().mutates());
        assert!(!read("decision", json!({"title": "Use one file"}))
            .unwrap()
            .mutates());
        assert!(read("decide", json!({"title": "t", "choice": "c"}))
            .unwrap()
            .mutates());
    }

    #[test]
    fn the_table_is_eight_tools_in_a_fixed_order_with_legal_names() {
        // Deterministic ordering is what lets a client cache the list and
        // keeps the tool block stable in a model's prompt. The name rules are
        // the specification's: letters, digits, underscore, hyphen and dot.
        let names: Vec<String> = definitions()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "remember",
                "recall",
                "about",
                "reviews",
                "decide",
                "decisions",
                "decision",
                "resolve_review"
            ]
        );
        for name in &names {
            assert!(is_known(name));
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)),
                "{name} is not a legal tool name"
            );
        }
        assert!(!is_known("forget"), "not offered yet, and not pretended");
    }

    #[test]
    fn every_schema_is_an_object_schema_that_refuses_what_it_does_not_name() {
        // `inputSchema` MUST be a valid JSON Schema object, and the
        // recommended shape for a tool with no parameters is an object schema
        // with `additionalProperties: false`. Setting it everywhere means a
        // caller's typo is a validation failure at the client rather than a
        // silently ignored argument here.
        for tool in definitions() {
            let name = tool["name"].as_str().unwrap();
            let schema = &tool["inputSchema"];
            assert_eq!(schema["type"], json!("object"), "{name}");
            assert_eq!(schema["additionalProperties"], json!(false), "{name}");
            assert!(!tool["description"].as_str().unwrap().is_empty(), "{name}");
        }
    }
}
