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

/// Where this session stands, for deciding what applies to it.
///
/// Read-side only, and deliberately: reach varies per decision, so a session
/// value would answer -- silently and usually wrongly -- the one question the
/// writer is uniquely placed to answer.
pub const SCOPE_ENV: &str = "RMEM_SCOPE";

/// The tools this server offers, honouring [`TOOLS_ENV`].
///
/// # Why a session should be able to ask for fewer
///
/// The tool table is sent on every turn of every session that has this server
/// configured, whether or not it is used. Measured: nine tools are about 2,600
/// tokens, which is more than a thirty-decision log costs to read in full -- so
/// a project that only ever consults decisions pays a log's worth of context
/// per turn to advertise six tools it will never call.
///
/// Names that match nothing are ignored rather than refused. This is read at
/// startup on a path with no good way to report, and a server that will not
/// start because a list has a typo in it is worse than one that offers fewer
/// tools than expected -- which `tools/list` shows plainly.
/// Characters per token for this table's JSON.
///
/// From the four counted rows in the README: 8,203/2,060, 5,650/1,420,
/// 4,475/1,130 and 2,385/610 give 3.91 to 3.98. Stated here once so the
/// README's rows, the comment on [`definitions`] and the test below all
/// derive from one number rather than three copies of it -- those last two
/// disagreed for a day in August 2026, when the README moved twice for the
/// clocks and for scope and the comment did not follow.
///
/// `cfg(test)` because nothing in a running server needs it: it exists so the
/// documentation's arithmetic is checkable, not so the code can do arithmetic.
#[cfg(test)]
pub(crate) const CHARS_PER_TOKEN: f64 = 3.97;

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
            "name": "note",
            "title": "Record a fact you already know",
            "description": "Record a fact you already know about someone or something. Costs one embedding and no completion, unlike remember, which reads prose and works out what the facts are. Use this when you already know the fact and can name it: who it is about, what the attribute is called, and its value. \"who\" is a name, and the store decides whether that is someone it already knows -- if it cannot tell, the fact is still recorded and the identity question is queued for a person to settle. Set absent to assert there is no value, which is different from never having been asked: \"has no direct reports\" and \"nobody has said\" are different answers and this is the only way to record the first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "who": {
                        "type": "string",
                        "description": "Who or what the fact is about, by name. This is what the store scores against everyone it already knows."
                    },
                    "attribute": {
                        "type": "string",
                        "description": "What is being recorded, as a short name it can be asked about by later -- \"role\", \"team\", \"employer\"."
                    },
                    "value": {
                        "type": "string",
                        "description": "The value. Omit it and set absent instead to assert there is none."
                    },
                    "absent": {
                        "type": "boolean",
                        "description": "Assert that there is no value. Different from leaving the attribute unrecorded, which reads as never discussed."
                    },
                    "kind": {
                        "type": "string",
                        "description": "What sort of thing this is. Defaults to person."
                    },
                    "fields": {
                        "type": "object",
                        "additionalProperties": {"type": "string"},
                        "description": "Extra identifying fields, like an email address. These describe WHO the subject is and are what the store compares to recognise them again -- not facts about them, which are attributes."
                    },
                    "valid_from": {
                        "type": "string",
                        "description": "The day this started being true, as YYYY-MM-DD, if that is not today. Sets when it held from, not when the store heard it."
                    },
                    "scope": {
                        "type": "string",
                        "description": "How far this fact reaches, if it is not true everywhere. Omit it for an ordinary fact about a person: with no scope it reaches every project, which is usually right."
                    }
                },
                "required": ["who", "attribute"],
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
                    },
                    "scope": {
                        "type": "string",
                        "description": "Search from this position instead of the session's own."
                    },
                    "all": {
                        "type": "boolean",
                        "description": "Ignore reach; search memories scoped elsewhere too."
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
                    "scope": {
                        "type": "string",
                        "description": "How far this decision reaches: a path like \"work/goldenmatch/fs\", or \"*\" for every project. Ask where it would still be true, not where you learned it -- a rule about this machine is \"*\" even if you found it in one project."
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
                "required": ["title", "choice", "scope"],
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
                    },
                    "scope": {
                        "type": "string",
                        "description": "Ask from this position instead of the session's own."
                    },
                    "all": {
                        "type": "boolean",
                        "description": "Ignore reach; include decisions scoped elsewhere."
                    },
                    "as_of": {
                        "type": ["string", "integer"],
                        "description": "The log as the store knew it on this date (YYYY-MM-DD). Later decisions are absent. Omit for now."
                    },
                    "valid_at": {
                        "type": ["string", "integer"],
                        "description": "What held on this date (YYYY-MM-DD). A decision backdated with decided_at holds from that day. Omit for now."
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
                    },
                    "scope": {
                        "type": "string",
                        "description": "Ask from this position instead of the session's own."
                    },
                    "all": {
                        "type": "boolean",
                        "description": "Ignore reach; include decisions scoped elsewhere."
                    },
                    "as_of": {
                        "type": ["string", "integer"],
                        "description": "As the store knew it on this date (YYYY-MM-DD), the supersession walk included, so a later replacement does not retire it. Omit for now."
                    },
                    "valid_at": {
                        "type": ["string", "integer"],
                        "description": "What held on this date (YYYY-MM-DD). Omit for now."
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
        scope: Option<String>,
        all: bool,
    },
    About {
        entity: StableId,
        attribute: String,
        valid_at: Option<Timestamp>,
        as_of: Option<Timestamp>,
    },
    Reviews,
    ResolveReview {
        id: ReviewId,
        same: bool,
    },
    Note {
        who: String,
        kind: String,
        attribute: String,
        /// `None` when `absent` was set: an asserted absence, not a gap.
        value: Option<String>,
        fields: Vec<(String, String)>,
        valid_from: Option<Timestamp>,
        scope: Option<String>,
        session: String,
    },
    Decide {
        title: String,
        choice: String,
        scope: String,
        status: Option<String>,
        decided_at: Option<rm_engine::Timestamp>,
        because: Option<String>,
        context: Option<String>,
        supersedes: Option<String>,
        session: String,
    },
    Decisions {
        status: Option<String>,
        valid_at: Option<Timestamp>,
        as_of: Option<Timestamp>,
        scope: Option<String>,
        all: bool,
    },
    /// Read one decision in full, by exact title.
    Decision {
        title: String,
        valid_at: Option<Timestamp>,
        as_of: Option<Timestamp>,
        scope: Option<String>,
        all: bool,
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
        // The agent, then the machine, then the run: `RM@bsev-002/abc123`,
        // matching what `rm_host::attribution` writes on the CLI side so both
        // hosts are comparable rather than one being useful and one being a
        // constant.
        let host = rm_host::attribution::host();
        let host = if host.trim().is_empty() {
            "unknown-host".to_string()
        } else {
            host
        };
        // No handshake identity: a client that did not name itself, which the
        // specification allows. `mcp` was the old default and stays the agent
        // part, so nothing that worked before now records less -- it records
        // the machine as well.
        let agent = client.unwrap_or("mcp");
        Ok(match session {
            Some(s) => format!("{agent}@{host}/{s}"),
            None => format!("{agent}@{host}"),
        })
    }

    /// Read a tool call from its arguments.
    ///
    /// Takes no clock. It used to, to default `about`'s two axes to now -- and
    /// that default was the bug: once applied, "what held in March" and "what
    /// holds now" are the same call, so nothing downstream could refuse a
    /// valid-time question the attribute could not answer. The axes travel as
    /// `Option`s and the default is applied where the refusal lives.
    pub fn read(name: &str, arguments: &Value, client: Option<&str>) -> Result<Call, Unreadable> {
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
                scope: optional_string(arguments, "scope")?,
                all: optional_bool(arguments, "all")?.unwrap_or(false),
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
                // Left unresolved, so `command::about` can tell a valid-time
                // question from its absence and refuse one the attribute's
                // strategy cannot answer.
                valid_at: optional_integer(arguments, "valid_at")?,
                as_of: optional_integer(arguments, "as_of")?,
            }),
            "reviews" => Ok(Call::Reviews),
            "decisions" => Ok(Call::Decisions {
                status: optional_string(arguments, "status")?,
                scope: optional_string(arguments, "scope")?,
                all: optional_bool(arguments, "all")?.unwrap_or(false),
                valid_at: optional_instant(arguments, "valid_at")?,
                as_of: optional_instant(arguments, "as_of")?,
            }),
            "decision" => Ok(Call::Decision {
                title: string(arguments, "title")?,
                scope: optional_string(arguments, "scope")?,
                all: optional_bool(arguments, "all")?.unwrap_or(false),
                valid_at: optional_instant(arguments, "valid_at")?,
                as_of: optional_instant(arguments, "as_of")?,
            }),
            "note" => {
                let value = optional_string(arguments, "value")?;
                let absent = arguments
                    .get("absent")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                // Refused rather than resolved by precedence: they
                // contradict each other, and guessing which was meant is how
                // an asserted absence silently becomes a value.
                if absent && value.is_some() {
                    return Err(Unreadable::from(
                        "a value and absent contradict each other: absent says there is no value, so do not also give one".to_string(),
                    ));
                }
                if !absent && value.is_none() {
                    return Err(Unreadable::from(
                        "a note needs a value, or absent set to assert there is none".to_string(),
                    ));
                }
                let fields = match arguments.get("fields").and_then(Value::as_object) {
                    None => Vec::new(),
                    Some(map) => map
                        .iter()
                        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                        .collect(),
                };
                Ok(Call::Note {
                    who: string(arguments, "who")?,
                    kind: optional_string(arguments, "kind")?
                        .unwrap_or_else(|| "person".to_string()),
                    attribute: string(arguments, "attribute")?,
                    value,
                    fields,
                    valid_from: optional_string(arguments, "valid_from")?
                        .map(|d| rm_host::time::parse_day(&d))
                        .transpose()?,
                    scope: optional_string(arguments, "scope")?,
                    session: Call::attributed(arguments, client)?,
                })
            }
            "decide" => Ok(Call::Decide {
                title: string(arguments, "title")?,
                choice: string(arguments, "choice")?,
                scope: string(arguments, "scope")?,
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
            Call::Remember { .. }
            | Call::ResolveReview { .. }
            | Call::Decide { .. }
            | Call::Note { .. } => true,
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

/// A point in time, given either way.
///
/// A JSON number is milliseconds, matching `about`'s `valid_at`/`as_of`. A
/// string is `YYYY-MM-DD` read as the end of that day, matching `decide`'s
/// `decided_at` and the CLI's flags. Both conventions already exist in this
/// file, and accepting either means the same parameter name does not mean two
/// different types depending on which tool it is on.
fn optional_instant(args: &Value, field: &str) -> Result<Option<Timestamp>, Unreadable> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(_)) => optional_string(args, field)?
            .map(|d| rm_host::time::parse_day_end(&d))
            .transpose(),
        Some(Value::Number(_)) => optional_integer(args, field),
        Some(_) => Err(format!(
            "{field} must be a date as YYYY-MM-DD or a time in milliseconds"
        )),
    }
}

