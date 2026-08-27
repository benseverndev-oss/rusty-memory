//! Snapshot and restore.
//!
//! The [`Ruleset`] and [`Policy`] are *not* persisted. They are configuration,
//! not state: a caller restoring an engine has them already, and writing them
//! into the snapshot would let a stale copy on disk silently override the
//! thresholds the caller believes are in force. That is a bug with no symptom —
//! nothing errors, the same mention simply resolves under rules nobody asked
//! for.

use std::collections::{BTreeMap, BTreeSet};

use rm_index::VectorIndex;
use rm_resolve::{Record, Ruleset};
use rm_store::{MemoryStore, StableId};
use serde::{Deserialize, Serialize};

use crate::{
    AssertionId, AssertionRef, Engine, EngineError, PendingReview, Policy, ReviewId, Settled,
};

/// Everything an engine has to carry across a restart.
///
/// `BTreeMap`/`BTreeSet` throughout, so the bytes are stable for a given state
/// and two engines can be compared by diffing their snapshots.
#[derive(Serialize, Deserialize)]
struct Persisted {
    /// The store's *own* snapshot, carried as an opaque string for the same
    /// reason as `index` below: it is the only way in that goes through
    /// [`MemoryStore::open`].
    ///
    /// A nested `MemoryStore` would arrive by derived `Deserialize`, which
    /// bypasses that door entirely — the store would come in through a window
    /// while the index came in through a validating entrance. `MemoryStore::open`
    /// checks that `next_id` still exceeds every entity it holds, and a snapshot
    /// that fails that check makes the next `create_entity` return a live id and
    /// overwrite an existing entity, silently. Same failure class as a rewound
    /// `next_assertion`, one layer down.
    store: String,
    /// The index's *own* snapshot, carried as an opaque string rather than as a
    /// nested [`VectorIndex`].
    ///
    /// A nested `VectorIndex` would round-trip through its derived
    /// `Deserialize`, and its `positions` map is `#[serde(skip)]` — so the
    /// restored index would come back with an empty id-to-row map. Nothing
    /// would error: `contains` would answer false for every id that is
    /// genuinely present, `remove` would find nothing, and the very check below
    /// that a snapshot's assertions all have vectors would reject every valid
    /// snapshot ever written. Holding the string instead means restore goes
    /// through [`VectorIndex::open`], which rebuilds that map *and* enforces the
    /// index's own invariants — validation this crate would otherwise have to
    /// write a second, weaker copy of.
    ///
    /// The cost is nested JSON, which is less pleasant to read by hand. It is
    /// still deterministic, so byte-stability holds.
    index: String,
    /// The authoritative set of entities the engine knows about — see
    /// [`Engine::open`] for why the store's entity list is not.
    identity: BTreeMap<StableId, Record>,
    assertions: BTreeMap<AssertionId, AssertionRef>,
    review: BTreeMap<ReviewId, PendingReview>,
    rejected: BTreeSet<Settled>,
    next_assertion: AssertionId,
    next_review: ReviewId,
    /// Sources read from, as distinct from written by. See
    /// `Engine::read`.
    ///
    /// `default` and `skip_serializing_if` so a store that has never
    /// been ingested into writes no field and a snapshot from before
    /// this existed still loads -- the same treatment `Version`'s
    /// `supersession` and `according_to` already carry.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    read: BTreeSet<String>,
}

impl Engine {
    /// Serialise to canonical JSON.
    ///
    /// Ordered containers throughout, so the bytes are stable for a given state
    /// and a snapshot diffs against its predecessor rather than reshuffling.
    pub fn snapshot(&self) -> String {
        self.persisted(self.index.snapshot())
    }

    /// The snapshot that goes to disk, with the vectors left for
    /// [`Engine::vectors_le`] to write beside it.
    ///
    /// [`Engine::snapshot`] keeps the self-contained shape, and is still what
    /// round-trips through [`Engine::open`]. This is the pair the host uses,
    /// because measured on a real store the vectors were 96.9% of the file and
    /// every write rewrote all of them to record one assertion.
    pub fn snapshot_meta(&self) -> String {
        self.persisted(self.index.snapshot_meta())
    }

    fn persisted(&self, index: String) -> String {
        let p = Persisted {
            store: self.store.snapshot(),
            index,
            read: self.read.clone(),
            identity: self.identity.clone(),
            assertions: self.assertions.clone(),
            review: self.review.clone(),
            rejected: self.rejected.clone(),
            next_assertion: self.next_assertion,
            next_review: self.next_review,
        };
        serde_json::to_string(&p).expect("Engine is always serialisable")
    }

