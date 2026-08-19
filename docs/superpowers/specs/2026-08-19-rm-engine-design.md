# rm-engine — design

Status: approved design, pre-implementation.

`remember()` / `recall()` / `forget()` over the five crates that already exist.
The first thing in the project that demonstrates they compose.

## Why this crate, and why now

`rm-core`, `rm-survivor`, `rm-store`, `rm-resolve` and `rm-index` are each
tested in isolation. Nothing yet shows them working together, and the thesis —
contradiction resolution as a query rather than a lossy write — is currently
only observable by calling `rm-survivor` and `rm-store` by hand, the way the
README example does.

The crate layout in the architecture sketch puts `rm-graph` before `rm-engine`.
This design takes them in the other order deliberately. Building k-hop retrieval
before anything consumes it means designing an interface against an imagined
caller; building the engine first surfaces the seams between the existing crates
while all five are still cheap to change. That prediction has already paid off
once — see [Changes to sibling crates](#changes-to-sibling-crates), which
documents a semantic conflict between `rm-survivor` and `rm-store` that only
appears when something tries to use both.

`rm-engine` stays offline. `rm-extract` is the only crate permitted to touch the
network, so embeddings arrive from the caller rather than being fetched here.

## Shape

A thin orchestrator. The engine owns the parts and coordinates them; it does not
reimplement them.

```rust
pub struct Engine {
    store: MemoryStore,
    index: VectorIndex,
    ruleset: Ruleset,
    policy: Policy,
    identity: BTreeMap<StableId, Record>,
    blocks: BTreeMap<String, Vec<StableId>>,
    assertions: BTreeMap<AssertionId, AssertionRef>,
    review: BTreeMap<ReviewId, PendingReview>,
    next_assertion: AssertionId,
    next_review: ReviewId,
}
```

`AssertionId` is the `EntryId` the vector index holds, so an index hit resolves
to a stored assertion with one map lookup and the two structures share one
identifier space rather than needing a translation table.

Two alternatives were considered and rejected:

- **A journal-backed aggregate**, with store and index as projections
  rebuildable from an append-only operation log. It buys crash consistency, a
  problem this library does not have: it is single-process and embeddable, and
  both snapshots are written together. A journal is what you add for concurrent
  writers or crash recovery, neither of which exists yet.
- **Trait-based ports** (`Store` / `Index` / `Resolver` traits taken by the
  engine). The sketch already commits to an approximate tier slotting under
  `rm-index`'s existing API when a store outgrows the scan, so the swap-seam for
  that concern exists one layer down. A second seam for the same purpose is cost
  without benefit.

The cost of the chosen shape, stated plainly: `forget` and `erase` mutate two
structures in sequence, so a panic between them can orphan a vector. For an
embeddable single-process library that is acceptable and documented. If this
ever runs as a long-lived server the calculus changes and the journal becomes
worth revisiting.

## Writing: `remember`

```rust
pub struct Observation {
    pub kind: String,
    pub mention: Record,
    pub attribute: String,
    pub value: Option<String>,
    pub valid: Interval,
    pub provenance: Provenance,
    pub embedding: Vec<f32>,
}

pub enum Remembered {
    Merged { entity: StableId, assertion: AssertionId },
    Created { entity: StableId, assertion: AssertionId },
    CreatedPendingReview {
        entity: StableId,
        assertion: AssertionId,
        review: Vec<ReviewId>,
    },
}

pub fn remember(&mut self, obs: Observation) -> Result<Remembered, EngineError>;
```

The order of operations is the correctness argument:

1. **Validate the embedding before anything is written.** `rm-index` rejects
   dimension mismatches, non-finite components and the zero vector under cosine.
   Running that check first means a rejected vector leaves both the store and the
   index untouched. Writing the fact first would leave it in the store with
   nothing able to find it — a memory that exists and cannot be recalled is worse
   than one that was refused, because nothing downstream can detect it.
2. **Resolve the mention** against blocked candidates, scoring each pair with
   `ruleset.score` and banding it with `ruleset.decide`.
3. **Act on the decision.** `Match` merges into the best-scoring matched entity.
   `NonMatch` creates a new one. `Review` creates a new entity *and* files the
   pair in the review queue.
4. **Append the version, then insert the vector**, then record the
   `AssertionId -> (entity, attribute, row)` mapping.

### Review never merges

A `Review` decision creates a separate entity and returns
`CreatedPendingReview`. It does not merge, and it does not block the write: the
fact is remembered either way, and what is uncertain is only whose it is.

This is the same discipline as `rm-survivor`'s refusals and `rm-store`'s
`Absent`/`Unknown` split. An agent that merges two people because they scored
in the middle band has corrupted its memory permanently and silently; an agent
that reports "I know a Ben Severn and a B. Severn, possibly the same person" has
done its job. Returning the review ids rather than logging them makes that
question reachable by the caller instead of discoverable only by inspection.

### Writes stay lossless

`remember` uses `store.assert()`, not `store.assert_resolved()`. Survivorship is
applied on read, in `about()`.

This is the project's thesis taken literally. Resolving on write picks a winner
and discards the losers, which is the behaviour the crate exists to argue
against. Resolving on read means the strategy for an attribute can change without
rewriting history, a caller can ask the same question under two strategies, and
`Strategy::ValidInterval` needs no special handling — it is just another strategy
whose outcome happens to be a timeline.

`assert_resolved` remains part of `rm-store`'s public API for callers who want a
resolved outcome materialised. The engine simply is not one of them.

## Reading: `recall` and `about`

```rust
pub struct Query {
    pub embedding: Vec<f32>,
    pub k: usize,
    /// (valid_t, tx_t). Defaults to (now, now) from the caller's clock.
    pub as_of: Option<(Timestamp, Timestamp)>,
    pub entity: Option<StableId>,
    pub source: Option<Source>,
    pub session: Option<String>,
}

pub struct Recalled {
    pub entity: StableId,
    pub assertion: AssertionId,
    pub attribute: String,
    /// `None` is a tombstone — this assertion claimed the attribute had no
    /// value. It is never "we have nothing"; an assertion that says nothing is
    /// not stored and cannot be recalled.
    pub value: Option<String>,
    pub valid: Interval,
    pub provenance: Provenance,
    pub score: f32,
    /// A later assertion superseded this one as of the query's `tx_t`.
    pub superseded: bool,
}

/// What the engine concluded an attribute held. The owned counterpart to
/// [`rm_store::Known`].
///
/// Owned rather than borrowed because survivorship on read produces its answer
/// from an `Outcome` built inside the call: there is no version in the store to
/// borrow from when the value returned is one the strategy computed rather than
/// one a single version carried.
pub enum Believed {
    Value(String),
    Absent,
    Unknown,
}

pub fn recall(&self, q: &Query) -> Result<Vec<Recalled>, EngineError>;
pub fn about(&self, entity: StableId, attribute: &str, valid_t: Timestamp, tx_t: Timestamp)
    -> Result<Believed, EngineError>;
```

`recall` scopes with `VectorIndex::search_filtered`, so filtering happens during
the scan. Post-filtering a top-k would silently return two results for "what do I
know about Alice in this session" whenever eight better-scoring assertions belong
to other sessions — the exact failure `rm-index` was built to avoid, and it would
be a waste to reintroduce it one layer up.

`superseded` is reported rather than filtered. Semantic search will surface facts
that were true and no longer are, and that is often what was wanted ("what did I
believe about her employer in May"). Dropping them would make historical recall
impossible; returning them unmarked would let a caller state a stale fact as
current. Marking them is the only option that does neither.

`about` is where survivorship runs: collect every version ingested by `tx_t`,
build candidates, apply the attribute's `Strategy`, and ask the outcome what held
at `valid_t`. An attribute with no versions ingested by `tx_t` is
`Believed::Unknown`; a winning tombstone is `Believed::Absent`; anything else is
`Believed::Value`.

`Believed::Unknown` therefore means "nothing to go on", and is returned for an
unknown entity, an attribute never discussed, and an attribute whose every
version arrived after `tx_t`. All three are the same statement — the store has no
opinion at that point on both axes — and none of them is an error.

`Policy` maps attribute name to `Strategy` with a default, so a host can say
"employer resolves by `ValidInterval`, display name by `MostRecent`" once rather
than at every call site.

A refusal from `rm-survivor` propagates as `EngineError::Refused`. The engine
does not fall back to a looser strategy — a memory chosen by a rule the caller
did not ask for is exactly the plausible-looking wrong answer the refusals exist
to prevent.

## The review queue

```rust
pub fn pending_review(&self) -> Vec<&PendingReview>;
pub fn confirm(&mut self, review: ReviewId) -> Result<StableId, EngineError>;
pub fn reject(&mut self, review: ReviewId) -> Result<(), EngineError>;
```

`confirm` merges the two entities: the lower `StableId` survives, the other's
attributes and assertions are re-pointed to it, and the merge is recorded so the
absorbed id is recognisable as absorbed rather than missing. `reject` records the
answer so the pair is not raised again on a later `remember`.

Both settle the pair permanently. A confirmed or rejected pair leaves the queue
and does not return.

## Forgetting

Two operations, because the word means two different things and collapsing them
would be the same mistake as collapsing `Absent` with `Unknown`.

```rust
pub fn forget(&mut self, entity: StableId, attribute: &str, at: Timestamp, prov: Provenance)
    -> Result<(), EngineError>;
pub fn erase(&mut self, entity: StableId, attribute: &str) -> Result<usize, EngineError>;
```

**`forget` stops recall.** It appends a tombstone valid from `at` forward and
removes that attribute's vectors from the index. `recall` goes quiet;
`about(..., valid_t, tx_t)` with a `valid_t` before `at` still answers, because
the fact was true then and the store's value is that this stays reconstructible.

**`erase` destroys the record.** It removes the versions, the vectors and the
mapping entries, and returns how many versions went. Its doc comment states
without hedging that it punches a hole in the audit trail, because a caller
reaching for it is usually answering a deletion request and needs to know exactly
what it does and does not guarantee.

## Changes to sibling crates

Three, each small, each justified by something the engine cannot do without.

### `rm-survivor`: a three-state candidate value

`rm_survivor::Candidate.value: None` means "this source had nothing to say —
absence never contradicts presence". `rm_store::Version.value: None` means
"asserted to have no value", a tombstone and a positive claim.

Those are opposites. Mapping a stored tombstone onto a survivorship candidate
directly would file a positive claim of absence as a non-observation, and it
would be dropped from the comparison instead of competing in it. It should
compete: "Acme, then unemployed, then Globex" is a legitimate timeline with a gap
in the middle, and under `ValidInterval` the gap is a fact with its own validity
range.

`Candidate` gains a three-state value distinguishing "said nothing" from
"asserted empty", mirroring the `Known` split `rm-store` already makes. Both
crates are `0.0.0` with no external consumers; this is the cheapest this change
will ever be.

Two rejected alternatives: a sentinel string for tombstones inside the engine
(any sentinel can collide with a real value, and it launders a semantic problem
into a data one), and handling tombstones outside survivorship in the engine
(duplicates the strategy logic, and gets it subtly wrong for every strategy that
is not `MostRecent`).

### `rm-resolve`: make `BlockingKey::keys_for` public

The engine keeps a blocking map updated per write. Without access to the key
derivation it would have to rebuild every block on each `remember` — O(n) per
write and quadratic over a session — or duplicate the key format and drift from
it. One method's visibility avoids designing in a known quadratic.

### `rm-store`: an explicit `erase`

The store is append-only by construction and has no way to remove a version.
That is right as a default and wrong as an absolute, since `erase` has to exist
somewhere. Adding one narrow method, documented as the only history-mutating call
in the crate, is better than having the engine reach around the store's API to do
it. `StoreError` gains nothing; the method returns a count.

## Errors

```rust
pub enum EngineError {
    Index(IndexError),
    Store(StoreError),
    Refused(Refused),
    UnknownEntity(StableId),
    UnknownReview(ReviewId),
    CorruptSnapshot(String),
}
```

Each wraps rather than flattens, so the explanation the inner crate wrote — which
in this codebase always names what was missing — reaches the caller intact.

`UnknownEntity` is a write-path error only. Naming a nonexistent entity in
`forget`, `erase` or `confirm` is a bug in the caller and is reported. Asking
*about* one is not: reads return `Believed::Unknown`, because "I have nothing on
this" is a true and useful answer to a question, where it would be a silent
no-op if accepted as an instruction. `rm-store` already draws the line in the
same place — `assert` errors on an unknown id, `as_of` returns `Unknown`.

## Persistence

`snapshot()` emits canonical JSON composing the store snapshot, the index
snapshot, and the engine's own maps. `BTreeMap` throughout, so it is byte-stable
and diffable.

`open()` validates before returning:

- every `AssertionId` in the index resolves in the assertions map, and vice versa
- every referenced `StableId` exists in the store
- every `PendingReview` names two entities that exist
- the blocking map is **rebuilt from `identity`, not persisted**

The blocking map is derived state, and `rm-index` has already taught this lesson
the expensive way: persisting derived state lets a snapshot disagree with itself,
and a restore path that does not validate is a panic waiting for its first query.
A restore path is a door, and this crate's premise is rejecting at the door.

## Testing

Behavioural names stating the property, matching the other crates.

**Composition**
- `the_readme_story_works_end_to_end` — Acme until July, Globex after, through
  the public API only
- `a_rejected_vector_leaves_the_store_untouched`
- `an_assertion_and_its_vector_stay_in_step_across_remember_and_forget`

**Resolution**
- `a_review_pair_is_never_merged_silently`
- `a_review_decision_still_remembers_the_fact`
- `confirming_a_review_merges_both_entities_assertions`
- `rejecting_a_review_stops_it_being_raised_again`
- `blocking_finds_the_same_matches_as_comparing_everything`

**Survivorship on read**
- `changing_the_policy_changes_the_answer_without_rewriting_history`
- `an_absence_competes_on_the_timeline_rather_than_vanishing`
- `a_refusal_propagates_instead_of_falling_back_to_a_looser_strategy`

**Recall**
- `filtering_by_session_happens_during_the_scan`
- `a_superseded_fact_is_returned_marked_not_dropped`
- `recall_as_of_a_past_tx_time_does_not_see_later_knowledge`

**Forgetting**
- `forget_stops_recall_but_leaves_history_answerable`
- `erase_removes_it_from_history_too`
- `forget_is_itself_a_fact_with_provenance`

**Persistence**
- `a_snapshot_round_trips_including_the_review_queue`
- `snapshots_are_byte_stable`
- `a_snapshot_whose_index_and_store_disagree_is_rejected_not_panicked_on`

## Out of scope

- **Embeddings.** Caller-supplied. `rm-extract` is the only networked crate.
- **k-hop retrieval.** `rm-graph`, once the engine gives it a caller.
- **Concurrency.** Single-writer, `&mut self`. No locking, no async.
- **Incremental re-resolution.** Confirming a review merges two entities; it does
  not re-run resolution across the store looking for pairs the merge newly
  implies. Transitive consequences are a later question, and doing it badly is
  worse than not doing it.
