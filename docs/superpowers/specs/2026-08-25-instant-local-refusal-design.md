# A refusal that fits the question

Make `Strategy::ValidInterval` refuse the instants that are genuinely
ambiguous, instead of refusing every read because one pair of writes collided
somewhere in the history.

## The rule is already in this repo, as ground truth

`rm-contrast/src/workload.rs:203` computes the truth a benchmark grades
against. Its ambiguity test is instant-local and always was:

```rust
visible.sort_by_key(|x| (x.valid_from, x.observed_at));
let winner = visible[visible.len() - 1];
if visible.iter().any(|x| {
    x.valid_from == winner.valid_from
        && x.observed_at == winner.observed_at
        && x.value != winner.value
}) {
    return Truth::Ambiguous;
}
```

Find the winner **for the instant asked about**, then ask whether anything
shares both of its clocks and disagrees. A collision elsewhere in the history
does not make this question ambiguous.

So the engine is not disagreeing with an opinion about ergonomics. It is
disagreeing with the oracle its own benchmark scores it against, and
`rm-contrast`'s 4,067 refusals out of 6,353 answerable questions **are** that
disagreement, counted.

## Re-deciding, and on what grounds

#48 recorded *"Make ValidInterval's refusal instant-local"* as **rejected**.
This reverses that. The log gets the reversal recorded as a re-decision, the
way `"Retire recall@10 as the headline metric"` already is — the log's one
worked example of a title decided twice, and the reason `decision` shows both
choices with the reason each was taken.

**The reversal condition #48 named has not fired.** It asked for a bulk import
carrying day-resolution timestamps on both axes. There is still no such import,
and there are still zero collisions across 1,086 attribute slots in the live
store. Nothing about that has changed and the spec does not pretend otherwise.

**One tempting ground is discarded as false.** The argument that `about
--valid-at` now directs users toward `valid_interval` in its refusal message
does not hold: that message landed in #45 (`0a54c24`), before #48 was written.
#48 already knew. Checked rather than assumed, and dropped.

**The ground that does hold is about the form of #48's argument.** It rejected
the fix because the collision has never fired on real data. `rm-conform`'s
README states this project's position on that kind of reasoning:

> correctness properties are universally quantified and an unrealistic input is
> still a valid one

#48 argued frequency about a universally quantified property, which is the
reasoning this project disavows two crates over. That inconsistency is the
reason to re-decide, and it is an argument from the repo's own stated position
rather than from evidence #48 lacked.

## What "contested" actually covers

Sort the asserting candidates by `(valid.from, observed_at)`, as both
implementations already do. For each distinct `valid.from`, only the
**greatest `observed_at`** at that `valid.from` decides what opens there;
anything heard earlier for the same moment was superseded before the question
was asked.

That sub-group opens a span at `valid.from`, closing where the next distinct
`valid.from` opens. The span is **contested** when the sub-group holds two or
more distinct `Held` values, and settled otherwise.

### Adjacent spans holding the same value still coalesce

Both implementations already collapse a restatement into the span already
standing — *"re-hearing a fact is not a change"* — and that survives unchanged.
Two adjacent `Held` spans carrying equal values are one span. A value that
returns after being superseded still yields three spans, because the spans are
not adjacent.

**Contested spans never coalesce**, even where their value sets match. Each one
records the `observed_at` its own collision happened at, which is what the
refusal message hands back to the writer, and merging two would have to discard
one of them. They are also not equal claims: two separate collisions are two
separate things to fix.

### This is narrower than "the same refusal over a smaller region"

The current check fires on any adjacent sorted pair sharing both clocks with
different values. Consider `A@(F,1)`, `B@(F,1)`, `C@(F,2)`:

- Today: `A` and `B` are adjacent, share both clocks, differ — the read
  refuses.
- Under the oracle: at every `t >= F` the winner is `C`, which nothing ties
  with. At every `t < F` nothing is visible. **No instant is ambiguous, and the
  engine refuses the whole history anyway.**

So this is not only a refusal that is too wide. It is a refusal that fires on
histories containing no ambiguous instant at all. That case becomes a test.

### Tombstones compete

`Held::Absent` is a claim, not a silence — `about_under` already builds it with
`Candidate::absent` for exactly this reason. `Value("Acme")` colliding with
`Absent` is two distinct held values and contests the span. Silence never
competes and never contests.

## The shape

`rm_survivor::Outcome` never crosses the MCP wire — the hosts match on
`Believed`, and `render.rs` is written against that — and the store persists
`Version`s rather than outcomes, because survivorship runs on read. So this is
a compile-time break inside the workspace, with no store-format migration and
no protocol change.

```rust
/// What a span of valid time holds.
pub enum Span {
    /// One value stood over this span.
    Held(Held),
    /// Two or more values opened here sharing an `observed_at`, so nothing
    /// orders them and no one of them can be said to have held.
    Contested {
        values: Vec<Held>,
        observed_at: Timestamp,
    },
}

pub struct Fact {
    pub span: Span,
    pub valid: Interval,
}

impl Outcome {
    pub fn held_at(&self, t: Timestamp) -> Result<Option<&Held>, Refused>;
    pub fn as_of(&self, t: Timestamp) -> Result<Option<&str>, Refused>;
}
```

`Contested` carries `observed_at` because the refusal message needs it — it is
the number that tells a writer which two writes to separate.

