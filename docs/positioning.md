# What this is, who it is for, and what has to be true before anyone can adopt it

**Status:** proposed
**Date:** 2026-08-26

## The one line

**A memory that knows what it doesn't know.**

Not "resolves contradictions deterministically". That names the least
distinctive third of the idea, and versioned facts are not rare.

## The idea, stated once

Three of this codebase's hardest decisions are the same decision:

| decision | what it refuses to fabricate |
|---|---|
| contradictions kept, survivorship at read time | a winner, at write time |
| `Believed` has three states, not two | a value, where there is only a gap |
| the review band refuses to merge | an identity, on evidence that fell short |

One principle, applied three times: **the store declines to produce a confident
answer it has not earned.**

That is worth saying plainly because it is the thing competitors do not do.
Systems in this space dedupe by embedding similarity and settle conflicts by
asking a model to re-summarise. Both operations are lossy and neither reports
that it happened. Ask one whether someone has an employer and it will tell you.
Ask this one and it can say `Unknown` — nobody has ever discussed it — which is
a different answer from `Absent`, and the difference is the product.

## Who it is for

**Rust developers building agents that need memory they can trust more than a
summariser.** Specifically agents where a confidently wrong answer costs more
than a missing one: anything touching people, money, records, or compliance.

And, sharpening it: **agents that share a store.** A single agent
misremembering costs one bad turn. A shared store that fabricates propagates a
confident wrong thing into every session that reads it, and nothing reports the
propagation because two plausible answers are not an error. The multi-agent
case is where refusing to guess stops being an elegance and starts being the
reason to choose this.

That is not a guess about the market. It is what this store has actually been
used for: 275 entities, every one a decision or a lesson, read by many sessions
across many repositories, several of which have corrected each other from it.

## The uncomfortable part

**The pain this solves is one adopters have not named yet.**

They feel *"my agent forgot."* They do not feel *"my agent confidently told me
the user has no employer, because nobody had mentioned one."* The second thing
happens constantly and is invisible, which is precisely why it goes unnamed.

Positioning against a felt pain is easy. Positioning against an unfelt one
means the demo has to *create* the recognition. That is the central marketing
problem, and it has a technical answer.

## The proof obligation

**Nothing currently measures the claim.** Recall is measured. Refusal is not.

`benches/locomo` already holds the right instrument and treats it as the
control: 382 answerable questions against **112 unanswerable** ones, whose
premise the conversation does not support. Under the recall framing the 112 are
noise. Under this identity they are the entire product.

The asset to build is a benchmark that reports both axes, for this store and
for the alternatives:

- **recall on the 382** — be honest here even if it is not best in class
- **correct refusal on the 112** — the number the identity claims

If competitors answer the unanswerable confidently and this one says `Unknown`,
that is a linkable, reproducible result, and it is the difference between "an
interesting architecture" and "I should use this." If they refuse just as well,
the positioning is wrong and better to know now.

The README already contains a warning against overclaiming here, and it should
be honoured: a cutoff tuned on LoCoMo marked a question with a perfect answer
as having nothing near it. The benchmark measures refusal *behaviour*, not a
tuned threshold, and the report says which is which.

## What is blocking adoption right now

Positioning is not the binding constraint. **The library cannot be adopted at
all**, and that is a shorter list of fixable things:

1. **Nothing is published.** Most crates are at `0.0.0`. There is no `cargo
   add` that works.
2. **There is no facade.** Fifteen `rm-*` crates and no `rusty-memory` crate
   re-exporting a coherent surface. An adopter cannot tell which of them is the
   public API, and picking wrong means depending on internals.
3. **The name `rusty-memory` is free on crates.io and should be claimed.** The
   binary name `rmem` is **already taken** by an unrelated 0.2.0 memory-usage
   CLI, so `cargo install rmem` installs someone else's tool. Shipping the
   binary from the `rusty-memory` crate works, but the collision on `PATH` is
   real and should be a deliberate choice rather than a surprise.
4. **Two audiences are half-served.** Rust embedders need crates.io, docs.rs
   and semver. MCP users need an installable binary and a config that works on
   the first try. These are different distribution problems and neither is
   finished.
5. **"Status: early" with no stability story.** `rm-core` and `rm-survivor` are
   0.1 and additive-only. That is a good story and it is not told anywhere an
   adopter would look.

MIT licensed, which is one thing that is not in the way.

## What not to build for adoption

**Extraction and the graph.** They serve the conversational-memory story, they
are where the competition is strongest, and they are not the differentiator.
`rm-extract` and `rm-graph` being dormant has read like a backlog; under this
identity it is a scope decision, and `note` shipping ahead of them was the
right instinct — deliberate records have a different quality bar from harvested
ones.

**A recall leaderboard.** Winning on recall means competing on their axis with
their metric. Report recall honestly; do not chase it.

## Sequence

1. **Prove the claim** — the two-axis benchmark. Until this exists, the
   positioning is an assertion, and everything else is decoration on it.
2. **Make it installable** — facade crate, claim the name, publish, docs.rs.
3. **Rewrite the README around the differentiator** — lead with an `Unknown`
   that saves the reader, not with Acme to Globex. The current headline example
   describes a use case this store has never once served.
4. **Then ergonomics** — the store-path fix and the tool-table cost, which are
   already specced and planned.

The resolution corpus already specced serves step 1 from the other side: a
wrong merge *is* a fabrication, and it is the first measurement of the honesty
thesis anywhere in the repository.
