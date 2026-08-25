# Where the machinery starts paying

Measure the conditions under which this store answers correctly and a flat
latest-wins store does not — and the conditions under which they tie.

## The claim that has never been measured

`benches/locomo` measured retrieval, found *"a twenty-line control beats the
pipeline on it, and none of the distinctive machinery serves it"*, and the
project answered:

> Raw turns cannot answer `about(entity, attribute, valid_t, tx_t)`. They cannot
> say a fact was corrected, or that two names are one person, or what was
> believed last Tuesday about last May. None of that is retrieval and none of it
> is in this number.

That is an argument, not a measurement. It has stood unexamined since, and it is
the project's entire defence of its distinctive machinery.

## What this is not

**Not a task built to show the machinery wins.** That task is trivial to build
and worth nothing — make every query retrospective and the flat store loses by
construction. This project's own standard rules it out: *"the first number this
project needs is one nobody has to trust."*

Whether bi-temporality *can* beat latest-wins is arithmetic, not a finding. A
flat store has overwritten the answer to "what did we believe in March", so it
loses every such question by construction. Reporting that would be reporting a
definition.

**What is unknown is where the crossover sits**: how much backdating, and how
many retrospective questions, a workload needs before the machinery earns its
cost. That number tells a reader whether their workload needs this store, it
cannot be rigged — the flat control wins the entire low end by design — and it
makes a negative result publishable. *"You need 30% backdating before this pays,
and nobody has 30%"* is a finding worth having.

## Two failure modes, and the second is the sharp one

A flat latest-wins store fails in two distinct ways:

**Retrospective queries.** It overwrote the answer, so it cannot answer at all.
Expected, and the boring half.

**Out-of-order arrival.** Learn in September that a job changed in July, then
learn in October a fact that was true in June, and latest-by-arrival now reports
the June fact as current. **The flat store is wrong about the present**, on its
own home turf, with no retrospective question asked.

The second is why the surface has two axes rather than one.

## The control

```rust
pub struct Flat {
    latest: HashMap<(StableId, String), String>,
}
```

One slot per key, overwritten in arrival order. **It takes no time parameter at
all** — that is the design, not a handicap. Asked what held in March it returns
what it holds now, because that is all it has.

Deliberately the naive thing, in the spirit of the control that beat the
pipeline in `benches/locomo`. A cleverer control — latest by *valid* time — was
considered and rejected: it is not the thing anyone actually builds, so beating
it would prove less about the real alternative.

**The flat store receives full credit whenever it happens to be right.** At low
backdating it answers many retrospective questions correctly, because the value
had not changed. No artificial penalty. That is what gives the surface a shape
rather than a cliff, and it is the difference between a measurement and a demo.

## Three outcomes, not two

Each store scores **right / wrong / declined**.

A refusal is neither a right answer nor a wrong one: it is the store saying
nothing orders these candidates. Counting it as an error punishes the most
distinctive behaviour the project built; counting it as a success rigs the
result. It gets its own column and the reader does the small remaining work.

`Flat` can never decline. It has no way to.

## The write path, and why this is free

Assertions go in through `Engine::remember_as` with the entity pinned and a
fixed vector, exactly as `rm-conform`'s harness does it — *"embeddings are
irrelevant to survivorship, so every observation carries the same fixed
vector"*. **No embedder, no completion model, no key, no socket.**

Reads are `Engine::about(entity, attribute, valid_t, tx_t)`, which is the
signature the claim under test names. `remember` proper is not used: it reaches
an extraction model, which would make this cost money and stop it running in CI.

## Ground truth

The generator knows what held when. Truth for `(entity, attribute, valid_t)` is
the assertion with the greatest `valid.from ≤ valid_t`, or **`Ambiguous`** when
two collide — which is exactly where refusal lives.

No labelling, no API calls, no money. That is the constraint `benches/locomo`
could not meet, and its own README records what that cost: four measured
findings shipped switched off because re-measuring was expensive enough that
nobody re-measured.

## The surface

| | |
|---|---|
| rows | backdate rate — the share of assertions whose valid time precedes their arrival |
| columns | retrospective share — the share of queries asking about a past instant |
| cells | both stores' accuracy |

**Accuracy is `right / (right + wrong + declined)`.** Declined is reported
beside it rather than removed from the denominator: a declined question is one
the caller did not get an answer to, and hiding that would flatter the store on
exactly the axis it is most distinctive.

Both axes include **0**, so the calibration cell exists and so the two
crossovers are readable straight off the grid: the `0%` retrospective column is
present-tense accuracy, the `100%` column is retrospective accuracy.

The **crossover** is reported explicitly: the backdate rate at which `Flat`
falls below 0.95, read separately off those two columns.

## The calibration cell

At **(0% backdating, 0% retrospective)** both stores must read **1.000**.

If this store misses, it is broken. **If `Flat` misses, the benchmark is
rigged** — a fair workload with no backdating and no retrospective queries is
precisely what a latest-wins store is for, and any result where it fails there
is measuring an unfair generator rather than a real difference.

A test asserts it, so the harness fails rather than quietly reporting a
flattering surface.

Three companions beside it:

1. Backdating genuinely produces out-of-order arrival — otherwise the x-axis
   moves nothing.
2. `Flat` answers *some* retrospective queries correctly — otherwise the query
   set is rigged against it.
3. This store actually declines sometimes — otherwise the third column is
   decoration.

## Where it lives

A new crate, `rm-contrast`, in the workspace.

**Not a row in `rm-conform`.** That crate's ethos is *"every row is a bug if it
is not 1.000"*. These numbers are supposed to vary, and putting them in that
table would corrupt the meaning of both.

It gets its own generator rather than importing `rm-conform`'s, for the reason
that crate gives for its own fixtures: a generator two measurements can
reconfigure is coupling neither can see.

## Runtime

A coarse grid — backdate rates `{0, 20, 40, 60}%` × query mixes `{0, 50, 100}%`
× 30 seeds — runs as an ordinary test on every push, so the claim cannot rot.
`--full` sweeps `{0, 10, 20, ..., 80}%` × `{0, 25, 50, 75, 100}%` × 200 seeds
for the README figure.

Two code paths, deliberately, because it is the only arrangement where the
number in the README and the number CI checks are both real. Budget: under 5s
for the coarse grid in debug tests.

## Out of scope

- **Entity resolution.** Out for `rm-conform`'s stated reason: generated names
  would measure the generator's name distribution and call it a resolver score.
  Mixing it in would test two things at once and let either explain the result.
- **Cost.** This store is more expensive per write and per read: an assertion
  appended to a version log and survivorship run over that log on every read,
  against a hash-map insert and a lookup. Stated in the README, not measured. A
  half-built cost model would be worse than one honest sentence, and the
  difference here is asymptotic rather than a constant factor.
- **Retrieval.** `benches/locomo`'s axis, already measured, already reported.

## The result this may produce

It may report that the machinery does not pay at any backdating rate a real
workload sees. That is a publishable finding and the README will say so plainly.

The project has been here before and got it right: `benches/locomo` reported a
number that went against the pipeline and recorded it rather than burying it.
This spec commits to the same, in advance, so the commitment predates the
result.
