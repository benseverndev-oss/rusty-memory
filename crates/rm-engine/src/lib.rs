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

mod ingest;
mod persist;
mod policy;
mod read;
mod review;

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use rm_store::MemoryStore;
use serde::{Deserialize, Serialize};

pub use ingest::{prepare, Closed, Embedder, EmbedderError, Ingested, Prepared};
pub use policy::Policy;
pub use read::{Believed, Located, Query, Recalled, Standing, Traced};
pub use review::{PendingReview, ReviewId, Settled};

// Everything a caller needs to construct an `Observation`, build the index and
// ruleset `Engine::new` takes, and read what comes back.
//
// Re-exported rather than left to the caller to depend on directly, because
// every one of these types appears in this crate's own signatures: `Ruleset`
// and `VectorIndex` in `new`, `Record`/`Interval`/`Provenance` in
// `Observation`, `StableId` in nearly everything, `Version` in the return type
// of `store_history`, `Edge` in `edges_from` and `edges_into`, and
// `EdgeVersion` in `edge_history`. A caller could not name the last of those at
// all without adding `rm-store` to their manifest, which makes an internal
// decomposition — five crates instead of one — into something the caller has to
// know about and track. `tests/readme.rs` exists to prove the public API is sufficient, and it
// was importing four sibling crates to compile.
//
// Only the surface those signatures reach is re-exported. `MemoryStore`,
// `Outcome` and the rest stay behind the engine, which owns them.
//
// `extract` and `prompt` are re-exported alongside the types they build and
// consume, for the same reason: a caller who has to reach into `rm-extract`
// for the function while naming its types through `rm-engine` has the worst
// of both, and `tests/extract.rs` calls `rm_engine::extract` to prove it does
// not have to. A caller ingesting a turn needs to name every type in
// `extract`'s signature and in `ingest`'s, which is why the full list below
// goes past what `Extraction` alone would require.
pub use rm_core::{Interval, Provenance, Source, Supersession, Timestamp};
pub use rm_extract::{
    extract, prompt, Closure, Completer, CompleterError, ExtractError, Extraction, Fact, Mention,
    Relation, Turn,
};
pub use rm_graph::{Direction, Neighborhood, Reached, Walk};
pub use rm_index::{IndexError, Metric, VectorIndex};
pub use rm_resolve::{BlockingKey, Comparator, Decision, FieldRule, Record, Ruleset};
pub use rm_store::{Edge, EdgeVersion, StableId, StoreError, Version};
pub use rm_survivor::{Refused, Strategy};

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
    /// A write named an entity that does not exist.
    ///
    /// The write-path error, and only the write path: naming a nonexistent
    /// entity in [`Engine::forget`], [`Engine::erase`], [`Engine::erase_edges`],
    /// [`Engine::relate`], [`Engine::unrelate`] or [`Engine::confirm`] is a bug
    /// in the caller and is reported as one. Asking *about* one is
    /// not — [`Engine::about`] answers [`Believed::Unknown`], because "I have
    /// nothing on this" is a true and useful answer to a question where it
    /// would be a silent no-op if accepted as an instruction. `rm_store` draws
    /// the line in the same place, and this variant is where its
    /// [`StoreError::UnknownEntity`] surfaces, relabelled rather than wrapped
    /// so the caller does not have to reach through `Store(..)` for the one
    /// thing the wrapper exists to tell them.
    UnknownEntity(StableId),
    UnknownReview(ReviewId),
    /// A fact, relation or closure named a mention index its own `Extraction`
    /// does not have.
    ///
    /// `rm_extract::extract` refuses this before an `Extraction` is ever
    /// produced, but `Extraction`'s fields are `pub` and `Engine::ingest` is
    /// `pub`, so nothing stops a caller building one directly with a bad
    /// index -- every test in this crate does exactly that to exercise
    /// `ingest`. A public function that panics on input its own type lets a
    /// caller construct contradicts what every other door in this workspace
    /// does (`MemoryStore::open` validates its snapshot, `relate` validates
    /// its endpoints), so `ingest` checks its own indices rather than trust a
    /// guarantee only `extract` enforces.
    BadMentionIndex {
        /// What named the index, e.g. `"fact subject"` or `"closure subject"`.
        what: &'static str,
        index: usize,
        /// How many mentions the extraction actually has.
        mentions: usize,
    },
    /// A relation named the same mention as both its subject and object.
    ///
    /// `rm_extract::extract` refuses this for its own output, by local index,
    /// the same way it refuses a bad [`EngineError::BadMentionIndex`]; a
    /// hand-built `Extraction` can still carry it. Left unchecked here it would
    /// still end in an error -- [`StoreError::SelfEdge`] -- but only after
    /// [`Engine::relate`] was reached, which is after every mention and fact
    /// this same call wrote had already landed. Checking it alongside
    /// [`EngineError::BadMentionIndex`], before the first write, gives a
    /// relation naming one index twice the same costs-nothing failure a bad
    /// index gets.
    ///
    /// **That guarantee covers the index case only.** A relation can also name
    /// two *different* mentions that resolution then lands on the same entity
    /// -- the same person said twice in one turn, which `extract` does not
    /// dedupe. Nothing here can catch that, because the check runs before the
    /// first write and the entity ids do not exist until the mention loop has
    /// run. It is [`Engine::relate`] that refuses it, by which point this
    /// call's mentions and facts are already in the store and the `Ingested`
    /// naming them is dropped with the error. The writes stand; the caller
    /// cannot learn which ones.
    ///
    /// Moving the check later would not fix that -- it would only move which
    /// writes had already happened -- so the honest statement is that `ingest`
    /// is atomic against a malformed extraction and not against a coincidence
    /// of resolution.
    SelfRelation(usize),
    /// A mention carried no name, and resolution has nothing else to match on.
    ///
    /// The third of the three refusals `rm_extract::extract` applies, and the
    /// one that costs most if it is skipped. [`Engine::ingest`] resolves a
    /// mention on a `Record` holding its name and its kind, and the blocking
    /// key that finds its candidates is a prefix of the name -- so an empty
    /// name yields the *same* key for every nameless mention, putting all of
    /// them in one block, where each scores against every other and finds the
    /// name blank on both sides, which agrees. The kind does not save it:
    /// nameless mentions sharing a kind agree on that too, and the pile of
    /// them a long conversation produces is mostly one or two kinds.
    ///
    /// What that agreement buys depends on the ruleset, and both answers are
    /// wrong. A ruleset that trusts a name match on its own merges distinct
    /// things into one entity; a stricter one files a review for every pair.
    /// Neither is an error, so neither is reported. Refusing the mention is the
    /// only outcome that does not depend on how a host tuned its thresholds --
    /// and an entity with no name could never be matched again anyway, so every
    /// later turn about it would create another.
    NamelessMention(usize),
    CorruptSnapshot(String),
    /// The host's embedder failed. Carries its explanation.
    Embed(EmbedderError),
}

