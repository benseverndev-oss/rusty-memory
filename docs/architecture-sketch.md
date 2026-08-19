# rusty-memory — architecture sketch

Status: design sketch, pre-implementation. Nothing here is built yet.

## Thesis

Agent memory systems today dedupe by embedding similarity and resolve
contradictions by asking an LLM to re-summarise. That is the weakest available
answer to the hardest problem in the domain: **facts about the same entity
change over time, and the store accumulates contradictions.**

`rusty-memory` takes the opposite position. Entity resolution and survivorship
are solved problems in master data management. Golden Suite already implements
them in Rust. Applying them to agent memory is the differentiator — not the
choice of language.

The pitch is *"agent memory that resolves contradictions deterministically"*,
not *"agent memory, but Rust"*. A faster reimplementation of someone else's
design competes on the axis adopters care about least.

## What already exists and is reusable

All from `benseverndev-oss/goldenmatch`, MIT, `packages/rust/extensions/`.
Line counts are source, measured at `goldenmatch@main` 2026-08-18.

| Crate | Lines | Role here |
|---|---:|---|
| `goldengraph-core::store` | 784 | **The substrate.** Bi-temporal, append-only, portable |
| `goldenphonetic-core` | 7,688 | Name comparators for resolution |
| `score-core` (+ `em_core`) | 2,619 | Probabilistic match scoring, EM-trained |
| `goldenfuzz-core` | 1,908 | String distance |
| `goldengraph-core` (rest) | 1,137 | Model, resolve, retrieve, community detection |
| `goldenhnsw` | 687 | Vector index |
| `survivorship-core` | 279 | Field-level merge rules |

~16k lines of existing, tested, `todo!()`-free Rust.

### The substrate is better than expected

`goldengraph-core/src/store.rs` is not a graph-in-memory. It is a bi-temporal
record store:

- `StableId` — "assigned once, monotonic, never reused", durable across builds
  (distinct from the within-build `EntityId: u32`)
- `as_of(valid_t, tx_t)` — filters on **both** the valid-time and
  transaction-time axes
- `history(id) -> Vec<HistoryEvent>` — `Merge { kept, absorbed, at }` and
  `Split { from, into, at }`
- `StoredEdge { valid_from, valid_to: Option<i64>, ingested_at, source_refs }`
- `append(StoreBatch)` — incremental, reconciling identity by record-key overlap
  under a "plurality-heir" rule
- `snapshot() -> String` — canonical JSON, parity-diffable

That is *exactly* the shape agent memory needs, and it is the piece that would
otherwise take longest to get right. The crate's `lib.rs` doc comment claiming
"no persistence (SP2+)" is stale — `store.rs` is the SP2 work and it landed.

`retrieve::neighborhood` already expands 1–8 hops with deterministic ordering
and caller-supplied node budget.

## What has to be built

Four things, in rough order of difficulty.

### 1. Attribute history — the one real gap in the substrate

`store.rs` scopes it out explicitly:

> identity (which id) and edge facts are bi-temporal; entity *attributes*
> (canonical_name / surface_names) reflect the latest state, not their value
> as-of `tx_t` (attribute history is out of SP2).

For MDM that is a reasonable line. For agent memory it is not: the attributes
*are* the payload. "The user's employer" is an attribute, and its history is the
entire point. This is where survivorship and bi-temporality have to meet, and
it is the core engineering work of the project.

### 2. Survivorship, with the refused strategies implemented

`survivorship-core` deliberately refuses two strategies:

```rust
"source_priority" => "...needs a sources list, which the Spark path does not
    supply -- Python raises rather than guessing..."
"most_recent"     => "...needs a dates list, which the Spark path does not
    supply... picking the first row would be an arbitrary answer wearing a
    deterministic hat."
```

**The memory domain supplies both.** Every memory has an observation time and a
source (user assertion / tool output / agent inference). The strategies that are
unimplementable in a Spark batch job are the two most valuable ones here.

So `rm-survivor` = `survivorship-core` + `most_recent` + `source_priority` +
one new strategy the store makes possible:

- `valid_interval` — do not pick a winner. Write both values with disjoint
  `[valid_from, valid_to)` ranges and let `as_of()` answer the question at query
  time. "Acme until July, Globex after" is not a conflict to resolve; it is two
  facts with different validity.

