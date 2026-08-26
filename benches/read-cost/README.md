# Read cost

Answers the question `rm-contrast`'s README left open:

> The difference is asymptotic rather than a constant factor, and it is not
> measured here -- a half-built cost model would be worse than this sentence.

Specifically: **at what history depth does the store's read cost stop being
negligible, and how far is the real store from there?**

Not in the workspace and not run by CI, as `ann-bakeoff` and `locomo` are not.
Free and deterministic apart from the clock: no embedder, no network, no key,
and about ten seconds end to end.

## Running it

```sh
cargo run --release --manifest-path benches/read-cost/Cargo.toml
```

## Checking a real store

```sh
cargo run --release --manifest-path benches/read-cost/Cargo.toml -- path/to/store.json
```

Reports versions per attribute slot for that store, and says whether the
deepest slot still matches `rm_contrast::cost::LIVE_STORE_DEPTH`.

**That constant is the load-bearing part of everything below.** It was measured
once, by hand, against one store, and two READMEs quote it as the reason cost
does not matter. A constant standing in for a moving thing with nothing able to
re-check it is the drift this project keeps finding, so this is the way to
re-check it:

```
# Store depth: D:/memory/decisions.json

entities: 219
slots:    1086
depth histogram (versions per slot -> slots): {1: 1086}
deepest:  1 versions, entity 0 "because"

deepest slot is 1, matching rm_contrast::cost::LIVE_STORE_DEPTH.
```

A store that has moved on is **reported, not failed**: this runs against
whatever it is pointed at, and a deeper store is news rather than a broken
build. It reads through `Engine::open_split` rather than picking the snapshot
apart, so it cannot drift from the format the store actually writes, and the
counting itself lives in `rm_contrast::cost::depth_histogram` where CI can test
it -- this crate is excluded from the workspace and never built there.

## What it measures

Three configurations, because two would answer the wrong question:

| | |
|---|---|
| `Flat` | the control, the same twenty lines `rm-contrast` uses |
| store under `most_recent` | **what ships**, and what `rm-contrast` has never measured |
| store under `valid_interval` | what `rm-contrast`'s accuracy column is measured under |

`rm-contrast`'s own `score.rs` explains why it picked the third: *"MostRecent
collapses to a winner and would answer every valid time the same, which is the
control's behaviour rather than this store's."* Which is exactly why the
shipped default deserved measuring rather than assuming.

## Results

Machine: 13th Gen Intel Core i5-1335U, Windows 10.0.26200, rustc 1.98.0,
`--release`. One laptop. A constant measured on one laptop is a constant
measured on one laptop.

```
# Read cost against history depth

Iterations scale with depth. Values are nanoseconds per `about()`.

| depth | distinct | flat | most_recent | valid_interval | mr ns/work | vi ns/work |
|---|---|---|---|---|---|---|
| 1 | 1 | 142 | 320 | 462 | 79.97 | 115.47 |
| 2 | 2 | 140 | 392 | 667 | 48.98 | 66.74 |
| 5 | 5 | 141 | 572 | 1297 | 28.61 | 41.03 |
| 10 | 10 | 142 | 1014 | 2600 | 25.35 | 35.51 |
| 50 | 50 | 215 | 1339 | 10862 | 6.70 | 22.53 |
| 100 | 100 | 141 | 1837 | 17129 | 4.59 | 16.09 |
| 500 | 500 | 149 | 4559 | 74342 | 2.28 | 11.47 |
| 1000 | 1000 | 142 | 8268 | 181667 | 2.07 | 13.01 |

most_recent: 320 ns at depth 1, marginal 1.91 ns per predicted unit (fit intercept 693). History overtakes the depth-1 cost at about depth 42.

valid_interval: 462 ns at depth 1, marginal 12.65 ns per predicted unit (fit intercept 1148). History overtakes the depth-1 cost at about depth 10.

The control: 140-215 ns across a 1000x depth range, which is the O(1) it claims to be.

The live store is at depth 1, where the variable term is one unit of the marginal cost above and everything else is fixed.
most_recent: marginal 1.91 ns/unit overall, 1.80 deep-only, drift 1.06x
valid_interval: marginal 12.65 ns/unit overall, 12.60 deep-only, drift 1.00x
```

**The answer, in one line: at depth 1 a read costs 320ns under `most_recent`
and 462ns under `valid_interval`, against 142ns for the control.** Everything
is sub-microsecond, and the difference between the two strategies is about
140ns.

Two things worth reading off the table:

**Fixed cost dominates until roughly depth 42.** A read pays for an entity
lookup, a `Vec` allocation and a returned `Believed` whatever the depth, and
below about forty versions per slot that is most of what it pays. The `ns/work`
columns fall by a factor of forty across the sweep for exactly that reason --
not because the model fails to track the engine, but because a constant is not
a function of depth.

**The control really is O(1).** It sits between 140 and 215ns across a
thousand-fold change in depth, which is what a hash lookup should do and is
worth confirming rather than assuming.

## Where the live store sits

**Depth 1, across all 1,086 attribute slots** -- 219 decisions, five attributes
each, and nothing revised. Measured from `decisions.json` on 2026-08-25.

Two caveats, because that anchor is doing a lot of work.

The store is two days old and was seeded once, so depth 1 says where it sits
and not that revisions are rare in general.

