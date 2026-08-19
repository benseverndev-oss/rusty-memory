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

use rm_core::{Interval, Provenance};
#[cfg(test)]
use rm_index::Metric;
use rm_index::{IndexError, VectorIndex};
use rm_resolve::{Decision, Record, Ruleset};
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
pub struct Engine {
    pub(crate) store: MemoryStore,
    pub(crate) index: VectorIndex,
    pub(crate) ruleset: Ruleset,
    /// Read by survivorship on recall, added in a later task.
    #[allow(dead_code)]
    pub(crate) policy: Policy,
    /// Resolution fields per entity, so a new mention can be scored against
    /// what we already know without reading them back out of the store.
    pub(crate) identity: BTreeMap<StableId, Record>,
    /// Blocking key to the entities carrying it. Derived from `identity` and
    /// rebuilt on load rather than persisted — `rm_index` already paid for the
    /// lesson that persisted derived state lets a snapshot disagree with itself.
    pub(crate) blocks: BTreeMap<String, Vec<StableId>>,
    pub(crate) assertions: BTreeMap<AssertionId, AssertionRef>,
    /// Filed by resolution's `Review` band, added in a later task.
    #[allow(dead_code)]
    pub(crate) review: BTreeMap<ReviewId, PendingReview>,
    pub(crate) next_assertion: AssertionId,
    /// Advanced when a review is filed, added in a later task.
    #[allow(dead_code)]
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

    /// Record one observation.
    ///
    /// The embedding is validated before anything is written. A rejected vector
    /// leaves the store and the index exactly as they were, because the
    /// alternative — a fact in the store with nothing able to find it — is a
    /// failure no caller can detect and no later query can report.
    ///
    /// The mention is then resolved against every blocked candidate before
    /// deciding whether this is a new entity: writing first and reconciling
    /// later would mean the store could momentarily hold two entities that
    /// should have been one, with no guarantee anything ever notices.
    pub fn remember(&mut self, obs: Observation) -> Result<Remembered, EngineError> {
        // Door first. `prepare` is what `insert` would run anyway; running it
        // here buys the guarantee that a refusal costs nothing.
        self.index.check(&obs.embedding)?;

        // Score the mention against every blocked candidate, keeping the best
        // match and every pair that landed in the review band.
        let mut best: Option<(StableId, f64)> = None;
        let mut review_pairs: Vec<(StableId, f64)> = Vec::new();
        for id in self.candidates(&obs.mention) {
            let Some(known) = self.identity.get(&id) else {
                continue;
            };
            let score = self.ruleset.score(&obs.mention, known);
            match self.ruleset.decide(score) {
                Decision::Match => {
                    if best.is_none_or(|(_, b)| score > b) {
                        best = Some((id, score));
                    }
                }
                Decision::Review => review_pairs.push((id, score)),
                Decision::NonMatch => {}
            }
        }

        // A confident match wins outright. Pairs that only reached the review
        // band are not raised in that case: the question "is this the same
        // person" has been answered by stronger evidence elsewhere.
        if let Some((entity, _)) = best {
            let assertion = self.write(entity, &obs)?;
            self.remember_identity(entity, &obs.mention);
            return Ok(Remembered::Merged { entity, assertion });
        }

        let entity = self.create_entity(&obs);
        let assertion = self.write(entity, &obs)?;

        if review_pairs.is_empty() {
            return Ok(Remembered::Created { entity, assertion });
        }

        let review = review_pairs
            .into_iter()
            .map(|(other, score)| self.file_review(entity, other, score))
            .collect();
        Ok(Remembered::CreatedPendingReview {
            entity,
            assertion,
            review,
        })
    }

