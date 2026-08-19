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

`goldenmatch@55c5259`, `hnsw_rs` 0.3.4, N=20,000, dim=128, 200 queries, k=10,
single-threaded insert on both sides.

Each table reports **mean top-1 cosine** — how close the queries actually land
to the corpus. It is printed because an earlier revision of this benchmark got
it wrong invisibly: `clustered()` generated its own centroids on every call, so
the corpus and the query set were drawn from two unrelated sets of 200 topics.
In 128 dimensions that makes a "clustered" query indistinguishable from noise,
and every column below still looked entirely plausible. The number is the guard;
expect ~0.9 clustered and ~0.3 uniform.

### Uniform on the sphere

Mean top-1 cosine 0.347.

| backend | build | query | recall@10 |
|---|---:|---:|---:|
| brute force (exact) | — | 4.2 ms | 1.000 |
| goldenhnsw (ef=50) | 26.70 s | 302 µs | 0.512 |
| hnsw_rs (ef=50) | 43.70 s | 873 µs | 0.499 |
| goldenhnsw (ef=200) | 32.55 s | 1.1 ms | 0.874 |
| hnsw_rs (ef=200) | 57.09 s | 2.9 ms | 0.841 |

### Clustered, 200 topics

Mean top-1 cosine 0.919.

| backend | build | query | recall@10 |
|---|---:|---:|---:|
| brute force (exact) | — | 2.7 ms | 1.000 |
| goldenhnsw (ef=50) | 6.83 s | 104 µs | 1.000 |
| hnsw_rs (ef=50) | 27.84 s | 582 µs | 0.987 |
| goldenhnsw (ef=200) | 15.60 s | 678 µs | 1.000 |
| hnsw_rs (ef=200) | 26.20 s | 1.1 ms | 0.990 |

## Reading them

**Both distributions are reported on purpose.** Uniform vectors on the sphere
are the case ANN benchmarks usually leave out: in 128 dimensions their pairwise
distances concentrate so hard that a navigable-graph index has almost nothing to
navigate, and recall collapses for both implementations — at `ef=200`, `hnsw_rs`
is *slower than brute force* while still missing 16% of the true neighbours.
Real embeddings cluster by topic, so the second table is the one that should
drive the decision. Quoting only the first would mislead in the flattering
direction.

**`goldenhnsw` wins.** On clustered data it returns perfect recall where
`hnsw_rs` returns 0.987–0.990, and answers roughly 5× faster at matched `ef`.
The architecture sketch had flagged it as the component most likely to need
replacing; that was wrong, and these numbers are why.

Two fairness caveats. `hnsw_rs` offers a parallel insert this comparison does not
use, so its build column is not its best achievable — recall, which matters more
here, is unaffected. And build timings vary widely run to run: `goldenhnsw` built
the same clustered index in 6.83 s and 15.60 s with identical construction
parameters. Treat build as an order of magnitude, not a measurement. Recall is
deterministic given the seed and is the column to trust.

**`rm-index` ships exact anyway — but on narrower grounds than first claimed.**
An earlier draft of this README argued the point partly on HNSW's "recall below
1.0". On correctly-distributed queries that is simply false: recall is 1.000 at
`ef=50`, and the argument does not get to lean on it. What survives is:

- exact brute force answers in 2.7 ms at N=20,000, and an agent turn that waits
  400 ms on an embedding API will not notice that;
- the approximate tier costs a graph that has to stay consistent across
  deletions, where exact search has nothing to rebuild;
- filtered search — "what do I know about Alice *in this session*" — is
  materially harder to get right over a graph than over a scan, and filtering is
  the common case for memory rather than the exception.

So the trade is real but not yet worth taking, and it is bought with complexity
rather than accuracy. When a store outgrows the scan, these numbers say build the
approximate tier on `goldenhnsw`'s design.

## Caveats

- N=20,000 is a personal-scale store. The ranking may not hold at 10⁶ — re-run
  before assuming it does.
- Synthetic data. Clustered Gaussians approximate embedding structure; they are
  not a substitute for measuring on the embedding model actually in use.
- Single run, no repetition or variance reporting. The recall gaps are large and
  deterministic; the timing columns are not — see the build-variance caveat
  above, and do not read a 2× timing difference as real without repeating it.
- Machine-dependent, and the machine must be otherwise idle. A run taken while
  the workspace was compiling reported brute force at 6.7 ms against 2.7 ms
  quiet — enough to change which conclusion the table appears to support.
