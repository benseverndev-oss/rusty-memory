# rm-contrast

Measures where this store's bi-temporal answering beats a flat latest-wins
store, and where the two tie.

```sh
cargo test -p rm-contrast                        # the coarse grid, as CI runs it
cargo run --release -p rm-contrast -- --report --full   # the surface below
```

Free and deterministic: writes go through `Engine::remember_as` with a fixed
vector, so there is no embedder, no completion model, no key and no socket. The
full sweep is 1.2 seconds. That is the constraint `benches/locomo` could not
meet, and its own README records the cost — four measured findings shipped
switched off because re-measuring was expensive enough that nobody re-measured.

## The claim under test

`benches/locomo` measured retrieval, found *"a twenty-line control beats the
pipeline on it, and none of the distinctive machinery serves it"*, and this
project answered:

> Raw turns cannot answer `about(entity, attribute, valid_t, tx_t)`. They cannot
> say a fact was corrected, or that two names are one person, or what was
> believed last Tuesday about last May. None of that is retrieval and none of it
> is in this number.

That was an argument, not a measurement. This is the measurement.

## Read this before the table

**The store's column is 1.000 by construction, and is not the finding.**

**And it is measured under `Strategy::ValidInterval`, which is not the
shipped default.** `rmem.toml`'s template defaults to `most_recent` and
opts one example attribute into `valid_interval`. That is deliberate on
both sides: `most_recent` is the right default -- it always answers, and
`about --valid-at` refuses under it rather than answering about the wrong
moment -- while measuring `most_recent` here would be close to measuring
the flat control against itself, since neither reads valid time. So this
column is what the store can do, not what it does out of the box, and a
reader should not take the two for the same thing.

Ground truth is computed the way `Strategy::ValidInterval` answers — the latest
value that had begun to hold, among those already heard. So the store agreeing
with it is close to a tautology, and a perfect column is what that design
predicts rather than an achievement. Checking the engine against an independent
reading of its own rules is `rm-conform`'s job, not this crate's.

**The finding is the control's curve**, which is measured against ground truth
that owes nothing to the control's design. And the finding that cuts *against*
this store is at the bottom of this page.

## The surface

Backdate rate down, retrospective query share across. Each cell is **flat /
store** accuracy, summed over 200 seeds.

| backdate | 0% retrospective | 25% retrospective | 50% retrospective | 75% retrospective | 100% retrospective |
|---|---|---|---|---|---|
| 0% | 1.000 / 1.000 | 0.798 / 1.000 | 0.597 / 1.000 | 0.395 / 1.000 | 0.196 / 1.000 |
| 10% | 0.916 / 1.000 | 0.736 / 1.000 | 0.551 / 1.000 | 0.362 / 1.000 | 0.186 / 1.000 |
| 20% | 0.873 / 1.000 | 0.703 / 1.000 | 0.535 / 1.000 | 0.363 / 1.000 | 0.199 / 1.000 |
| 30% | 0.812 / 1.000 | 0.663 / 1.000 | 0.514 / 1.000 | 0.359 / 1.000 | 0.207 / 1.000 |
| 40% | 0.726 / 1.000 | 0.592 / 1.000 | 0.465 / 1.000 | 0.340 / 1.000 | 0.208 / 1.000 |
| 50% | 0.705 / 1.000 | 0.578 / 1.000 | 0.454 / 1.000 | 0.329 / 1.000 | 0.205 / 1.000 |
| 60% | 0.650 / 1.000 | 0.545 / 1.000 | 0.430 / 1.000 | 0.311 / 1.000 | 0.198 / 1.000 |
| 70% | 0.588 / 1.000 | 0.492 / 1.000 | 0.391 / 1.000 | 0.288 / 1.000 | 0.198 / 1.000 |
| 80% | 0.594 / 1.000 | 0.499 / 1.000 | 0.398 / 1.000 | 0.292 / 1.000 | 0.203 / 1.000 |

