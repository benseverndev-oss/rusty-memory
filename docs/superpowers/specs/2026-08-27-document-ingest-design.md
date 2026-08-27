# Documentation ingest

**Status:** proposed
**Date:** 2026-08-27

**Positioning:** `docs/positioning.md` — the input boundary. This request is
what trips the trigger that document names for extraction.

## The problem

A project's documentation holds facts nobody has told the store: who owns a
system, what a threshold was calibrated against, which decision retired which
other one. Today the only ways in are `note`, where somebody types the fact, and
`remember`, which takes a single conversational turn.

Neither scales to a `docs/` tree, so the tree stays outside the store and the
store keeps answering `Unknown` about things that are written down.

## What this is, and what it costs to be honest about it

**It extracts facts.** Documents go in, assertions come out, recallable as
`Value` / `Absent` / `Unknown` like anything else. It is not a document index:
a query returns an answer, not a passage with a score. That distinction is the
whole reason this project exists, and an ingest that returned passages would
undo it.

That also means this is the **harvested-facts path** the project has
deliberately kept shut. `note` exists because extraction costs a completion per
fact. `rm-extract` has been dormant behind an explicit trigger. The positioning
document is blunt about why: harvested facts have a different quality bar from
deliberate ones.

Ingest is a *volume* feature, and that changes the risk rather than the kind.
One document is a handful of facts; a tree is hundreds. An extractor that
cannot say "this text is ambiguous about who" asserts a reading and is right
most of the time — and the wrong ones are indistinguishable from the right ones
once stored.

## Sequencing, which is part of the design

1. **Ingest, to a scratch store only.** No writes to a live store.
2. **`docs/superpowers/specs/2026-08-26-extraction-declines-design.md`**, built
   against the evidence step 1 produces rather than against reasoning.
3. **Ingest to a live store**, once an extractor can decline.

Step 1 is deliberately first. The declines spec was written from argument, and
its central measurement — that asking one more question per fact cost 19% of
the facts — was taken on conversational turns. Whether that holds for
documents is not known, and step 1 is how it becomes known. Nothing permanent
is risked meanwhile, for the same reason the coworker register went to a
scratch store before the live one.

## The design

### The document is the source, not the subject

Extraction already finds mentions and asserts facts about them. A page about
circ-tools mentions people and systems; those are the subjects. The document
identifies itself in `provenance.source_ref`, which is already a host-defined
string.

No document entity, no new kind. A document is where a fact came from, which is
what provenance is for.

### Re-ingest corrects. Removal does not tombstone.

The decision to defend hardest.

Running ingest again over an edited document re-asserts what it now says.
Where a value changed, the new assertion corrects the old one and both are
kept — which is what the store already does.

**Where a sentence has disappeared, nothing is written.** A removed sentence is
not an assertion of absence: nobody said "there is none", the document simply
stopped saying it. Writing a tombstone there would manufacture absences at the
rate documents get edited, in a store whose entire claim is that it does not
fabricate them.

The cost is stated plainly: a fact deleted from a document goes on standing
until something contradicts it. That is the correct behaviour and it will
occasionally look wrong, which is why it is written down here rather than
discovered later.

### Idempotency by content hash, per chunk

Each chunk carries the hash of its text. An unchanged hash is skipped before
the model is called.

This is a cost control, not an optimisation. Re-ingesting a tree where one file
moved should cost one file's completions, not the tree's — the difference
between something runnable on a schedule and something run once and abandoned.

### Chunk on markdown headings

Headings are the author's own segmentation, and a heading path makes a
`source_ref` a reader can act on:
`docs/positioning.md#the-uncomfortable-part`. A fact whose provenance is a file
path alone tells you which of nine hundred lines to go and read, which is no
help.

Non-markdown files are out of scope. Adding them means a second segmentation
rule with no author-supplied structure to lean on, and that is its own piece of
work.

### Valid time stays unstated; transaction time comes from the run

A file's mtime is when somebody edited a file, not when a fact became true.
Using it as `valid_from` would collapse the store's two clocks into one, which
is the distinction bi-temporality exists to keep.

So an ingested fact is valid from when the store heard it, unless the document
says otherwise in words the extractor reads. That is the same honest default
`note` uses.

## What it does not do

**It does not summarise.** No model-written abstract of a document, at any
level. See `docs/superpowers/specs/2026-08-27-tiering-without-summarising-design.md`
for why the project treats that as the operation it defines itself against.

**It does not make a document an entity.** Asking "what does this file say" is
a different product — a document index — and the answer would be a passage and
a score rather than one of three answers.

**It does not tombstone on deletion.** See above.

**It does not write to a live store.** Not until step 3.

**It does not ingest non-markdown.** See above.

## Measuring it

Three numbers, and the second is the one that decides whether this ships:

- **facts per document**, so the cost of a run is predictable before it is paid.
- **completions per re-run of an unchanged tree.** This must be **zero**. If
  the hash check does not hold, ingest is unrunnable on a schedule and the
  feature is a one-shot import.
- **the review-band rate**, which is the evidence the declines work needs: how
  often does the resolver file a question when the subjects come from documents
  rather than from a conversation? The coworker register produced four
  questions from thirty-four writes, and there is no reason to expect documents
  to behave the same.

## Risks

**It fills the store faster than anyone can check it.** The honest mitigation is
the sequencing above, not a promise to be careful. Step 1 writes nowhere
permanent.

**The 19% is a conversational measurement.** `prompt`'s documentation records
that asking one more question per fact cost 19% of the facts, measured on
LoCoMo turns. Documents are denser and better structured, and that number may
not transfer in either direction. Step 1 is what finds out, and the declines
spec should be revisited afterwards rather than built on the old figure.

**Provenance is only as good as the chunker.** A heading path that drifts from
the document — a renamed heading, a moved section — leaves facts pointing at
somewhere that no longer exists. The content hash detects the change; it does
not repair the reference. Worth knowing before a reader trusts a `source_ref`
from six months ago.
