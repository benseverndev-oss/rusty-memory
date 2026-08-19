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

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use rm_core::{Interval, Provenance, Timestamp};
#[cfg(test)]
use rm_index::Metric;
use rm_index::{IndexError, VectorIndex};
use rm_resolve::{Decision, Record, Ruleset};
use rm_store::{MemoryStore, StableId, StoreError};
use rm_survivor::Refused;
use serde::{Deserialize, Serialize};

pub use policy::Policy;
pub use read::{Believed, Query, Recalled};
pub use review::{PendingReview, ReviewId, Settled};

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
    /// Read by survivorship in [`Engine::about`]. Swappable via
    /// [`Engine::with_policy`] without touching a single stored version.
    pub(crate) policy: Policy,
    /// Resolution fields per entity, so a new mention can be scored against
    /// what we already know without reading them back out of the store.
    pub(crate) identity: BTreeMap<StableId, Record>,
    /// Blocking key to the entities carrying it. Derived from `identity` and
    /// rebuilt on load rather than persisted — `rm_index` already paid for the
    /// lesson that persisted derived state lets a snapshot disagree with itself.
    pub(crate) blocks: BTreeMap<String, Vec<StableId>>,
    pub(crate) assertions: BTreeMap<AssertionId, AssertionRef>,
    /// Filed by resolution's `Review` band: pairs the resolver could not call,
    /// waiting on an answer from `pending_review`.
    pub(crate) review: BTreeMap<ReviewId, PendingReview>,
    /// Pairs answered "not the same". Kept for the lifetime of the engine: an
    /// answered question that gets asked again is worse than an unasked one,
    /// because it teaches the caller the queue is noise.
    pub(crate) rejected: BTreeSet<Settled>,
    pub(crate) next_assertion: AssertionId,
    /// Advanced each time `file_review` opens a question.
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
            rejected: BTreeSet::new(),
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

        // Drop any pair someone has already answered "not the same". The
        // emptiness check has to come *after* this filter rather than before
        // it, because the filter is what can empty the list.
        review_pairs.retain(|(other, _)| !self.already_rejected(entity, *other));
        let review: Vec<ReviewId> = review_pairs
            .into_iter()
            .map(|(other, score)| self.file_review(entity, other, score))
            .collect();

        if review.is_empty() {
            return Ok(Remembered::Created { entity, assertion });
        }
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

    /// Fold a new mention's fields into what we already hold for an entity, and
    /// register the blocking keys those new fields unlock.
    ///
    /// Fields already present are kept: the first spelling seen is the one the
    /// blocking map was built from, and rewriting it would leave stale keys
    /// pointing at this entity.
    ///
    /// A field arriving for the first time is the opposite case, and folding it
    /// into `identity` without keying it was a real defect. `identity` is what
    /// the blocking map is *derived from* — it is rebuilt from `identity` on
    /// load rather than persisted — so an unkeyed field makes the live map a
    /// strict subset of the one a reload would produce, and the same engine
    /// then resolves the same mention differently either side of a snapshot
    /// round trip. That is the worst shape a bug can take here: no error, no
    /// difference in the stored facts, only a merge that happens or does not
    /// depending on when the process last restarted.
    fn remember_identity(&mut self, entity: StableId, mention: &Record) {
        let Some(known) = self.identity.get_mut(&entity) else {
            return;
        };
        let mut learned = false;
        for (field, value) in &mention.fields {
            if let Entry::Vacant(slot) = known.fields.entry(field.clone()) {
                slot.insert(value.clone());
                learned = true;
            }
        }
        if !learned {
            return;
        }

        // Re-key from the *folded* record, not from the mention: a key can be
        // derived from a field the mention did not carry, and re-keying the
        // whole record is idempotent where keying the delta alone is not.
        let folded = self.identity[&entity].clone();
        self.key_entity(entity, &folded);
    }

    /// Register one entity under every blocking key its record derives.
    ///
    /// The only writer that adds to `blocks`, so the dedup rule is stated once
    /// instead of three times. It used to be three: `create_entity` and
    /// `rebuild_blocks` pushed blind while `remember_identity` checked first,
    /// and the three only agreed because each happened to be called at a moment
    /// where a duplicate could not arise. That is a property of the callers, not
    /// of the map, and a duplicate id in a block is silent — it makes
    /// `candidates` score the same pair twice and, in `confirm`, makes a single
    /// `retain` insufficient to remove an id.
    pub(crate) fn key_entity(&mut self, entity: StableId, record: &Record) {
        for key in self.keys_for(record) {
            let ids = self.blocks.entry(key).or_default();
            if !ids.contains(&entity) {
                ids.push(entity);
            }
        }
    }

    /// Record an open question about two entities.
    ///
    /// Ordered so the lower id is always `a`, making a pair's identity
    /// independent of which observation arrived first.
    fn file_review(&mut self, a: StableId, b: StableId, score: f64) -> ReviewId {
        let id = self.next_review;
        self.next_review += 1;
        self.review.insert(
            id,
            PendingReview {
                id,
                a: a.min(b),
                b: a.max(b),
                score,
            },
        );
        id
    }

    /// Whether this pair has already been answered "not the same".
    ///
    /// Checked by id *and* by identity record. The id check is the cheap,
    /// obvious one. The record check exists because rejecting a pair and then
    /// hearing the same ambiguous mention again produces a *new* id, and
    /// matching on ids alone would ask the identical question every time.
    fn already_rejected(&self, a: StableId, b: StableId) -> bool {
        if self.rejected.contains(&(a.min(b), a.max(b))) {
            return true;
        }
        let (Some(ra), Some(rb)) = (self.identity.get(&a), self.identity.get(&b)) else {
            return false;
        };
        self.rejected.iter().any(|(x, y)| {
            let (Some(rx), Some(ry)) = (self.identity.get(x), self.identity.get(y)) else {
                return false;
            };
            (rx == ra && ry == rb) || (rx == rb && ry == ra)
        })
    }

    /// Answer a review with "yes, the same" and merge the two entities.
    ///
    /// The lower id survives, because it is the one other records are more
    /// likely to already reference, and `rm_store` promises ids are never
    /// reused — so the absorbed id stays recognisable as absorbed rather than
    /// missing.
    ///
    /// The absorbed entity's versions are re-appended under the survivor and
    /// then erased, which is the one call in `rm_store` that destroys history.
    /// Copy first, erase second: the copy is the half that can fail, and
    /// failing after the erase would lose the facts outright.
    ///
    /// Appending to the survivor's logs moves every absorbed assertion's
    /// position in them, so the lengths those logs had before the copy are read
    /// first and the assertions are renumbered against them afterwards — see
    /// [`Engine::adopt_assertions`].
    pub fn confirm(&mut self, review: ReviewId) -> Result<StableId, EngineError> {
        let pair = self
            .review
            .remove(&review)
            .ok_or(EngineError::UnknownReview(review))?;
        let (kept, absorbed) = (pair.a.min(pair.b), pair.a.max(pair.b));
        if kept == absorbed {
            return Ok(kept);
        }

        // How long each of the survivor's own logs is *before* anything is
        // appended to it. This is the whole of the renumbering argument, and it
        // is read from the store rather than counted off the assertion map on
        // purpose — see [`Engine::adopt_assertions`].
        let attributes: Vec<String> = self
            .store
            .entity(absorbed)
            .map(|e| e.attributes.keys().cloned().collect())
            .unwrap_or_default();
        let offsets: BTreeMap<String, usize> = attributes
            .iter()
            .map(|a| (a.clone(), self.store.history(kept, a).len()))
            .collect();

        // Move the store's versions across, preserving append order.
        for attribute in &attributes {
            let versions: Vec<_> = self.store.history(absorbed, attribute).to_vec();
            for v in versions {
                self.store
                    .assert(kept, attribute.clone(), v.value, v.valid, v.provenance)?;
            }
            self.store.erase(absorbed, attribute)?;
        }

        // Ownership and position move together, after the copy: the offsets
        // were read before it, so the assertion map is only touched once the
        // log it points into is in its final shape.
        self.adopt_assertions(absorbed, kept, &offsets);

        // Fold identity, and let the folded record say what the survivor is
        // keyed under.
        //
        // The absorbed id is struck from every key it stood under, and nothing
        // is pointed at the survivor that its own record cannot derive.
        // Re-pointing the absorbed record's keys instead — which is what this
        // did — re-broke the invariant `remember_identity` documents at length:
        // `blocks` is derived from `identity`, and `remember_identity` keeps
        // the first spelling of a field it already holds, so when both entities
        // carry a blocking field with different values nothing folds and the
        // survivor cannot derive the absorbed entity's key. The live map became
        // a strict superset of what `rebuild_blocks` produces, and the same
        // mention then resolved differently either side of a snapshot: no
        // error, no difference in the stored facts, only a merge that happens
        // or does not depending on when the process last restarted.
        if let Some(record) = self.identity.remove(&absorbed) {
            for ids in self.blocks.values_mut() {
                ids.retain(|&id| id != absorbed);
            }
            // A key nobody stands under any more goes, because `rebuild_blocks`
            // never creates an empty one and the two maps have to be equal, not
            // merely equivalent.
            self.blocks.retain(|_, ids| !ids.is_empty());
            // Re-keys the survivor from the folded record whenever the fold
            // learned a field; where it learned nothing the survivor's own keys
            // are already in place, because every writer keys the whole record.
            self.remember_identity(kept, &record);
        }

        // Any other open question naming the absorbed entity now names the
        // survivor; a question about the survivor and itself is settled. Two
        // questions that collapse onto the same pair become one, for the same
        // reason `rejected` exists at all: the same pair asked twice reads as
        // noise.
        let mut seen: BTreeSet<Settled> = BTreeSet::new();
        self.review.retain(|_, p| {
            if p.a == absorbed {
                p.a = kept;
            }
            if p.b == absorbed {
                p.b = kept;
            }
            if p.a == p.b {
                return false;
            }
            (p.a, p.b) = (p.a.min(p.b), p.a.max(p.b));
            seen.insert((p.a, p.b))
        });

        // A settled "different" survives the merge: if X is not the absorbed
        // entity, and the absorbed entity turns out to be the survivor, then X
        // is not the survivor either.
        self.rejected = self
            .rejected
            .iter()
            .filter_map(|&(x, y)| {
                let x = if x == absorbed { kept } else { x };
                let y = if y == absorbed { kept } else { y };
                (x != y).then_some((x.min(y), x.max(y)))
            })
            .collect();

        Ok(kept)
    }

    /// Re-point the absorbed entity's assertions at the survivor, renumbering
    /// each one's position in the version log it now reads from.
    ///
    /// `version` is a position in an attribute's version log, so re-appending
    /// the absorbed entity's versions under the survivor invalidates every
    /// position the absorbed entity's assertions held. Left stale, a search hit
    /// resolves to a real, well-formed, *wrong* fact: nothing errors and
    /// nothing downstream can tell.
    ///
    /// `offsets` is how long each of the survivor's own logs was when the copy
    /// started, read from the store before a single version moved. `rm_store`
    /// only ever appends, so the survivor's own positions do not move at all,
    /// and an absorbed version lands at exactly its old position plus that
    /// offset. Nothing else about either entity's history is consulted, which
    /// is the point.
    ///
    /// The rejected alternative was to renumber by sorting the survivor's
    /// assertions and handing out consecutive positions from zero — in effect
    /// deriving the log from the assertion map. It assumes a bijection between
    /// the two, and [`Engine::forget`] breaks that in both directions at once:
    /// it appends a tombstone with no assertion behind it, and it drops an
    /// attribute's assertions while their versions stay in the log. A survivor
    /// that had been forgotten then handed an absorbed assertion the position
    /// of its own *first* value, returned under the absorbed entity's
    /// provenance. Reading the log directly needs no such correspondence to
    /// hold, so there is no invariant left to break — which is why it is the
    /// version that shipped, rather than teaching `forget` to file an assertion
    /// for its tombstone. That would have restored the count while breaking a
    /// different rule: `Engine::open` requires every assertion to have a
    /// vector, and dropping the vector is the entire point of `forget`.
    fn adopt_assertions(
        &mut self,
        absorbed: StableId,
        kept: StableId,
        offsets: &BTreeMap<String, usize>,
    ) {
        for entry in self.assertions.values_mut() {
            if entry.entity != absorbed {
                continue;
            }
            entry.entity = kept;
            // An attribute with no absorbed versions had nothing appended for
            // it, so it shifts by nothing — and cannot be named by an absorbed
            // assertion in the first place, since that assertion would already
            // be pointing past the end of a log that does not exist.
            entry.version += offsets.get(&entry.attribute).copied().unwrap_or(0);
        }
    }

    /// Answer a review with "no, different", and do not ask again.
    ///
    /// Recorded as a pair rather than acted on: there is nothing to undo in the
    /// store, because the engine never merged them. All that happens is the
    /// question stops being open, and stops coming back.
    pub fn reject(&mut self, review: ReviewId) -> Result<(), EngineError> {
        let pair = self
            .review
            .remove(&review)
            .ok_or(EngineError::UnknownReview(review))?;
        self.rejected
            .insert((pair.a.min(pair.b), pair.a.max(pair.b)));
        Ok(())
    }

    /// Every pair still waiting on an answer, oldest first.
    ///
    /// Returned rather than logged because a review nobody can reach is the
    /// same as no review: the whole point of keeping the middle band is that
    /// someone gets asked.
    pub fn pending_review(&self) -> Vec<&PendingReview> {
        self.review.values().collect()
    }

    /// Create an entity and register its identity fields.
    fn create_entity(&mut self, obs: &Observation) -> StableId {
        let id = self
            .store
            .create_entity(&obs.kind, obs.provenance.observed_at);
        self.key_entity(id, &obs.mention);
        self.identity.insert(id, obs.mention.clone());
        id
    }

    /// Every blocking key a mention falls under.
    pub(crate) fn keys_for(&self, mention: &Record) -> Vec<String> {
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

    /// Stop recalling an attribute, without destroying what was true.
    ///
    /// Appends a tombstone valid from `at` and drops the attribute's vectors, so
    /// semantic recall goes quiet while `about` with an earlier `valid_t` still
    /// answers. This is "stop telling me this", and it is deliberately not the
    /// same operation as [`Engine::erase`] — collapsing the two would make it
    /// impossible to honour a deletion request and a preference with the same
    /// API while meaning different things by them.
    pub fn forget(
        &mut self,
        entity: StableId,
        attribute: &str,
        at: Timestamp,
        prov: Provenance,
    ) -> Result<(), EngineError> {
        self.store.assert(
            entity,
            attribute.to_string(),
            None,
            Interval::since(at),
            prov,
        )?;
        self.drop_vectors(entity, attribute);
        Ok(())
    }

    /// Destroy every version of an attribute and its vectors.
    ///
    /// Returns how many versions went. This punches a hole in the audit trail —
    /// see `rm_store::MemoryStore::erase`. Reach for it when someone has asked
    /// that a fact about them stop existing, and for nothing else: unlike
    /// [`Engine::forget`], there is no tombstone left behind and no way to
    /// answer `about` for a time before the erasure — the store no longer has
    /// an opinion, not even that something used to be true.
    ///
    /// `store.erase` runs first, and its count is kept before `drop_vectors`
    /// touches anything. Dropping vectors first would mean a failing erase
    /// (unknown entity) leaves the index already stripped while the store
    /// still holds the versions — a caller retrying with a valid id would then
    /// find the fact intact but unsearchable, with no error to explain why.
    /// Validating with the store first, exactly as `remember` validates the
    /// embedding before writing, keeps a rejected call free of side effects.
    pub fn erase(&mut self, entity: StableId, attribute: &str) -> Result<usize, EngineError> {
        let removed = self.store.erase(entity, attribute)?;
        self.drop_vectors(entity, attribute);
        Ok(removed)
    }

    /// Remove every indexed vector for one attribute of one entity.
    fn drop_vectors(&mut self, entity: StableId, attribute: &str) {
        let doomed: Vec<AssertionId> = self
            .assertions
            .iter()
            .filter(|(_, r)| r.entity == entity && r.attribute == attribute)
            .map(|(&id, _)| id)
            .collect();
        for id in doomed {
            self.index.remove(id);
            self.assertions.remove(&id);
        }
    }

    /// The raw version log, for callers that want the audit trail rather than
    /// an answer.
    pub fn store_history(&self, entity: StableId, attribute: &str) -> &[rm_store::Version] {
        self.store.history(entity, attribute)
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

    /// A near-miss name: close enough to be worth asking about, not close
    /// enough to merge.
    fn ambiguous() -> Observation {
        let mut obs = observation("Ben Severne", "employer", "Globex", 2);
        obs.mention = Record::new()
            .with("name", "Ben Severne")
            .with("city", "Bath");
        obs
    }

    #[test]
    fn a_review_pair_is_never_merged_silently() {
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let out = e.remember(ambiguous()).unwrap();

        let Remembered::CreatedPendingReview { review, .. } = out else {
            panic!("a middle-band score must not merge; got {out:?}");
        };
        assert_eq!(review.len(), 1);
        assert_eq!(e.entity_count(), 2, "kept apart until someone answers");
    }

    #[test]
    fn a_review_decision_still_remembers_the_fact() {
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let out = e.remember(ambiguous()).unwrap();
        let Remembered::CreatedPendingReview { assertion, .. } = out else {
            panic!("expected a review");
        };
        // Uncertainty is about whose fact it is, never about whether we heard it.
        assert!(e.assertion(assertion).is_some());
        assert_eq!(e.index_len(), 2);
    }

    #[test]
    fn confirming_a_review_merges_both_entities_assertions() {
        let mut e = engine();
        let first = e
            .remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let second = e.remember(ambiguous()).unwrap();
        let (
            Remembered::Created { entity: kept, .. },
            Remembered::CreatedPendingReview {
                entity: absorbed,
                assertion,
                review,
                ..
            },
        ) = (first, second)
        else {
            panic!("setup")
        };

        let survivor = e.confirm(review[0]).unwrap();
        assert_eq!(survivor, kept.min(absorbed));
        assert_eq!(e.entity_count(), 1);
        assert_eq!(
            e.assertion(assertion).unwrap().entity,
            survivor,
            "the absorbed entity's assertions must follow it"
        );
        assert!(e.pending_review().is_empty());
    }

    /// The value an assertion currently points at, read straight out of the
    /// store's version log the way `recall` will.
    fn value_of(e: &Engine, assertion: AssertionId) -> Option<String> {
        let r = e.assertion(assertion).expect("assertion exists");
        e.store.history(r.entity, &r.attribute)[r.version]
            .value
            .clone()
    }

    #[test]
    fn a_merged_assertion_still_resolves_to_its_own_value() {
        // The two entities' observations interleave on purpose: the survivor
        // has a fact either side of the absorbed entity's one. That is what
        // makes a merge invalidate version indices in both directions, and it
        // fails silently — every index still points at a real, plausible fact.
        let mut e = engine();
        let first = e
            .remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let second = e.remember(ambiguous()).unwrap();
        let third = e
            .remember(observation("Ben Severn", "employer", "Initech", 3))
            .unwrap();

        let (
            Remembered::Created {
                assertion: acme, ..
            },
            Remembered::CreatedPendingReview {
                assertion: globex,
                review,
                ..
            },
            Remembered::Merged {
                assertion: initech, ..
            },
        ) = (first, second, third)
        else {
            panic!("setup")
        };

        e.confirm(review[0]).unwrap();

        assert_eq!(value_of(&e, acme).as_deref(), Some("Acme"));
        assert_eq!(
            value_of(&e, globex).as_deref(),
            Some("Globex"),
            "an absorbed assertion must still name the fact it was made about"
        );
        assert_eq!(value_of(&e, initech).as_deref(), Some("Initech"));
    }

    #[test]
    fn a_second_merge_still_leaves_every_assertion_on_its_own_value() {
        // One merge is not enough to protect the renumbering. It leaves the
        // survivor holding assertions whose ids no longer run in version
        // order — here assertion 1 ends up at version 2 — so the *next* merge
        // has to shift an already-shifted assertion by the survivor's new log
        // length rather than its original one. A renumbering that reset
        // positions from zero, or that read a stale offset, only goes wrong on
        // the second merge.
        let mut e = engine();
        let first = e
            .remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let second = e.remember(ambiguous()).unwrap();
        let third = e
            .remember(observation("Ben Severn", "employer", "Initech", 3))
            .unwrap();

        let (
            Remembered::Created {
                assertion: acme, ..
            },
            Remembered::CreatedPendingReview {
                assertion: globex,
                review: first_review,
                ..
            },
            Remembered::Merged {
                assertion: initech, ..
            },
        ) = (first, second, third)
        else {
            panic!("setup")
        };
        e.confirm(first_review[0]).unwrap();

        // A third entity, near-missing the survivor the same way the second
        // did, and merged in turn.
        let mut later = ambiguous();
        later.value = Some("Umbrella".to_string());
        let fourth = e.remember(later).unwrap();
        let Remembered::CreatedPendingReview {
            assertion: umbrella,
            review: second_review,
            ..
        } = fourth
        else {
            panic!("setup: the third entity must arrive as a question, got {fourth:?}")
        };
        e.confirm(second_review[0]).unwrap();

        assert_eq!(e.entity_count(), 1);
        assert_eq!(value_of(&e, acme).as_deref(), Some("Acme"));
        assert_eq!(
            value_of(&e, globex).as_deref(),
            Some("Globex"),
            "an assertion absorbed by the first merge must survive the second"
        );
        assert_eq!(value_of(&e, initech).as_deref(), Some("Initech"));
        assert_eq!(value_of(&e, umbrella).as_deref(), Some("Umbrella"));
    }

    #[test]
    fn a_merge_renumbers_each_attribute_against_its_own_log() {
        // Two attributes whose logs are different lengths on the survivor. One
        // offset per entity rather than one per attribute would land the
        // absorbed nickname at the employer log's position — still a real
        // version, still a plausible string, and about something else.
        let mut e = engine();
        let Remembered::Created {
            entity: kept,
            assertion: acme,
        } = e
            .remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap()
        else {
            panic!("setup")
        };
        let Remembered::Merged {
            assertion: initech, ..
        } = e
            .remember(observation("Ben Severn", "employer", "Initech", 2))
            .unwrap()
        else {
            panic!("setup")
        };
        let Remembered::Merged {
            assertion: benny, ..
        } = e
            .remember(observation("Ben Severn", "nickname", "Benny", 3))
            .unwrap()
        else {
            panic!("setup")
        };

        let out = e.remember(ambiguous()).unwrap();
        let Remembered::CreatedPendingReview {
            assertion: globex,
            review,
            ..
        } = out
        else {
            panic!("setup: expected a review, got {out:?}")
        };
        let mut nickname = ambiguous();
        nickname.attribute = "nickname".to_string();
        nickname.value = Some("Bez".to_string());
        let out = e.remember(nickname).unwrap();
        let Remembered::Merged { assertion: bez, .. } = out else {
            panic!("setup: the repeated near-miss must land on its own entity, got {out:?}")
        };

        assert_eq!(e.confirm(review[0]).unwrap(), kept);

        assert_eq!(value_of(&e, acme).as_deref(), Some("Acme"));
        assert_eq!(value_of(&e, initech).as_deref(), Some("Initech"));
        assert_eq!(value_of(&e, benny).as_deref(), Some("Benny"));
        assert_eq!(
            value_of(&e, globex).as_deref(),
            Some("Globex"),
            "the absorbed employer shifts by the survivor's two employer versions"
        );
        assert_eq!(
            value_of(&e, bez).as_deref(),
            Some("Bez"),
            "and the absorbed nickname by its one nickname version, not by two"
        );
    }

    #[test]
    fn a_merge_after_a_forget_still_leaves_every_assertion_on_its_own_value() {
        // `forget` breaks any correspondence between the assertion map and the
        // version log in both directions at once: it appends a tombstone the
        // index never sees, and it drops the attribute's existing assertions
        // while their versions stay in the log. The survivor here ends up with
        // two versions and no assertions at all, so a renumbering that counts
        // assertions rather than reading the log hands the absorbed entity's
        // assertion version 0 — the survivor's *first* value, returned under
        // the absorbed entity's provenance. Well-formed, confidently wrong,
        // and nothing errors.
        let mut e = engine();
        let Remembered::Created { entity: kept, .. } = e
            .remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap()
        else {
            panic!("setup")
        };
        e.forget(
            kept,
            "employer",
            2,
            Provenance::new(Source::UserAssertion, 2, "s2"),
        )
        .unwrap();
        assert_eq!(
            e.store_history(kept, "employer").len(),
            2,
            "setup: two versions, and forget left neither of them an assertion"
        );

        let out = e.remember(ambiguous()).unwrap();
        let Remembered::CreatedPendingReview {
            entity: absorbed,
            assertion: globex,
            review,
        } = out
        else {
            panic!("setup: expected a review, got {out:?}")
        };
        assert_eq!(e.confirm(review[0]).unwrap(), kept.min(absorbed));

        assert_eq!(
            value_of(&e, globex).as_deref(),
            Some("Globex"),
            "an absorbed assertion must name its own fact even where the \
             survivor's log holds versions no assertion points at"
        );
        // And through the door a caller actually uses.
        let hits = e.recall(&Query::new(vec![1.0, 0.0, 0.0], 5)).unwrap();
        assert_eq!(hits.len(), 1, "forget took the survivor's vector with it");
        assert_eq!(hits[0].value.as_deref(), Some("Globex"));
    }

    #[test]
    fn rejecting_a_review_stops_it_being_raised_again() {
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let out = e.remember(ambiguous()).unwrap();
        let Remembered::CreatedPendingReview { review, .. } = out else {
            panic!("setup")
        };

        e.reject(review[0]).unwrap();
        assert!(e.pending_review().is_empty());
        assert_eq!(e.entity_count(), 2, "rejection keeps them apart");

        // The same ambiguous mention again must not re-raise the settled pair.
        e.remember(ambiguous()).unwrap();
        assert!(
            e.pending_review().is_empty(),
            "an answered question must not be asked twice"
        );
    }

    /// A ruleset where even perfect agreement falls short of `match_at`: one
    /// weak field against a high bar, so the strongest evidence available is
    /// still only a question. Contrived, and it is the shape under which
    /// suppression has to work — every repeat mention files a review, so a
    /// rejection that only matched on ids would be asked again forever.
    fn never_confident_ruleset() -> Ruleset {
        Ruleset::new(
            vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
            vec![BlockingKey::Prefix("name".to_string(), 3)],
            4.0,
            20.0,
        )
        .unwrap()
    }

    #[test]
    fn a_mention_matching_a_rejected_pair_is_not_asked_about_under_a_new_id() {
        let mut e = Engine::new(
            VectorIndex::new(3, Metric::Cosine),
            never_confident_ruleset(),
            Policy::new(Strategy::MostRecent),
        );
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let out = e
            .remember(observation("Ben Severn", "employer", "Acme", 2))
            .unwrap();
        let Remembered::CreatedPendingReview { review, .. } = out else {
            panic!("setup: expected a review, got {out:?}")
        };
        e.reject(review[0]).unwrap();

        // A third identical mention gets a third id, so the rejected pair of
        // ids says nothing about it. What settles it is that the records are
        // the ones already answered "different".
        let out = e
            .remember(observation("Ben Severn", "employer", "Acme", 3))
            .unwrap();
        assert!(matches!(out, Remembered::Created { .. }), "got {out:?}");
        assert!(
            e.pending_review().is_empty(),
            "a new id for an already-answered pair must not reopen it"
        );
    }

    #[test]
    fn answering_a_review_that_does_not_exist_is_an_error() {
        let mut e = engine();
        assert_eq!(e.confirm(42), Err(EngineError::UnknownReview(42)));
        assert_eq!(e.reject(42), Err(EngineError::UnknownReview(42)));
    }

    /// A ruleset that also blocks on `email` — a field `test_ruleset`'s records
    /// never carry. Separate rather than an extra key on the shared one,
    /// because the shared ruleset's thresholds are what put "Ben Severn" and
    /// "Ben Severne" in the review band, and every other test here depends on
    /// that placement staying exactly where it is.
    fn email_blocking_ruleset() -> Ruleset {
        Ruleset::new(
            vec![
                FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01),
                FieldRule::new("email", Comparator::Normalized, 0.95, 0.001),
            ],
            vec![
                BlockingKey::Prefix("name".to_string(), 3),
                BlockingKey::Exact("email".to_string()),
            ],
            2.0,
            5.0,
        )
        .unwrap()
    }

    #[test]
    fn an_entity_gaining_a_blocking_field_later_is_still_found_by_it() {
        let mut e = Engine::new(
            VectorIndex::new(3, Metric::Cosine),
            email_blocking_ruleset(),
            Policy::new(Strategy::MostRecent),
        );

        let mut first = observation("Ben Severn", "employer", "Acme", 1);
        first.mention = Record::new().with("name", "Ben Severn");
        let Remembered::Created { entity, .. } = e.remember(first).unwrap() else {
            panic!("setup")
        };

        // The email arrives on a later observation, so it was not in the
        // blocking map when the entity was created.
        let mut second = observation("Ben Severn", "city", "Bristol", 2);
        second.mention = Record::new()
            .with("name", "Ben Severn")
            .with("email", "ben@example.com");
        let out = e.remember(second).unwrap();
        assert!(matches!(out, Remembered::Merged { .. }), "got {out:?}");

        let by_email = Record::new().with("email", "ben@example.com");
        assert!(
            e.candidates(&by_email).contains(&entity),
            "a blocking key learned on a later observation must still reach the entity"
        );
    }

    #[test]
    fn a_confirmed_merge_leaves_the_blocking_map_a_reload_would_rebuild() {
        // `blocks` is derived from `identity`, and `remember_identity` keeps
        // the first spelling of a field it already holds — so when both
        // entities carry a blocking field with different values, nothing folds
        // and the survivor's record cannot derive the absorbed entity's key.
        // Re-pointing the absorbed record's keys at the survivor therefore
        // leaves the live map holding a key `rebuild_blocks` would never
        // produce, and the same mention resolves differently either side of a
        // snapshot. `test_ruleset`'s single `Prefix("name", 3)` cannot show
        // this: both spellings derive the same key, so nothing diverges.
        let mut e = Engine::new(
            VectorIndex::new(3, Metric::Cosine),
            email_blocking_ruleset(),
            Policy::new(Strategy::MostRecent),
        );
        let mut first = observation("Ben Severn", "employer", "Acme", 1);
        first.mention = Record::new()
            .with("name", "Ben Severn")
            .with("email", "ben@example.com");
        e.remember(first).unwrap();

        let mut second = observation("Ben Severn", "employer", "Globex", 2);
        second.mention = Record::new()
            .with("name", "Ben Severn")
            .with("email", "b.severn@example.com");
        let out = e.remember(second).unwrap();
        let Remembered::CreatedPendingReview { review, .. } = out else {
            panic!("setup: two emails on one name is a question, got {out:?}")
        };
        e.confirm(review[0]).unwrap();

        let restored = Engine::open(
            &e.snapshot(),
            email_blocking_ruleset(),
            Policy::new(Strategy::MostRecent),
        )
        .unwrap();
        assert_eq!(
            e.blocks, restored.blocks,
            "a live blocking map that outruns the one a reload rebuilds is a \
             merge that happens or does not depending on when the process last \
             restarted"
        );

        // The same statement as something a caller can observe.
        let absorbed_email = Record::new().with("email", "b.severn@example.com");
        assert_eq!(
            e.candidates(&absorbed_email),
            restored.candidates(&absorbed_email)
        );
    }

    #[test]
    fn open_questions_are_reported_deterministically() {
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        e.remember(ambiguous()).unwrap();
        let open = e.pending_review();
        assert_eq!(open.len(), 1);
        assert!(
            open[0].score > 0.0,
            "a review pair has real evidence behind it"
        );
    }

    #[test]
    fn changing_the_policy_changes_the_answer_without_rewriting_history() {
        // The same two facts, read two ways. This is the thesis: resolution is
        // a query, so the store never had to pick.
        let mut e = Engine::new(
            VectorIndex::new(3, Metric::Cosine),
            test_ruleset(),
            Policy::new(Strategy::MostRecent),
        );
        let out = e
            .remember(observation("Ben Severn", "employer", "Acme", 10))
            .unwrap();
        let Remembered::Created { entity, .. } = out else {
            panic!("setup")
        };
        e.remember(observation("Ben Severn", "employer", "Globex", 20))
            .unwrap();

        // MostRecent: one winner, at every instant.
        assert_eq!(
            e.about(entity, "employer", 15, 100).unwrap(),
            Believed::Value("Globex".into())
        );

        // ValidInterval: both survive, and the answer depends on when you ask.
        let e = e.with_policy(Policy::new(Strategy::ValidInterval));
        assert_eq!(
            e.about(entity, "employer", 15, 100).unwrap(),
            Believed::Value("Acme".into())
        );
        assert_eq!(
            e.about(entity, "employer", 25, 100).unwrap(),
            Believed::Value("Globex".into())
        );
    }

    #[test]
    fn an_attribute_never_discussed_is_unknown_not_absent() {
        let mut e = engine();
        let out = e
            .remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let Remembered::Created { entity, .. } = out else {
            panic!("setup")
        };
        assert_eq!(e.about(entity, "spouse", 5, 5).unwrap(), Believed::Unknown);
    }

    #[test]
    fn a_tombstone_reads_as_absent_not_unknown() {
        let mut e = engine();
        let mut obs = observation("Ben Severn", "employer", "Acme", 1);
        let Remembered::Created { entity, .. } = e.remember(obs.clone()).unwrap() else {
            panic!("setup")
        };
        obs.value = None;
        obs.provenance = Provenance::new(Source::UserAssertion, 5, "session-2");
        obs.valid = Interval::since(5);
        e.remember(obs).unwrap();
        assert_eq!(
            e.about(entity, "employer", 10, 10).unwrap(),
            Believed::Absent
        );
    }

    #[test]
    fn asking_about_an_unknown_entity_is_unknown_not_an_error() {
        let e = engine();
        assert_eq!(e.about(9999, "employer", 1, 1).unwrap(), Believed::Unknown);
    }

    #[test]
    fn a_refusal_propagates_instead_of_falling_back_to_a_looser_strategy() {
        // Two different values at the same instant: MostRecent has no answer,
        // and a memory chosen by a rule the caller did not ask for is exactly
        // the plausible wrong answer the refusals exist to prevent.
        let mut e = engine();
        let Remembered::Created { entity, .. } = e
            .remember(observation("Ben Severn", "employer", "Acme", 7))
            .unwrap()
        else {
            panic!("setup")
        };
        let mut same_instant = observation("Ben Severn", "employer", "Globex", 7);
        same_instant.embedding = vec![0.0, 1.0, 0.0];
        e.remember(same_instant).unwrap();

        assert!(matches!(
            e.about(entity, "employer", 10, 10),
            Err(EngineError::Refused(_))
        ));
    }

    /// An observation carrying a caller-chosen embedding, for exercising
    /// `recall`'s geometry directly instead of through whatever
    /// `observation()`'s fixed vector happens to produce.
    fn embedded(
        name: &str,
        attribute: &str,
        value: &str,
        at: Timestamp,
        v: [f32; 3],
    ) -> Observation {
        let mut obs = observation(name, attribute, value, at);
        obs.embedding = v.to_vec();
        obs
    }

    #[test]
    fn recall_returns_the_nearest_assertions_with_their_provenance() {
        let mut e = engine();
        e.remember(embedded(
            "Ben Severn",
            "employer",
            "Acme",
            1,
            [1.0, 0.0, 0.0],
        ))
        .unwrap();
        e.remember(embedded(
            "Wei Zhang",
            "employer",
            "Globex",
            2,
            [0.0, 1.0, 0.0],
        ))
        .unwrap();

        let hits = e.recall(&Query::new(vec![1.0, 0.0, 0.0], 1)).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].value.as_deref(), Some("Acme"));
        assert_eq!(hits[0].provenance.source_ref, "session-1");
    }

    #[test]
    fn filtering_by_session_happens_during_the_scan() {
        // Ten better-scoring assertions from another session must not crowd out
        // the one that matches: post-filtering a top-k would return nothing.
        let mut e = engine();
        for i in 0..10 {
            let mut obs = embedded("Person A", "note", "other", i + 1, [1.0, 0.0, 0.0]);
            obs.provenance = Provenance::new(Source::UserAssertion, i + 1, "session-other");
            e.remember(obs).unwrap();
        }
        let mut wanted = embedded("Person B", "note", "wanted", 20, [0.6, 0.8, 0.0]);
        wanted.provenance = Provenance::new(Source::UserAssertion, 20, "session-mine");
        e.remember(wanted).unwrap();

        let q = Query::new(vec![1.0, 0.0, 0.0], 5).in_session("session-mine");
        let hits = e.recall(&q).unwrap();
        assert_eq!(hits.len(), 1, "post-filtering would have returned 0");
        assert_eq!(hits[0].value.as_deref(), Some("wanted"));
    }

    #[test]
    fn a_superseded_fact_is_returned_marked_not_dropped() {
        // "What did I believe about her employer in May" needs the old fact, and
        // a caller stating it as current needs to be stopped from doing so.
        let mut e = engine();
        e.remember(embedded(
            "Ben Severn",
            "employer",
            "Acme",
            10,
            [1.0, 0.0, 0.0],
        ))
        .unwrap();
        e.remember(embedded(
            "Ben Severn",
            "employer",
            "Globex",
            20,
            [0.9, 0.1, 0.0],
        ))
        .unwrap();

        let hits = e.recall(&Query::new(vec![1.0, 0.0, 0.0], 2)).unwrap();
        assert_eq!(hits.len(), 2);
        let acme = hits
            .iter()
            .find(|h| h.value.as_deref() == Some("Acme"))
            .unwrap();
        assert!(acme.superseded, "an old fact must be returned marked");
        let globex = hits
            .iter()
            .find(|h| h.value.as_deref() == Some("Globex"))
            .unwrap();
        assert!(!globex.superseded);
    }

    #[test]
    fn recall_as_of_a_past_tx_time_does_not_see_later_knowledge() {
        let mut e = engine();
        e.remember(embedded(
            "Ben Severn",
            "employer",
            "Acme",
            10,
            [1.0, 0.0, 0.0],
        ))
        .unwrap();
        e.remember(embedded(
            "Ben Severn",
            "employer",
            "Globex",
            20,
            [0.9, 0.1, 0.0],
        ))
        .unwrap();

        let q = Query::new(vec![1.0, 0.0, 0.0], 5).as_of(15, 15);
        let hits = e.recall(&q).unwrap();
        assert_eq!(hits.len(), 1, "September's news is not August's knowledge");
        assert_eq!(hits[0].value.as_deref(), Some("Acme"));
        assert!(
            !hits[0].superseded,
            "Globex was learned after the horizon, so it must not count as \
             superseding Acme — only later knowledge the horizon has already \
             seen can do that"
        );
    }

    #[test]
    fn a_query_whose_vector_is_rejected_reports_it() {
        let e = engine();
        assert!(e.recall(&Query::new(vec![1.0, 0.0], 1)).is_err());
    }

    #[test]
    fn forget_stops_recall_but_leaves_history_answerable() {
        // `ValidInterval`, not `engine()`'s default `MostRecent`: "what was
        // true in May" is a question about a point in time, and `MostRecent`
        // answers with the single latest winner at *every* instant (see
        // `changing_the_policy_changes_the_answer_without_rewriting_history`).
        // Only `ValidInterval` produces a timeline `about` can read a `valid_t`
        // back out of.
        let mut e = engine().with_policy(Policy::new(Strategy::ValidInterval));
        let Remembered::Created { entity, .. } = e
            .remember(embedded(
                "Ben Severn",
                "employer",
                "Acme",
                10,
                [1.0, 0.0, 0.0],
            ))
            .unwrap()
        else {
            panic!("setup")
        };

        e.forget(
            entity,
            "employer",
            20,
            Provenance::new(Source::UserAssertion, 20, "s2"),
        )
        .unwrap();

        // Nothing surfaces semantically any more.
        assert!(e
            .recall(&Query::new(vec![1.0, 0.0, 0.0], 5))
            .unwrap()
            .is_empty());
        // But it was true in May, and that stays reconstructible.
        assert_eq!(
            e.about(entity, "employer", 15, 30).unwrap(),
            Believed::Value("Acme".into())
        );
        // And now it is asserted to be nothing.
        assert_eq!(
            e.about(entity, "employer", 25, 30).unwrap(),
            Believed::Absent
        );
    }

    #[test]
    fn forget_is_itself_a_fact_with_provenance() {
        let mut e = engine();
        let Remembered::Created { entity, .. } = e
            .remember(embedded(
                "Ben Severn",
                "employer",
                "Acme",
                10,
                [1.0, 0.0, 0.0],
            ))
            .unwrap()
        else {
            panic!("setup")
        };
        e.forget(
            entity,
            "employer",
            20,
            Provenance::new(Source::UserAssertion, 20, "s2"),
        )
        .unwrap();

        let history = e.store_history(entity, "employer");
        assert_eq!(history.len(), 2);
        assert_eq!(
            history[1].provenance.source_ref, "s2",
            "who asked is recorded"
        );
    }

    #[test]
    fn erase_removes_it_from_history_too() {
        let mut e = engine();
        let Remembered::Created { entity, .. } = e
            .remember(embedded(
                "Ben Severn",
                "employer",
                "Acme",
                10,
                [1.0, 0.0, 0.0],
            ))
            .unwrap()
        else {
            panic!("setup")
        };

        assert_eq!(e.erase(entity, "employer").unwrap(), 1);
        assert_eq!(
            e.about(entity, "employer", 15, 30).unwrap(),
            Believed::Unknown
        );
        assert!(e
            .recall(&Query::new(vec![1.0, 0.0, 0.0], 5))
            .unwrap()
            .is_empty());
        assert_eq!(e.index_len(), 0, "the vector goes with the fact");
    }

    #[test]
    fn forgetting_on_an_unknown_entity_is_an_error() {
        let mut e = engine();
        let p = Provenance::new(Source::UserAssertion, 1, "s");
        assert!(matches!(
            e.forget(9999, "employer", 1, p),
            Err(EngineError::Store(_)) | Err(EngineError::UnknownEntity(_))
        ));
    }

    #[test]
    fn erasing_an_unknown_entity_is_an_error_and_leaves_an_unrelated_entity_alone() {
        // Note what this does *not* prove: an id the store does not hold is,
        // by construction, an id `self.assertions` has no entries for either
        // (every entry is written by `write`, which requires the store to
        // already know the entity). So `drop_vectors` is a structural no-op
        // on this path regardless of which statement in `erase` runs first.
        // This test only proves the call errors without disturbing an
        // unrelated, valid entity. The ordering itself is exercised by
        // `a_failing_erase_runs_store_first_so_drop_vectors_never_executes`.
        let mut e = engine();
        let Remembered::Created { entity, .. } = e
            .remember(embedded(
                "Ben Severn",
                "employer",
                "Acme",
                10,
                [1.0, 0.0, 0.0],
            ))
            .unwrap()
        else {
            panic!("setup")
        };

        assert!(matches!(
            e.erase(9999, "employer"),
            Err(EngineError::Store(_)) | Err(EngineError::UnknownEntity(_))
        ));

        // The unrelated entity's vector and history are untouched by the
        // failed call.
        assert_eq!(e.index_len(), 1);
        assert_eq!(
            e.about(entity, "employer", 15, 30).unwrap(),
            Believed::Value("Acme".into())
        );
    }

    #[test]
    fn a_failing_erase_runs_store_first_so_drop_vectors_never_executes() {
        // `store.erase`'s only failure is `UnknownEntity`, and reaching that
        // id through `remember`/`erase` alone can never leave it holding
        // assertions — so the ordering guarantee is unobservable through the
        // public API in the ordinary case (see the test above). To actually
        // discriminate the two statements' order, manufacture the one state
        // the public API cannot produce on its own: an assertion, and its
        // indexed vector, recorded against an id the store does not hold.
        // Reaching into `assertions`/`index` directly is legitimate here
        // precisely because it is the only way to make the two orderings
        // diverge — everything else about this scenario is unreachable
        // through `remember`.
        //
        // If `drop_vectors` ran before `store.erase`, this entry would
        // already be gone by the time the call returns its error. With the
        // implemented ordering — `store.erase` first — it survives, because
        // the `?` on the unknown-entity error returns before `drop_vectors`
        // is ever called.
        let mut e = engine();
        e.assertions.insert(
            0,
            AssertionRef {
                entity: 9999,
                attribute: "employer".to_string(),
                version: 0,
            },
        );
        e.index.insert(0, &[1.0, 0.0, 0.0]).unwrap();

        assert!(matches!(
            e.erase(9999, "employer"),
            Err(EngineError::Store(_)) | Err(EngineError::UnknownEntity(_))
        ));

        assert!(
            e.assertions.contains_key(&0),
            "drop_vectors must not run before store.erase has a chance to fail"
        );
        assert_eq!(e.index_len(), 1);
    }

    #[test]
    fn a_snapshot_round_trips_including_the_review_queue() {
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        e.remember(ambiguous()).unwrap();

        let restored = Engine::open(
            &e.snapshot(),
            test_ruleset(),
            Policy::new(Strategy::MostRecent),
        )
        .unwrap();
        assert_eq!(restored.entity_count(), e.entity_count());
        assert_eq!(
            restored.pending_review().len(),
            1,
            "an open question that does not survive a restart is an answered one"
        );
        assert_eq!(restored.index_len(), e.index_len());
        // Counting rows is not the same as being searchable: the index's
        // id-to-row map is derived and rebuilt on open, and a restore that
        // skipped rebuilding it would still report the right length while
        // answering nothing.
        assert_eq!(
            restored
                .recall(&Query::new(vec![1.0, 0.0, 0.0], 5))
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn a_restored_engine_still_resolves_against_what_it_knew() {
        // The blocking map is rebuilt, not persisted -- it has to actually work.
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let mut restored = Engine::open(
            &e.snapshot(),
            test_ruleset(),
            Policy::new(Strategy::MostRecent),
        )
        .unwrap();

        let out = restored
            .remember(observation("Ben Severn", "city", "Bristol", 2))
            .unwrap();
        assert!(matches!(out, Remembered::Merged { .. }), "got {out:?}");
        assert_eq!(restored.entity_count(), 1);
    }

    #[test]
    fn snapshots_are_byte_stable() {
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let once = e.snapshot();
        let twice = Engine::open(&once, test_ruleset(), Policy::new(Strategy::MostRecent))
            .unwrap()
            .snapshot();
        assert_eq!(once, twice);
    }

    #[test]
    fn a_confirmed_merge_survives_a_snapshot_round_trip() {
        // `confirm` copies the absorbed entity's versions onto the survivor and
        // then erases them, and `rm_store::MemoryStore::erase` removes the
        // attribute, not the entity. So the absorbed entity is still in the
        // store afterwards, holding nothing; only `identity` records that it
        // stopped being someone. An `open` that rebuilt the entity set from the
        // store's entity list would hand it back on reload, and the merge would
        // undo itself across a restart with nothing anywhere to say so.
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let out = e.remember(ambiguous()).unwrap();
        let Remembered::CreatedPendingReview {
            entity: absorbed,
            assertion: globex,
            review,
        } = out
        else {
            panic!("setup: expected a review, got {out:?}");
        };
        let survivor = e.confirm(review[0]).unwrap();
        assert_ne!(survivor, absorbed, "setup: the merge has to move something");
        assert!(
            e.store.entity(absorbed).is_some(),
            "setup: the absorbed entity is still in the store, holding nothing --              that is the trap this test exists for"
        );
        assert_eq!(e.entity_count(), 1, "setup: identity is what dropped it");

        let restored = Engine::open(
            &e.snapshot(),
            test_ruleset(),
            Policy::new(Strategy::MostRecent),
        )
        .unwrap();

        assert_eq!(
            restored.entity_count(),
            1,
            "a merged-away entity must not come back on reload"
        );
        assert_eq!(
            restored.assertion(globex).unwrap().entity,
            survivor,
            "the absorbed entity's assertions still belong to the survivor"
        );
        assert_eq!(
            value_of(&restored, globex).as_deref(),
            Some("Globex"),
            "and still name the fact they were made about"
        );
        assert!(restored.pending_review().is_empty());
    }

    #[test]
    fn a_restored_engine_reads_under_the_policy_it_was_handed_not_a_stored_one() {
        // The policy is configuration, not state, and is deliberately absent
        // from the snapshot. If a copy ever crept in, the caller's argument
        // would be silently ignored and the only evidence would be an answer
        // that is plausible and wrong.
        let mut e = engine();
        let out = e
            .remember(observation("Ben Severn", "employer", "Acme", 10))
            .unwrap();
        let Remembered::Created { entity, .. } = out else {
            panic!("setup")
        };
        e.remember(observation("Ben Severn", "employer", "Globex", 20))
            .unwrap();
        assert_eq!(
            e.about(entity, "employer", 15, 100).unwrap(),
            Believed::Value("Globex".into()),
            "setup: MostRecent picks one winner at every instant"
        );

        let restored = Engine::open(
            &e.snapshot(),
            test_ruleset(),
            Policy::new(Strategy::ValidInterval),
        )
        .unwrap();
        assert_eq!(
            restored.about(entity, "employer", 15, 100).unwrap(),
            Believed::Value("Acme".into()),
            "the strategy in force is the one open() was given"
        );
    }

    #[test]
    fn a_snapshot_whose_index_and_store_disagree_is_rejected_not_panicked_on() {
        // An assertion naming an entity the store does not hold. Parses fine,
        // and every read of it would panic or lie. The nested `store` and
        // `index` are each that crate's own snapshot carried as a string, so
        // they appear here escaped -- see `Persisted` for why neither is a
        // nested object. Both are individually valid, so the only thing wrong
        // with this snapshot is the disagreement between them.
        let broken = r#"{"store":"{\"entities\":{},\"next_id\":0}",
            "index":"{\"dim\":3,\"metric\":\"Cosine\",\"ids\":[0],\"vectors\":[1.0,0.0,0.0]}",
            "assertions":{"0":{"entity":7,"attribute":"employer","version":0}},
            "review":{},"identity":{},"rejected":[],"next_assertion":1,"next_review":0}"#;
        // A `let Err(..) else` rather than `unwrap_err`, which would want
        // `Engine: Debug`. Adding a derive to a public type to satisfy a test
        // is the wrong way round.
        let Err(err) = Engine::open(broken, test_ruleset(), Policy::new(Strategy::MostRecent))
        else {
            panic!("a snapshot describing an impossible engine must not open");
        };
        assert!(
            matches!(err, EngineError::CorruptSnapshot(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn an_assertion_belonging_to_no_known_identity_is_rejected_not_restored() {
        // Everything else about this snapshot is valid: the store holds the
        // entity, the version index is in range, the vector is there. Only
        // `identity` -- the engine's actual entity set -- has no record of it,
        // which is precisely the state an entity set rebuilt from the store
        // would wave through, and the fact would then be recalled while being
        // uncounted and unresolvable-against forever.
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let mut doc: serde_json::Value = serde_json::from_str(&e.snapshot()).unwrap();
        doc["identity"] = serde_json::json!({});

        let Err(err) = Engine::open(
            &doc.to_string(),
            test_ruleset(),
            Policy::new(Strategy::MostRecent),
        ) else {
            panic!("an assertion with no identity behind it must not open");
        };
        assert!(
            matches!(err, EngineError::CorruptSnapshot(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn a_vector_no_assertion_claims_is_rejected_not_restored() {
        // The mirror of `an_assertion_belonging_to_no_known_identity...`: every
        // assertion here has its vector, and the index has one more besides.
        // Nothing about it is malformed, so `VectorIndex::open` waves it
        // through, and nothing downstream reports it either — the scan visits
        // it, `recall` drops the hit it cannot resolve, and `index_len` keeps
        // counting it as searchable.
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let mut doc: serde_json::Value = serde_json::from_str(&e.snapshot()).unwrap();
        let mut index: serde_json::Value =
            serde_json::from_str(doc["index"].as_str().unwrap()).unwrap();
        index["ids"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!(1));
        index["vectors"]
            .as_array_mut()
            .unwrap()
            .extend([serde_json::json!(0.0), serde_json::json!(1.0)]);
        index["vectors"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!(0.0));
        doc["index"] = serde_json::json!(index.to_string());

        let Err(err) = Engine::open(
            &doc.to_string(),
            test_ruleset(),
            Policy::new(Strategy::MostRecent),
        ) else {
            panic!("an orphan vector must not open");
        };
        assert!(
            matches!(err, EngineError::CorruptSnapshot(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn an_identity_the_store_never_heard_of_is_rejected_not_restored() {
        // The symmetric case to an assertion naming a missing entity, and the
        // one that used to get through. It is not inert: `rebuild_blocks` keys
        // it, so it stands in front of every future mention, and the `Match` it
        // eventually wins sends `remember` into `store.assert` with an id the
        // store rejects. The caller sees `UnknownEntity` naming an entity the
        // engine had just resolved against.
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let mut doc: serde_json::Value = serde_json::from_str(&e.snapshot()).unwrap();
        doc["identity"]["99"] = serde_json::json!({"fields": {"name": "Ben Severn"}});

        let Err(err) = Engine::open(
            &doc.to_string(),
            test_ruleset(),
            Policy::new(Strategy::MostRecent),
        ) else {
            panic!("an identity the store does not hold must not open");
        };
        assert!(
            matches!(err, EngineError::CorruptSnapshot(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn a_corrupt_nested_index_is_rejected_by_its_own_door() {
        // One id at three dimensions needs three floats. `rm_index` already
        // rejects this and already has tests for it; restoring through
        // `VectorIndex::open` rather than a derived `Deserialize` is what makes
        // that work count here instead of being written a second time, weaker.
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let mut doc: serde_json::Value = serde_json::from_str(&e.snapshot()).unwrap();
        doc["index"] =
            serde_json::json!(r#"{"dim":3,"metric":"Cosine","ids":[0],"vectors":[1.0,0.0]}"#);

        let Err(err) = Engine::open(
            &doc.to_string(),
            test_ruleset(),
            Policy::new(Strategy::MostRecent),
        ) else {
            panic!("a snapshot whose index cannot be rebuilt must not open");
        };
        assert!(
            matches!(err, EngineError::Index(IndexError::CorruptSnapshot(_))),
            "got {err:?}"
        );
    }

    #[test]
    fn a_snapshot_whose_assertion_counter_was_rewound_is_rejected_not_restored() {
        // Every other check passes: the store holds the entity, identity knows
        // it, the version indices are in range, every assertion has its vector.
        // Only the counter lies, and it lies about the *next* write rather than
        // about anything already stored -- so the damage lands after `open`
        // returns Ok, on a `remember` that itself returns Ok. `assertions`
        // takes the overwrite, `VectorIndex::insert` overwrites the vector of
        // an id it already holds in place, `index_len()` does not move, and a
        // fact is gone with nothing anywhere reporting it.
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        e.remember(observation("Ben Severn", "city", "Bristol", 2))
            .unwrap();
        let mut doc: serde_json::Value = serde_json::from_str(&e.snapshot()).unwrap();
        assert_eq!(
            doc["next_assertion"], 2,
            "setup: two assertions were written"
        );
        // Rewound to the id of a live assertion, not below it: the boundary is
        // where the counter names an id already in use, so `<=` is the test.
        doc["next_assertion"] = serde_json::json!(1);

        let Err(err) = Engine::open(
            &doc.to_string(),
            test_ruleset(),
            Policy::new(Strategy::MostRecent),
        ) else {
            panic!("a counter that would reissue a live assertion id must not open");
        };
        assert!(
            matches!(err, EngineError::CorruptSnapshot(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn a_snapshot_whose_review_counter_was_rewound_is_rejected_not_restored() {
        // The same shape one queue over. A reissued review id overwrites an
        // open question -- one someone was going to be asked and now never
        // will be -- and `file_review` cannot notice, because inserting over a
        // `BTreeMap` key is not an error anywhere.
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        e.remember(ambiguous()).unwrap();
        let mut doc: serde_json::Value = serde_json::from_str(&e.snapshot()).unwrap();
        assert_eq!(doc["next_review"], 1, "setup: one question was filed");
        doc["next_review"] = serde_json::json!(0);

        let Err(err) = Engine::open(
            &doc.to_string(),
            test_ruleset(),
            Policy::new(Strategy::MostRecent),
        ) else {
            panic!("a counter that would reissue an open review id must not open");
        };
        assert!(
            matches!(err, EngineError::CorruptSnapshot(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn a_snapshot_whose_store_counter_was_rewound_is_rejected_by_the_stores_own_door() {
        // The third counter of the same shape, and the one this crate cannot
        // check itself. It is caught because the store is restored through
        // `MemoryStore::open` rather than by derived `Deserialize` -- the same
        // reason the index's own invariants are enforced here without this
        // crate restating them.
        let mut e = engine();
        e.remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        let mut doc: serde_json::Value = serde_json::from_str(&e.snapshot()).unwrap();

        // The store is nested as its own snapshot string, so reach through it
        // and put it back the same way.
        let mut store: serde_json::Value =
            serde_json::from_str(doc["store"].as_str().unwrap()).unwrap();
        assert_eq!(store["next_id"], 1, "setup: one entity was created");
        store["next_id"] = serde_json::json!(0);
        doc["store"] = serde_json::json!(store.to_string());

        let Err(err) = Engine::open(
            &doc.to_string(),
            test_ruleset(),
            Policy::new(Strategy::MostRecent),
        ) else {
            panic!("a counter that would reissue a live entity id must not open");
        };
        assert!(
            matches!(err, EngineError::Store(StoreError::CorruptSnapshot(_))),
            "got {err:?}"
        );
    }

    #[test]
    fn a_malformed_snapshot_is_reported_not_panicked_on() {
        assert!(Engine::open(
            "{ not json",
            test_ruleset(),
            Policy::new(Strategy::MostRecent)
        )
        .is_err());
    }
}