## The crossovers

- **0% retrospective** — the control first drops below 0.95 at **10%
  backdating**.
- **25% and above** — the control is already below 0.95 at **0% backdating**.

Two things follow, and the first is the sharper one.

**Out-of-order arrival costs the control the present, not just the past.** At
80% backdating and *no retrospective questions at all*, it answers 0.594. Every
one of those questions was "what is true now", which is the only question a
latest-wins store claims to answer. Learning in September that something changed
in July, then learning in October something true in June, leaves it reporting
June as current.

**Any retrospective share at all puts it under the floor.** At 25% the control
is at 0.798 with no backdating whatever. That is close to arithmetic — a quarter
of questions ask about a past it overwrote — and it is the boring half of the
result, reported for completeness rather than as a discovery.

## The calibration cell

At **0% backdating and 0% retrospective queries, both stores score 1.000.**

That cell is the guard, and it is a test rather than a note: if the *control*
missed it, the harness would fail and the report would print **RIGGED** instead
of a surface. A benchmark whose control cannot win the workload it was built for
is measuring an unfair generator.

Three companions sit beside it: backdating must actually cost the control
something, the control must get *some* retrospective questions right — it does,
whenever the value had not changed — and unanswerable questions must actually
occur.

## What cut against this store, and what happened to it

At a 25% tie rate, of 8,000 questions asked, 1,647 had no right answer. **Of
the remainder the store refused 4,067 it could have answered.** The control
refused none, because it has no way to.

`Strategy::ValidInterval` could not build a timeline when two segments collided,
so it refused **the whole read** — including for an instant where nothing was
ambiguous. The refusal was history-wide rather than instant-local, and on a
history with one in four writes colliding that was most of the store’s
usefulness gone.

It was found by the calibration cell failing on its first run, recorded as a
decision rather than fixed, and then fixed in #50 once the argument for leaving
it turned out to be the wrong shape: it rested on the collision never having
fired on real data, which is a frequency claim about a universally quantified
property. **That count is now 0.**

The assertion that used to pin the defect — `store.declined > 0`, with a comment
saying that if it stopped happening the measurement had gone quiet rather than
clean — is now `assert_eq!(store.declined, 0)`, and pins the fix. Zero rather
than a threshold, deliberately: the instants the store contests and the instants
`workload.rs` calls ambiguous are meant to be the same set, so a residue would
be the two rules disagreeing rather than a number to relax.

This is still measured separately from the grid, and the grid is still tie-free,
because ambiguity is a different phenomenon from the two temporal axes and
mixing them would confound the surface.

An unanswerable question is still excluded from both stores’ accuracy rather
than counted for or against either. Marking it either way is a thumb on the
scale: against, and refusal is punished; for, and the result is rigged. The
store scores no points for detecting its own ambiguity.

## The control

Twenty lines, quoted in full so nobody has to take on trust that it was not
sabotaged:

```rust
pub struct Flat {
    latest: HashMap<(StableId, String), Option<String>>,
}

impl Flat {
    pub fn remember(&mut self, entity: StableId, attribute: &str, value: Option<&str>) {
        self.latest
            .insert((entity, attribute.to_string()), value.map(str::to_string));
    }

    pub fn about(&self, entity: StableId, attribute: &str) -> Option<Option<String>> {
        self.latest.get(&(entity, attribute.to_string())).cloned()
    }
}
```

It takes no time parameter, which is the design rather than a handicap. A
cleverer control — latest by *valid* time — was considered and turned down: it
is not what anyone actually builds, so beating it would prove less about the
real alternative.

**It gets full credit whenever it happens to be right.** At low backdating it
answers many retrospective questions correctly because the value had not
changed, which is why the columns slope rather than cliff.

## Cost, measured

