# rm-conform

Scores this store on what it claims to do — contradiction, supersession and
time — against ground truth computed without asking the code under test.

```sh
cargo test -p rm-conform                      # the properties, as CI runs them
cargo run --release -p rm-conform -- --report # the table below, recomputed
```

Free, deterministic, and in the workspace, so CI runs it on every push. That is
deliberate and it is the opposite of `benches/locomo`, which lives outside the
workspace because it costs money and minutes — with the consequence, visible in
its own README, that four measured findings shipped switched off because
re-measuring was expensive enough that nobody re-measured.

## Why not recall@10

recall@10 is not a bad metric. It is the wrong *kind* of metric for the claim
this project makes.

The README says the store "resolves contradictions deterministically". That is
a **correctness** claim, and no value of recall@10 — including 1.000 — can
express it, because recall@10 averages over a sample and correctness quantifies
over all inputs. `benches/locomo` recorded the consequence itself: *"a
twenty-line control beats the pipeline on it, and none of the distinctive
machinery serves it."*

So the headline here is a claim to hold rather than a score to raise. Every row
below is a bug if it is not 1.000.

The last row was `0.000` when this crate landed, and it was not a bug — it was
the honest report of a decision API that took no time parameters, so no probe
could reach it. It measures correctness like the others now.

## The table

Seeds `0..500` for the merge sweep and `0..60` for the applicability rows, 20
probes per history, 12 assertions each. Fewer seeds for the second group
because each one builds a real engine and writes a dozen decisions through the
command path, where the merge sweep compares two pure functions.

| property | result |
|---|---|
| merge agreement, 8 strategies | 1.000 |
| refusal correctness | 1.000 |
| transaction-time monotonicity | 1.000 |
| arrival-order independence | 1.000 |
| decision-layer time coverage | 1.000 |
| applicability agreement | 1.000 |
| depth monotonicity | 1.000 |
| rescope keeps its history | 1.000 |

750 of 4,000 comparisons reached a refusal; 3,250 answered.

That second number matters as much as the first. A suite in which nothing ever
refuses reports perfect refusal correctness having measured none of it, so the
proportion is asserted rather than hoped for — as is its mirror, since one in
which everything refused would be equally empty.

## How ground truth is computed

`reference.rs` is a second implementation of survivorship, written for
auditability rather than performance: no index, no persistence, no locking, no
resolution. It shadows `rm_survivor::merge` with an identical signature, so the
differential test is one comparison.

It is written **from the doc comments on `Strategy`**, never from that crate's
implementation. An oracle derived from the code it judges is not an oracle.

It reuses `rm_survivor`'s data types while implementing the logic
independently, so a bug in what `Interval` *means* would be shared. The
metamorphic properties are the cover for that: they are derived from what
bi-temporality means rather than from either implementation, so a shared
misunderstanding that also satisfies transaction-time monotonicity is a much
narrower target.

The applicability rows have their own oracle, in `applicability.rs`, and it is
independent in a stricter sense than the survivorship one: it never imports
`rm_host::scope` at all, not even the `"*"` constant. Importing it would make
the oracle track a change to the rule silently rather than reporting a
disagreement. A test reads this module's own source to assert the import never
appears — checked by adding one, which turns the suite red.

Its second row earns its place the same way `transaction-time monotonicity`
does. `depth monotonicity` says a deeper position never sees less, which
follows from what ancestor-or-self *means* rather than from either
implementation, so it survives the oracle and the engine sharing one author's
misunderstanding.

## What it found

Four corrections were needed to reach the green table. All four were in the
reference model rather than the engine, which is the expected direction — the
engine has ~590 tests behind it. Two of them are findings about the store.

**`Strategy::ValidInterval`'s doc comment was stale — since corrected.** It
said the strategy refuses when "two different values share an observation
timestamp". It did not: it refuses only when `valid.from` *and* `observed_at`
*and* the values all collide. The narrower behaviour was the correct one — the
sentence was written when a `Candidate` carried no valid time and the timeline
was cut at observation, at which point two values sharing an observation really
did have no order between them. Adding valid time gave them one. The code moved
and the prose did not.

The comment now states the rule the code follows. Worth keeping on this list
anyway: the finding was that a written premise and a behaviour had drifted
apart with nothing checking, and closing one instance does not retire the
class.

**`--valid-at` is inert under the shipped default.** `Engine::about` applies
`held_at(valid_t)` to the *outcome*, and under `Strategy::MostRecent` the
outcome is a `Survivor`, which has no time dimension — so the same value comes
back for every `valid_t`. That is coherent on its own. But `rmem.toml`'s
template ships `[policy] default = "most_recent"` with only `employer` set to
`valid_interval`, and `rmem about` advertises `--valid-at` as "asks what was
true then". On every attribute but one, the flag is accepted, does nothing, and
nothing says so.

Each piece of that is individually defensible; the combination is the defect,
and it is invisible from any single file.

**The applicability rows found nothing.** They were added after the rule had
shipped and was already governing a live store, which is the wrong order and is
why a green result was the likely one. Worth recording rather than quietly
enjoying: a green row proves the measurement exists, not that it was hard to
pass. What makes these three worth keeping is that each was checked against a
deliberate mutation — a string-prefix rule, and a correction backdated to its
decision's start — and each went red.

Both are pinned as named tests
(`sharing_an_observation_instant_is_not_enough_to_refuse`,
`valid_time_is_inert_under_the_default_strategy`) so they fail loudly if either
changes. Both still pass, and should: they pin behaviour, and neither behaviour
moved. Correcting the first finding changed a sentence; the decision reads
stopped going through the second.

## What this deliberately does not do

- **No LLM judge.** `benches/locomo`'s constraint holds and is why the
  reference model exists: *"the first number this project needs is one nobody
  has to trust."*
- **No entity-resolution scoring.** Entities are pinned with `remember_as`
  rather than resolved. Generated names would measure the generator's name
  distribution and call it a resolver score.
- **No retrieval scoring.** That is a second axis and a quality measure, not a
  correctness one. It stays in `benches/locomo`.
- **No fix for what it found.** The first finding above is reported, not
  closed. The second is what the decision-layer coverage row was measuring, and
  it is closed: `rmem decisions` and `rmem decision` take both clocks. The
  sidestep is deliberate and narrow — those reads build their own timeline from
  the versions of a decision's `choice` rather than going through survivorship,
  so `--valid-at` works on them whatever `[policy]` says. **`rmem about
  --valid-at` is still inert under `most_recent`.**

## What a green table does not prove

It proves the engine agrees with a small model on generated inputs, and that
two properties of bi-temporality hold across them.

It does not prove the store is useful, that titles are findable, or that anyone
will record a decision. Generated histories are not realistic ones — acceptable
here in a way it would not be for a retrieval benchmark, because correctness
properties are universally quantified and an unrealistic input is still a valid
one. The risk is spending effort on regions of the input space nobody reaches,
not measuring the wrong thing.
