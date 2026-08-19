//! `remember()` / `recall()` / `forget()` over the rest of rusty-memory.
//!
//! # What this crate does and does not own
//!
//! It orchestrates. The store, the index and the resolver each keep their own
//! guarantees, and this crate's job is to call them in an order that keeps them
//! agreeing with each other. Its own state is only what none of them can hold:
//! which assertion a vector belongs to, which entities are candidates for which
//! blocking key, and which pairs are still open questions.
//!
//! # Writes are lossless; resolution is a query
//!
//! [`Engine::remember`] appends. It does not run survivorship, because resolving
//! on write picks a winner and discards the losers, which is the behaviour this
//! project exists to argue against. [`Engine::about`] applies the strategy at
//! read time, so the same history can be asked under two different rules and the
//! rule can change without rewriting anything.
//!
//! # A middle-band match is a question, not a merge
//!
//! `rm_resolve` produces three bands. [`Engine::remember`] merges on `Match`,
//! creates on `NonMatch`, and on `Review` creates a new entity *and* files the
//! pair for someone to answer. It never merges on a score it could not call: an
//! agent that fuses two people because they scored in the middle has corrupted
//! its memory permanently and silently.

mod persist;
mod policy;
mod read;
mod review;

use std::collections::BTreeMap;

#[cfg(test)]
use rm_index::Metric;
use rm_index::{IndexError, VectorIndex};
use rm_resolve::{Record, Ruleset};
use rm_store::{MemoryStore, StableId, StoreError};
use rm_survivor::Refused;
use serde::{Deserialize, Serialize};

pub use policy::Policy;
pub use read::{Believed, Query, Recalled};
pub use review::{PendingReview, ReviewId};

/// Identifies one stored assertion. Doubles as the vector index's `EntryId`, so
/// a search hit resolves to a stored fact with one lookup rather than a
/// translation table that could disagree with itself.
pub type AssertionId = u64;

/// Where an assertion lives in the store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertionRef {
    pub entity: StableId,
    pub attribute: String,
    /// Index into that attribute's version log.
    pub version: usize,
}

/// Something the caller has to decide about.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineError {
    Index(IndexError),
    Store(StoreError),
    /// Survivorship declined to guess. Carries its explanation.
    Refused(Refused),
    UnknownEntity(StableId),
    UnknownReview(ReviewId),
    CorruptSnapshot(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Index(e) => write!(f, "{e}"),
            EngineError::Store(e) => write!(f, "{e}"),
            EngineError::Refused(e) => write!(f, "{e}"),
            EngineError::UnknownEntity(id) => write!(f, "no entity with id {id}"),
            EngineError::UnknownReview(id) => write!(f, "no open review with id {id}"),
            EngineError::CorruptSnapshot(why) => {
                write!(
                    f,
                    "snapshot parsed but describes an impossible engine: {why}"
                )
            }
        }
    }
}

impl std::error::Error for EngineError {}

impl From<IndexError> for EngineError {
    fn from(e: IndexError) -> Self {
        EngineError::Index(e)
    }
}

impl From<StoreError> for EngineError {
    fn from(e: StoreError) -> Self {
        EngineError::Store(e)
    }
}

impl From<Refused> for EngineError {
    fn from(e: Refused) -> Self {
        EngineError::Refused(e)
    }
}

/// The engine.
///
/// `#[allow(dead_code)]`: this task only wires up construction and
/// `entity_count`. Every other field is read by `remember`, `about`, `forget`
/// and snapshot round-tripping, added over the next several tasks — the first
/// of which (`remember`) removes this allow. Scoped to the struct rather than
/// the module so nothing else in this crate can quietly grow unread state
/// under the same cover.
#[allow(dead_code)]
pub struct Engine {
    pub(crate) store: MemoryStore,
    pub(crate) index: VectorIndex,
    pub(crate) ruleset: Ruleset,
    pub(crate) policy: Policy,
    /// Resolution fields per entity, so a new mention can be scored against
    /// what we already know without reading them back out of the store.
    pub(crate) identity: BTreeMap<StableId, Record>,
    /// Blocking key to the entities carrying it. Derived from `identity` and
    /// rebuilt on load rather than persisted — `rm_index` already paid for the
    /// lesson that persisted derived state lets a snapshot disagree with itself.
    pub(crate) blocks: BTreeMap<String, Vec<StableId>>,
    pub(crate) assertions: BTreeMap<AssertionId, AssertionRef>,
    pub(crate) review: BTreeMap<ReviewId, PendingReview>,
    pub(crate) next_assertion: AssertionId,
    pub(crate) next_review: ReviewId,
}

impl Engine {
    /// A new engine over an empty store.
    ///
    /// The index is supplied rather than constructed because its dimension and
    /// metric are properties of the caller's embedding model, and `rm_index`
    /// deliberately refuses to default the metric.
    pub fn new(index: VectorIndex, ruleset: Ruleset, policy: Policy) -> Self {
        Engine {
            store: MemoryStore::new(),
            index,
            ruleset,
            policy,
            identity: BTreeMap::new(),
            blocks: BTreeMap::new(),
            assertions: BTreeMap::new(),
            review: BTreeMap::new(),
            next_assertion: 0,
            next_review: 0,
        }
    }

    /// How many entities the engine knows about.
    pub fn entity_count(&self) -> usize {
        self.identity.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_resolve::{BlockingKey, Comparator, FieldRule};
    use rm_survivor::Strategy;

    /// A deliberately ordinary ruleset: names are typo-prone but discriminating,
    /// cities agree often by chance.
    pub(crate) fn test_ruleset() -> Ruleset {
        Ruleset::new(
            vec![
                FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01),
                FieldRule::new("city", Comparator::Normalized, 0.8, 0.2),
            ],
            vec![BlockingKey::Prefix("name".to_string(), 3)],
            4.0,
            8.0,
        )
        .unwrap()
    }

    #[test]
    fn a_new_engine_knows_nothing() {
        let engine = Engine::new(
            VectorIndex::new(3, Metric::Cosine),
            test_ruleset(),
            Policy::new(Strategy::MostRecent),
        );
        assert_eq!(engine.entity_count(), 0);
    }
}
