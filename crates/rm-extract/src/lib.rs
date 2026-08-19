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
