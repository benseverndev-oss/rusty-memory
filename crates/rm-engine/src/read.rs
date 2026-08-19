//! Filled in by a later task.

use rm_core::{Interval, Provenance, Source, Timestamp};
use rm_store::StableId;

use crate::AssertionId;

/// What the engine concluded an attribute held.
///
/// Owned rather than borrowed: survivorship on read builds its answer from an
/// outcome computed inside the call, so when the strategy produces a value no
/// single stored version carried, there is nothing in the store to borrow from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Believed {
    Value(String),
    Absent,
    Unknown,
}

/// A recall request.
#[derive(Clone, Debug)]
pub struct Query {
    pub embedding: Vec<f32>,
    pub k: usize,
    /// `(valid_t, tx_t)`. Both axes, because "what did I believe last Tuesday
    /// about what was true in May" is a different question from either half.
    pub as_of: Option<(Timestamp, Timestamp)>,
    pub entity: Option<StableId>,
    pub source: Option<Source>,
    pub session: Option<String>,
}

/// One recalled assertion.
#[derive(Clone, Debug, PartialEq)]
pub struct Recalled {
    pub entity: StableId,
    pub assertion: AssertionId,
    pub attribute: String,
    /// `None` is a tombstone — this assertion claimed the attribute had no
    /// value. It is never "we have nothing": an assertion that says nothing is
    /// not stored and cannot be recalled.
    pub value: Option<String>,
    pub valid: Interval,
    pub provenance: Provenance,
    pub score: f32,
    /// A later assertion superseded this one as of the query's `tx_t`.
    pub superseded: bool,
}