/// A boolean argument, when the caller gave one.
///
/// Mirrors `optional_string`: absent and null are both "not given", and
/// anything that is not a boolean is a caller mistake worth naming rather than
/// coercing.
fn optional_bool(args: &Value, field: &str) -> Result<Option<bool>, Unreadable> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("{field} must be true or false")),
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

    fn read(name: &str, args: Value) -> Result<Call, Unreadable> {
        Call::read(name, &args, None)
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
            (
                "note",
                json!({"who": "Jon Severn", "attribute": "role", "value": "leads circ"}),
            ),
            ("recall", json!({"query": "where do I work"})),
            ("about", json!({"entity": 0, "attribute": "employer"})),
            ("reviews", json!({})),
            (
                "decide",
                json!({"title": "Use one file", "choice": "One snapshot per store", "scope": "work"}),
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
    /// The machine, or a stated stand-in. Computed rather than written down:
    /// a hardcoded name would pass on one box and fail on every other.
    fn host_or_unknown() -> String {
        let h = rm_host::attribution::host();
        if h.trim().is_empty() {
            "unknown-host".to_string()
        } else {
            h
        }
    }

    #[test]
    fn a_write_is_attributed_to_the_client_that_made_it() {
        let with = |client: Option<&str>, args: Value| match Call::read("decide", &args, client)
            .unwrap()
        {
            Call::Decide { session, .. } => session,
            other => panic!("{other:?}"),
        };
        let bare = json!({"title": "T", "choice": "C", "scope": "work"});
        // The machine, computed rather than written down: a hardcoded name
        // would pass on one box and fail on every other.
        let h = rm_host::attribution::host();
        let h = if h.trim().is_empty() {
            "unknown-host".to_string()
        } else {
            h
        };

        // The handshake name, and where it ran.
        assert_eq!(
            with(Some("claude-code 2.1"), bare.clone()),
            format!("claude-code 2.1@{h}")
        );

        // Both, and neither hides the other: an agent can name its conversation
        // without erasing which agent it was.
        assert_eq!(
            with(
                Some("claude-code 2.1"),
                json!({"title":"T","choice":"C","scope":"work","session":"refactor"})
            ),
            format!("claude-code 2.1@{h}/refactor")
        );

        // A client that named itself in the handshake and nothing else still
        // gets attribution, which is the case this exists for.
        assert_eq!(with(Some("agent-B"), bare.clone()), format!("agent-B@{h}"));

        // No handshake identity: the specification allows it, and nothing that
        // worked before now records less.
        assert_eq!(with(None, bare), format!("mcp@{h}"));
        assert_eq!(
            with(
                None,
                json!({"title":"T","choice":"C","scope":"work","session":"s"})
            ),
            format!("mcp@{h}/s")
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
                session: format!("mcp@{}", host_or_unknown()),
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
                session: format!("mcp@{}", host_or_unknown()),
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
                k: 5,
                scope: None,
                all: false
            }
        );
        assert_eq!(
            read("about", json!({"entity": 2, "attribute": "employer"})).unwrap(),
            Call::About {
                entity: 2,
                attribute: "employer".into(),
                // Absent, not defaulted to now. Applying the default here is
                // what made "what held in May" and "what holds now" the same
                // call, so nothing downstream could refuse the first.
                valid_at: None,
                as_of: None
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
                valid_at: Some(500),
                as_of: Some(900)
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
        assert!(read(
            "decide",
            json!({"title": "t", "choice": "c", "scope": "work"})
        )
        .unwrap()
        .mutates());
    }

    #[test]
    fn the_table_is_nine_tools_in_a_fixed_order_with_legal_names() {
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
                "note",
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

    /// Either convention, because both already exist in this file: `about`
    /// takes these two as integers and `decide` takes its date as a string.
    /// One parameter name meaning two types across tools is a footgun for a
    /// model caller, so these take both.
    #[test]
    fn the_decision_reads_take_either_a_date_or_an_instant() {
        let Call::Decision {
            valid_at, as_of, ..
        } = read(
            "decision",
            json!({"title": "Pin the compiler", "as_of": "2026-08-24"}),
        )
        .unwrap()
        else {
            panic!("not a decision call")
        };
        assert_eq!(as_of, Some(1_787_529_600_000 + 86_399_999));
        assert_eq!(valid_at, None);

        let Call::Decisions { as_of, .. } =
            read("decisions", json!({"as_of": 1_787_529_600_000i64})).unwrap()
        else {
            panic!("not a decisions call")
        };
        assert_eq!(as_of, Some(1_787_529_600_000), "a number is milliseconds");

        assert!(
            read("decision", json!({"title": "X", "as_of": "not-a-date"})).is_err(),
            "a date that is not one must be refused"
        );
        assert!(
            read("decision", json!({"title": "X", "as_of": true})).is_err(),
            "a boolean is neither"
        );
    }

    #[test]
    fn an_agent_cannot_record_a_decision_without_stating_its_reach() {
        assert!(
            read("decide", json!({"title": "A title", "choice": "A choice"})).is_err(),
            "scope is required"
        );

        let Call::Decide { scope, .. } = read(
            "decide",
            json!({"title": "A title", "choice": "A choice", "scope": "work/goldenmatch"}),
        )
        .unwrap() else {
            panic!("not a decide call")
        };
        assert_eq!(scope, "work/goldenmatch");
    }

    /// The schema has to say it too, or a model never learns the argument
    /// exists and every call fails at the parse instead.
    #[test]
    fn the_decide_schema_marks_scope_required() {
        let decide = all_definitions()
            .into_iter()
            .find(|t| t["name"] == "decide")
            .expect("decide is defined");
        let required = decide["inputSchema"]["required"]
            .as_array()
            .expect("a required list");
        assert!(
            required.iter().any(|v| v == "scope"),
            "scope must be required: {required:?}"
        );
        assert!(decide["inputSchema"]["properties"]["scope"].is_object());
    }

    #[test]
    fn the_reads_take_a_position_and_a_way_to_ignore_it() {
        let Call::Decisions { scope, all, .. } =
            read("decisions", json!({"scope": "personal"})).unwrap()
        else {
            panic!("not a decisions call")
        };
        assert_eq!(scope.as_deref(), Some("personal"));
        assert!(!all);

        let Call::Decision { all, .. } =
            read("decision", json!({"title": "A title", "all": true})).unwrap()
        else {
            panic!("not a decision call")
        };
        assert!(all);

        assert!(
            read("decisions", json!({"all": "yes"})).is_err(),
            "a string is not a boolean"
        );
    }

    #[test]
    fn recall_takes_a_position_and_a_way_to_ignore_it() {
        let Call::Recall { scope, all, .. } =
            read("recall", json!({"query": "x", "scope": "personal"})).unwrap()
        else {
            panic!("not a recall call")
        };
        assert_eq!(scope.as_deref(), Some("personal"));
        assert!(!all);

        let Call::Recall { all, .. } = read("recall", json!({"query": "x", "all": true})).unwrap()
        else {
            panic!("not a recall call")
        };
        assert!(all);
    }
    /// The handshake name is an agent, and an agent is on a machine.
    ///
    /// `attributed` already recorded who the client said it was; what it could
    /// not say is where. Two agents called `Print` on two machines were
    /// indistinguishable, and on the machine this was written for there were
    /// five.
    #[test]
    fn the_author_names_the_machine_as_well_as_the_client() {
        let host = rm_host::attribution::host();
        let got = Call::attributed(&json!({}), Some("RM")).unwrap();
        assert!(got.starts_with("RM@"), "{got}");
        if !host.is_empty() {
            assert!(got.contains(&host), "{got} should name {host}");
        }
    }

    /// A client that gives a session id keeps it, after the host.
    #[test]
    fn a_client_supplied_session_follows_the_machine() {
        let got = Call::attributed(&json!({"session": "abc"}), Some("RM")).unwrap();
        assert!(got.starts_with("RM@"), "{got}");
        assert!(got.ends_with("/abc"), "{got}");
    }

    /// A client that never named itself still records where it ran. The
    /// specification allows an anonymous client, so this must not become a
    /// refusal -- but `mcp` alone was as uninformative as the CLI's `cli`.
    #[test]
    fn an_anonymous_client_still_records_the_machine() {
        let got = Call::attributed(&json!({}), None).unwrap();
        assert!(got.starts_with("mcp@"), "{got}");
    }
    /// The note tool reads its arguments, and an absence is a claim it can
    /// express.
    #[test]
    fn the_note_tool_reads_who_what_and_an_absence() {
        let Call::Note {
            who,
            attribute,
            value,
            ..
        } = Call::read(
            "note",
            &json!({"who": "Jon Severn", "attribute": "role", "value": "leads circ"}),
            Some("RM"),
        )
        .unwrap()
        else {
            panic!("expected Note")
        };
        assert_eq!(who, "Jon Severn");
        assert_eq!(attribute, "role");
        assert_eq!(value.as_deref(), Some("leads circ"));

        let Call::Note { value, .. } = Call::read(
            "note",
            &json!({"who": "Jon", "attribute": "reports", "absent": true}),
            Some("RM"),
        )
        .unwrap() else {
            panic!("expected Note")
        };
        assert_eq!(value, None, "absent is an asserted absence, not a gap");
    }

    /// A value and `absent` contradict each other, and are refused rather than
    /// resolved by precedence -- guessing which was meant is how an asserted
    /// absence silently becomes a value.
    #[test]
    fn the_note_tool_refuses_a_value_and_an_absence_together() {
        let err = Call::read(
            "note",
            &json!({"who": "Jon", "attribute": "reports", "value": "none", "absent": true}),
            Some("RM"),
        )
        .unwrap_err();
        assert!(format!("{err:?}").contains("absent"), "{err:?}");
    }
    /// The table is the size the documentation says it is.
    ///
    /// The byte count is the measurement; the token figure is derived from
    /// it, so only one of the two can rot. The band is wide on purpose: a
    /// reworded description must not fail this, a tool appearing or vanishing
    /// must.
    #[test]
    fn the_tool_table_is_the_size_the_documentation_says() {
        let chars = serde_json::to_string(&all_definitions()).unwrap().len();
        assert!(
            (10_000..11_000).contains(&chars),
            "the table is {chars} chars; update the README's row and the comment on `definitions` together"
        );
        let tokens = chars as f64 / CHARS_PER_TOKEN;
        assert!((tokens - 2_600.0).abs() < 150.0, "~{tokens:.0} tokens");
    }
}
