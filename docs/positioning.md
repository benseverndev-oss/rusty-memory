# What this is, what it grows into, and what has to be true before anyone can adopt it

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

## Where the idea has not reached yet

A memory has boundaries: things come in, are stored, are identified, are
related, are asked about, and are shared. The principle above is a claim about
**all six**, and it currently holds at four of them.

| boundary | crate | does it refuse to fabricate? |
|---|---|---|
| storage — competing values over time | `rm-survivor` | **yes.** Both kept, resolved at read time |
| identity — is this the same thing? | `rm-resolve` | **yes.** The review band files a question |
| query — what does it hold? | `rm-engine` | **yes.** Three states, not two |
| sharing — whose view is this? | `rm-engine` | **yes.** A holder's assertions are their own slot, and disagreement is kept rather than settled by arrival |
| input — what did that text assert? | `rm-extract` | **not yet.** Extraction asserts; it does not decline |
| relations — how do these connect? | `rm-graph` | **not yet.** No edge has ever been written |
| retrieval — is this near enough? | `rm-index` | **partly.** `weak_below` labels rather than filters, and is off by default because no cutoff transferred between corpora |

Sharing was the most recent to close: an assertion can now name whose
view it is, and two people differing is kept rather than settled by
arrival order. Retrieval also moved — a caller can ask for an answer
without the text that established it, which is the same refusal applied
to what a hit costs rather than to what it claims.

This is the growth story, and it is why the dormant crates are not a backlog.
`rm-extract` and `rm-graph` are the same idea not yet extended to the two
boundaries where it would be most visible:

- **An extractor that declines.** Reading prose and asserting what it found is
  what every extractor does. One that marks a fact as *uncertain* rather than
  asserting it — that files the ambiguous reading the way the resolver files an
  ambiguous identity — is the thesis applied at the input boundary, where the
  most fabrication enters.
- **A graph with `Unknown` edges.** "Does A report to B?" has three answers, and
  every graph store gives two. A relation that can be *asserted absent* — they
  do not report to each other — separately from *never discussed* is the
  clearest possible demonstration of the whole idea, because in a graph the gap
  is visible as a missing line and reads as a fact.

Neither is a distraction from the identity. They are the identity, at
boundaries it has not been carried to yet. They are also, deliberately, **not
next** — see the sequencing below.

## Who it is for

**Anyone building agents where a confidently wrong answer costs more than a
missing one.** Anything touching people, money, records, or compliance.

Reach is wider than the implementation language. The library is Rust, and Rust
developers embed `rm-engine` directly. But the MCP server makes the same store
reachable from **any agent that speaks MCP**, whatever it is written in, and
that is the larger audience. Both are first-class and they need different
things — crates.io, docs.rs and semver for one; an installable binary and a
config that works first time for the other.

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

The asset to build is a benchmark reporting both axes, for this store and for
the alternatives:

- **recall on the 382** — be honest here even if it is not best in class
- **correct refusal on the 112** — the number the identity claims

If competitors answer the unanswerable confidently and this one says `Unknown`,
that is a linkable, reproducible result, and it is the difference between "an
interesting architecture" and "I should use this." If they refuse just as well,
the positioning is wrong and better to know now.

That last sentence is not a formality. The claim that summarise-and-dedupe
architectures fabricate on unanswerable questions is an **inference from how
they work, not a measurement**. It is the first thing the benchmark should
test rather than assume.

The evidence surface grows with the boundaries. Each row of the table above
wants its own measurement, and two are already specified: the resolution corpus
scores wrong merges at the identity boundary, and a wrong merge *is* a
fabrication. An extractor that declines would need the same treatment — a
labelled set where the right answer is "this text does not say."

## What is blocking adoption right now

Positioning is not the binding constraint. **The library cannot be adopted at
all**, and that is a shorter list of fixable things:

1. **Nothing is published.** Most crates are at `0.0.0`. There is no `cargo
   add` that works.
2. **There is no facade.** Fifteen `rm-*` crates and no `rusty-memory` crate
   re-exporting a coherent surface. An adopter cannot tell which of them is the
   public API, and picking wrong means depending on internals. A facade is also
   what lets the internals keep moving without breaking anyone — it is a
   growth mechanism, not just a convenience.
3. **The name `rusty-memory` is free on crates.io and should be claimed.** The
   binary name `rmem` is **already taken** by an unrelated 0.2.0 memory-usage
   CLI, so `cargo install rmem` installs someone else's tool. Shipping the
   binary from the `rusty-memory` crate works, but the collision on `PATH` is
   real and should be a deliberate choice rather than a surprise.
4. **"Status: early" with no stability story.** `rm-core` and `rm-survivor` are
   0.1 and additive-only. That is a good story and it is not told anywhere an
   adopter would look. Saying which crates are stable is what makes it safe to
   change the ones that are not.

MIT licensed, which is one thing that is not in the way.

## Sequencing, and what earns the next step

Order, with the reason each step waits for the one before it:

1. **Prove the claim** — the two-axis benchmark. Until this exists the
   positioning is an assertion and everything else decorates it.
2. **Make it installable** — facade crate, claim the name, publish, docs.rs.
   The facade is what makes steps 4 and 5 possible without breaking adopters.
3. **Rewrite the README around the differentiator** — lead with an `Unknown`
   that saves the reader, not with Acme to Globex. The current headline example
   describes a use case this store has never once served.
4. **Ergonomics** — the store-path fix and the tool-table cost, already specced
   and planned.
5. **Carry the principle to the remaining boundaries** — the graph first, then
   extraction.

Steps 4 and 5 are not a prohibition on the dormant crates, and the earlier
draft of this document was wrong to frame them as competitor turf. They are
waiting on a trigger, and the triggers are nameable:

- **The graph becomes next** when there is a real relational dataset in a
  store and a question being asked of it that the attribute model answers
  badly. Three-state edges are the best demonstration of the thesis available,
  and building them before anything needs them would produce a demo rather than
  a feature.
- **Extraction becomes next** when the input boundary is the one letting
  fabrication in — which will show up as records nobody meant to write. Today
  the input boundary is `note` and `decide`, where a person or an agent asserts
  something deliberately, and deliberate records have a different quality bar
  from harvested ones.
- **Recall work becomes next** if the benchmark shows recall is bad enough to
  disqualify the store before anyone gets to the refusal number. Do not chase a
  recall leaderboard otherwise: winning on their axis with their metric is not
  what makes anyone switch.

The discipline that keeps this from becoming a wishlist: **every step has to be
the same principle at a new boundary, and has to be earned by evidence rather
than by ambition.** A feature that does not refuse to fabricate something is
not this project growing — it is this project becoming one of its competitors.
