# rm-graph — design

Status: approved design, pre-implementation.

Relationships between entities, and k-hop retrieval over them. Edges live in
`rm-store`; `rm-graph` is stateless traversal.

## Why now, and why this shape

The architecture sketch put `rm-graph` before `rm-engine`. That order was
inverted deliberately, and the bet paid: building the engine first surfaced a
semantic conflict between `rm-survivor` and `rm-store` that is invisible until
something uses both. The same reasoning now argues *for* the graph — it has a
caller. `rm-engine` can answer "what do I know about Alice"; it cannot answer
"what do I know about Alice's employer".

Nothing in the workspace models relationships today. Every crate deals in
entity → attribute → versions. This is genuinely new state, and the decision
about where it lives shapes everything else.

## Edges live in `rm-store`

`rm-store` already *is* the bi-temporal substrate: versions, provenance, valid
intervals, a destructive `erase` that is documented as the only history-mutating
call, and a restore path that validates. Edges get all of it by being modelled
the same way.

The alternative — `rm-graph` owning its own edge store — was rejected on the
evidence of the previous branch. `rm-engine` already keeps three structures
consistent, and the single Critical defect found in whole-branch review came
from exactly that seam: an operation in one crate silently invalidating state in
another. A fourth independent store, with a second bi-temporal implementation to
keep honest against the first, multiplies that risk for a separation of concerns
nothing is asking for.

A third option — edges as attributes whose value is an entity id — was rejected
for losing edge typing and degrading traversal to a scan of every entity's
attributes.

## The model

An edge mirrors an attribute, which is what makes it cheap:

```rust
/// Subject, predicate, object. The identity of a relationship.
pub type EdgeKey = (StableId, String, StableId);

pub struct EdgeVersion {
    /// `false` asserts the relationship did *not* hold over `valid` — a
    /// tombstone, and the exact counterpart of an attribute's `value: None`.
    pub present: bool,
    pub valid: Interval,
    pub provenance: Provenance,
}

// in MemoryStore
edges: BTreeMap<EdgeKey, Vec<EdgeVersion>>,
```

Reads hand back a borrowed view rather than the stored version, so a caller
never has to reassemble the key it asked about:

```rust
/// One edge in force at the queried point on both axes.
pub struct Edge<'a> {
    pub subject: StableId,
    pub predicate: &'a str,
    pub object: StableId,
    pub valid: Interval,
    pub provenance: &'a Provenance,
}
```

Reading uses the rule attributes already use: for each key, the latest-ingested
version with `ingested_at <= tx_t` whose `valid` covers `valid_t`. The edge is
in force if that version is `present`.

`BTreeMap` for the same reason as everywhere else in this workspace — snapshots
must be byte-stable and diffable.

### Edges do not compete

Two `employed_by` edges pointing at different objects are different keys, and
both stand. There is no survivorship on edges and no predicate cardinality.

"Acme until July, Globex after" is therefore two edges with disjoint validity —
the `ValidInterval` answer, arrived at for free rather than through a strategy.

The case this refuses to handle silently: a source says "Alice works at Globex"
in July without saying she left Acme. Both edges then stand, open-ended and
overlapping, and Alice appears to work at two places. That is the honest record
of what was actually said. Closing the Acme edge is an *inference* — arrival
does not entail departure — and inferring it inside the store is precisely the
move this project exists to argue against. The extractor or the host decides;
the store records.

Predicate cardinality (marking `employed_by` functional so an assertion closes
its predecessor) is a real feature and a plausible future one. It is out of
scope because nothing has asked for it, and because adding it later is additive.

## Store API

```rust
pub fn relate(&mut self, subject: StableId, predicate: impl Into<String>,
              object: StableId, valid: Interval, prov: Provenance)
    -> Result<(), StoreError>;

pub fn unrelate(&mut self, subject: StableId, predicate: &str, object: StableId,
                at: Timestamp, prov: Provenance) -> Result<(), StoreError>;

pub fn edges_from(&self, subject: StableId, valid_t: Timestamp, tx_t: Timestamp)
    -> Vec<Edge<'_>>;
pub fn edges_into(&self, object: StableId, valid_t: Timestamp, tx_t: Timestamp)
    -> Vec<Edge<'_>>;

pub fn edge_history(&self, key: &EdgeKey) -> &[EdgeVersion];
pub fn erase_edges(&mut self, entity: StableId) -> usize;
```

`relate` rejects either endpoint being an entity the store does not hold. A
dangling edge is a lie the graph would traverse without complaint, and the id
that names nothing today may name something else never — `StableId`s are never
reused, so a dangling reference stays recognisable rather than silently
resolving.

`unrelate` appends a version with `present: false`, exactly as `forget` appends
a tombstone. Ending a relationship is a fact with provenance, not an untraceable
edit, and `edge_history` still shows that it held.

`erase_edges` mirrors `erase`: destructive, documented as destroying the audit
trail, and the only edge call that does.

### `erase` and `erase_edges` stay separate

Erasing an entity's attribute says nothing about its edges, and neither call
implies the other. A caller reaching for either is usually answering a deletion
request and needs to know exactly what was removed; a convenience that quietly
does both makes that question unanswerable. Two explicit calls is the same
discipline as splitting `forget` from `erase` in the first place.

### The reverse index

`edges_into` needs an object → keys map. It is derived state, so it is
`#[serde(skip)]`, rebuilt inside `MemoryStore::open`, and never persisted.

This is now the third time this workspace has needed that pattern, and it has
been learned twice the hard way: `VectorIndex::positions` shipped persisted and
made snapshots non-deterministic while letting a restored index disagree with
itself, and the engine's `blocks` map produced a snapshot round-trip that
changed which mentions resolved together. Derived state that is persisted is
derived state that can lie.