That strategy is only expressible because the substrate is bi-temporal. It is
the thesis in one function.

Keep the crate's refusal discipline verbatim — the `a_refusal_beats_a_plausible_wrong_survivor`
test is the right instinct for memory too. A wrong memory that looks right is
worse than a gap.

### 3. Index persistence and filtered search

`goldenhnsw` is a flat in-memory index: `new(dim, params)`, `add(&[f32]) -> u32`,
`search(&[f32], k) -> Vec<(u32, f32)>`. No persistence, no deletion, no metadata
filter. Memory needs all three — recall is almost always scoped ("what do I know
about X *in this session*", "*as of* last week").

Flagging honestly: at 687 lines this is the component most likely to need
replacing. Benchmark against `hnsw_rs` / `usearch` for recall@k and build time
before committing to it. Nothing else in the stack has a credible off-the-shelf
substitute; this one does.

### 4. Extraction

Turn → mentions + edges. Needs an LLM, and `goldengraph-core` is deliberately
"no LLM, no embeddings". Isolate it: `rm-extract` should be the only crate that
touches the network, so everything below it stays testable offline and the core
stays embeddable.

Also required: `record_key` computation. The store keys identity on
host-supplied fingerprints and "the core never computes it" — that is the host's
job, i.e. ours.

## Crate layout

```
rusty-memory/
  crates/
    rm-core/       # Provenance + bi-temporal model. serde only.
    rm-store/      # goldengraph-core::store + attribute history
    rm-graph/      # model / resolve / retrieve / community
    rm-resolve/    # score-core + goldenfuzz + goldenphonetic over memories
    rm-survivor/   # survivorship-core + most_recent/source_priority/valid_interval
    rm-index/      # vector index + persistence + filtered search
    rm-extract/    # turn -> mentions/edges. The only networked crate.
    rm-engine/     # remember() / recall() / forget(). Ties it together.
    rm-mcp/        # MCP server binary
    rm-cli/        # `rmem` binary
```

The deliverable is a **single static binary and an embeddable library**. No
Python runtime, no CMake, no ABI-tag matching, no Compose file. That is the
structural advantage over OpenViking, which ships a compiled Rust extension
inside a Python package and has to solve the `ragfs_python` ABI problem on every
interpreter.

## Consuming the Golden Suite crates

MIT to MIT, same owner — legally free. The question is only coupling.

Recommended split:

- **Vendor `survivorship-core`.** It has to diverge (we are adding the refused
  strategies) so a shared dependency would fight us. ~279 lines; carry a
  `parity/` fixture set against the upstream behaviour for the shared strategies,
  matching the pattern `goldenmatch/parity/` already uses.
- **Depend on the rest** by git rev initially, and push to publish
  `goldengraph-core`, `score-core`, `goldenfuzz-core`, `goldenphonetic-core` to
  crates.io. They are clean, pyo3-free, documented, and useful standalone —
  publishing them is a small OSS win independent of this project, and it keeps
  `rusty-memory` from taking a dependency on a 6,951-file monorepo.

Avoid the third option — fork and drift. Two copies of the resolution scorer
diverging silently is a worse outcome than either.

## Licensing boundary

`rusty-memory` is MIT. OpenViking's root is **AGPL-3.0**; its `crates/` are
Apache-2.0.

Read `volcengine/OpenViking`'s Rust crates for design orientation. **Do not read
the AGPL Python** while writing equivalent functionality. "I studied the Python
and reimplemented it" is a materially worse position to defend than "I built on
the Apache-2.0 layer", and the distinction costs nothing to maintain from day
one and is impossible to retrofit.

## Non-goals

- Beating OpenViking on retrieval latency. Wall clock in this domain is
  dominated by embedding and LLM round-trips; interpreter overhead is not the
  bottleneck, and "5–219x faster" ports of OpenViking already exist with
  single-digit star counts.
- Feature parity with OpenViking. Its surface is a filesystem abstraction, three
  SDKs, a web studio, and a plugin ecosystem. Competing there is a losing race
  against a funded team.
- Porting GoldenMatch. The Python matching engine is 207,690 lines of source
  against 184,196 lines of tests. It is not moving, and it does not need to.
