# Two clocks on the decision reads

Give `rmem decisions` and `rmem decision "<title>"` — and the MCP tools of the
same names — the two time axes the rest of the store already has.

## Why this and not something else

The shared store at `D:/memory` holds **219 entities. Every one of them is a
decision.** Four attributes exist in it: `because`, `choice`, `status`,
`context`. `identities` is zero, so entity resolution has never run against
real data, and there is not one fact from `remember` in there.

Three measurements agree about what that means:

- `benches/locomo`: *"a twenty-line control beats the pipeline on it, and none
  of the distinctive machinery serves it."*
- `rm-conform`, merged as #36: **decision-layer time coverage 0.000.**
- The store itself: 219 of 219 entities are decisions.

The README leads with entity resolution and survivorship over conflicting
facts. The only real deployment uses neither. The one surface used in anger is
the one with no temporal API at all.

That is not an argument that the thesis is wrong. A decision log *is* a
contradiction-over-time problem: supersession chains are survivorship by
another name, and `--at` already moves valid time while leaving transaction
time alone. The thesis is unexercised because the ergonomic surface never got
the two clocks, not because the surface is the wrong one.

## What is already there

The capability exists. `rmem about 30 choice --as-of 2026-03-01` works today.
Two specific things are missing at the decision layer:

**The graph reads throw the clocks away.** `chain()` calls
`engine.edges_from`/`edges_into(at, Timestamp::MAX, Timestamp::MAX)` at every
hop (`rm-host/src/command.rs:1027-1028`), and `decisions` does the same at
`:820`. Both axes are already parameters. They are pinned to `MAX`.

**The attribute reads bypass everything.** Both functions define a local
`latest()` closure over `engine.store_history(id, attr)`, a raw newest-version
read that touches neither survivorship nor either clock. That is a second read
path beside `about`, and it is the part that is not simply threading.

## Decisions taken

### Both axes, with valid time built natively

`decision` already assembles its own choice timeline from `store_history` as
`(valid.from, value)` pairs. A valid-time cut over that timeline is "the last
entry at or before `t`" and needs no survivorship strategy at all.

The alternative — routing through `about_under(&Policy::new(ValidInterval))` —
was rejected. It would make the decision layer's answer depend on policy
configuration, and would inherit the defect #36 recorded: `--valid-at` is inert
under `Strategy::MostRecent`, which is what the template ships for every
attribute except `employer`. Building the cut natively sidesteps that rather
than reproducing it.

### One `At` value rather than two parameters

```rust
/// The two clocks a decision read is answered under.
pub struct At { pub valid: Timestamp, pub tx: Timestamp }

impl At {
    /// Everything the store holds.
    pub fn latest() -> Self { At { valid: Timestamp::MAX, tx: Timestamp::MAX } }
}
```

Two `Timestamp` parameters threaded through three layers — `decisions` →
`chain` → `edges_into` — is a swap that compiles and returns a plausible wrong
answer. `Engine::about` and `edges_into` take them bare, but there the reader
is one line from the signature.

No `Default` impl, deliberately. `Engine::edges_from`'s own doc comment argues
the case: *"Both axes are required and neither is defaulted... an edge read
without a `tx_t` is a claim about now that quietly stops being reproducible."*
`At::latest()` makes every call site name what it is asking.

`At::latest()` is `MAX`/`MAX` rather than `now`, because that is what
`decisions` does today. Using `now` would silently drop any decision recorded
with a future `--at`, which is a behaviour change nobody asked for.

### A decision that did not exist yet is its own answer

`Outcome::Decision(Option<Box<DecisionDetail>>)` becomes:

```rust
pub enum Found {
    Decision(Box<DecisionDetail>),
    /// The title resolves; the store knew nothing of it by `tx`.
    NotYetRecorded { title: String, first_recorded: Timestamp },
    /// No decision by that title.
    Unknown,
}
```

Collapsing `NotYetRecorded` into `Unknown` loses information the store holds.
The title resolved, so the decision exists; answering "no such decision" reads
as a typo and sends the reader looking for a spelling mistake. This store
already insists on the distinction everywhere else — `Believed::Absent` is
"someone said there is none" and `Believed::Unknown` is "it has never come up".

`Outcome::Decision` carries a `Found` rather than an `Option<Box<..>>`. An enum
rather than a third `Outcome` variant, so the compiler walks every render site.

