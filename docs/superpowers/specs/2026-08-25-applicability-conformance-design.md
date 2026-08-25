# Measuring the rule that decides what you see

Give `rm-conform` an applicability axis, so the scope rule is a measured claim
rather than a described one.

## Why now

`rm-conform` exists because *"recall@10 is the wrong kind of metric for the
claim this project makes"* — a correctness claim is universally quantified and
a sampled score cannot express it.

Scope is exactly such a claim. One rule, stated once:

> A memory applies where its scope is an **ancestor-or-self** of the asker's
> position.

It is on the read path of every decision the store holds, it decides what a
session is shown, and the live store now has 219 records governed by it. The
headline table has five rows and none of them is this.

The gap is not hypothetical. The `RMEM_SCOPE=`-is-not-unset defect was found by
reading the live store while checking someone else's work, not by the suite —
every unit test passed throughout, because an empty environment variable was
outside what anything asserted. A sweep is the systematic version of the luck
that caught it.

## What is measured

Three rows, matching the crate's existing shape: differential rows beside
metamorphic ones.

| row | claim |
|---|---|
| `applicability agreement` | for every generated (store, position), `command::decisions(here)` returns **exactly** the decisions whose scope is an ancestor-or-self of that position |
| `depth monotonicity` | for positions `P ⊏ Q`, `visible(P) ⊆ visible(Q)` — moving deeper only ever adds |
| `rescope keeps its history` | after a correction, the bi-temporal grid answers all three cases correctly |

**Every row is a bug if it is not 1.000.**

### Why the second row exists

The reference and the engine are written by the same author against the same
mental model, so they can agree enthusiastically on a shared misunderstanding.
Depth monotonicity is derived from what *ancestor-or-self means* rather than
from either implementation: if a scope reaches a position, it reaches every
position below it, so descending can only add. That is the same role
`monotonic_in_transaction_time` plays for the temporal axis.

## The oracle

The expected set is computed from what the generator wrote, using an
independent formulation of the rule:

```rust
fn reaches(scope: &str, position: &str) -> bool {
    // "*" written out, not `rm_host::scope::UNIVERSAL`. Importing the constant
    // would make the oracle track a change to it silently; spelling it means a
    // change shows up as a disagreement, which is the point of a second
    // implementation.
    scope == "*"
        || position == scope
        || position.starts_with(&format!("{scope}/"))
}
```

Separator-anchored string work rather than a segment-iterator zip. Different
derivation, same claim, and it still refuses `prod` against `production` —
which is the mistake the rule exists to prevent and therefore the one an oracle
must not share by construction.

**This module never imports `rm_host::scope`** — not `applies_at`, not
`validate`, not `UNIVERSAL`. An oracle derived from the code it judges is not an
oracle, and that constraint is the reason the crate's findings have been worth
anything. Scopes are written into the engine through `command::decide` and
`command::plan_rescope`/`commit_rescope` like any other caller, so the *store*
is exercised normally; only the expectation is computed independently.

Noted while checking: `decide` has a convenience wrapper over its plan/commit
split and `rescope` does not, so callers outside the lock must drive both
halves. Not fixed here — it is an ergonomics asymmetry, not a defect, and this
spec is about measurement.

## The generator

A scope tree, positions drawn from it and below it, and decisions assigned
scopes from it.

**The alphabet deliberately contains prefix collisions** — `prod` beside
`production`, `work` beside `workshop`. Without them the segment-versus-string
property is never exercised and `applicability agreement` reports 1.000 having
tested the only thing it was built to catch exactly zero times. An anti-vacuity
test asserts the collisions actually occur in generated trees.

Parameters: tree `depth`, `branching`, how many `decisions` to place, and the
share scoped `*`. A share of universal decisions matters because they are the
row that must appear at *every* position, and the live store's own `*` bucket
is 32 of 219.

Positions are drawn from three places, because they fail differently: a node in
the tree, a node below the deepest generated scope, and a path that shares a
segment prefix with a real scope without being under it.

## Rescope, and where item 2 closes

The correction branch — a scope replacing one already recorded — is
**unexercised in production**. All 219 records in the live store were backfills,
so it has only ever run in unit tests. It is also the branch carrying the
bi-temporal argument, which makes it the one worth measuring rather than
asserting.

Three cases, all checked over a `(valid, tx)` grid:

- **backfill**, no previous scope: the scope is dated from the decision's own
  earliest `choice`, so it applies at every valid time from there. At a
  transaction time *before* the rescope, no scope is visible at all and the
  decision reaches everywhere — the legacy rule, still correct.
- **correction**, a previous scope: dated from now, so a valid time before the
  correction sees the **old** reach and after it sees the new. Dating this from
  the decision's start would assert the decision always reached somewhere it
  did not.
- **no-op**, same scope again: nothing is written and the version count does
  not move.

## Anti-vacuity

The crate's existing discipline, applied to the new axis. Four companions,
because a green row that measured nothing is worse than a red one:

1. Prefix-colliding names actually occur in generated trees.
2. Across the sweep the applicable set is neither always empty nor always
   everything — either would make agreement trivially true.
3. Some generated position genuinely excludes some generated decision. The
   mirror of (2), and the one that fails if every scope ends up `*`.
4. The rescope correction branch actually fires, rather than every generated
   operation being a backfill.

## Where it lives

`crates/rm-conform/src/applicability.rs`, a new module beside `decisions.rs`.

It exposes its grids `pub` — a position set and a `(valid, tx)` probe grid for
the rescope row — the way `invariants::probe_grid` and
`decisions::coverage_probes` already are, so `report.rs` and the tests read the
same grid rather than two that could drift apart and make the README and the
suite disagree about what was measured.

The three rows appear in the table under exactly these names:
`applicability agreement`, `depth monotonicity`, `rescope keeps its history`.

## Runtime

Measured before designing: the existing 500-seed sweep computes in **0.27s** in
release, and the crate's whole test run is 6.6s including the build. The
addition is set comparisons over small in-memory engines.

Budget: under **1s** added in release, under **3s** in debug tests. If it
exceeds that, cut seeds rather than coverage, and say in the README what was
cut — a silent truncation reads as "covered everything" when it did not.

## Out of scope

- **The live store.** Generated data only. The crate is free, deterministic and
  runs in CI on every push; pointing it at 219 private records on one machine
  breaks all three, and it is the trade `benches/locomo` already lost.
- **The MCP layer.** The rule lives in `command::`; the MCP surface parses a
  position and passes it through, and its own tests cover that.
- **`recall` and `about`.** Still unscoped, still a separate axis, still a
  retrieval-quality question rather than a correctness one.
- **Fixing anything this finds.** Reported, not closed — the same rule the
  crate has followed since it landed.