This store appends to a version log and runs survivorship over that log on every
read. The control does a hash-map insert and a lookup. That difference is
asymptotic rather than a constant factor, and it is now measured:
`benches/read-cost` sweeps history depth for the control and for both
strategies, and `src/cost.rs` holds the model it checks against.

On one laptop, at the depth that matters:

| depth | flat | most_recent | valid_interval |
|---|---|---|---|
| 1 | 142 ns | 320 ns | 462 ns |
| 1000 | 142 ns | 8,268 ns | 181,667 ns |

**Fixed cost dominates a read until roughly depth 42.** Below that, most of
what a read pays is an entity lookup, a `Vec` allocation and a returned
`Believed`, none of which depends on history at all. The marginal cost is
1.9 ns per predicted unit under `most_recent` and 10.8 under `valid_interval`.

The number that matters for a reader choosing between the two is where the real
store sits on that curve. **A live store of 219 decisions holds 1,086 attribute
slots, every one of them at depth 1** — nothing has been revised — and at depth
1 there is no history to scan and no timeline to sort. The whole difference
between the two strategies there is about 140 nanoseconds.

Two caveats on that anchor, because it is doing a lot of work.

The store is two days old and was seeded once, so depth 1 says where it sits
and not that revisions are rare.

**And depth may be a fact about when a store is written to rather than how
often things are re-decided.** A peer session running its own store hit a
real supersession -- a decision made, shipped, found wanting, and remade
within hours -- and recorded *one* record holding the final state, because
it wrote its decisions at the end of the day once everything had settled.
The intermediate belief never reached the store. Recorded when decided, that
slot would sit at depth 2.

This store shows the same signature: of 219 decisions, **one is `rejected`,
and its `status` slot is at depth 1.** A decision that ended up rejected was
presumably considered first, and no intermediate state was ever written.

That is a different claim from "nothing gets revised", and a more awkward
one, because age does not cure it: it predicts depth stays at 1 in a store
used for months, so long as the writer keeps recording after the fact. Any
depth figure read off a retrospectively-written store is measuring a habit
as much as a history.

An earlier version of this section carried a second caveat suggesting `rescope`
might be overwriting rather than appending, on the grounds that a `rescope` pass
ran across all 219 records and left every `scope` slot at depth 1. That was the
wrong inference, and the session that ran the pass supplied the third case the
framing did not have.

`commit_rescope` branches on whether a scope is already held: `Some(_)` is a
correction and dates from today, `None` is a backfill and dates from the
decision's own start. **All 219 took the backfill branch**, because the
pre-run backup has 219 entities and zero with a `scope` attribute — the store
already existed, fully populated and scope-less, and the scopes arrived a day
later in a separate pass. A backfill writes the first version of an attribute,
and a first version is depth 1 by definition.

So the depth figure is **not evidence about `rescope` in either direction**.
This store contains only first writes, so the correction branch has never run
here — which makes depth 1 a fact about this store's history rather than about
the command. `rescope` does append: `commit_rescope` calls `write_field`, which
calls `remember_as` with `Supersession::Corrects`, and there is no overwrite
path in it. The correction branch is un-lived rather than untested —
`rm-conform`'s *rescope keeps its history* row rescopes an already-scoped
decision across 60 seeds and reports 1.000.

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

The measurement deliberately does not decide the shipped default. That is a
separate argument with a correctness half, and letting a benchmark author settle
it here would be the thumb on the scale this crate is built to avoid.

## What this does not say

- **Nothing about retrieval.** That is `benches/locomo`'s axis, already measured
  and already reported against this project.
- **Nothing about entity resolution.** Out for `rm-conform`'s stated reason:
  generated names would measure the generator's name distribution and call it a
  resolver score. Entities are pinned.
- **Nothing about realistic workloads.** Generated histories are not real ones.
  The crossover is a property of this generator's shape, and the honest use of
  it is to ask what *your* backdating rate is, not to read 10% as a law.
- **Nothing about whether the store is correct.** That is `rm-conform`, and its
  answer is on its own page.
