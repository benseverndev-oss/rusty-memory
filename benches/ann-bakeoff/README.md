# ANN bake-off

Answers two questions for `rm-index`:

1. Does a personal memory store need approximate search at all?
2. If it does, is `goldenhnsw` good enough to build on, or should we take an
   established crate?

Not in the workspace and not run by CI: it pulls `hnsw_rs`, and a run takes
several minutes.

## Running it

`goldenhnsw` is not published to crates.io, so drop it in first:

```sh
git clone --depth 1 https://github.com/benseverndev-oss/goldenmatch /tmp/goldenmatch
cp /tmp/goldenmatch/packages/rust/extensions/goldenhnsw/src/lib.rs \
   benches/ann-bakeoff/src/goldenhnsw.rs
cargo run --release --manifest-path benches/ann-bakeoff/Cargo.toml
```

It generates deterministic data (SplitMix64, seed 42), computes exact top-10 by
brute force as ground truth, and reports build time, mean query time and
recall@10 for each backend.

## Results

`goldenmatch@3590b47`, `hnsw_rs` 0.3.4, N=20,000, dim=128, 200 queries, k=10,
single-threaded insert on both sides.

### Uniform on the sphere

| backend | build | query | recall@10 |
|---|---:|---:|---:|
| brute force (exact) | — | 2.4 ms | 1.000 |
| goldenhnsw (ef=50) | 20.82 s | 148 µs | 0.512 |
| hnsw_rs (ef=50) | 36.36 s | 790 µs | 0.481 |
| goldenhnsw (ef=200) | 21.70 s | 565 µs | 0.874 |
| hnsw_rs (ef=200) | 37.93 s | 2.7 ms | 0.839 |

### Clustered, 200 topics

| backend | build | query | recall@10 |
|---|---:|---:|---:|
| brute force (exact) | — | 2.2 ms | 1.000 |
| goldenhnsw (ef=50) | 4.94 s | 93 µs | 0.733 |
| hnsw_rs (ef=50) | 16.46 s | 341 µs | 0.704 |
| goldenhnsw (ef=200) | 5.26 s | 292 µs | 0.976 |
| hnsw_rs (ef=200) | 17.55 s | 1.0 ms | 0.951 |

## Reading them

**Both distributions are reported on purpose.** Uniform vectors on the sphere
are the case ANN benchmarks usually leave out: in 128 dimensions their pairwise
distances concentrate so hard that a navigable-graph index has almost nothing to
navigate, and recall collapses for both implementations — at `ef=200`, `hnsw_rs`
is *slower than brute force* while still missing 16% of the true neighbours.
Real embeddings cluster by topic, so the second table is the one that should
drive the decision. Quoting only the first would mislead in the flattering
direction.

**`goldenhnsw` wins.** On clustered data it builds 3.3× faster, queries 3.4×
faster, and has better recall (0.976 vs 0.951). The architecture sketch had
flagged it as the component most likely to need replacing; that was wrong, and
these numbers are why.

One fairness caveat: `hnsw_rs` offers a parallel insert this comparison does not
use. Its build column is not its best achievable — though the query and recall
columns, which matter more here, are unaffected.

**But `rm-index` ships exact anyway.** At N=20,000, exact brute force answers in
2.2 ms. HNSW answers in 292 µs, bought with ~5 s of build time, a graph to keep
consistent across deletions, recall below 1.0, and a filtered-search story that
is materially harder to get right. An agent turn that waits 400 ms on an
embedding API will not notice 2 ms. When a store outgrows that, these numbers say
build the approximate tier on `goldenhnsw`'s design.

## Caveats

- N=20,000 is a personal-scale store. The ranking may not hold at 10⁶ — re-run
  before assuming it does.
- Synthetic data. Clustered Gaussians approximate embedding structure; they are
  not a substitute for measuring on the embedding model actually in use.
- Single run, no repetition or variance reporting. The gaps here are large
  enough not to need it; a closer race would.