**And depth may be a fact about when a store is written to rather than how
often things are re-decided.** A peer session running its own store had a
genuine supersession inside a few hours and recorded one record holding the
final state, because it wrote its decisions at end of day once things had
settled -- so the intermediate belief never reached the store. This store
shows the same signature: one of its 219 decisions is `rejected`, and its
`status` slot is at depth 1.

Age does not cure that. It predicts depth stays at 1 in a store used for
months, so long as the writer keeps recording after the fact -- which means
a depth figure read off a retrospectively-written store measures a habit as
much as a history.

### What depth 1 does not tell you about `rescope`

An earlier version of this README suggested `rescope` might be overwriting
rather than appending, since a `rescope` pass ran across all 219 records and
left every `scope` slot at depth 1. That inference was wrong, and correcting it
is worth more than deleting it.

`commit_rescope` branches on whether a scope is already held:

```rust
let valid_from = match previous {
    // Correction: the reach changed today, and only today.
    Some(_) => observed_at,
    // Backfill: it always reached this far.
    None => visible(engine, entity, "choice", At::latest())
        .first()
        .map_or(observed_at, |v| v.valid.from),
};
```

All 219 took the **backfill** branch. The pre-run backup
(`decisions.json.bak-20260825-090622`) holds 219 entities and **zero** with a
`scope` attribute, so the store already existed fully populated and scope-less;
`choice` carries an `observed_at` about sixteen hours earlier than `scope`, so
the scopes arrived in a separate later pass. A backfill writes the first version
of an attribute, and a first version is depth 1 by definition.

So **the depth figure is not evidence about `rescope` in either direction.**
This store holds only first writes, so the correction branch has never executed
against it. Depth 1 is a fact about this store's history, not about the command.

For the record, since the original caveat raised the possibility: `rescope` does
append. `commit_rescope` calls `write_field`, which calls `remember_as` with
`Supersession::Corrects`; there is no overwrite path, and the only early return
is a no-op guard that declines to write a second identical version. The
correction branch is **un-lived rather than untested** — `rm-conform`'s
*rescope keeps its history* row rescopes an already-scoped decision across 60
seeds and reports 1.000.

### Falsified, the same day it was published

**Depth 1 was an artefact of construction, and the mechanism was none of the
ones guessed above.** Not age, and not recording habits. Depth arrives when
people *correct* things, and a two-day-old store had not been corrected yet.

One afternoon in which five sessions repaired each other's records took the
store from 1,086 slots all at depth 1 to:

```
entities: 254   slots: 1256
depth histogram: {1: 1238, 2: 17, 3: 1}
```

Eighteen slots deeper than one, and a depth-3, inside an hour. **Reads are not
fixed-cost dominated in a store anybody maintains.**

The distinction that matters for read cost, and which a histogram cannot carry:
most of that depth is `scope` slots corrected by a metadata fix, but entities
246 and 252 went to depth 2 on `choice` and `because` as well — genuine
re-decisions rather than field repairs. "A field was corrected" and "the
decision changed" cost the same to read and mean different things.

The measurement in `benches/read-cost` is unaffected — it sweeps synthetic
depth and never depended on this number. What is affected is the *conclusion*
drawn from it: that the shipped-default question could be settled on
correctness because cost was three orders of magnitude away. At depth 1 it was.
Whether it stays that way is now an open question rather than a closed one, and
`read-cost <store.json>` is how to keep asking it.

## The generator would lie if it were allowed to

`merge` returns early when every assertion agrees
(`rm-survivor/src/lib.rs:424`). A slot holding one value a thousand times exits
there after a single pass -- fast, flat across the whole sweep, and completely
plausible. It would be measuring the early-out rather than survivorship.

So every version carries a distinct value, and two guards assert it rather than
trusting it: achieved depth per rung, and distinct values per rung. Both fail
the run rather than printing a flattering table. The `distinct` column is in
the output so a reader can see the guard held.

## The guard, and what it does not protect

The asymptotic claim is about the **marginal** coefficient, so that is what is
checked. The whole sweep and the deep rungs alone are fitted separately, and
their slopes must agree within 2x. A read path that went quadratic would make
the deep-only slope run away from the overall one.

Measured drift is 1.07x for `most_recent` and 1.03x for `valid_interval`. The
band was set after seeing those, not before -- a threshold picked in advance is
a threshold picked to pass -- with enough headroom for a different machine and
a noisy run, and still tight enough that an order-of-magnitude change trips it.

An earlier version of this bench checked a single measured-over-predicted ratio
instead, and its spread came out at 66x. That was not a failing engine; it was
a cost model with no fixed term being asked to predict reads that are mostly
fixed cost. Splitting the fit is what made the guard mean anything.

**It only fires when someone runs this.** `benches/` is excluded from the
workspace and CI does not touch it. A superlinear read path would land, break
nothing, and wait here to be noticed.

## What this does not say

- **Nothing about the write path.** Both sides are O(1)-ish on write.
- **Nothing about retrieval.** No embedder is involved; `about()` takes an
  entity id.
- **It does not decide the shipped default.** That argument has a correctness
  half, and settling it inside a benchmark would be the thumb on the scale
  `rm-contrast` is built to avoid. This is the cost half, and it is now
  measured.
