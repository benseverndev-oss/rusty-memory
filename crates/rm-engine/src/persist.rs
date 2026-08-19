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
    store: MemoryStore,
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
}

impl Engine {
    /// Serialise to canonical JSON.
    ///
    /// Ordered containers throughout, so the bytes are stable for a given state
    /// and a snapshot diffs against its predecessor rather than reshuffling.
    pub fn snapshot(&self) -> String {
        let p = Persisted {
            store: self.store.clone(),
            index: self.index.snapshot(),
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
    /// One counter of the same kind is *not* checked, and deliberately named
    /// here rather than left to be discovered: `MemoryStore`'s own `next_id`.
    /// Rewound below a live `StableId`, a later `create_entity` would overwrite
    /// an existing entity exactly as a rewound `next_assertion` would overwrite
    /// an assertion. `rm_store` exposes no accessor for it, so this crate
    /// cannot inspect it without reaching around the type; closing it belongs
    /// in `rm_store`, next to the counter itself.
    pub fn open(snapshot: &str, ruleset: Ruleset, policy: Policy) -> Result<Engine, EngineError> {
        let p: Persisted = serde_json::from_str(snapshot)
            .map_err(|e| EngineError::CorruptSnapshot(e.to_string()))?;

        // Reuses the index's own door rather than reimplementing it: this both
        // rebuilds the derived position map and rejects an index whose rows and
        // ids disagree. `IndexError` converts, so a corrupt index arrives as
        // itself rather than flattened into a string that loses which invariant
        // broke.
        let index = VectorIndex::open(&p.index)?;

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
                    "next_assertion is {}, but assertion {highest} already exists, so the next                      write would overwrite a stored fact",
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
                    "next_review is {}, but review {highest} is already open, so the next                      question would overwrite it",
                    p.next_review
                )));
            }
        }

        for (id, entry) in &p.assertions {
            if p.store.entity(entry.entity).is_none() {
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
            if p.store.history(entry.entity, &entry.attribute).len() <= entry.version {
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
            store: p.store,
            index,
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
    fn rebuild_blocks(&mut self) {
        self.blocks.clear();
        let entries: Vec<(StableId, Record)> = self
            .identity
            .iter()
            .map(|(&id, r)| (id, r.clone()))
            .collect();
        for (id, record) in entries {
            for key in self.keys_for(&record) {
                self.blocks.entry(key).or_default().push(id);
            }
        }
    }
}
