# What reading documents actually did

**Measured 2026-08-27**, against three of this repository's own documents —
`positioning.md`, `tiering-cost.md`, `absence-benchmark.md`, 494 lines — into a
scratch store, with `gpt-4o-mini` and the offline embedder.

Step 1 of `docs/superpowers/specs/2026-08-27-document-ingest-design.md`. Nothing
was written to a live store.

## The headline: idempotency does not hold

The spec made this the ship-or-drop condition. It fails.

```
first run:   30 chunks, 30 read,  0 unchanged, 52 facts
second run:  30 chunks, 23 read,  7 unchanged, 22 facts
```

A second run should read nothing. It read 23 of 30.

**Why:** the store holds **9 distinct `source_ref`s** from 30 chunks. Twenty-one
chunks extracted to no facts, wrote no assertions, and so left no trace that
they had ever been read. The store cannot be its own ledger of what was *read*
when the only thing it records is what was *written*.

That is a flaw in the design, not in the implementation. It was chosen over a
sidecar ledger because a sidecar can desync from the store — which is still
true, and is now a trade rather than a free win.

`a_chunk_that_yields_nothing_is_still_remembered_as_read` is committed
**failing and ignored**, so the defect is visible rather than remembered.

### Why no test caught it

Every test used a stub that always returns a fact, so a chunk that yields
nothing never existed. The zero-yield path was not tested badly; it was not
reachable.

This is the same shape as the other verification failures this week: an
instrument that cannot observe the thing, reporting nothing and being read as
evidence of absence.

## The second finding: prose does not yield facts

**Nine of thirty chunks produced anything at all**, and 52 facts came from those
nine.

That is worth more than it looks. This project's documentation is mostly
argument — why a threshold moved, what a measurement ruled out, which of two
designs was refused and on what grounds. Extraction found little to assert in
it, which is the correct outcome and not a failure: there is genuinely no fact
in "a cutoff that has not been measured against the corpus it will run on
produces confident warnings on good answers".

The consequence for the feature is sharper than the yield number. **The
documents worth ingesting are reference, not reasoning** — a register, a
runbook, a schema, an API description. A `docs/` tree of design argument is the
worst case, and it is what was measured here.

## What this changes about the next step

The declines spec —
`docs/superpowers/specs/2026-08-26-extraction-declines-design.md` — was written
from argument, and this run was supposed to give it evidence. It gives less
than hoped, for an honest reason: **too few facts were produced to sample
twenty and categorise how they were wrong.** Fifty-two facts from nine chunks
of one document is not a sample of extraction's failure modes; it is a sample
of one document.

So the twenty-fact reading the plan called for is **not done**, and saying so
is better than presenting nine chunks' worth as though it were.

What the run does establish for that spec:

- The 19% figure it rests on was measured on conversational turns. Nothing here
  contradicts it and nothing here confirms it — the yield was too low and the
  corpus too small.
- A cheaper signal than a per-fact question may exist: **21 of 30 chunks
  produced nothing**, and an extractor that could say so before being asked
  would save most of a run's completions. That is a different idea from
  declining an ambiguous reading, and it may be the more valuable one.

## Three things that did work

- **Chunking.** Heading paths are legible in provenance:
  `absence-benchmark.md#The absence benchmark > What was not run@6654bd77`.
- **`--dry-run`.** `docs/` is 705 chunks, which is 705 completions and roughly
  twenty minutes. Knowing that before paying it is why the sample above was
  three documents rather than the tree.
- **The refusal.** Ingest declines any store holding an assertion whose
  `source_ref` carries no `@`, so it cannot be pointed at a real store by
  accident.

## What has not been decided

How the ledger should work now. Three options, none free:

1. **A sidecar ledger.** Handles zero-yield chunks. Can desync from the store,
   which is the failure this design was avoiding.
2. **A marker assertion per chunk.** Keeps one source of truth, and makes a
   document an entity in all but name — which the spec forbids, for the reason
   that "what does this file say" is a different product.
3. **Accept the re-reads.** Cheapest to build and wrong on the spec's own
   terms: a run that re-reads two thirds of a tree is not runnable on a
   schedule.

This is a design decision and it belongs to a person, which is why the defect is
pinned rather than patched.
