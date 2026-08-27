# Tiering without summarising

**Status:** proposed
**Date:** 2026-08-27

## The problem

`recall` returns whole assertions. Every hit carries its value text, its
validity interval, its provenance and its standing, whether the caller needed
them or not.

That cost is paid on a surface this project has already measured carefully. The
MCP tool table is ~2,450 tokens per turn, and cutting it from ~2,600 took a
careful prose edit and a measurement to find where the bytes actually were. Hit
payloads are the other half of the same bill and nobody has looked at them.

## Why not OpenViking's tiering

OpenViking processes every entry on write into L0 (~100-token abstract), L1
(~2k overview) and L2 (full detail), loading only as deep as the task requires.
The idea is right and the mechanism is wrong for this project, twice over:

- **The layers are model-written summaries.** That is lossy re-summarisation —
  the operation this store defines itself against, applied on the write path.
- **It costs a completion per write.** `note` exists precisely to make a fact
  cost one embedding and no completion. Tiering that reintroduces a model call
  per write undoes the door it was built to open.

**The layers this store needs already exist, structurally, for free.** They are
not summaries of the content; they are the content at different resolutions of
*question*.

| level | what it answers | already in the model |
|---|---|---|
| **L0** | what was found, and does it still stand | `entity`, `attribute`, `standing`, `score` |
| **L1** | what does it say | `value`, `valid` |
| **L2** | who said it, when, and what it replaced | `provenance`, and the version history |

`about` is already L0-shaped. `store_history` is already L2. Nothing exposes
them as levels, and `recall` always returns L1.

## The design

A depth on the query, defaulting to today's behaviour.

```rust
pub enum Depth {
    /// Entity, attribute, standing, score. No value text.
    Located,
    /// ...and what the assertion says. Today's behaviour, and the default.
    Stated,
    /// ...and provenance, and the versions it stands against.
    Traced,
}
```

`Query` gains `pub depth: Depth`, and `Recalled`'s value-bearing fields become
optional at `Located`. **`Stated` is the default**, so every existing caller is
unchanged and the feature is opt-in in the direction that saves money.

### What `Located` is for

```
recall("who owns Okta", depth = Located)
  323  role      stands   0.609
  308  employer  stands   0.331
  317  role      stands   0.318
```

Roughly 15 tokens a hit against roughly 60. A caller sees what was found and
fetches text for the one hit that matters — and often does not need to fetch
anything, because `323 role` plus a follow-up `about(323, "role")` is the whole
interaction.

### What `Traced` is for

The honest answer to a question this store cannot currently answer: **why did
this come back?** Today that is a cosine score, and a cosine score is not an
explanation — which is the same complaint this project makes of everyone else's
retrieval. `Traced` returns provenance and the versions an assertion stands
against, so "this is the answer, here is who said it and what it replaced" is
one call.

That is not full explainability. It does not say why the vector matched. It
does say what the answer rests on, which is the part a caller can act on.

## What this does not do

**It does not summarise anything.** No model call, no generated text, nothing
lost between levels. `Located` omits; it does not compress. The same assertion
at `Traced` is byte-identical to what `recall` returns today plus history.

**It does not change `about`.** `about` returns one answer and is already as
cheap as it gets. The three-way answer is not tiered — `Absent` and `Unknown`
have no deeper level to fetch.

**It does not cache or precompute.** Every level is derived from what is
already stored, at query time. A precomputed layer is a second copy that can
drift from the first, which is the failure this repository has now hit with a
token count and a competitor claim in the same week.

**It does not tier the tool table.** That is a separate, already-specced piece
of work about the *schemas*, not the results.

## Measuring it

The claim is a token saving, so it needs the same treatment the tool table got —
a measurement, in one place, that fails when it rots.

Measure the serialised bytes of a fixed corpus of hits at each depth, assert the
ratio, and state it beside the feature. `CHARS_PER_TOKEN` already exists for
exactly this and should be reused rather than restated.

The number to beat is not arbitrary: `Located` has to save enough to be worth a
second call for the cases that need text. If the measured saving is small, the
honest outcome is that only `Traced` ships and `Located` does not.

## Risks

**Two calls where there was one.** `Located` trades bytes for round trips. For
an MCP client each round trip is a turn, and a turn costs more than the bytes
saved. This is the risk that could sink `Located` entirely, and the measurement
above is what decides it — a saving per hit only pays if a typical query returns
several hits and the caller wants text from one.

**A caller that always asks for `Traced`.** The default is `Stated` and the
cheapest level is opt-in, so a lazy caller pays today's price rather than more.
But an agent told "more context is better" will ask for `Traced` every time and
make things worse than before. The tool description has to say what each level
is for, and that is prose whose value cannot be tested here — the same honest
limit the tool-table work already carries.