## Traversal

`rm-graph` holds no state. One entry point.

```rust
pub enum Direction { Out, In, Both }

pub struct Walk {
    pub seeds: Vec<StableId>,
    pub hops: u8,
    pub budget: usize,
    pub direction: Direction,
    /// `None` traverses every predicate.
    pub predicates: Option<Vec<String>>,
    pub valid_t: Timestamp,
    pub tx_t: Timestamp,
}

pub struct Reached {
    pub entity: StableId,
    pub distance: u8,
}

pub struct Neighborhood {
    /// Reached entities, ordered by `(distance, entity)`.
    pub reached: Vec<Reached>,
    /// The budget stopped the walk before it ran out of graph.
    pub truncated: bool,
}

pub fn neighborhood(store: &MemoryStore, walk: &Walk) -> Neighborhood;
```

Breadth-first, expanding in `(distance, entity)` order so two runs over the same
store return the same list. Because it is breadth-first, `distance` is the
shortest hop count to that entity, not the first path that happened to reach it.
Determinism is not a nicety here: it is the same requirement that makes
`rm-index` break score ties by id and `rm-survivor` tally in an
insertion-ordered `Vec` rather than a `HashMap`.

Three things the signature leaves open, decided here rather than left to the
implementer:

- **Seeds are included, at distance 0.** A neighborhood that omits what it was
  asked about forces every caller to re-add them, and a caller who forgets has a
  bug the type system cannot see. `hops: 0` is therefore meaningful and returns
  exactly the seeds that exist.
- **`budget` counts entities, not edges.** It bounds the size of the answer,
  which is what a caller is protecting — memory and downstream work scale with
  entities returned, not with edges crossed. Seeds count against it.
- **A seed the store does not hold is skipped, not an error.** Asking about
  something the store has never met is the same shape of question as
  `about()` on an unknown entity, which answers `Unknown` rather than failing.

Both time axes are required and neither is defaulted. A walk answers "who was
connected to Alice in May, as far as we knew in August", and every edge is
filtered on both axes as it is crossed — not filtered afterwards, which would
let a later-learned edge carry the walk somewhere it could not have reached at
`tx_t`.

Cycles terminate on the visited set, which breadth-first search needs anyway.

### `truncated` is not optional

A budget that silently drops the tail returns a short neighborhood
indistinguishable from a genuinely small one. That is the same failure as
post-filtering a top-`k`: the caller sees a plausible answer and has no way to
learn it was cut. `rm-index` was built to avoid it and `rm-engine` had to defend
it again in `recall`; the graph should not reintroduce it for the sake of one
bool.

## Engine surface

```rust
pub fn relate(&mut self, subject, predicate, object, valid, prov) -> Result<(), EngineError>;
pub fn unrelate(&mut self, subject, predicate, object, at, prov) -> Result<(), EngineError>;
pub fn neighborhood(&self, walk: &Walk) -> Neighborhood;
```

`recall` stays purely semantic. Traversal is a separate method, and the caller
composes them — seed with `recall`, expand with `neighborhood`, and weigh the
two however the application needs.

Folding hops into `Query` was rejected. It would require ranking a two-hop
neighbour against a 0.9-cosine hit, and there is no honest ordering between
them: any single combined score is a number the crate invented and would then
have to defend. The caller knows what the two are worth in its context and the
crate does not. If real usage later shows a weighting that holds, adding it is
additive; inventing one now is not reversible once callers depend on the
ordering.

### `confirm` must re-point edges

Merging entity B into A leaves every edge naming B pointing at a dead id.
Attributes and assertions already move; edges must move with them, in both
directions, and an edge the merge turns into a self-edge (A → A) is dropped
rather than stored.

This is the same class of defect as the version re-numbering that whole-branch
review caught on the previous branch: an operation in one crate silently
invalidating state in another, producing a well-formed wrong answer with no
error. It is called out here so it is a requirement rather than something a
reviewer has to notice.

## Errors

No new error type. `relate` and `unrelate` return `StoreError` /`EngineError`,
reusing `UnknownEntity` for a missing endpoint and `CorruptSnapshot` for an edge
that survives parsing but names an entity the restored store does not hold.

## Testing

Behavioural names stating the property, matching every other crate.

**The model**
- `an_edge_is_bitemporal_like_an_attribute` — what we knew in August about July
- `unrelate_stops_the_edge_without_erasing_that_it_held`
- `two_employers_at_once_both_stand`
- `an_edge_naming_an_unknown_entity_is_rejected_on_relate_and_on_restore`
- `erase_edges_does_not_touch_attributes_and_erase_does_not_touch_edges`

**Traversal**
- `a_walk_returns_the_same_order_every_run`
- `a_truncated_walk_says_so`
- `a_walk_as_of_a_past_tx_time_does_not_cross_edges_learned_later`
- `a_walk_respects_direction_and_predicate_filters`
- `a_cycle_terminates`
- `hop_distance_is_the_shortest_path_not_the_first_seen`

**Integration**
- `a_merge_repoints_edges_in_both_directions_and_drops_the_self_edge`
- `a_snapshot_round_trips_edges_and_rebuilds_the_reverse_map`

## Out of scope

- **Predicate cardinality.** Additive later; nothing asks for it now.
- **Fused graph and vector ranking.** See the engine surface above.
- **Community detection, edge weights, path-finding beyond neighborhood
  expansion.** Each is real; none has a caller.
- **Performance work.** Every traversal filters edges by `as_of` as it goes
  rather than reading a pre-filtered structure, so a dense entity does real work
  per query. This is the same bet `rm-index` made shipping exact search over
  HNSW, and it was right there. If it stops being right, an adjacency index in
  `rm-graph` is the escape hatch, and nothing in this API forecloses it.
