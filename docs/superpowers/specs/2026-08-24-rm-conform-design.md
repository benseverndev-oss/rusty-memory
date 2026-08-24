# rm-conform — design

Status: approved design, pre-implementation.

A conformance harness that measures what this store is *for* — contradiction,
supersession, and time — against ground truth computed independently of the
code under test.

## What this is for

Everything measured in this project so far measures retrieval. `benches/locomo`
reports recall@10 over a real corpus, and its own README records the finding
that ended that line of work:

> a twenty-line control beats the pipeline on it, and none of the distinctive
> machinery serves it

That sentence is the whole problem. `rm-survivor`, the bi-temporal read in
`rm-engine`, `Supersession`, `Standing` — the parts of this workspace that are
not in every other memory store — contribute *nothing* to the number the
project reports. The decision log recorded the consequence as `proposed` and
left it there, blocked:

> Retire recall@10 as the headline metric — measure what the store is for —
> contradiction, supersession, time — instead. But LoCoMo labels no ground
> truth for the alternative.

The blocker was real and is now gone, because it was an assumption about where
ground truth has to come from. LoCoMo has no supersession labels. A history
this harness *generates* has them by construction: record A, correct it with B,
and what should stand is not a judgement call.

## The central argument: correctness is not quality

recall@10 is not a bad metric. It is the wrong *kind* of metric for the claim
this project makes.

The README claims the store "resolves contradictions deterministically." That
is a **correctness** claim. A quality score in `[0,1]` cannot express it — there
is no value of recall@10, including 1.000, that means "and it is never wrong,"
because recall@10 is an average over a sample and correctness is a property
over all inputs.

So the instrument changes shape. The headline stops being a score to raise and
becomes a claim to hold:

> **0 disagreements with the reference model across N generated histories.**

Any deviation is a bug, not a lower score. This is the one number recall@10 can
never produce at any value, and it is the number that corresponds to what the
store says it does.

### Demote, not retire

The decision log's `proposed` entry says *retire*. That is too strong, and the
reason is a use report rather than an argument.

The store's first external consumer states that the query they most want is
"has anyone tried X" — find the decision by description, having half-forgotten
it. That is a retrieval query. Retrieval is the daily-use path for anything
shaped like a decision log, because the alternative is remembering that the
decision exists, and anyone who could do that reliably would not need the
store.

So retrieval stays, as a second axis, scored as the quality measure it is.
Correctness takes the headline because it is the distinctive claim and because
nothing currently tests it. When that entry is superseded, it should be
superseded with *demote*, not *retire*.

## Ground truth by construction requires a reference model

"Known by construction" is only true if the expected answer can be computed
without consulting the code under test. A generator that asks the engine what
it did and records the reply has measured nothing.

So the harness carries a **reference model**: a second implementation of
survivorship over `(valid, tx)`, written for auditability rather than
performance. No index, no persistence, no locking, no resolution — a list of
assertions and a fold over it. Target under 100 lines, small enough to be read
end to end and believed.

The real engine is not that. It carries a vector index, a snapshot format, an
advisory lock, entity resolution, and a plan/commit split with network calls
above the lock. Correctness there is not visible by inspection, which is
exactly why it needs an oracle that is.

**This makes the reference model the crux, and it gets tests of its own,
first.** An untested oracle is worth nothing — it converts every disagreement
into an argument about which side is wrong. TDD applies to the reference model
before the generator exists.

When engine and reference disagree, the harness shrinks the history to a
minimal failing case and reports both answers. It does not assume the engine is
the wrong one.

## The generator

A history is a sequence of assertions, each carrying:

- an entity and an attribute (a small alphabet, so collisions are frequent
  rather than rare)
- a value
- a valid interval (`Interval`)
- an `observed_at` (transaction time), which is *not* required to agree with
  valid time — backdating is the interesting case
- a `Supersession`: `Corrects`, `Joins`, or `Unstated`

The generator is parameterised so difficulty is a knob rather than a rewrite:
history length, alphabet size, rate of backdating, rate of out-of-order
arrival, rate of exact timestamp ties, and the mix of `Supersession` values.

Timestamp ties are called out because they are where three strategies are
specified to *refuse*, and a generator that never produces them would never
reach that code.

## What is scored

Three groups, at the layer that can actually answer each.

### 1. Bi-temporal agreement

For each generated history, probe `Engine::about(entity, attribute, valid_t,
tx_t)` at randomly chosen points on both axes, including points before anything
was asserted and after everything was. Compare with the reference.

This is where the store's motivating example lives — a job change mentioned
late is true from when it happened, not from when it was said — and it is
scored over generated histories rather than the single fixture that currently
encodes it.

### 2. Refusal correctness

`Strategy` has nine variants and three of them refuse under stated conditions:

| Strategy | Refuses when |
|---|---|
| `MostRecent` | the latest observation ties between different values |
| `ValidInterval` | two different values share an observation timestamp |
| `SourcePriority` | an asserting source is absent from the priority list |

A refusal is a defined answer, not an error, and it has two failure modes that
a quality score cannot distinguish: refusing when it should not (the store is
useless) and answering when it should refuse (the store is silently wrong).

**The property is exact: refuse if and only if the reference refuses.** Nothing
currently tests this systematically, and the generator can construct the ties
deliberately rather than waiting to stumble on them.

### 3. Supersession and standing

At the decision layer, through `command::decide` and `command::decision`:

- a generated chain of decisions, each superseding the last, is recovered in
  the right order
- `Standing` matches the reference for every assertion —
  `Latest` / `Joined` / `Corrected` / `Unsettled`
- `still_stands()` is true exactly where the reference says the fact may be
  stated as current