**One list, and the invariant is structural.** A span is `Held` or `Contested`,
never both and never neither, so there is no consistency between two
collections to drift. The alternative considered was
`Outcome::Timeline { facts, contested }` with `Fact` untouched: a smaller diff,
but the non-overlap of the two lists would live in a comment. This project has
spent three pull requests on a premise drifting from a behaviour; a new
invariant that only prose enforces is the wrong trade at any diff size.

`Held::Contested` was rejected outright. `Held` means *what actually held* and
feeds `Believed`. An arm meaning "not a value at all" makes every existing
match on `Held` wrong by default.

**An `Outcome::Survivor` never refuses.** It has no time dimension — that is
what `keeps_a_timeline` reports — so `held_at` on one is `Ok` at every instant,
and the new `Result` is a shape the timeline arm needs rather than a behaviour
every strategy acquires.

`Outcome::as_of` becomes fallible along with `held_at` rather than collapsing a
contested span to `None`. `None` there already means *no coverage*, and
flattening "two values and nothing orders them" into it is the same collapse
the `Absent`/`Unknown` distinction exists to prevent. It has no callers outside
`rm-survivor`'s own tests, so the cost is nil.

## Where each refusal now lives

**The read path indexes in.** `rm-engine/src/read.rs:309` is the whole defect
in one character:

```rust
let outcome = merge(&candidates, policy.for_attribute(attribute))?;
Ok(match outcome.held_at(valid_t) { .. })
```

The `?` escapes before `held_at` is ever reached, which is why a collision
anywhere refuses a question about anywhere else. It moves to the `held_at`
call. `merge` stops returning `Err` for a `ValidInterval` collision; every
other strategy's refusals are untouched.

**The write path keeps refusing whole.** `rm-store/src/lib.rs:446` materializes
a merge result into stored versions, and `Version.value` is an
`Option<String>` — there is no representation for a contested span, and
inventing one would be a storage-format change to record something no reader
asks for. So the write path scans the timeline and refuses the entire
resolution if any span is contested, which is exactly what it does today.

One rule, two policies about what to do with a hole. The asymmetry is real and
belongs in the doc comment rather than being discovered later: a question can
be asked about one instant, and a materialized resolution cannot.

## The message becomes actionable

Today's refusal names the two values and the shared timestamp. The instant-local
one can also say that the rest of the history is fine, which turns it from a
dead end into an instruction:

```
"Acme" and "Globex" both opened at 2026-03-01 and were observed at 1750,
so neither supersedes the other and neither can be said to have held on
2026-03-01. Distinguish their observation times, or ask about an instant
outside [2026-03-01, 2026-06-01).
```

Naming the contested interval is the part that matters. A caller who gets this
back knows both that the question was refused and where the answer resumes.

## What checks it

**`rm-conform`'s differential sweep, in the order that makes it mean
something.** The reference model in `reference.rs` is written from the prose,
independently of the engine — that independence is the entire reason a green
table is evidence. So the order is fixed and is not a matter of taste:

1. Change `Strategy::ValidInterval`'s doc comment to state the instant-local
   rule.
2. Rewrite `reference::valid_interval` and `reference::held_at` from the
   changed prose.
3. Change the engine.
4. Let the sweep report.

Written the other way round the reference model is a transcription of the
engine and the sweep is a tautology. This is the same loop that caught the
stale `ValidInterval` sentence by disagreeing on 53 generated histories, and it
only worked because the two were written apart.

**`rm-contrast`'s grid can carry ties.** It is tie-free today *because* of this
refusal — `score.rs:150` maps a refusal on an ambiguous question to
`Ungradeable`, and the README says the grid excludes ties so the temporal axes
are not confounded with the refusal behaviour. With the fix, ties can enter the
grid: genuinely ambiguous questions still refuse and stay `Ungradeable`, while
the 4,067 that had answers become answers. That turns this change into a
measured number rather than a claim, and it is the number the pull request
reports.

**Grading does not move.** `Truth::Ambiguous` stays `Ungradeable`, so the store
scores no points for detecting its own ambiguity. The whole measured effect is
refusals that should not have happened, gone. A design where the store
*answered* `Contested` was considered and turned down for this reason among
others: it would have required changing what the benchmark counts at the same
time as changing what the store does, and a benchmark rewritten alongside the
thing it measures proves less.

**Anti-vacuity.** A suite where no question ever lands in a contested span
would report the fix working having tested nothing. The generator must produce
histories where some instants are contested and others are not *in the same
history*, and that must be asserted rather than hoped for — the same guard
`rm-conform` already applies to its refusal proportion and `rm-contrast` to its
calibration cell.

## Out of scope

- **No change to any other strategy.** `MostRecent` refuses on a contradictory
  tie at the latest observation, which is a statement about one instant
  already. `SourcePriority` refuses on an unlisted source, which is a statement
  about configuration and has no time dimension to localise.
- **No change to the default policy.** `MostRecent` stays the shipped default,
  so nothing about the live store's behaviour changes. This alters what happens
  for attributes explicitly configured to `valid_interval` in `rmem.toml`.
- **No new representation in storage.** The write path refuses exactly as it
  does now.
- **Nothing about the decision reads.** `decisions` and `decision` build their
  own timeline from a decision's `choice` versions rather than going through
  survivorship — deliberately, and recorded as such — so they neither gain nor
  lose anything here.
- **`Believed` gains no variant.** Settled by the same choice that keeps
  `rm-contrast`'s grading fixed.