    /// Entities sharing at least one blocking key with this mention.
    ///
    /// Deduplicated: a pair sharing several keys is one candidate, not several.
    /// With no blocking rules configured this returns every entity, matching
    /// `Ruleset::candidate_pairs`, which is correct and quadratic.
    fn candidates(&self, mention: &Record) -> Vec<StableId> {
        if self.ruleset.blocking().is_empty() {
            return self.identity.keys().copied().collect();
        }
        let mut seen: Vec<StableId> = Vec::new();
        for key in self.keys_for(mention) {
            for &id in self.blocks.get(&key).map_or(&[][..], |v| v.as_slice()) {
                if !seen.contains(&id) {
                    seen.push(id);
                }
            }
        }
        seen.sort_unstable();
        seen
    }

    /// Fold a new mention's fields into what we already hold for an entity.
    ///
    /// Fields already present are kept: the first spelling seen is the one the
    /// blocking map was built from, and rewriting it would leave stale keys
    /// pointing at this entity.
    fn remember_identity(&mut self, entity: StableId, mention: &Record) {
        let Some(known) = self.identity.get_mut(&entity) else {
            return;
        };
        for (field, value) in &mention.fields {
            known
                .fields
                .entry(field.clone())
                .or_insert_with(|| value.clone());
        }
    }

    /// File an open question: two entities that may be the same, and the
    /// evidence that could not decide either way.
    ///
    /// Unimplemented until Task 7. No test in this task should reach it — the
    /// test ruleset's blocking keeps genuinely different names apart, and a
    /// pair of identical mentions scores a confident `Match`, not `Review`.
    fn file_review(&mut self, _a: StableId, _b: StableId, _score: f64) -> ReviewId {
        unimplemented!("Task 7")
    }

    /// Create an entity and register its identity fields.
    fn create_entity(&mut self, obs: &Observation) -> StableId {
        let id = self
            .store
            .create_entity(&obs.kind, obs.provenance.observed_at);
        for key in self.keys_for(&obs.mention) {
            self.blocks.entry(key).or_default().push(id);
        }
        self.identity.insert(id, obs.mention.clone());
        id
    }

    /// Every blocking key a mention falls under.
    fn keys_for(&self, mention: &Record) -> Vec<String> {
        self.ruleset
            .blocking()
            .iter()
            .flat_map(|k| k.keys_for(mention))
            .collect()
    }

    /// Append the version, index its vector, and record the mapping.
    ///
    /// Uses `assert`, not `assert_resolved`: writes stay lossless and
    /// survivorship runs on read. See the module docs.
    fn write(&mut self, entity: StableId, obs: &Observation) -> Result<AssertionId, EngineError> {
        self.store.assert(
            entity,
            obs.attribute.clone(),
            obs.value.clone(),
            obs.valid,
            obs.provenance.clone(),
        )?;
        let version = self.store.history(entity, &obs.attribute).len() - 1;

        let id = self.next_assertion;
        self.next_assertion += 1;
        self.index.insert(id, &obs.embedding)?;
        self.assertions.insert(
            id,
            AssertionRef {
                entity,
                attribute: obs.attribute.clone(),
                version,
            },
        );
        Ok(id)
    }

    /// Where a given assertion lives.
    pub fn assertion(&self, id: AssertionId) -> Option<&AssertionRef> {
        self.assertions.get(&id)
    }

    /// How many vectors are searchable.
    pub fn index_len(&self) -> usize {
        self.index.len()
    }
}

/// One thing learned: what it is about, what it says, and how we know.
#[derive(Clone, Debug)]
pub struct Observation {
    /// The entity's kind, e.g. `"person"`. Passed to the store on creation.
    pub kind: String,
    /// The fields identifying who or what this is about, for resolution.
    pub mention: Record,
    pub attribute: String,
    /// `None` asserts the attribute has no value — a tombstone, distinct from
    /// having said nothing. An observation that says nothing is not an
    /// observation and should not be passed here.
    pub value: Option<String>,
    pub valid: Interval,
    pub provenance: Provenance,
    /// Caller-supplied. `rm_extract` is the only crate permitted to reach the
    /// network, so nothing here computes an embedding.
    pub embedding: Vec<f32>,
}