`Standing::Unsettled` deserves its own attention. Its doc comment says a store
that guesses there "is wrong a quarter of the time and never says so." That is
a claim about a rate, and this harness can measure whether the rate of
`Unsettled` under generated histories matches the rate the reference computes —
turning a remark into a number.

## Metamorphic invariants

Properties that need no oracle, asserted on the same generated histories. They
cost almost nothing once the generator exists and they catch classes the
reference model cannot, because they hold regardless of what the right answer
is:

- **Transaction-time monotonicity.** Adding an assertion observed at `t` must
  never change any answer at `tx < t`. What you believed last Tuesday does not
  move because you learned something today. This is the defining property of
  the transaction axis and nothing currently asserts it.
- **Arrival-order independence.** Permuting the order assertions are ingested,
  with `observed_at` and valid intervals fixed, must produce identical beliefs
  at every probe. If it does not, the store depends on ingestion order it
  claims not to.
- **Chain acyclicity.** `supersedes` edges form a DAG. `command::chain` already
  carries a visited-set cycle guard, which is evidence someone expected cycles;
  this asserts they cannot be constructed through the public API.

Arrival-order independence is the one most likely to find something, because it
is the property most easily broken by an optimisation.

## Two layers, and the gap between them

`Engine::about` takes `valid_t` and `tx_t`. `command::decisions` and
`command::decision` take **neither** — verified at `crates/rm-host/src/command.rs:788`
and `:968`.

So the decision log, which is the product direction, cannot be asked a
temporal question at all. The store's distinguishing capability is not reachable
from its product surface.

The harness reports this as a coverage number — *what fraction of the
bi-temporal probe set can the decision API answer* — which is currently zero.
It does **not** fix it. Making that a number that reappears on every CI run,
rather than a thing one session happened to notice, is the whole point of
measuring it.

It leads no table. The one real consumer says chains matter more to them than
recency, and the user confirmed the worse failure is a stale answer given
confidently rather than an unanswerable date query. It stays reported and
un-promoted until someone asks for it.

## Where it lives, and why not `benches/`

`benches/locomo` and `benches/ann-bakeoff` are excluded from the workspace, for
a stated reason:

> Not in the workspace and not run by CI: it costs money and takes minutes.

That is correct for those two and wrong for this one. The consequence of living
in `benches/` is visible in the record: four measured findings shipped switched
off, in part because re-measuring is expensive enough that nobody re-measures.
A conformance suite that runs when someone remembers to run it is a conformance
suite that goes stale.

This harness costs nothing. The local embedder needs no key and opens no
socket, and most of what is scored needs no embedder at all. So:

**`crates/rm-conform`, in the workspace, gating CI.** `cargo test` runs a fixed
seed set on every push; a `--report` binary mode runs the large sweep and emits
the headline table for the README.

Seeds are fixed and printed. A failure that cannot be reproduced from its seed
is not a finding.

## The headline table

| property | target |
|---|---|
| chain integrity | 1.000, exact |
| standing accuracy | 1.000, exact |
| refusal correctness | 1.000, exact |
| as-of agreement, both clocks | 1.000, exact |
| transaction-time monotonicity | holds |
| arrival-order independence | holds |
| decision-layer time coverage | currently 0 |
| *retrieval quality (second axis)* | *a score, not a target* |

The first six rows are correctness properties: anything short of the stated
target is a bug, not a lower score. The seventh is a coverage number nobody has
yet asked to be non-zero. Only the last is a score to raise.

## What this deliberately does not do

- **No LLM judge.** The LoCoMo README's constraint holds and is the reason the
  reference model exists: "the first number this project needs is one nobody
  has to trust."
- **No fix for the decision-layer time gap.** Reported, not closed.
- **No fix for the two `reindex` defects** found by the first external
  consumer. `"reindex" => Ok(Command::Reindex)` at `args.rs:299` discards every
  trailing argument with no arity check, so a typo'd flag on a mutating command
  is silently accepted. Separate work.
- **No entity-resolution scoring.** The Fellegi–Sunter thresholds are
  calibrated against a real corpus and generated names would measure the
  generator's name distribution, not the resolver. Out of scope, deliberately.
- **No hand-authored scenario catalogue up front.** Named regression cases are
  frozen *from failures the generator finds*, so the catalogue accumulates from
  evidence rather than from imagination. This is the direct answer to the
  README's own criticism of fixtures — that pinning a property you thought of
  is "the right way to pin behaviour and it cannot tell you whether the
  behaviour is any good."

## Build order

1. Reference model, with its own tests. Nothing else is meaningful until this
   is trustworthy.
2. History representation and a deterministic, seeded generator.
3. Bi-temporal agreement scoring against `Engine::about`.
4. Metamorphic invariants on the same histories — cheap once 2 exists.
5. Refusal correctness across the nine strategies.
6. Decision-layer chain and standing scoring.
7. `--report` mode and the README table.

Steps 1–4 are the core. If the work stops after 4, the project still gains the
first correctness number it has ever had.

## Risks

**The reference model could be wrong in the same way the engine is.** Written
by the same author against the same mental model, it can encode a shared
misunderstanding and agree enthusiastically. The metamorphic invariants are the
mitigation, because they are derived from what bi-temporality *means* rather
than from how it was implemented — a shared bug that also satisfies
transaction-time monotonicity is a much narrower target.

**Generated histories may not resemble real ones.** They will not. That is
acceptable here in a way it would not be for a retrieval benchmark: correctness
properties are universally quantified, so an unrealistic history is still a
valid input. The risk is spending effort on regions of the input space nobody
reaches, not measuring the wrong thing.

**A green suite proves less than it appears to.** It proves the engine agrees
with a small model on generated inputs. It does not prove the store is useful,
that titles are findable, or that anyone will record a decision. Those are the
retrieval axis and the use report, and both stay.