    /// Restore from a snapshot, with the configuration supplied fresh.
    ///
    /// Validates before returning. A restore path is a door, and this workspace
    /// has already paid once for treating one as a formality: `rm_index`
    /// shipped an `open` that accepted structurally-valid nonsense and panicked
    /// on the first query. Everything checked here is something the rest of the
    /// crate indexes on faith — an assertion is read by looking up its entity,
    /// then that entity's version log at a stored position, then its vector.
    /// Each of those is a panic or a confident lie if the snapshot disagrees
    /// with itself.
    ///
    /// The entity set is rebuilt from `identity`, never from the store's entity
    /// list. [`Engine::confirm`] merges by copying the absorbed entity's
    /// versions onto the survivor and then erasing them, which leaves the
    /// absorbed entity present in the store with no attributes while
    /// `identity` — the engine's actual notion of who exists — correctly drops
    /// it. Trusting the store's list would resurrect a merged-away entity on
    /// every reload, and the merge would quietly undo itself across a restart.
    ///
    /// The store and the index are both restored through their own crates'
    /// `open`, never by derived `Deserialize`, so each arrives having enforced
    /// invariants this crate does not have to know about — `MemoryStore::open`
    /// checks its `next_id`, `VectorIndex::open` rebuilds its id-to-row map and
    /// checks its rows. What is left here is only what neither of them can see:
    /// whether the two agree with each other, and with this crate's own state.
    /// The vectors, to be written beside the snapshot.
    ///
    /// [`Engine::snapshot`] no longer carries them: measured on a real store
    /// they were 96.9% of it, and every write rewrote all of them to record one
    /// assertion. See [`rm_index::VectorIndex::snapshot_meta`].
    pub fn vectors_le(&self) -> Vec<u8> {
        self.index.vectors_le()
    }

    /// Restore from a snapshot and the vector file beside it.
    ///
    /// `vectors` is `None` for a store written before the two were split, whose
    /// snapshot still carries its own. That is not a legacy path to be tidied
    /// away later -- it is what lets an existing store be opened and migrated
    /// on its next save rather than refused.
    pub fn open_split(
        snapshot: &str,
        vectors: Option<&[u8]>,
        ruleset: Ruleset,
        policy: Policy,
    ) -> Result<Engine, EngineError> {
        match vectors {
            None => Engine::open(snapshot, ruleset, policy),
            Some(bytes) => Engine::restore(snapshot, ruleset, policy, |meta| {
                VectorIndex::open_split(meta, bytes)
            }),
        }
    }

    pub fn open(snapshot: &str, ruleset: Ruleset, policy: Policy) -> Result<Engine, EngineError> {
        Engine::restore(snapshot, ruleset, policy, VectorIndex::open)
    }

