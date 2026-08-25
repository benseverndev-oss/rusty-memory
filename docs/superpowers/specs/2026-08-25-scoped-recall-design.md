# A search that answers from where you stand

Give `recall` the applicability rule the decision reads already honour, so a
session sees one consistent world however it asks.

## Why now, and why it is not urgent

Decisions carry reach and `decisions`/`decision` honour it. `recall` does not:
it returns every match regardless of position.

**Nothing is leaking today.** `recall` is not exposed in the live
configuration — `RMEM_TOOLS=decide,decisions,decision` — and the store holds
only decisions, 219 of them, with no facts from `remember`.

What makes this worth building is that **the trigger is one word.** The
README's own `RMEM_TOOLS` table offers `decide,decisions,decision,recall` at
~1,210 tokens as a supported configuration. Taking it undoes the narrowing that
was just wired: a session at `work/goldenmatch` lists 78 of 219 decisions and
then searches all 219, and nothing says so. The inconsistency is latent, cheap
to close now, and silent when it fires.

## Filter, not boost

`rm-engine`'s read path already draws this distinction and explains it.
`Query::entity` is a filter; `Query::boost` is a boost, because

> filtering on it would discard the answer outright every time it guessed
> wrong, while a boost only costs the guess its advantage

— measured at J = 0.33 separability for turning a name into an entity.

**That argument does not transfer.** Scope is a stored string compared to a
declared position: exact, deterministic, not a guess. Filtering discards
nothing on a bad inference because there is no inference. So scope joins
`entity`, `source` and `session` as a filter, and `--all` widens, exactly as it
does on the decision reads.

## Where the filter runs

Inside the index scan, with the others. `Engine::recall` already passes
`in_scope` as a predicate to `index.search_adjusted`, and the file states why:

> Re-ranking a fetched top-`k` could only ever promote assertions that raw
> similarity had already surfaced, and the one this exists to rescue is the
> assertion about the right person sitting at rank 40.

Filtering during the scan keeps `k` meaning **"k results that apply"** rather
than "k candidates, some of which survive". Post-filtering would silently
shrink every result set and make `k` a lie.

`Query` gains one field:

```rust
/// Where the asker stands. `None` suspends the applicability rule.
pub position: Option<String>,
```

and `in_scope` gains one clause, reading the candidate entity's `scope`
attribute at the query's own `as_of` clocks — so a scoped `recall` and a scoped
`decisions` agree about the same instant. When `as_of` is `None`, the scope is
read at the latest of both axes, matching what an unqualified `decisions` does.

An entity with no `scope` recorded reaches everywhere, as in the decision reads.
`remember`'s facts carry none, so a scoped recall never hides them.

## The rule moves to `rm-core`

`applies_at`, `validate` and `UNIVERSAL` move from `rm-host` to `rm-core`.
`rm-host` keeps a `scope` module that re-exports them, so `crate::scope::validate`
in `plan_decide`/`plan_rescope` and `rm_host::scope::position` in the two
binaries keep working unchanged.

**`position` does not move.** It normalises a configured value — an empty or
whitespace `RMEM_SCOPE` is no position — which is a fact about reading
configuration rather than about the rule. `rm-core` holds what a scope *means*;
`rm-host` holds how a host learns one.

`Query` lives in `rm-engine`, which depends on `rm-core` and not on `rm-host`,
so the rule has to move down for the engine to use it. **The alternative is a
second implementation of ancestor-or-self in `rm-engine`, which is precisely the
drift this project keeps finding** — the stale `ValidInterval` sentence and the
inert `--valid-at` flag were both one premise diverging from one behaviour.

`rm-core` is `0.1`, so this must be **additive only**: new module, re-export at
the old path, no signature changes. That is what the version promised.

### The import ban needs the new path

`rm-conform`'s `applicability` module asserts it never imports the code it
judges, over a list of banned strings:

```rust
for banned in ["rm_host::scope", "scope::applies_at", "scope::UNIVERSAL"]
```

A bare `use rm_core::scope;` matches none of them. **The list must gain
`rm_core::scope`**, or the guard silently stops catching the import it exists to
catch — the differential becoming a tautology while its own test still passes.
Found by checking the ban against the move rather than assuming it survived.

## Surfaces

- **CLI** — `rmem recall "<query>" [--scope <s>] [--all]`, with `RMEM_SCOPE` as
  the default position. Same precedence as everywhere else: `--all` beats
  `--scope` beats the environment, and an empty or whitespace value is no
  position.
- **MCP** — two optional parameters on the `recall` tool, matching the decision
  reads. This adds to the per-turn cost of any configuration exposing `recall`;
  the figure is measured and recorded in the README table rather than estimated.

## It ships with a row, not just tests

*"A recall returns only what applies"* is universally quantified, so by this
project's own standard it needs a measurement rather than an example.
`rm-conform`'s `applicability` module already builds engines full of scoped
decisions; a **`recall applicability`** row comparing the returned set against
the oracle costs little on top.

With the companion that matters here: **the filter must actually exclude
something that similarity would otherwise have returned.** A recall row that
never excludes anything is measuring the generator rather than the filter, and
would report 1.000 having tested nothing — the same vacuity the existing rows
guard against.

## Out of scope

- **`about` stays unscoped.** It takes an explicit entity id, which is a
  deliberate act of naming rather than a search. Refusing something you asked
  for by id is unhelpful, and the analogous surface already handles the named
  case better: `decision "<title>"` answers `NotHere` and says where it lives
  rather than hiding it. Scope filters what you are *shown*; it should not gate
  what you *named*.
- **No retrieval-quality claim.** This measures *which* assertions come back,
  not whether they are the right ones. That is `benches/locomo`'s axis, it costs
  money, and its answer is already on the record.
- **No change to `boost`.** It stays a boost for the reason the code gives.