/// Relabel the store's "no such entity" as the engine's own on a write.
///
/// Every other `StoreError` keeps its wrapper: each is an explanation the store
/// wrote, and flattening them would lose which invariant broke. This one is
/// different because the engine's own error type already names it, and a
/// variant nothing constructs is a design stated in a comment rather than in
/// the type.
fn on_write(e: StoreError) -> EngineError {
    match e {
        StoreError::UnknownEntity(id) => EngineError::UnknownEntity(id),
        other => EngineError::Store(other),
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Index(e) => write!(f, "{e}"),
            EngineError::Store(e) => write!(f, "{e}"),
            EngineError::Refused(e) => write!(f, "{e}"),
            EngineError::UnknownEntity(id) => write!(f, "no entity with id {id}"),
            EngineError::UnknownReview(id) => write!(f, "no open review with id {id}"),
            EngineError::BadMentionIndex {
                what,
                index,
                mentions,
            } => write!(
                f,
                "{what} names mention {index}, but this extraction has only {mentions} mention(s)"
            ),
            EngineError::SelfRelation(index) => write!(
                f,
                "a relation names mention {index} as both its own subject and object, which a self-edge cannot represent"
            ),
            EngineError::NamelessMention(index) => write!(
                f,
                "mention {index} has no name, and resolution matches on the name -- every nameless mention blocks together and scores identically, so depending on the ruleset distinct things either merge into one entity or file a review for every pair, and neither is reported as an error"
            ),
            EngineError::CorruptSnapshot(why) => {
                write!(
                    f,
                    "snapshot parsed but describes an impossible engine: {why}"
                )
            }
            EngineError::Embed(e) => write!(f, "{e}"),
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

impl From<EmbedderError> for EngineError {
    fn from(e: EmbedderError) -> Self {
        EngineError::Embed(e)
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

    /// Every entity this engine knows, in ascending id order.
    ///
    /// For callers that need to tell a newly created entity from a recognised
    /// one. `entity_count()` cannot answer that: it says how many exist, not
    /// which of several mentions was the new one.
    pub fn entity_ids(&self) -> Vec<StableId> {
        self.identity.keys().copied().collect()
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

    /// Record an observation about an entity the caller has already identified.
    ///
    /// [`Engine::remember`] works out which entity an observation belongs to by
    /// scoring its mention against everything already known. That is right when
    /// the identity has to be inferred from what somebody said, which is the
    /// case this engine was built for.
    ///
    /// It is wrong when the caller holds the identifier itself -- a decision's
    /// title, a row key, an id from another system. There, "close enough" is
    /// not a better answer than "not found": it is a silently wrong one. A
    /// decision recorded as `Adopt SQLite WAL` scored above the match threshold
    /// against an existing `Adopt SQLite` and was written onto it, keeping the
    /// older title, so the new decision existed nowhere and the command that
    /// wrote it reported success.
    ///
    /// So this door takes the answer instead of computing it. `None` creates a
    /// new entity. Nothing is scored, no review is filed, and no existing
    /// entity is considered however close its name -- which is the guarantee,
    /// not an optimisation.
    ///
    /// Naming an entity the store does not have is [`EngineError::UnknownEntity`],
    /// on the same rule as every other write path here: asking *about* an
    /// unknown entity is a question with a true answer, writing to one is a bug
    /// in the caller.
    pub fn remember_as(
        &mut self,
        entity: Option<StableId>,
        obs: Observation,
    ) -> Result<(StableId, AssertionId), EngineError> {
        // Door first, as in `remember`: a refusal costs nothing, and in
        // particular does not leave a fresh entity behind with no assertion on
        // it -- which `create_entity` below would do if the vector were only
        // checked on the way into the index.
        self.index.check(&obs.embedding)?;

        let entity = match entity {
            Some(id) => {
                if self.identity_of(id).is_none() {
                    return Err(EngineError::UnknownEntity(id));
                }
                // Fold in any field this mention carries that the entity does
                // not, exactly as `remember` does when it merges. Without this
                // an entity identified from outside never gains blocking keys,
                // and would be invisible to `remember` for evermore.
                self.remember_identity(id, &obs.mention);
                id
            }
            None => self.create_entity(&obs),
        };
        let assertion = self.write(entity, &obs)?;
        Ok((entity, assertion))
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
    /// Compared by identity record, never by id. An id comparison is the
    /// obvious first thing to write and it is dead code here: the only caller
    /// is [`Engine::remember`], and the entity it asks about is one it created
    /// moments earlier, so the pair cannot already be in `rejected` under those
    /// ids. It was removed rather than kept as a fast path, because a branch
    /// that cannot run is a claim about the code that stops being true silently.
    ///
    /// The record comparison is the one that does the work: rejecting a pair
    /// and then hearing the same ambiguous mention again produces a *new* id,
    /// and matching on ids would ask the identical question every time. That is
    /// also why `rejected` still stores ids — they are how the answer is looked
    /// up again through `identity`, and `confirm` rewrites them on a merge.
    fn already_rejected(&self, a: StableId, b: StableId) -> bool {
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
    /// `adopt_assertions`, below.
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
        // purpose — see `adopt_assertions`, below.
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
                // The claim moves with the version. A merge decides *whose*
                // slot these assertions are in and nothing about whether they
                // corrected one another; re-deciding that here would let two
                // entities becoming one silently rewrite what each of them had
                // said.
                self.store
                    .assert(
                        kept,
                        attribute.clone(),
                        v.value,
                        v.valid,
                        v.provenance,
                        v.supersession,
                        // A merge moves versions; it does not reattribute
                        // them. Whoever held a view still holds it.
                        v.according_to,
                    )
                    .map_err(on_write)?;
            }
            self.store.erase(absorbed, attribute).map_err(on_write)?;
        }

        // Edges follow the merge. Left alone they name an id nothing resolves,
        // and a walk would cross into it — the same class of defect as leaving
        // an assertion pointing at a stale version index: a well-formed wrong
        // answer with nothing to raise an error about. Done here, after both
        // ids are still meaningful (the absorbed entity is not erased from the
        // store) and before `adopt_assertions` renumbers the version logs, so
        // repointing edges — which touches only the store's edge tables — has
        // no way to disturb the offsets that renumbering depends on.
        self.store.repoint_edges(absorbed, kept).map_err(on_write)?;

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

    /// The identity fields an entity resolved on.
    ///
    /// Exists because [`pending_review`](Self::pending_review) returns two ids
    /// and a score, and nobody can answer "are entity 3 and entity 11 the same"
    /// from that. The question is only askable if the asker can see what the
    /// two entities are called -- which is exactly this record, the one
    /// resolution scored to raise the question in the first place.
    ///
    /// Returns `None` for an id no entity holds.
    pub fn identity_of(&self, entity: StableId) -> Option<&Record> {
        self.identity.get(&entity)
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
    ///
    /// Three mutations in sequence, and the promise that a `remember` either
    /// happens completely or not at all rests on neither of the first two being
    /// able to fail here. `store.assert`'s only error is `UnknownEntity`, and
    /// `remember` reaches this with an id it either just created or read out of
    /// `identity` — which `Engine::open` checks the store holds. `index.insert`
    /// re-runs the validation `remember` already ran through `index.check`
    /// before touching anything, which is the entire reason that check is a
    /// separate call. Take either of those away and the failure is a version in
    /// the store with no vector able to find it: no error, nothing downstream
    /// able to detect it, and the exact outcome `remember`'s ordering exists to
    /// prevent.
    fn write(&mut self, entity: StableId, obs: &Observation) -> Result<AssertionId, EngineError> {
        self.store.assert(
            entity,
            obs.attribute.clone(),
            obs.value.clone(),
            obs.valid,
            obs.provenance.clone(),
            obs.supersession,
            obs.according_to,
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

    /// Every assertion in the store, with the entity and attribute it belongs
    /// to, in id order.
    ///
    /// For a caller that needs to reconstruct what each one was embedded from.
    /// The engine cannot do that itself: it never sees the text, only the
    /// vector the host handed it.
    pub fn assertion_ids(&self) -> Vec<(AssertionId, StableId, String)> {
        self.assertions
            .iter()
            .map(|(id, e)| (*id, e.entity, e.attribute.clone()))
            .collect()
    }

    /// Replace the index with one built from vectors the caller supplies.
    ///
    /// # Why the store cannot do this on its own
    ///
    /// A `Version` keeps its value, its interval and its provenance. The text
    /// that was *embedded* -- a sentence the extractor wrote, or a line the
    /// host composed -- is handed to the embedder and dropped. So the vectors
    /// are the only surviving representation of it, and changing embedder is a
    /// one-way door: a different model, or the same model at a different
    /// dimension, strands every vector already written.
    ///
    /// This is the way back through, for callers that *can* reconstruct the
    /// text. It is not a general repair: whoever calls it has to know what each
    /// assertion was embedded from, and only some do.
    ///
    /// # It is all or nothing
    ///
    /// `Engine::open` refuses a snapshot in which any assertion lacks a vector,
    /// because such an assertion could never be recalled and nothing would say
    /// so. A partial rebuild would either break that or -- worse -- leave two
    /// embedding models' output in one index, where every distance between
    /// them is silently meaningless rather than merely wrong. So a vector for
    /// every assertion, or the index is left exactly as it was.
    pub fn rebuild_index(
        &mut self,
        dimension: usize,
        metric: Metric,
        vectors: Vec<(AssertionId, Vec<f32>)>,
    ) -> Result<(), EngineError> {
        let mut fresh = VectorIndex::new(dimension, metric);
        // Built to one side and swapped at the end, so a vector the new index
        // refuses -- wrong length, non-finite, zero under cosine -- leaves the
        // engine with the index it came in with rather than half of one.
        for (id, v) in &vectors {
            if !self.assertions.contains_key(id) {
                return Err(EngineError::CorruptSnapshot(format!(
                    "a vector was supplied for assertion {id}, which this store does not have"
                )));
            }
            fresh.insert(*id, v)?;
        }
        if fresh.len() != self.assertions.len() {
            let missing: Vec<AssertionId> = self
                .assertions
                .keys()
                .filter(|id| !vectors.iter().any(|(v, _)| v == *id))
                .take(3)
                .copied()
                .collect();
            return Err(EngineError::CorruptSnapshot(format!(
                "the rebuild covers {} of {} assertions, and one without a vector could never be recalled. Missing, for example: {missing:?}",
                fresh.len(),
                self.assertions.len()
            )));
        }
        self.index = fresh;
        Ok(())
    }

    /// How many vectors are searchable.
    pub fn index_len(&self) -> usize {
        self.index.len()
    }

    /// The dimension and metric backing this engine's vector index.
    ///
    /// Exists so a caller restoring an engine from a snapshot can check the
    /// restored index agrees with what its own configuration currently
    /// expects, before a disagreement surfaces far from its cause as a
    /// `WrongDimension` on the first `remember` or `recall` rather than at
    /// the door where `Engine::open` already validates everything else it
    /// can see about a snapshot on its own.
    pub fn index_shape(&self) -> (usize, Metric) {
        (self.index.dim(), self.index.metric())
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
        self.store
            .assert(
                entity,
                attribute.to_string(),
                None,
                Interval::since(at),
                prov,
                // Redundant -- the store makes every tombstone a correction --
                // and stated anyway, because reading `Unstated` here would
                // suggest the question was open. "Stop telling me this" is the
                // least open question in the crate.
                Supersession::Corrects,
                // `forget` silences the attribute for everyone. A tombstone
                // held by one person would only silence their view, which is
                // not what being asked to stop is.
                None,
            )
            .map_err(on_write)?;
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
        let removed = self.store.erase(entity, attribute).map_err(on_write)?;
        self.drop_vectors(entity, attribute);
        Ok(removed)
    }

    /// Destroy every edge touching `entity`, in both directions.
    ///
    /// The edge counterpart of [`Engine::erase`], and deliberately separate
    /// from it: attributes and relationships are different halves of what an
    /// entity carries, and a caller answering "what did you remove" has to be
    /// able to say which. A combined "erase everything about this entity"
    /// would leave that question unanswerable, and would also make a caller
    /// who only wanted relationships gone pay for wiping attributes they
    /// never asked to lose. Neither call implies the other; a caller wanting
    /// both makes both calls.
    ///
    /// Nothing here touches the vector index: an edge has no text and no
    /// embedding, exactly as [`Engine::relate`] notes, so there is nothing
    /// for `drop_vectors` to find.
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownEntity`] if `entity` names no entity the store
    /// holds — see `rm_store::MemoryStore::erase_edges` for why that, and not
    /// a bare `Ok(0)`, is what an unknown id gets. Named rather than linked,
    /// as [`Engine::erase`] names `erase`: `MemoryStore` is deliberately not
    /// re-exported, so a link would take a reader to a type they cannot write
    /// down without adding `rm-store` to their manifest.
    pub fn erase_edges(&mut self, entity: StableId) -> Result<usize, EngineError> {
        self.store.erase_edges(entity).map_err(on_write)
    }

    /// Record that a relationship held between two entities.
    ///
    /// Delegates to the store, which rejects an endpoint it does not hold and
    /// a self-edge. Unlike [`Engine::remember`], nothing here is indexed: an
    /// edge has no text and no embedding, so there is no vector to keep in
    /// step with it.
    pub fn relate(
        &mut self,
        subject: StableId,
        predicate: impl Into<String>,
        object: StableId,
        valid: Interval,
        prov: Provenance,
    ) -> Result<(), EngineError> {
        self.store
            .relate(subject, predicate, object, valid, prov)
            .map_err(on_write)
    }

    /// Record that a relationship stopped holding at `at`.
    ///
    /// The edge counterpart of [`Engine::forget`]: a walk stops crossing it,
    /// and [`Engine::edge_history`] still shows that it held and who said so.
    /// Appending a tombstone rather than deleting is only defensible because
    /// that call exists — a record no caller can read is not a record.
    pub fn unrelate(
        &mut self,
        subject: StableId,
        predicate: &str,
        object: StableId,
        at: Timestamp,
        prov: Provenance,
    ) -> Result<(), EngineError> {
        self.store
            .unrelate(subject, predicate, object, at, prov)
            .map_err(on_write)
    }

    /// Entities reachable from the walk's seeds.
    ///
    /// Deliberately separate from [`Engine::recall`], which stays purely
    /// semantic. Fusing them would mean ranking a two-hop neighbour against a
    /// 0.9-cosine hit, and there is no honest ordering between those: any
    /// single combined score is a number this crate invented and would then
    /// have to defend. The caller knows what each is worth in its context,
    /// and composes them — seed with `recall`, expand with this.
    pub fn neighborhood(&self, walk: &Walk) -> Neighborhood {
        rm_graph::neighborhood(&self.store, walk)
    }

    /// Relationships out of `subject` in force at `valid_t`, as known by
    /// `tx_t`.
    ///
    /// [`Engine::neighborhood`] answers which entities a walk reaches and how
    /// far away they are; it deliberately hands back ids and distances and
    /// nothing else, because a walk of any depth cannot say which edge carried
    /// it without inventing a path when several did. This is the other
    /// question — one hop, fully described: the predicate, the interval it
    /// held over, and who said so. A caller rendering "where does Alice work,
    /// and on whose word" needs that and cannot reconstruct it from a
    /// [`Reached`].
    ///
    /// Both axes are required and neither is defaulted, for the same reason
    /// [`Walk`] requires them: an edge read without a `tx_t` is a claim about
    /// now that quietly stops being reproducible.
    pub fn edges_from(
        &self,
        subject: StableId,
        valid_t: Timestamp,
        tx_t: Timestamp,
    ) -> Vec<Edge<'_>> {
        self.store.edges_from(subject, valid_t, tx_t)
    }

    /// Relationships into `object` in force at `valid_t`, as known by `tx_t`.
    ///
    /// The mirror of [`Engine::edges_from`], answering "who works at Acme"
    /// rather than "where does Alice work". Both directions are exposed
    /// because the store keeps both and [`Direction`] already lets a walk go
    /// either way: offering only the forward half would make the reverse
    /// question answerable by traversal but not by inspection.
    ///
    /// An id the store does not hold has no edges rather than being an error,
    /// exactly as [`Engine::about`] answers [`Believed::Unknown`]. Asking is
    /// not an instruction.
    pub fn edges_into(
        &self,
        object: StableId,
        valid_t: Timestamp,
        tx_t: Timestamp,
    ) -> Vec<Edge<'_>> {
        self.store.edges_into(object, valid_t, tx_t)
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

    /// Every attribute this entity has a version log for, in name order.
    ///
    /// The missing half of [`Engine::store_history`], which can only be
    /// called by someone who already knows the name. Without this there is no
    /// way to walk a store and see what is in it -- which is what a tool
    /// reporting on a real store needs, and what `benches/read-cost` uses to
    /// re-measure the depth `rm_contrast::cost::LIVE_STORE_DEPTH` records.
    ///
    /// Empty for an entity this engine does not know, which is the same
    /// answer as an entity that has no attributes yet: both mean there is
    /// nothing to read, and neither is an error.
    pub fn attributes_of(&self, entity: StableId) -> Vec<&str> {
        self.store
            .entity(entity)
            .map(|e| e.attributes.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// The raw version log, for callers that want the audit trail rather than
    /// an answer.
    pub fn store_history(&self, entity: StableId, attribute: &str) -> &[rm_store::Version] {
        self.store.history(entity, attribute)
    }

    /// The raw version log for one relationship, in append order.
    ///
    /// The edge counterpart of [`Engine::store_history`], and the call that
    /// makes [`Engine::unrelate`] honest. `unrelate` appends a tombstone
    /// instead of deleting, and justifies that by saying the record still
    /// shows the relationship held and who said so — a promise worth nothing
    /// if the only type that can read it is one the engine keeps to itself.
    /// This is where an audit, a "why do you think that" answer, or a
    /// correction that has to see what it is correcting reaches it.
    ///
    /// No time arguments, deliberately: this is the log, not a query over it.
    /// [`Engine::edges_from`] resolves a point on both axes; asking for the
    /// history and then filtering it is a different job, and one whose rules
    /// the caller should be able to see rather than inherit.
    ///
    /// Empty for a triple never discussed. Asking about a relationship nobody
    /// ever asserted is a question, not a mistake, and it answers the same as
    /// a triple whose versions were erased — [`Engine::erase_edges`] destroys
    /// the trail, which is exactly what it is documented to do.
    pub fn edge_history(
        &self,
        subject: StableId,
        predicate: &str,
        object: StableId,
    ) -> &[EdgeVersion] {
        self.store.edge_history(subject, predicate, object)
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
    /// Whether this observation replaces what the attribute already held.
    ///
    /// [`Supersession::Unstated`] is the honest answer for a host that has not
    /// thought about it, and it is what the recall path will report: a later
    /// assertion exists, and nobody said whether it corrected this one.
    pub supersession: Supersession,
    /// Caller-supplied. `rm_extract` is the only crate permitted to reach the
    /// network, so nothing here computes an embedding.
    /// Whose view this is, when it is a view rather than a fact.
    ///
    /// An entity, not a label, so a holder can be asked about like anyone
    /// else and two spellings of one person cannot become two holders.
    ///
    /// `None` is the store's own assertion, and is what every observation
    /// written before this field existed is. Survivorship partitions a slot
    /// by this, so one holder correcting themselves is a correction and two
    /// holders differing is not.
    pub according_to: Option<StableId>,
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
            // The default a host that has not thought about it would produce,
            // so the tests exercise the same path an unconsidered caller takes.
            // `correcting` marks the ones that are about a fact changing.
            supersession: Supersession::Unstated,
            according_to: None,
            embedding: vec![1.0, 0.0, 0.0],
        }
    }

    /// An observation that claims to replace what its attribute already held.
    fn correcting(mut obs: Observation) -> Observation {
        obs.supersession = Supersession::Corrects;
        obs
    }

    /// The store's own motivating example, asked of the engine.
    ///
    /// `rm_store`'s module docs open with it: "In September a user mentions
    /// they changed jobs back in July. The valid time starts in July; the
    /// transaction time starts in September." The whole argument for carrying
    /// two axes is that a single-axis store has to choose, and choosing is
    /// wrong either way.
    ///
    /// So: what did they hold in August, asked now? They changed in July, so
    /// the answer is the new employer. `MemoryStore::as_of` gets this right --
    /// it filters `v.valid.contains(valid_t)` directly. `Engine::about` runs
    /// survivorship first, and `rm_survivor::Candidate` carries a value and a
    /// provenance and no interval at all, so the valid time cannot reach the
    /// strategy that is supposed to use it. `timeline` builds its spans from
    /// `provenance.observed_at`.
    ///
    /// It failed when it was written. `rm_survivor::Candidate` carried a value
    /// and a provenance and no interval, so `Strategy::ValidInterval` cut its
    /// timeline at `provenance.observed_at` and answered August with the old
    /// employer. The candidate carries its validity now.
    #[test]
    fn a_job_change_mentioned_late_is_true_from_when_it_happened() {
        const JANUARY: Timestamp = 100;
        const JULY: Timestamp = 700;
        const AUGUST: Timestamp = 800;
        const SEPTEMBER: Timestamp = 900;
        const NOW: Timestamp = 1000;

        let mut e = engine().with_policy(Policy::new(Strategy::ValidInterval));
        let mut said = |value: &str, valid_from: Timestamp, heard: Timestamp| {
            e.remember(Observation {
                kind: "person".to_string(),
                // `city` too: `test_ruleset` wants a corroborating field to
                // put a name-only match above the line, so a mention without
                // one lands on a fresh entity and this test would measure the
                // resolver rather than the timeline.
                mention: Record::new()
                    .with("name", "Ben Severn")
                    .with("city", "Bristol"),
                attribute: "employer".to_string(),
                value: Some(value.to_string()),
                // The two axes, set apart. This is the whole point.
                valid: Interval::since(valid_from),
                provenance: Provenance::new(Source::UserAssertion, heard, "s"),
                supersession: Supersession::Unstated,
                according_to: None,
                embedding: vec![1.0, 0.0, 0.0],
            })
            .unwrap()
        };

        // Told in January, true from January.
        let Remembered::Created { entity, .. } = said("Acme", JANUARY, JANUARY) else {
            panic!("setup")
        };
        // Told in September, true from July.
        said("Globex", JULY, SEPTEMBER);

        // What the store itself says, which is right: it filters both axes.
        assert_eq!(
            e.store_history(entity, "employer").len(),
            2,
            "both assertions are kept"
        );

        // The data is recorded correctly. A version says Globex, and its valid
        // interval contains August -- so nothing is lost on the way in, and
        // `MemoryStore::as_of` would answer this by filtering
        // `v.valid.contains(valid_t)` directly. What follows is the read path
        // losing it.
        assert!(
            e.store_history(entity, "employer")
                .iter()
                .any(|v| { v.value.as_deref() == Some("Globex") && v.valid.contains(AUGUST) }),
            "the store holds a Globex version valid in August; the loss is downstream"
        );

        // In August they were at Globex -- they changed in July. Asked now, so
        // transaction time is not the obstacle.
        assert_eq!(
            e.about(entity, "employer", AUGUST, NOW).unwrap(),
            Believed::Value("Globex".into()),
            "they changed jobs in July, so in August they were at Globex --              the timeline is cut at when the store was *told*, not when it was true"
        );

        // And the answers either side stay right.
        assert_eq!(
            e.about(entity, "employer", JANUARY + 1, NOW).unwrap(),
            Believed::Value("Acme".into()),
            "in January they really were at Acme"
        );
        assert_eq!(
            e.about(entity, "employer", AUGUST, AUGUST).unwrap(),
            Believed::Value("Acme".into()),
            "asked in August, the store had not been told yet -- transaction              time is a separate question and this one it gets right"
        );
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
    fn a_merge_repoints_edges_in_both_directions() {
        // The absorbed entity's relationships are the survivor's now. Left
        // alone they would point at a dead id, and a walk would follow them.
        let mut e = engine();
        let Remembered::Created { entity: kept, .. } = e
            .remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap()
        else {
            panic!("setup")
        };
        let Remembered::Created { entity: acme, .. } = e
            .remember(observation("Acme Corp", "kind", "company", 2))
            .unwrap()
        else {
            panic!("setup")
        };
        let Remembered::Created { entity: boss, .. } = e
            .remember(observation("Wei Zhang", "role", "manager", 3))
            .unwrap()
        else {
            panic!("setup")
        };
        let out = e.remember(ambiguous()).unwrap();
        let Remembered::CreatedPendingReview {
            entity: absorbed,
            review,
            ..
        } = out
        else {
            panic!("expected a review, got {out:?}")
        };

        let prov = Provenance::new(Source::UserAssertion, 4, "s");
        e.relate(
            absorbed,
            "employed_by",
            acme,
            Interval::since(1),
            prov.clone(),
        )
        .unwrap();
        e.relate(boss, "manages", absorbed, Interval::since(1), prov)
            .unwrap();

        let survivor = e.confirm(review[0]).unwrap();
        assert_eq!(survivor, kept.min(absorbed));

        let out_edges = e.neighborhood(&Walk::new(vec![survivor], 1, 10, 5, 5));
        assert!(
            out_edges.reached.iter().any(|r| r.entity == acme),
            "outgoing followed the merge"
        );

        let in_edges =
            e.neighborhood(&Walk::new(vec![survivor], 1, 10, 5, 5).direction(Direction::In));
        assert!(
            in_edges.reached.iter().any(|r| r.entity == boss),
            "incoming followed the merge"
        );

        let dead = e.neighborhood(&Walk::new(vec![kept.max(absorbed)], 1, 10, 5, 5));
        assert!(
            dead.reached.len() <= 1,
            "nothing still hangs off the absorbed id"
        );
    }

    #[test]
    fn a_merge_drops_an_edge_that_would_become_a_self_edge() {
        // The two turned out to be the same person, so "A manages B" is now
        // "A manages A" -- which relate() refuses to create, and repoint_edges
        // must not smuggle one in through a merge instead.
        //
        // A walk cannot witness this: `neighborhood` inserts a seed into its
        // `seen` set *before* it looks at that seed's own neighbours, so a
        // survivor -> survivor edge can never be reached a second time from
        // the survivor itself. That is a property of breadth-first search
        // from a single seed, not a gap in this fixture -- no choice of
        // `Direction` or hop count makes a self-edge observable that way. So
        // this reaches for `erase_edges`'s return count instead: it is a
        // flat scan of every edge touching the entity, self-edges included,
        // and a phantom survivor -> survivor edge would make it count one
        // higher than the real edges alone.
        let mut e = engine();
        let Remembered::Created { entity: kept, .. } = e
            .remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap()
        else {
            panic!("setup")
        };
        let Remembered::Created { entity: boss, .. } = e
            .remember(observation("Wei Zhang", "role", "manager", 2))
            .unwrap()
        else {
            panic!("setup")
        };
        let out = e.remember(ambiguous()).unwrap();
        let Remembered::CreatedPendingReview {
            entity: absorbed,
            review,
            ..
        } = out
        else {
            panic!("expected a review")
        };

        // Collapses into a self-edge once the merge lands, and must not
        // survive it.
        e.relate(
            kept,
            "knows",
            absorbed,
            Interval::since(1),
            Provenance::new(Source::UserAssertion, 4, "s"),
        )
        .unwrap();
        // Untouched by the merge, so its presence in the count tells the two
        // cases apart: a lingering self-edge shows up as one edge too many,
        // not as the only edge present.
        e.relate(
            boss,
            "manages",
            kept,
            Interval::since(1),
            Provenance::new(Source::UserAssertion, 5, "s"),
        )
        .unwrap();

        let survivor = e.confirm(review[0]).unwrap();
        assert_eq!(
            e.erase_edges(survivor).unwrap(),
            1,
            "only the real manages-edge should remain; a surviving self-edge \
             would count as a second"
        );
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
    fn one_history_answers_two_ways_without_the_engine_moving() {
        // The same contrast as the test below, but on a single `&engine` and in
        // successive lines. That shape is the claim: nothing is reconfigured
        // between the two reads, nothing is rewritten, and the engine is not
        // consumed and rebuilt -- only the question changes.
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

        let recent = Policy::new(Strategy::MostRecent);
        let interval = Policy::new(Strategy::ValidInterval);

        // May, under two rules, from one borrow.
        assert_eq!(
            e.about_under(&recent, entity, "employer", 15, 100).unwrap(),
            Believed::Value("Globex".into()),
            "MostRecent names one winner, whatever instant is asked about"
        );
        assert_eq!(
            e.about_under(&interval, entity, "employer", 15, 100)
                .unwrap(),
            Believed::Value("Acme".into()),
            "ValidInterval keeps both and answers by time"
        );

        // And the engine's own policy is untouched by either call.
        assert_eq!(
            e.about(entity, "employer", 15, 100).unwrap(),
            Believed::Value("Globex".into()),
            "about_under must not leave the engine reading under a borrowed policy"
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
    fn scoping_a_recall_to_one_entity_happens_during_the_scan() {
        // The same shape as the session filter, on the axis a caller reaches
        // for most: "what do I know about *this* person". The entity asked
        // about is the *worse* geometric match of the two, so a post-filtered
        // top-1 would hand back the other one's fact — or, once the filter had
        // run, nothing at all.
        let mut e = engine();
        let Remembered::Created { entity: wanted, .. } = e
            .remember(embedded(
                "Ben Severn",
                "employer",
                "Acme",
                1,
                [0.6, 0.8, 0.0],
            ))
            .unwrap()
        else {
            panic!("setup")
        };
        e.remember(embedded(
            "Wei Zhang",
            "employer",
            "Globex",
            2,
            [1.0, 0.0, 0.0],
        ))
        .unwrap();
        assert_eq!(e.entity_count(), 2, "setup: two entities to choose between");

        let hits = e
            .recall(&Query::new(vec![1.0, 0.0, 0.0], 1).about_entity(wanted))
            .unwrap();
        assert_eq!(hits.len(), 1, "post-filtering would have returned 0");
        assert_eq!(hits[0].entity, wanted);
        assert_eq!(hits[0].value.as_deref(), Some("Acme"));
    }

    #[test]
    fn scoping_a_recall_to_one_source_keeps_out_what_another_source_said() {
        // Provenance is not decoration: "what did the CRM tell me" has to be
        // answerable separately from "what did the user tell me", or a caller
        // cannot tell a stated fact from an inferred one.
        let mut e = engine();
        let mut inferred = embedded("Ben Severn", "employer", "guessed", 1, [1.0, 0.0, 0.0]);
        inferred.provenance = Provenance::new(Source::AgentInference, 1, "session-1");
        e.remember(inferred).unwrap();

        let mut stated = embedded("Ben Severn", "employer", "stated", 2, [0.99, 0.1, 0.0]);
        stated.provenance = Provenance::new(Source::UserAssertion, 2, "session-1");
        e.remember(stated).unwrap();

        let hits = e
            .recall(&Query::new(vec![1.0, 0.0, 0.0], 5).from_source(Source::UserAssertion))
            .unwrap();
        assert_eq!(hits.len(), 1, "an inference is not something the user said");
        assert_eq!(hits[0].value.as_deref(), Some("stated"));
        assert_eq!(hits[0].provenance.source, Source::UserAssertion);
    }

    #[test]
    fn a_corrected_fact_is_returned_marked_not_dropped() {
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
        e.remember(correcting(embedded(
            "Ben Severn",
            "employer",
            "Globex",
            20,
            [0.9, 0.1, 0.0],
        )))
        .unwrap();

        let hits = e.recall(&Query::new(vec![1.0, 0.0, 0.0], 2)).unwrap();
        assert_eq!(hits.len(), 2);
        let acme = hits
            .iter()
            .find(|h| h.value.as_deref() == Some("Acme"))
            .unwrap();
        assert_eq!(
            acme.standing,
            Standing::Corrected,
            "an old fact must be returned marked"
        );
        assert!(!acme.standing.still_stands());
        let globex = hits
            .iter()
            .find(|h| h.value.as_deref() == Some("Globex"))
            .unwrap();
        assert_eq!(globex.standing, Standing::Latest);
    }

    #[test]
    fn a_second_pet_does_not_supersede_the_first() {
        // The defect this whole field exists for, at its smallest. Before it,
        // recall marked "a dog" as replaced by "a cat" because the cat arrived
        // second, and an agent reading that out forgets the dog. Measured over
        // ten LoCoMo conversations, arrival order alone flagged 26% of every
        // assertion in the store.
        let mut e = engine();
        let mut dog = embedded("Ben Severn", "pet", "a dog", 10, [1.0, 0.0, 0.0]);
        dog.supersession = Supersession::Joins;
        let mut cat = embedded("Ben Severn", "pet", "a cat", 20, [0.9, 0.1, 0.0]);
        cat.supersession = Supersession::Joins;
        e.remember(dog).unwrap();
        e.remember(cat).unwrap();

        let hits = e.recall(&Query::new(vec![1.0, 0.0, 0.0], 2)).unwrap();
        let dog = hits
            .iter()
            .find(|h| h.value.as_deref() == Some("a dog"))
            .unwrap();
        assert_eq!(dog.standing, Standing::Joined, "the dog is still a pet");
        assert!(dog.standing.still_stands());
    }

    #[test]
    fn an_unanswered_slot_says_it_is_unsettled_rather_than_picking_a_side() {
        // What every assertion written before this field existed looks like.
        // Reading it as `Joined` would retroactively un-correct every job
        // change ever stored; reading it as `Corrected` is the inference the
        // field exists to stop making. So it reports the question as open.
        let mut e = engine();
        e.remember(embedded("Ben Severn", "mood", "tired", 10, [1.0, 0.0, 0.0]))
            .unwrap();
        e.remember(embedded(
            "Ben Severn",
            "mood",
            "cheerful",
            20,
            [0.9, 0.1, 0.0],
        ))
        .unwrap();

        let hits = e.recall(&Query::new(vec![1.0, 0.0, 0.0], 2)).unwrap();
        let tired = hits
            .iter()
            .find(|h| h.value.as_deref() == Some("tired"))
            .unwrap();
        assert_eq!(tired.standing, Standing::Unsettled);
        assert!(
            tired.standing.still_stands(),
            "an open question is not a correction"
        );
    }

    #[test]
    fn one_correction_settles_a_slot_that_also_holds_additions() {
        // "another pet", "another pet", "actually she has none of them now".
        // The correction speaks about the whole slot, so it outranks the
        // additions stacked above the fact it corrects -- unanimity is required
        // to *keep* a fact standing, not to knock it down.
        let mut e = engine();
        let mut first = embedded("Ben Severn", "pet", "a dog", 10, [1.0, 0.0, 0.0]);
        first.supersession = Supersession::Joins;
        let mut second = embedded("Ben Severn", "pet", "a cat", 20, [0.9, 0.1, 0.0]);
        second.supersession = Supersession::Joins;
        e.remember(first).unwrap();
        e.remember(second).unwrap();
        e.remember(correcting(embedded(
            "Ben Severn",
            "pet",
            "none any more",
            30,
            [0.8, 0.2, 0.0],
        )))
        .unwrap();

        let hits = e.recall(&Query::new(vec![1.0, 0.0, 0.0], 3)).unwrap();
        for value in ["a dog", "a cat"] {
            let h = hits
                .iter()
                .find(|h| h.value.as_deref() == Some(value))
                .unwrap();
            assert_eq!(h.standing, Standing::Corrected, "{value}");
        }
    }

    #[test]
    fn two_assertions_at_one_instant_do_not_rank_each_other() {
        // One turn saying "I have a dog and a cat" writes both at the same
        // transaction time. Which one `Vec::push` reached first is not a fact
        // about the world, and reporting it as a correction would be a claim
        // built out of an index.
        let mut e = engine();
        e.remember(embedded("Ben Severn", "pet", "a dog", 10, [1.0, 0.0, 0.0]))
            .unwrap();
        e.remember(embedded("Ben Severn", "pet", "a cat", 10, [0.9, 0.1, 0.0]))
            .unwrap();

        let hits = e.recall(&Query::new(vec![1.0, 0.0, 0.0], 2)).unwrap();
        assert_eq!(hits.len(), 2);
        for h in &hits {
            assert_eq!(h.standing, Standing::Latest, "{:?}", h.value);
        }
    }

    #[test]
    fn forgetting_an_attribute_corrects_it_rather_than_leaving_it_open() {
        // "Stop telling me this" is the least ambiguous instruction the crate
        // takes, and it is checked in the history rather than through recall
        // because `forget` drops the attribute's vectors -- semantic recall is
        // meant to go quiet, which is the whole point of it.
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
            panic!("expected a new entity");
        };
        e.forget(
            entity,
            "employer",
            20,
            Provenance::new(Source::UserAssertion, 20, "session-2"),
        )
        .unwrap();

        let history = e.store_history(entity, "employer");
        assert_eq!(
            history.len(),
            2,
            "the tombstone is appended, not written over"
        );
        assert_eq!(
            history[1].supersession,
            Supersession::Corrects,
            "a tombstone leaves nothing under it standing"
        );
        assert!(
            e.recall(&Query::new(vec![1.0, 0.0, 0.0], 5))
                .unwrap()
                .is_empty(),
            "and recall goes quiet, which is what was asked for"
        );
    }

    #[test]
    fn a_replaced_fact_does_not_outrank_the_one_that_replaced_it() {
        // Measured: 3.2% of questions across ten conversations came back led by
        // an assertion something later had replaced, and 30% of those had the
        // live value sitting further down the same list -- rising to 61% once
        // `rm_extract::arity` began filling `Supersession`. An agent that reads
        // the first hit states the stale one.
        //
        // Acme is given the query's exact vector so it outscores Globex; the
        // demotion has to beat the score, or it is not doing anything.
        let mut e = engine();
        e.remember(embedded(
            "Ben Severn",
            "employer",
            "Acme",
            10,
            [1.0, 0.0, 0.0],
        ))
        .unwrap();
        e.remember(correcting(embedded(
            "Ben Severn",
            "employer",
            "Globex",
            20,
            [0.9, 0.1, 0.0],
        )))
        .unwrap();

        let hits = e.recall(&Query::new(vec![1.0, 0.0, 0.0], 5)).unwrap();
        assert_eq!(
            hits.len(),
            2,
            "both are still returned; only the order moves"
        );
        assert_eq!(
            hits[0].value.as_deref(),
            Some("Globex"),
            "the live value leads even though the replaced one scores higher"
        );
        assert_eq!(hits[1].value.as_deref(), Some("Acme"));
        assert_eq!(hits[1].standing, Standing::Corrected);
    }

    #[test]
    fn a_replaced_fact_keeps_its_place_when_nothing_replaced_it_here() {
        // Same slot only, and this is the case that rule exists for. A
        // corrected fact is not demoted below an unrelated live fact: relevance
        // is what the score is for, and "what did I believe about her employer
        // in May" needs the stale employer to lead when nothing current about
        // employment came back at all.
        let mut e = engine();
        e.remember(embedded(
            "Ben Severn",
            "employer",
            "Acme",
            10,
            [1.0, 0.0, 0.0],
        ))
        .unwrap();
        e.remember(correcting(embedded(
            "Ben Severn",
            "employer",
            "Globex",
            20,
            [0.0, 1.0, 0.0],
        )))
        .unwrap();
        e.remember(embedded("Ben Severn", "pet", "a dog", 30, [0.9, 0.1, 0.0]))
            .unwrap();

        // k=2 keeps Globex out of the results entirely, so Acme's slot has no
        // live answer present and nothing should move.
        let hits = e.recall(&Query::new(vec![1.0, 0.0, 0.0], 2)).unwrap();
        assert_eq!(
            hits[0].value.as_deref(),
            Some("Acme"),
            "no live employer came back, so the stale one still leads on score"
        );
        assert_eq!(hits[0].standing, Standing::Corrected);
    }

    #[test]
    fn demotion_is_stable_and_changes_no_membership() {
        // The guarantee that lets this sit beside recall@k without flattering
        // it: the same assertions come back, reordered.
        let mut e = engine();
        for (attr, value, at, v) in [
            ("employer", "Acme", 10, [1.0, 0.0, 0.0]),
            ("pet", "a dog", 11, [0.99, 0.01, 0.0]),
            ("city", "Bristol", 12, [0.98, 0.02, 0.0]),
        ] {
            e.remember(embedded("Ben Severn", attr, value, at, v))
                .unwrap();
        }
        e.remember(correcting(embedded(
            "Ben Severn",
            "employer",
            "Globex",
            20,
            [0.5, 0.5, 0.0],
        )))
        .unwrap();

        let hits = e.recall(&Query::new(vec![1.0, 0.0, 0.0], 10)).unwrap();
        let values: BTreeSet<&str> = hits.iter().filter_map(|h| h.value.as_deref()).collect();
        assert_eq!(
            values,
            ["Acme", "Bristol", "Globex", "a dog"].into_iter().collect(),
            "membership is untouched"
        );
        // The two that were never demoted keep their score order relative to
        // each other.
        let pos = |v: &str| {
            hits.iter()
                .position(|h| h.value.as_deref() == Some(v))
                .unwrap()
        };
        assert!(pos("a dog") < pos("Bristol"), "stable among the undemoted");
        assert!(pos("Globex") < pos("Acme"), "and the replaced one is last");
    }

    #[test]
    fn a_boost_lifts_the_right_subject_past_a_closer_stranger() {
        // The case the whole thing exists for. A question about Ben pulls up
        // Ada's fact because that is the sentence it sounds like; boosting Ben
        // puts his answer first without discarding hers.
        let mut e = engine();
        let Remembered::Created { entity: ben, .. } = e
            .remember(embedded(
                "Ben Severn",
                "employer",
                "Acme",
                10,
                [0.8, 0.6, 0.0],
            ))
            .unwrap()
        else {
            panic!("expected a new entity")
        };
        e.remember(embedded(
            "Ada Lovelace",
            "employer",
            "Globex",
            11,
            [1.0, 0.0, 0.0],
        ))
        .unwrap();

        let q = Query::new(vec![1.0, 0.0, 0.0], 2);
        let plain = e.recall(&q).unwrap();
        assert_eq!(
            plain[0].value.as_deref(),
            Some("Globex"),
            "unboosted, the closer stranger leads"
        );

        let boosted = e.recall(&q.clone().boosting([ben], 0.5)).unwrap();
        assert_eq!(boosted[0].value.as_deref(), Some("Acme"));
        assert_eq!(
            boosted.len(),
            2,
            "and the other one is still there -- this is a preference, not a filter"
        );
    }

    #[test]
    fn a_boost_reaches_past_k_rather_than_reordering_what_k_surfaced() {
        // The property that makes this worth putting inside the scan. Ben's
        // fact is the least similar of the four, so a caller that fetched the
        // top two and re-ranked them could never find it.
        let mut e = engine();
        let Remembered::Created { entity: ben, .. } = e
            .remember(embedded(
                "Ben Severn",
                "employer",
                "Acme",
                10,
                [0.1, 0.99, 0.0],
            ))
            .unwrap()
        else {
            panic!("expected a new entity")
        };
        for (name, value, at, v) in [
            ("Ada Lovelace", "employer", 11, [1.0, 0.0, 0.0]),
            ("Cal Vaughn", "employer", 12, [0.98, 0.02, 0.0]),
            ("Dee Okafor", "employer", 13, [0.96, 0.04, 0.0]),
        ] {
            e.remember(embedded(name, value, "Globex", at, v)).unwrap();
        }

        let q = Query::new(vec![1.0, 0.0, 0.0], 2);
        assert!(
            !e.recall(&q)
                .unwrap()
                .iter()
                .any(|h| h.value.as_deref() == Some("Acme")),
            "unboosted, Ben is nowhere near the top two"
        );
        let boosted = e.recall(&q.clone().boosting([ben], 0.9)).unwrap();
        assert_eq!(
            boosted[0].value.as_deref(),
            Some("Acme"),
            "boosting has to be able to reach an assertion k never surfaced"
        );
    }

    #[test]
    fn an_empty_boost_changes_nothing() {
        // A caller that could not identify a subject passes what it found
        // without branching, and gets the query it would have had.
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
            "Ada Lovelace",
            "employer",
            "Globex",
            11,
            [0.9, 0.1, 0.0],
        ))
        .unwrap();

        let q = Query::new(vec![1.0, 0.0, 0.0], 5);
        let plain = e.recall(&q).unwrap();
        assert_eq!(plain, e.recall(&q.clone().boosting([], 0.5)).unwrap());
        assert_eq!(plain, e.recall(&q.clone().boosting([1, 2], 0.0)).unwrap());
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
        assert_eq!(
            hits[0].standing,
            Standing::Latest,
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
        assert_eq!(
            e.forget(9999, "employer", 1, p),
            Err(EngineError::UnknownEntity(9999)),
            "the write path names the missing entity itself, not through a wrapper"
        );
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

        assert_eq!(
            e.erase(9999, "employer"),
            Err(EngineError::UnknownEntity(9999))
        );

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

        assert_eq!(
            e.erase(9999, "employer"),
            Err(EngineError::UnknownEntity(9999))
        );

        assert!(
            e.assertions.contains_key(&0),
            "drop_vectors must not run before store.erase has a chance to fail"
        );
        assert_eq!(e.index_len(), 1);
    }

    #[test]
    fn an_engine_relates_two_entities_it_remembered() {
        let mut e = engine();
        let Remembered::Created { entity: alice, .. } = e
            .remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap()
        else {
            panic!("setup")
        };
        let Remembered::Created { entity: acme, .. } = e
            .remember(observation("Acme Corp", "kind", "company", 2))
            .unwrap()
        else {
            panic!("setup")
        };

        e.relate(
            alice,
            "employed_by",
            acme,
            Interval::since(1),
            Provenance::new(Source::UserAssertion, 1, "s"),
        )
        .unwrap();

        let n = e.neighborhood(&Walk::new(vec![alice], 1, 10, 5, 5));
        assert_eq!(n.reached.len(), 2);
        assert!(n.reached.iter().any(|r| r.entity == acme));
    }

    #[test]
    fn relating_to_an_entity_the_engine_never_met_is_an_error() {
        let mut e = engine();
        let Remembered::Created { entity: alice, .. } = e
            .remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap()
        else {
            panic!("setup")
        };
        assert!(e
            .relate(
                alice,
                "employed_by",
                9999,
                Interval::since(1),
                Provenance::new(Source::UserAssertion, 1, "s"),
            )
            .is_err());
    }

    #[test]
    fn unrelate_stops_a_walk_crossing_without_erasing_that_it_held() {
        let mut e = engine();
        let Remembered::Created { entity: alice, .. } = e
            .remember(observation("Ben Severn", "employer", "Acme", 1))
            .unwrap()
        else {
            panic!("setup")
        };
        let Remembered::Created { entity: acme, .. } = e
            .remember(observation("Acme Corp", "kind", "company", 2))
            .unwrap()
        else {
            panic!("setup")
        };
        let prov = Provenance::new(Source::UserAssertion, 1, "s");
        e.relate(alice, "employed_by", acme, Interval::since(1), prov.clone())
            .unwrap();
        e.unrelate(
            alice,
            "employed_by",
            acme,
            5,
            Provenance::new(Source::UserAssertion, 5, "s2"),
        )
        .unwrap();

        assert_eq!(
            e.neighborhood(&Walk::new(vec![alice], 1, 10, 7, 9))
                .reached
                .len(),
            1
        );
        assert_eq!(
            e.neighborhood(&Walk::new(vec![alice], 1, 10, 3, 9))
                .reached
                .len(),
            2
        );
    }

    #[test]
    fn erasing_edges_leaves_attributes_untouched() {
        // Erasing relationships and erasing attributes are different
        // requests, answered by different methods (`erase_edges` and
        // `erase`), and neither should have a side effect that belongs to
        // the other. This proves the edge half: after `erase_edges`, the
        // attribute recorded on the same entity is still there, both to
        // `about` and to `recall`.
        let mut e = engine();
        let Remembered::Created { entity: alice, .. } = e
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
        let Remembered::Created { entity: acme, .. } = e
            .remember(observation("Acme Corp", "kind", "company", 2))
            .unwrap()
        else {
            panic!("setup")
        };
        e.relate(
            alice,
            "employed_by",
            acme,
            Interval::since(1),
            Provenance::new(Source::UserAssertion, 1, "s"),
        )
        .unwrap();

        assert_eq!(e.erase_edges(alice).unwrap(), 1);

        assert_eq!(
            e.neighborhood(&Walk::new(vec![alice], 1, 10, 5, 5))
                .reached
                .len(),
            1,
            "the edge is gone"
        );
        assert_eq!(
            e.about(alice, "employer", 15, 30).unwrap(),
            Believed::Value("Acme".into()),
            "the attribute erase_edges was never asked to touch is untouched"
        );
        assert!(
            !e.recall(&Query::new(vec![1.0, 0.0, 0.0], 5))
                .unwrap()
                .is_empty(),
            "the attribute's vector is still searchable"
        );
    }

    #[test]
    fn erasing_an_attribute_leaves_edges_untouched() {
        // The mirror of the test above: `erase` on one attribute must not
        // disturb a relationship recorded on the same entity.
        let mut e = engine();
        let Remembered::Created { entity: alice, .. } = e
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
        let Remembered::Created { entity: acme, .. } = e
            .remember(observation("Acme Corp", "kind", "company", 2))
            .unwrap()
        else {
            panic!("setup")
        };
        e.relate(
            alice,
            "employed_by",
            acme,
            Interval::since(1),
            Provenance::new(Source::UserAssertion, 1, "s"),
        )
        .unwrap();

        assert_eq!(e.erase(alice, "employer").unwrap(), 1);

        assert_eq!(
            e.about(alice, "employer", 15, 30).unwrap(),
            Believed::Unknown
        );
        let n = e.neighborhood(&Walk::new(vec![alice], 1, 10, 5, 5));
        assert_eq!(n.reached.len(), 2, "the relationship survived the erase");
        assert!(n.reached.iter().any(|r| r.entity == acme));
    }

    #[test]
    fn erasing_edges_on_an_unknown_entity_is_an_error() {
        let mut e = engine();
        assert_eq!(
            e.erase_edges(9999),
            Err(EngineError::UnknownEntity(9999)),
            "the write path names the missing entity itself, not through a wrapper"
        );
    }

    #[test]
    fn identity_of_names_a_review_pair_and_is_none_for_an_id_nothing_holds() {
        let mut e = engine();
        let obs = observation("Ben Severn", "employer", "Globex", 100);
        let id = match e.remember(obs).unwrap() {
            Remembered::Created { entity, .. } => entity,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            e.identity_of(id).and_then(|r| r.get("name")),
            Some("Ben Severn")
        );
        // An id nothing holds is an absence, not a panic and not a blank
        // record: a caller rendering a review needs to tell those apart.
        assert!(e.identity_of(id + 999).is_none());
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
            "setup: the absorbed entity is still in the store, holding nothing -- that is the trap this test exists for"
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

    /// Like `Buckets`, except for one text it fails on. Selective rather than
    /// blanket-failing, so a test can put the failure anywhere in an
    /// extraction -- a second mention, or a fact rather than its subject --
    /// and prove nothing written before it survives.
    struct FailsOn(&'static str);

    impl Embedder for FailsOn {
        fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedderError> {
            if text == self.0 {
                return Err(EmbedderError("the embedding service is down".to_string()));
            }
            Buckets.embed(text)
        }
    }

    fn mention(kind: &str, name: &str) -> rm_extract::Mention {
        rm_extract::Mention {
            kind: kind.to_string(),
            name: name.to_string(),
            text: name.to_string(),
        }
    }

    fn a_turn() -> rm_extract::Turn {
        rm_extract::Turn {
            text: "Ben works at Globex in Bristol".to_string(),
            speaker: Some("Ben Severn".to_string()),
            observed_at: 100,
            session: "session-1".to_string(),
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

        let out = e.ingest(&a_turn(), &extraction, &Buckets).unwrap();
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
        let out = e.ingest(&a_turn(), &extraction, &Buckets).unwrap();
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

        let err = e.ingest(&a_turn(), &extraction, &NoEmbedder).unwrap_err();
        assert!(matches!(err, EngineError::Embed(_)), "{err:?}");
        assert!(
            err.to_string().contains("embedding service is down"),
            "the host's own explanation must survive: {err}"
        );
        assert_eq!(e.entity_count(), 0);
        assert_eq!(e.index_len(), 0);
    }

    #[test]
    fn a_failing_embedder_on_a_later_mention_leaves_no_earlier_write_behind() {
        // A single-mention extraction cannot tell "embed everything, then
        // write everything" apart from "embed one, write one, embed the
        // next" -- `remember` already validates a lone vector before writing
        // it, so that case passes either way. Two mentions, failing on the
        // second, is what actually exercises the outer pass: an interleaved
        // implementation would have written the first mention before ever
        // reaching the second's embedding.
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn"), mention("place", "Bristol")],
            ..Default::default()
        };

        let err = e
            .ingest(&a_turn(), &extraction, &FailsOn("Bristol"))
            .unwrap_err();
        assert!(matches!(err, EngineError::Embed(_)), "{err:?}");
        assert_eq!(e.entity_count(), 0, "the first mention must not survive");
        assert_eq!(e.index_len(), 0);
    }

    #[test]
    fn a_failing_embedder_on_a_fact_also_leaves_the_store_and_the_index_untouched() {
        // The trap the two-phase pass exists to close: embedding a fact's
        // text inside the fact loop would let the mention above it land
        // first, and the mention-only tests above cannot show that -- this is
        // the case that would catch it.
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn")],
            facts: vec![rm_extract::Fact {
                subject: 0,
                attribute: "employer".to_string(),
                value: Some("Globex".to_string()),
                text: "Ben works at Globex".to_string(),
                valid_from: 100,
                supersession: Supersession::Corrects,
            }],
            ..Default::default()
        };

        let err = e
            .ingest(&a_turn(), &extraction, &FailsOn("Ben works at Globex"))
            .unwrap_err();
        assert!(matches!(err, EngineError::Embed(_)), "{err:?}");
        assert_eq!(e.entity_count(), 0, "the mention must not survive either");
        assert_eq!(e.index_len(), 0);
    }

    #[test]
    fn a_fact_naming_a_mention_that_does_not_exist_is_an_error_not_a_panic() {
        // `Extraction`'s fields are `pub` and `ingest` is `pub`, so a
        // hand-built extraction with an out-of-range subject is a caller
        // mistake `ingest` has to report, not a guarantee only `extract`
        // enforces -- this is exactly how every test in this crate builds one.
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn")],
            facts: vec![rm_extract::Fact {
                subject: 1,
                attribute: "employer".to_string(),
                value: Some("Globex".to_string()),
                text: "Ben works at Globex".to_string(),
                valid_from: 100,
                supersession: Supersession::Corrects,
            }],
            ..Default::default()
        };

        let err = e.ingest(&a_turn(), &extraction, &Buckets).unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::BadMentionIndex {
                    what: "fact subject",
                    index: 1,
                    mentions: 1,
                }
            ),
            "{err:?}"
        );
        assert_eq!(e.entity_count(), 0, "checked before the first write");
        assert_eq!(e.index_len(), 0);
    }

    #[test]
    fn a_relation_naming_a_mention_that_does_not_exist_is_an_error_not_a_panic() {
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn")],
            relations: vec![rm_extract::Relation {
                subject: 0,
                predicate: "employed_by".to_string(),
                object: 1,
                valid_from: 100,
            }],
            ..Default::default()
        };

        let err = e.ingest(&a_turn(), &extraction, &Buckets).unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::BadMentionIndex {
                    what: "relation object",
                    index: 1,
                    mentions: 1,
                }
            ),
            "{err:?}"
        );
        assert_eq!(e.entity_count(), 0, "checked before the first write");
        assert_eq!(e.index_len(), 0);
    }

    #[test]
    fn a_closure_naming_a_mention_that_does_not_exist_is_an_error_not_a_panic() {
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn")],
            closures: vec![rm_extract::Closure {
                subject: 1,
                predicate: "employed_by".to_string(),
                at: 100,
                because: "a new job".to_string(),
            }],
            ..Default::default()
        };

        let err = e.ingest(&a_turn(), &extraction, &Buckets).unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::BadMentionIndex {
                    what: "closure subject",
                    index: 1,
                    mentions: 1,
                }
            ),
            "{err:?}"
        );
        assert_eq!(e.entity_count(), 0, "checked before the first write");
        assert_eq!(e.index_len(), 0);
    }

    #[test]
    fn the_assertions_come_back_in_write_order_so_a_caller_can_tell_what_produced_each() {
        // Mentions first, in the extraction's mention order, then facts in the
        // extraction's fact order. An `AssertionId` carries nothing about its
        // origin, and a mention's `kind` assertion has the same shape as a
        // fact's, so this ordering is the only route from an id back to the
        // sentence it came from. Pinned here because it is documented on
        // `Ingested::assertions` as a promise, and a promise nothing checks is
        // one a later refactor can quietly break.
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn"), mention("place", "Bristol")],
            facts: vec![
                rm_extract::Fact {
                    subject: 0,
                    attribute: "employer".to_string(),
                    value: Some("Globex".to_string()),
                    text: "Ben works at Globex".to_string(),
                    valid_from: 100,
                    supersession: Supersession::Unstated,
                },
                rm_extract::Fact {
                    subject: 1,
                    attribute: "country".to_string(),
                    value: Some("England".to_string()),
                    text: "Bristol is in England".to_string(),
                    valid_from: 100,
                    supersession: Supersession::Unstated,
                },
            ],
            ..Default::default()
        };

        let out = e.ingest(&a_turn(), &extraction, &Buckets).unwrap();
        assert_eq!(
            out.assertions.len(),
            4,
            "one per mention, then one per fact"
        );

        let at = |i: usize| e.assertion(out.assertions[i]).unwrap().clone();
        assert_eq!(at(0).attribute, "kind");
        assert_eq!(at(0).entity, out.entities[0]);
        assert_eq!(at(1).attribute, "kind");
        assert_eq!(at(1).entity, out.entities[1]);
        assert_eq!(at(2).attribute, "employer");
        assert_eq!(at(2).entity, out.entities[0]);
        assert_eq!(at(3).attribute, "country");
        assert_eq!(at(3).entity, out.entities[1]);
    }

    #[test]
    fn a_mention_with_no_name_is_refused_before_it_can_be_resolved_against_anything() {
        // The third of `extract`'s three refusals, and the one `ingest` was
        // missing. It matters more here than it does there: `ingest` resolves a
        // mention on a `Record` carrying nothing but `name`, so an empty name
        // leaves resolution with no evidence at all -- and it does not report
        // that, it just scores what it was given.
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "  ")],
            ..Default::default()
        };

        let err = e.ingest(&a_turn(), &extraction, &Buckets).unwrap_err();
        assert!(matches!(err, EngineError::NamelessMention(0)), "{err:?}");
        assert_eq!(e.entity_count(), 0, "checked before the first write");
        assert_eq!(e.index_len(), 0);
    }

    #[test]
    fn two_nameless_mentions_would_have_landed_on_one_entity_which_is_why_ingest_refuses_them() {
        // The reason for the check above, demonstrated rather than asserted in
        // a comment. `BlockingKey::Prefix("name", 3)` on an empty name yields
        // the key `name~`, so every nameless mention lands in the same block,
        // and inside it each scores against every other on the one field they
        // share -- which is blank on both sides and therefore agrees.
        //
        // What that agreement then buys depends on the ruleset. This one trusts
        // a name match on its own, so the second nameless thing merges into the
        // first: two distinct things, one entity, and nothing anywhere says the
        // match was made on nothing. `test_ruleset` is stricter and files a
        // review instead -- also wrong, and also not an error, just louder.
        // Refusing the mention is the only outcome that does not depend on how
        // a host happened to tune its thresholds.
        let mut e = Engine::new(
            VectorIndex::new(3, Metric::Cosine),
            Ruleset::new(
                vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
                vec![BlockingKey::Prefix("name".to_string(), 3)],
                2.0,
                4.0,
            )
            .unwrap(),
            Policy::new(Strategy::MostRecent),
        );
        let nameless = |at: Timestamp| Observation {
            kind: "person".to_string(),
            mention: Record::new().with("name", ""),
            attribute: "kind".to_string(),
            value: Some("person".to_string()),
            valid: Interval::since(at),
            provenance: Provenance::new(Source::ToolOutput, at, "session-1"),
            supersession: Supersession::Unstated,
            according_to: None,
            embedding: vec![1.0, 0.0, 0.0],
        };

        assert!(matches!(
            e.remember(nameless(1)).unwrap(),
            Remembered::Created { .. }
        ));
        let second = e.remember(nameless(2)).unwrap();
        assert!(
            matches!(second, Remembered::Merged { .. }),
            "a blank name matching a blank name is what `ingest` has to refuse: {second:?}"
        );
        assert_eq!(e.entity_count(), 1, "two things became one, silently");
    }

    #[test]
    fn a_relation_from_a_mention_to_itself_is_an_error_costing_nothing_not_a_late_store_refusal() {
        // `rm_store::relate` already refuses a self-edge, so letting this
        // through to `Engine::relate` would still end in an error -- just
        // after the mention loop above had already written Ben. Checked
        // alongside the index-range checks instead, so a relation that can
        // never be created costs nothing rather than merely failing no worse
        // than it would have anyway.
        let mut e = engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![mention("person", "Ben Severn")],
            relations: vec![rm_extract::Relation {
                subject: 0,
                predicate: "employed_by".to_string(),
                object: 0,
                valid_from: 100,
            }],
            ..Default::default()
        };

        let err = e.ingest(&a_turn(), &extraction, &Buckets).unwrap_err();
        assert!(matches!(err, EngineError::SelfRelation(0)), "{err:?}");
        assert_eq!(e.entity_count(), 0, "checked before the first write");
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
        e.ingest(&a_turn(), &first, &Buckets).unwrap();

        let second = rm_extract::Extraction {
            mentions: vec![rm_extract::Mention {
                kind: "person".to_string(),
                name: "Ben Severne".to_string(),
                text: "Ben Severne".to_string(),
            }],
            ..Default::default()
        };
        let out = e.ingest(&a_turn(), &second, &Buckets).unwrap();

        assert_eq!(out.reviews.len(), 1, "the near-miss has to be asked about");
        assert_eq!(
            e.entity_count(),
            2,
            "and they stay apart until someone answers"
        );
        assert_eq!(e.pending_review().len(), 1);
    }

    #[test]
    fn ingesting_nothing_writes_nothing_and_is_not_an_error() {
        let mut e = engine();
        let out = e
            .ingest(&a_turn(), &rm_extract::Extraction::default(), &Buckets)
            .unwrap();
        assert!(out.entities.is_empty());
        assert_eq!(e.entity_count(), 0);
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
                supersession: Supersession::Corrects,
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
                supersession: Supersession::Corrects,
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
            mentions: vec![
                mention("person", "Ben Severn"),
                mention("organisation", "Globex"),
            ],
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
                supersession: Supersession::Corrects,
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
                supersession: Supersession::Unstated,
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

    /// `test_ruleset`'s `match_at` is 8.0, but two identical name-only records
    /// score at most `log2(0.9/0.01) ~= 6.49` bits under its "name" rule --
    /// `city` never enters the comparison because `ingest` never puts one in
    /// a mention's `Record`. So under `test_ruleset` a second turn's mention
    /// of someone already known can only ever land in the review band and
    /// create a new entity, never merge back onto the one the first turn
    /// created. The closure tests below need exactly that merge, to prove a
    /// closure sees an edge a *prior* turn wrote, so they get their own
    /// ruleset with a `match_at` a repeated name can actually clear -- 6.0,
    /// the same value `tests/readme.rs` uses for the identical reason. This
    /// is a property of the fixture, not of `ingest`: a caller supplying a
    /// ruleset whose `match_at` a name can reach merges fine, which is what
    /// `tests/readme.rs` demonstrates end to end.
    fn closure_ruleset() -> Ruleset {
        Ruleset::new(
            vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
            vec![BlockingKey::Prefix("name".to_string(), 3)],
            4.0,
            6.0,
        )
        .unwrap()
    }

    fn closure_engine() -> Engine {
        Engine::new(
            VectorIndex::new(3, Metric::Cosine),
            closure_ruleset(),
            Policy::new(Strategy::MostRecent),
        )
    }

    #[test]
    fn a_closure_ends_the_prior_edge_and_records_that_an_agent_inferred_it() {
        // "I started at Globex" does not say Ben left Acme. Closing that edge
        // is an inference, and it is recorded as one -- traceable in
        // edge_history, and outrankable by anything the user says directly.
        let mut e = closure_engine();
        let first = rm_extract::Extraction {
            mentions: vec![
                mention("person", "Ben Severn"),
                mention("organisation", "Acme"),
            ],
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
            mentions: vec![
                mention("person", "Ben Severn"),
                mention("organisation", "Globex"),
            ],
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
        assert_eq!(
            out.closed[0].because,
            "starting a new job ends the previous one"
        );

        // The walk no longer crosses to Acme, but does reach Globex.
        let now = Walk::new(vec![ben], 1, 10, 150, 300);
        let reached: Vec<_> = e
            .neighborhood(&now)
            .reached
            .iter()
            .map(|r| r.entity)
            .collect();
        assert!(!reached.contains(&acme), "Acme should be behind us");
        assert!(reached.contains(&out.entities[1]), "Globex should not be");

        // And the tombstone says who concluded it.
        let history = e.edge_history(ben, "employed_by", acme);
        assert_eq!(
            history.last().unwrap().provenance.source,
            Source::AgentInference
        );
        assert!(!history.last().unwrap().present);
    }

    #[test]
    fn a_closure_does_not_end_an_edge_asserted_in_the_same_turn() {
        // Otherwise "I moved from Acme to Globex" would close Globex as fast as
        // it opened it.
        let mut e = closure_engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![
                mention("person", "Ben Severn"),
                mention("organisation", "Globex"),
            ],
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
        let mut e = closure_engine();
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
            e.neighborhood(&walk)
                .reached
                .iter()
                .any(|r| r.entity == bristol),
            "where he lives has nothing to do with where he works"
        );
    }

    #[test]
    fn a_closure_spares_a_same_turn_edge_named_by_a_different_mention_of_its_subject() {
        // `extract` does not dedupe mentions: the same person can appear at
        // more than one local index in one turn. Ben is mentioned at index 0
        // (the relation's subject) and again at index 2 (the closure's
        // subject); both resolve to the same entity. Comparing `spared` by
        // local index rather than resolved entity id would miss this and
        // wrongly close the edge this same turn just asserted.
        let mut e = closure_engine();
        let extraction = rm_extract::Extraction {
            mentions: vec![
                mention("person", "Ben Severn"),
                mention("organisation", "Globex"),
                mention("person", "Ben Severn"),
            ],
            relations: vec![rm_extract::Relation {
                subject: 0,
                predicate: "employed_by".to_string(),
                object: 1,
                valid_from: 100,
            }],
            closures: vec![rm_extract::Closure {
                subject: 2,
                predicate: "employed_by".to_string(),
                at: 100,
                because: "a new job ends the old one".to_string(),
            }],
            ..Default::default()
        };
        let out = e.ingest(&a_turn(), &extraction, &Buckets).unwrap();

        assert_eq!(
            out.entities[0], out.entities[2],
            "both mentions of Ben Severn must resolve to the same entity for this test to mean anything"
        );
        assert!(
            out.closed.is_empty(),
            "the edge this same turn asserted from index 0 must not be closed just because the closure named the same subject at index 2"
        );
        let walk = Walk::new(vec![out.entities[0]], 1, 10, 150, 300);
        assert!(
            e.neighborhood(&walk)
                .reached
                .iter()
                .any(|r| r.entity == out.entities[1]),
            "Globex should still be reachable"
        );
    }

    /// Write one assertion, optionally giving its entity a reach, and return
    /// the entity. The scope goes in as an ordinary attribute because that is
    /// how `decide` writes it, and the filter has to find it the same way.
    fn scoped(e: &mut Engine, scope: Option<&str>, attribute: &str, at: Timestamp) -> StableId {
        let (id, _) = e
            .remember_as(None, observation(attribute, attribute, "a value", at))
            .expect("pinned write");
        if let Some(scope) = scope {
            e.remember_as(Some(id), observation(attribute, "scope", scope, at))
                .expect("pinned write");
        }
        id
    }

    /// A recall answers from where it is asked. The filter runs inside the
    /// scan, so `k` still means "k results that apply" rather than "k
    /// candidates, some of which survive".
    #[test]
    fn a_recall_returns_only_what_reaches_the_position_it_was_asked_from() {
        let mut e = engine();
        scoped(&mut e, Some("*"), "everywhere", 1_000);
        scoped(&mut e, Some("work/goldenmatch"), "here", 1_100);
        scoped(&mut e, Some("work/other"), "sibling", 1_200);

        let named = |hits: Vec<Recalled>| {
            let mut v: Vec<String> = hits
                .into_iter()
                .filter(|r| r.attribute != "scope")
                .map(|r| r.attribute)
                .collect();
            v.sort();
            v
        };

        let all = named(e.recall(&Query::new(vec![1.0, 0.0, 0.0], 50)).unwrap());
        assert_eq!(all, vec!["everywhere", "here", "sibling"], "unscoped");

        let here = named(
            e.recall(&Query::new(vec![1.0, 0.0, 0.0], 50).at("work/goldenmatch"))
                .unwrap(),
        );
        assert_eq!(
            here,
            vec!["everywhere", "here"],
            "the universal one and this project's, not the sibling"
        );

        let elsewhere = named(
            e.recall(&Query::new(vec![1.0, 0.0, 0.0], 50).at("personal"))
                .unwrap(),
        );
        assert_eq!(elsewhere, vec!["everywhere"], "only the universal one");
    }

    /// An entity with no scope recorded reaches everywhere, exactly as in the
    /// decision reads. `remember`'s facts carry none, so a scoped recall must
    /// never hide them.
    #[test]
    fn an_assertion_whose_entity_has_no_scope_is_never_hidden() {
        let mut e = engine();
        scoped(&mut e, None, "unscoped", 1_000);
        let hits = e
            .recall(&Query::new(vec![1.0, 0.0, 0.0], 50).at("anywhere/at/all"))
            .unwrap();
        assert_eq!(hits.len(), 1, "no scope recorded means it reaches here");
    }
    /// `attributes_of` names what `store_history` can be asked about.
    ///
    /// The pair is the point: `store_history` needs a name, and until this
    /// existed there was no way to obtain one, so a store could not be walked
    /// at all from outside.
    #[test]
    fn attributes_of_names_every_slot_and_nothing_else() {
        let mut e = engine();
        let (id, _) = e
            .remember_as(None, observation("Ben Severn", "employer", "Acme", 1))
            .unwrap();
        e.remember_as(Some(id), observation("Ben Severn", "spouse", "Sam", 2))
            .unwrap();

        let names = e.attributes_of(id);
        assert!(
            names.contains(&"employer") && names.contains(&"spouse"),
            "{names:?}"
        );
        for name in &names {
            assert!(
                !e.store_history(id, name).is_empty(),
                "{name} was named but has no history"
            );
        }

        // An entity nobody has heard of reads as nothing, not as an error.
        assert!(e.attributes_of(9_999).is_empty());
    }
}