/// What `remember` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Remembered {
    /// Landed on an entity we already knew.
    Merged {
        entity: StableId,
        assertion: AssertionId,
    },
    /// Nothing matched; this is a new entity.
    Created {
        entity: StableId,
        assertion: AssertionId,
    },
    /// A new entity *and* one or more open questions: it scored in the review
    /// band against something already known. The fact is remembered either way;
    /// what is uncertain is only whose it is.
    CreatedPendingReview {
        entity: StableId,
        assertion: AssertionId,
        review: Vec<ReviewId>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_core::{Source, Timestamp};
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

    fn observation(name: &str, attribute: &str, value: &str, at: Timestamp) -> Observation {
        Observation {
            kind: "person".to_string(),
            mention: Record::new().with("name", name).with("city", "Bristol"),
            attribute: attribute.to_string(),
            value: Some(value.to_string()),
            valid: Interval::since(at),
            provenance: Provenance::new(Source::UserAssertion, at, "session-1"),
            embedding: vec![1.0, 0.0, 0.0],
        }
    }

    fn engine() -> Engine {
        Engine::new(
            VectorIndex::new(3, Metric::Cosine),
            test_ruleset(),
            Policy::new(Strategy::MostRecent),
        )
    }

    #[test]
    fn remembering_something_new_creates_an_entity_and_indexes_it() {
        let mut e = engine();
        let out = e
            .remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let Remembered::Created { entity, assertion } = out else {
            panic!("expected Created, got {out:?}");
        };
        assert_eq!(e.entity_count(), 1);
        assert_eq!(e.assertion(assertion).unwrap().entity, entity);
        assert_eq!(e.assertion(assertion).unwrap().attribute, "employer");
    }

    #[test]
    fn a_rejected_vector_leaves_the_store_untouched() {
        // Wrong dimension. The fact must not land: a memory that exists and
        // cannot be found is undetectable from the outside.
        let mut e = engine();
        let mut obs = observation("Ben Severn", "employer", "Acme", 1);
        obs.embedding = vec![1.0, 0.0];

        let err = e.remember(obs).unwrap_err();
        assert!(matches!(err, EngineError::Index(_)), "got {err:?}");
        assert_eq!(e.entity_count(), 0, "no entity may survive a refused write");
        assert_eq!(e.index_len(), 0);
    }

    #[test]
    fn a_zero_vector_under_cosine_is_refused_before_anything_is_written() {
        let mut e = engine();
        let mut obs = observation("Ben Severn", "employer", "Acme", 1);
        obs.embedding = vec![0.0, 0.0, 0.0];
        assert!(e.remember(obs).is_err());
        assert_eq!(e.entity_count(), 0);
    }

    #[test]
    fn a_second_observation_about_the_same_person_merges() {
        let mut e = engine();
        let first = e
            .remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let second = e
            .remember(observation("Ben Severn", "city", "Bristol", 2))
            .unwrap();

        let (Remembered::Created { entity: a, .. }, Remembered::Merged { entity: b, .. }) =
            (first, second)
        else {
            panic!("expected Created then Merged");
        };
        assert_eq!(a, b);
        assert_eq!(e.entity_count(), 1);
    }

    #[test]
    fn a_clearly_different_person_gets_their_own_entity() {
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let out = e
            .remember(observation("Wei Zhang", "employer", "Globex", 2))
            .unwrap();
        assert!(matches!(out, Remembered::Created { .. }), "got {out:?}");
        assert_eq!(e.entity_count(), 2);
    }

    #[test]
    fn blocking_finds_the_same_matches_as_comparing_everything() {
        // The incremental blocking map must agree with rm-resolve's batch
        // candidate_pairs, or the engine silently loses true matches.
        let mut e = engine();
        for (i, name) in ["Ben Severn", "Wei Zhang", "Ben Severn"].iter().enumerate() {
            e.remember(observation(name, "employer", "Acme", i as Timestamp + 1))
                .unwrap();
        }
        assert_eq!(e.entity_count(), 2, "the two Ben Severns are one entity");
    }
}