    /// The shared body, told how to turn `index` into one.
    ///
    /// A closure rather than two copies: everything below is the same for both
    /// shapes, and the pair drifting apart is a store that opens through one
    /// door and not the other.
    fn restore(
        snapshot: &str,
        ruleset: Ruleset,
        policy: Policy,
        open_index: impl FnOnce(&str) -> Result<VectorIndex, rm_index::IndexError>,
    ) -> Result<Engine, EngineError> {
        let p: Persisted = serde_json::from_str(snapshot)
            .map_err(|e| EngineError::CorruptSnapshot(e.to_string()))?;

        // Both nested crates come in through their own door rather than by
        // derived `Deserialize`, so each reimplements nothing here: the store
        // checks its own `next_id`, and the index rebuilds its derived position
        // map and rejects rows and ids that disagree. `StoreError` and
        // `IndexError` both convert, so a corrupt one arrives as itself rather
        // than flattened into a string that loses which invariant broke.
        let store = MemoryStore::open(&p.store)?;
        let index = open_index(&p.index)?;

        // The counters name the *next* id to hand out, so anything at or below
        // an id already in use is a snapshot that lies on its first write
        // rather than on load. Nothing downstream can catch it: `remember`
        // would call `assertions.insert`, which overwrites the existing
        // `AssertionRef`, and `VectorIndex::insert`, which overwrites an
        // existing id's vector in place rather than erroring. One stored fact
        // would be destroyed, `index_len()` would not move, and the call would
        // return `Ok`. That is the same "parses but lies on first use" shape
        // the rest of this block exists to reject, so it is rejected here.
        if let Some(&highest) = p.assertions.keys().next_back() {
            if p.next_assertion <= highest {
                return Err(EngineError::CorruptSnapshot(format!(
                    "next_assertion is {}, but assertion {highest} exists, so the next write would overwrite it",
                    p.next_assertion
                )));
            }
        }
        // The same shape one queue over. A reused review id overwrites an open
        // question, which is a question someone was going to be asked and now
        // never will be.
        if let Some(&highest) = p.review.keys().next_back() {
            if p.next_review <= highest {
                return Err(EngineError::CorruptSnapshot(format!(
                    "next_review is {}, but review {highest} is open, so the next question would overwrite it",
                    p.next_review
                )));
            }
        }

        // The mirror of the assertion check below. `identity` is the engine's
        // entity set, and an entry in it that the store has never heard of is
        // not inert: it is keyed into the blocking map by `rebuild_blocks`, so
        // it stands in front of every future mention as a candidate, and a
        // `Match` against it sends `remember` into `store.assert` with an id
        // the store rejects — an `UnknownEntity` naming an entity the caller
        // was just told exists. One check at the door instead of a misleading
        // error on the first write that happens to resolve this way.
        for id in p.identity.keys() {
            if store.entity(*id).is_none() {
                return Err(EngineError::CorruptSnapshot(format!(
                    "identity {id} is not an entity the store holds, so resolving \
                     against it would produce a match nothing can be written to"
                )));
            }
        }

        for (id, entry) in &p.assertions {
            if store.entity(entry.entity).is_none() {
                return Err(EngineError::CorruptSnapshot(format!(
                    "assertion {id} belongs to entity {}, which the store does not hold",
                    entry.entity
                )));
            }
            // `identity` is the engine's entity set, so an assertion naming an
            // entity missing from it is a fact nothing can ever resolve
            // against: recall would return it, `entity_count` would not count
            // it, and the next mention of that person would create a duplicate
            // rather than merging.
            if !p.identity.contains_key(&entry.entity) {
                return Err(EngineError::CorruptSnapshot(format!(
                    "assertion {id} belongs to entity {}, which is not a known identity",
                    entry.entity
                )));
            }
            if store.history(entry.entity, &entry.attribute).len() <= entry.version {
                return Err(EngineError::CorruptSnapshot(format!(
                    "assertion {id} names version {} of {:?}, which has fewer",
                    entry.version, entry.attribute
                )));
            }
            if !index.contains(*id) {
                return Err(EngineError::CorruptSnapshot(format!(
                    "assertion {id} has no vector, so it could never be recalled"
                )));
            }
        }
        // And the reverse direction, which the loop above cannot see. Every
        // assertion now has a vector, so the index holds at least as many as
        // there are assertions; anything beyond that is a vector no assertion
        // claims. It is not harmless: `search_filtered` scans it, `recall`
        // silently drops the hit it cannot resolve, and `index_len` reports a
        // store larger than anything a query can reach — the same "counts
        // right, answers wrong" shape the id-to-row map already taught this
        // crate to distrust.
        //
        // Compared by length rather than by iterating the index's ids, because
        // `VectorIndex` deliberately exposes no id iterator. Given the check
        // above, equal lengths are exactly the bijection the spec asks for, so
        // widening `rm_index`'s API to restate it here would buy nothing.
        if index.len() != p.assertions.len() {
            return Err(EngineError::CorruptSnapshot(format!(
                "the index holds {} vectors but only {} assertions claim one, so \
                 {} could be found and never resolved",
                index.len(),
                p.assertions.len(),
                index.len() - p.assertions.len()
            )));
        }
        for pending in p.review.values() {
            for id in [pending.a, pending.b] {
                if !p.identity.contains_key(&id) {
                    return Err(EngineError::CorruptSnapshot(format!(
                        "review {} names entity {id}, which is not known",
                        pending.id
                    )));
                }
            }
        }

        let mut engine = Engine {
            store,
            index,
            read: p.read,

            ruleset,
            policy,
            identity: p.identity,
            blocks: BTreeMap::new(),
            assertions: p.assertions,
            review: p.review,
            rejected: p.rejected,
            next_assertion: p.next_assertion,
            next_review: p.next_review,
        };
        engine.rebuild_blocks();
        Ok(engine)
    }

    /// Rebuild the blocking map from `identity`.
    ///
    /// Derived state, so it is never persisted: a stored copy could disagree
    /// with the identity records it claims to index, and a blocking map that
    /// disagrees loses true matches without erroring. It is rebuilt from
    /// `identity` rather than from the store for the reason [`Engine::open`]
    /// gives — a merged-away entity is still in the store, and reviving it here
    /// would put a phantom candidate in front of every future mention.
    ///
    /// The ruleset is the live one supplied to `open`, not whichever one the
    /// snapshot was written under, which is the point: change the blocking
    /// rules and the map reflects them on the next load.
    ///
    /// This is the definition every live mutation of `blocks` has to agree
    /// with, so it keys through [`Engine::key_entity`] rather than pushing
    /// directly — two spellings of the same rule are how a live map and a
    /// rebuilt one come to disagree.
    fn rebuild_blocks(&mut self) {
        self.blocks.clear();
        let entries: Vec<(StableId, Record)> = self
            .identity
            .iter()
            .map(|(&id, r)| (id, r.clone()))
            .collect();
        for (id, record) in entries {
            self.key_entity(id, &record);
        }
    }
}