This works because `find_decision` matches on the identity record's `name`,
which is not versioned — a decision recorded after `at.tx` still resolves by
title. That is what makes the distinction available at all.

**Trigger and definition, stated exactly:** the answer is `NotYetRecorded` when
`find_decision` resolves the title but `held(engine, id, "status", at)` is
`None`. `first_recorded` is the minimum `provenance.observed_at` over that
entity's `status` history, which is well defined because `commit_decide` always
writes `status` — the same fact `command.rs:773-781` already relies on.

`decisions --as-of` needs no third state: an entity the store had not heard of
is absent from the list, which says the same thing.

### `still_stands` becomes relative, and says so

`still_stands` is `superseded_by.is_empty() && status == DEFAULT_STATUS`. Both
halves become time-relative for free once `chain()` and the read helper take
`at`.

The field keeps its name — it is the same predicate, evaluated at `at`. The
*rendering* changes: under a past clock the CLI says "stood as of 2026-03-01"
rather than "still stands". A present-tense sentence under a past clock is the
precise failure this project has now found twice.

### The MCP tools get the parameters

Roughly 150–200 tokens on the ~810-token `decide,decisions,decision`
configuration, paid every turn of every session that has it wired. Accepted: an
agent reading a shared log is exactly who needs to ask what was believed at the
time a call was made, and a CLI-only feature is one the agents cannot use.

## Design

### The read helper

The two `latest()` closures collapse into one free function:

```rust
fn held(engine: &Engine, id: StableId, attr: &str, at: At) -> Option<String>
```

filtering `store_history` by `provenance.observed_at <= at.tx`, then
`valid.from <= at.valid`, then taking the last.

**This unifies an existing disagreement.** `decisions` (`:812`) reads
`.filter_map(|v| v.value.clone()).next_back()` — the last non-tombstone.
`decision` (`:971`) reads `.last().and_then(|v| v.value.clone())` — `None` when
the newest version is a tombstone. A tombstoned `choice` therefore shows the
old choice in the list and an empty one in the detail. Latent, since `decide`
only ever writes values, but it is two answers to one question and unifying
them is free here.

The unified reading is `decision`'s: a tombstone is an assertion that the
attribute has no value, and skipping it to report a superseded value would be
the store answering with something it has been told is no longer true.

### Signatures

```rust
pub fn decisions(engine: &Engine, only: Option<&str>, at: At) -> Result<Outcome, HostError>
pub fn decision(engine: &Engine, title: &str, at: At) -> Result<Outcome, HostError>
fn chain(engine: &Engine, start: StableId, dir: Direction, at: At) -> Vec<(StableId, String)>
```

`chain` passes `at.valid` and `at.tx` to the edge reads it already calls, in
place of the `Timestamp::MAX` pair. `decisions` does the same at its own direct
`edges_into` call (`:820`), which is not routed through `chain`.

### Surfaces

- **CLI** — `--as-of` / `--valid-at` on both commands, through the existing
  `day()` helper in `args.rs:255` and `rm_host::time::parse_day_end`, so a date
  names a whole day read as its end. Same as `about`, documented as such.
- **MCP** — optional `as_of` / `valid_at` string parameters on the `decisions`
  and `decision` tools, parsed the same way and refused the same way.
- **README** — the decision-log section.

## What proves it

`rm_conform::decisions::time_coverage()` stops being a hardcoded `0.0` and
becomes a measurement: build a chain with known record times, probe a
`(valid, tx)` grid, and count the fraction answered *correctly* against a
hand-computed expectation rather than the fraction that merely returns
something.

`report.rs` already calls it, so the README table recomputes itself and the
number cannot be typed by hand.

With #36's anti-vacuity discipline, which is the reason that suite means
anything: a test asserting that some probes land before the chain existed and
some inside it, so the coverage figure is not measured over a grid where every
probe is trivially "now".

## Out of scope

**This does not fix finding #2.** `rmem about --valid-at` stays inert under
`Strategy::MostRecent`, which is what the template ships for every attribute
but `employer`. The decision layer gets its own native timeline, which
sidesteps that defect without closing it. Saying so here rather than letting a
green coverage number imply it was handled.

**No change to `decide`.** It already has `--at`, which moves valid time and
leaves transaction time alone. This is the read half of that.

**No change to survivorship, resolution, or the index.**
