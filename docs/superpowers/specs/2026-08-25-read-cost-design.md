# The cost this benchmark declined to measure

`rm-contrast`'s README ends by handing the reader a decision and withholding
half of what it needs:

> This store appends to a version log and runs survivorship over that log on
> every read. The control does a hash-map insert and a lookup. The difference is
> asymptotic rather than a constant factor, and it is not measured here — a
> half-built cost model would be worse than this sentence.
>
> Nothing in the surface above accounts for it. A reader deciding between the
> two should weigh a control that is cheaper and right at the origin against a
> store that holds its accuracy as the workload leaves it.

That was the right call when it was written. It is now blocking a live
question.

## Why now

`rm-contrast` measures its store column under `Strategy::ValidInterval`
(`score.rs:84`). The shipped `rmem.toml` template does not
(`rm-host/src/config.rs:179`):

```toml
[policy]
default = "most_recent"

[policy.attribute]
employer = "valid_interval"
```

So the benchmark's 1.000 column is the store configured **unlike** how it
ships. Out of the box every attribute but the template's example answers by
most-recently-observed: correct on the transaction clock, blind on the valid
one. `rmem about --valid-at` refuses there, correctly and since #45, so the
store says so rather than lying — but the property this project exists for is
off by default.

`ValidInterval` was a poor default partly because it refused a whole read over
one collision anywhere in a history. #50 removed that. The default question is
newly open, and it cannot be answered without knowing what the default would
cost.

## What the number is for

Not "how fast is the store", which is unfalsifiable without a target. One
question:

**At what history depth does the store's read cost stop being negligible, and
how far is the real store from there?**

The live store is at **depth 1, uniformly** — measured from
`D:\memory\decisions.json` on 2026-08-25: 219 entities, 1,086 attribute slots,
every one holding exactly one version. No slot has been revised.

If the crossover is orders of magnitude away, the default-policy decision is a
correctness decision, and this measurement is what licenses saying so instead
of assuming it. If it is close, that is a finding worth more than the default.

**Depth 1 is not evidence that revisions are rare in general.** This is a
two-day-old store seeded once. It is an anchor for where *this* store sits, not
a claim about workloads.

**One thing it turned up that wants confirming rather than assuming.** The
scope backfill ran `rescope` across 219 records, and the log's own decision says
a reach is corrected *"with `rescope` rather than by re-deciding"*. If `rescope`
appends a version, those slots should be at depth 2. They are at depth 1. Either
the store was rebuilt rather than amended, or `rescope` overwrites — and which
one is true is worth knowing before anybody reads depth 1 as a fact about
`rescope`.

## Two pieces, each answering one clause of the sentence

The README makes two claims. "Asymptotic rather than a constant factor" is
about shape. "A reader deciding between the two should weigh" is about the
constant. Either half alone is the half-built model it warned against.

### `crates/rm-contrast/src/cost.rs` — the shape

In the workspace, free, deterministic, run by CI, inside the crate's stated
contract.

```rust
/// Predicted work for one `about()` at history depth `v`.
///
/// Units are arbitrary. Only ratios between strategies and across depths mean
/// anything, and the bench is what checks those ratios against reality.
pub fn predicted_work(v: usize, strategy: &Strategy) -> f64
```

Each term names the code it models:

| term | what it models |
|---|---|
| `v` | candidate construction from tx-visible versions, `rm-engine/src/read.rs:284` |
| `v` | the unanimity early-out's scan, `rm-survivor/src/lib.rs:424` |
| `v` | `most_recent`'s max-and-dedup, `rm-survivor/src/lib.rs:531` |
| `v·log₂v + v` | `ValidInterval`'s sort and grouping pass, `rm-survivor/src/lib.rs:619` |
| `1` | the control's hash lookup, `rm-contrast/src/flat.rs:42` |

**The model assumes the value changes.** The unanimity early-out below
short-circuits a slot whose versions all agree, so predicted work is *not*
additive in that case — the strategy term is never paid. `predicted_work`
models the non-unanimous path, which is the only path the sweep generates, and
its doc comment says so. A model that ignored the early-out would overpredict
exactly the case the sweep is built to avoid, and the two errors would cancel
invisibly.

