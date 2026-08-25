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
below but the last is a bug if it is not 1.000.

## The table

Seeds `0..500`, 20 probes per history, 12 assertions each.

| property | result |
|---|---|
| merge agreement, 8 strategies | 1.000 |
| refusal correctness | 1.000 |
| transaction-time monotonicity | 1.000 |
| arrival-order independence | 1.000 |
| decision-layer time coverage | 0.000 |

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

## What it found

Four corrections were needed to reach the green table. All four were in the
reference model rather than the engine, which is the expected direction — the
engine has ~590 tests behind it. Two of them are findings about the store.

**`Strategy::ValidInterval`'s doc comment is stale.** It says the strategy
refuses when "two different values share an observation timestamp". It does
not: it refuses only when `valid.from` *and* `observed_at` *and* the values all
collide. The narrower behaviour is the correct one — the sentence was written
when a `Candidate` carried no valid time and the timeline was cut at
observation, at which point two values sharing an observation really did have
no order between them. Adding valid time gave them one. The code moved and the
prose did not.

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

Both are pinned as named tests
(`sharing_an_observation_instant_is_not_enough_to_refuse`,
`valid_time_is_inert_under_the_default_strategy`) so they fail loudly if either
changes.

## What this deliberately does not do

- **No LLM judge.** `benches/locomo`'s constraint holds and is why the
  reference model exists: *"the first number this project needs is one nobody
  has to trust."*
- **No entity-resolution scoring.** Entities are pinned with `remember_as`
  rather than resolved. Generated names would measure the generator's name
  distribution and call it a resolver score.
- **No retrieval scoring.** That is a second axis and a quality measure, not a
  correctness one. It stays in `benches/locomo`.
- **No fix for what it found.** Both findings above are reported, not closed.

## What a green table does not prove

It proves the engine agrees with a small model on generated inputs, and that
two properties of bi-temporality hold across them.

It does not prove the store is useful, that titles are findable, or that anyone
will record a decision. Generated histories are not realistic ones — acceptable
here in a way it would not be for a retrieval benchmark, because correctness
properties are universally quantified and an unrealistic input is still a valid
one. The risk is spending effort on regions of the input space nobody reaches,
not measuring the wrong thing.