**What CI can honestly assert, and what it cannot.** Asserting the model is
linear asserts what I wrote — a tautology, and the same trap this crate already
names about its own 1.000 column: *"the store's column is 1.000 by
construction, and is not the finding."* So the tests assert only **derived
answers**:

- `crossover_depth(factor)` — the depth at which predicted store work first
  exceeds the control's by `factor`. A number, computed, not typed.
- At `LIVE_STORE_DEPTH`, the predicted ratio stays under a stated bound. The
  constant carries its provenance in a doc comment, so a model change that
  makes depth 1 expensive fails loudly rather than quietly.

### `benches/read-cost` — the constant

A new crate under the root `Cargo.toml`'s `exclude` list, beside
`benches/ann-bakeoff` and `benches/locomo`. Plain `std::time::Instant`, no
criterion — there is none in this repo and this is not the place to introduce
one. Not run by CI, and its README says so, as `ann-bakeoff`'s does.

Three configurations, because two would answer the wrong question:

| | why it is here |
|---|---|
| `Flat` | the control, and the anchor `rm-contrast` already uses |
| store under `most_recent` | **what actually ships, and what has never been measured** |
| store under `valid_interval` | what `rm-contrast`'s accuracy column is measured under |

Swept over depths **1, 2, 5, 10, 50, 100, 500, 1000**, reporting nanoseconds
per `about()`. Powers-of-ten-ish rather than uniform, because the claim under
test is about shape and a linear sweep spends most of its time in the range
where nothing is happening. Depth 1 is in the sweep because that is where the
real store sits.

## The guard, and where it really lives

Measured ns/read divided by `predicted_work` should be roughly constant across
depths. The bench asserts that ratio's spread — `max / min` across the rungs —
stays inside a band.

**The band is measured first and then written down, not guessed.** A threshold
picked in advance is a threshold picked to pass. The first run's observed spread
goes into the bench's README with the machine it came from, and the assertion is
set at a stated multiple of it, with that multiple justified in a comment rather
than left as a magic number.

A nested loop added inside `merge` leaves the model unchanged, so the measured
ratio blows up at the deep end and the check fails. The model is the reference,
wall-clock is the engine — the same idiom as `rm-conform`, for the same reason:
a model written from the code it judges catches nothing.

**It fires on demand, not in CI.** That is a real limitation, it follows from
`benches/` being excluded, and the README states it rather than implying
continuous protection.

## The generator will lie if it is allowed to

`rm-survivor/src/lib.rs:424` returns early when every assertion agrees:

```rust
// Every assertion agrees -> that answer, whatever the strategy.
let first = asserted[0].value;
if asserted.iter().all(|c| c.value == first) {
    return Ok(Outcome::Survivor(Some(held(first))));
}
```

**A depth-1000 slot holding the same value a thousand times exits there after
one pass.** It would be fast, flat across the sweep, and completely plausible —
and it would be measuring the early-out rather than survivorship.

So the sweep writes *changing* values, and the bench prints two guards rather
than trusting the generator, in the spirit of `ann-bakeoff`'s README recording
exactly this class of silent error:

- **achieved depth per rung**, because a sweep that quietly built depth-1 stores
  at every rung produces a flat table that looks like a finding
- **distinct values per slot**, because that is what decides whether the
  early-out fires

The `ValidInterval` column has a third hazard of its own: it coalesces adjacent
equal values, so its timeline length is a function of how often the value
*changes*, not of depth. The bench reports timeline length beside the timing.

## Scope

- **`about()` only.** No embedding, no I/O, no persistence, no index. The
  README's claim is about survivorship on read.
- **No write-side number.** Both sides are O(1)-ish on write and nobody asked.
  A number nobody asked for is how a cost model becomes half-built.
- **One machine, named**, for the constant — as `ann-bakeoff` does. A constant
  measured on one laptop is a constant measured on one laptop.
- **No change to the default policy.** This produces the number that decision
  needs. Making the decision is a separate piece of work with its own argument,
  and folding it in here would let a benchmark author pick the answer.
- **No change to `rm-contrast`'s accuracy surface.** The grid, the calibration
  cell and the crossovers are untouched.
