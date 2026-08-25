# Scope: how far a memory reaches

Give a decision a stated reach, and give a session a stated position, so the
store can answer "what applies here" instead of "everything anyone ever
recorded".

## The distinction the design turns on

Scope is **not** a label of origin. It is a declaration of reach.

Those look alike and behave nothing alike. The live store makes the difference
concrete. *"local box: never run scale or perf benchmarks on the Windows box"*
was written while working on goldenmatch, but it applies to every project on
this machine. *"goldenmatch fs routing gap"* applies to one. Under an origin
model both are tagged `goldenmatch`, and the machine-wide rule disappears the
moment the next session is about something else — which is precisely backwards.

So the two facts are separate, and they live in separate places:

| | what it says | where it lives | when it is set |
|---|---|---|---|
| `scope` | how far this memory reaches | an attribute on the decision | stated on every `decide` |
| `RMEM_SCOPE` | where the asker is | the environment, per MCP entry | once, per session |

**Reach varies per decision, so a session-level default cannot supply it.** The
agent knows whether the thing it just learned is machine-wide or project-local;
the environment does not. `RMEM_SCOPE` is read-side only and is never a write
default.

## What is in the store today

219 entities, every one a decision, four attributes: `because`, `choice`,
`status`, `context`. No scope anywhere.

The convention that grew instead is in the titles. **155 of 219 (71%) begin
with the literal word `goldenmatch`**, and 79% carry a `prefix:` of some kind
across 145 distinct prefixes, 135 of them used exactly once. The prefix is
doing two unrelated jobs — sometimes a real scope (`ci` ×8, `measurement` ×6,
`process` ×6), sometimes a topic phrase belonging to one decision.

That is the workaround this replaces, and it costs three things: `decisions` is
unusable as an index at 219 lines with no filter but `--status`; the project
name is load-bearing in the title, which is also the primary key, so renaming
an effort breaks every lookup; and the word sits inside 155 embedded strings.

The last of those is a **hypothesis, not a finding.** A controlled 65→219
comparison found no retrieval degradation. A scope attribute would make it
testable, which is worth more than assuming it.

**None of this is a design input.** The existing data is a migration problem.
The model below is derived from what a scope has to mean, not from what these
particular titles happen to look like.

## The rule

There is one, and everything follows from it:

> A memory applies where its scope is an **ancestor-or-self** of the asker's
> position.

A session at `work/goldenmatch/fs` sees memories scoped to
`work/goldenmatch/fs`, `work/goldenmatch`, `work`, and `*`. It does not see
`work/goldenmatch/er`, and it does not see `personal`.

**Segment-wise, never string-prefix.** `prod` must not match `production`.
Positions and scopes split on `/` and are compared segment by segment.

The store never interprets the segments. `work`, `personal/finance`,
`clients/acme/migration` are opaque strings that happen to contain a separator.
Depth is unbounded and unenforced; naming is the user's business.

### Spellings

- `*` is the universal reach: an ancestor of every position. `/` is a
  separator and `*` is the one *value* the store ascribes meaning to; the
  segments themselves stay opaque. It is legal only as the entire value —
  `work/*` is refused, because it would read as a wildcard the rule does not
  have.
- On reads, `--all` (a flag, not a scope) suspends the rule and shows
  everything. It is deliberately *not* spelled `--scope all`, so that the
  browse switch cannot be confused with a scope value. On `decision <title>` it
  means the answer is never `NotHere`.
- `--scope <s>` on a read asks from a different position than `RMEM_SCOPE`.

### Validation

A scope is refused unless it is `*`, or a non-empty `/`-separated sequence of
non-empty segments with no leading or trailing separator and no whitespace-only
segment. This is what stops `work` and `work/` both existing and meaning the
same thing. Comparison is exact and case-sensitive; the store normalises
nothing, because normalising is interpreting.

## Storage

`scope` becomes a fifth bi-temporal attribute beside `status`, `choice`,
`because` and `context`.

Versioned rather than fixed metadata on the identity record, because an effort
gets renamed or absorbed, and recording that as a change with both clocks is
what this store is for. Putting it on the identity record would make moving a
decision a rewrite — the one-way door this project refuses everywhere else. It
also means `--as-of`, which shipped in the previous change, answers *what
applied here then*.

Cost, stated plainly: `decide` currently performs three embeddings (`status`,
`choice`, `because` — one each). A fourth is a third more work per write. The
reindex text is derivable in the same shape as the others, so
`rmem reindex` keeps working on a decision-only store.

## The write path refuses

`decide` errors without a scope, naming what was missing — the way it already
refuses a bare `superseded` status with a pointer to `--supersedes`.

On the MCP side, `scope` joins `"required"` in the `decide` tool schema, so a
model cannot omit it. The same rule applies to `rmem decide`: one rule beats
two, and a person typing the command is as able to state reach as an agent is.

This is the one place friction is deliberate. A default would answer, silently
and usually wrongly, the only question the writer is uniquely positioned to
answer.

## The read path

### Applicability filters the index. It never filters a chain.

`decisions` returns what applies at the asker's position. `decision "<title>"`
walks supersession **without** applying the rule.

This is not an inconsistency. The chain exists so that a reader holding a
retired decision is carried to the one that replaced it. If a replacement were
hidden for being out of reach, the reader would be shown a decision marked
"do not act on this" and nothing else — strictly worse than not filtering at
all. Reach decides what you are *shown when browsing*; it never decides what
you are *warned about*.

The same holds for the `replaced by entity N` line `decisions` prints beside a
retired decision: that successor is named whether or not it applies here. A
line that says a decision is retired and withholds what retired it is the state
this rule exists to prevent.

### An out-of-reach title is its own answer

`Found` grows a fourth variant:

```rust
NotHere { title: String, scope: String, asked_from: String },
```

Asking for an exact title that exists but does not apply here answers with the
scope it does apply to. The reasoning is the same as `NotYetRecorded` in the
previous change: the title resolved, so "no decision by that title" would read
as a spelling mistake. You named it exactly; you get told where it lives.

### An unset position turns the feature off

With no `RMEM_SCOPE` and no `--scope`, the asker has no position, so
applicability cannot be computed and nothing is filtered. Reads behave exactly
as they do today.

That is the migration story for every caller that has not opted in, and it is
also the honest answer: a store cannot tell you what applies here if nobody has
said where here is.

## The 219

They carry no `scope` attribute at all. That is **not** the same as a new write
omitting one, and this store already has the vocabulary for the difference:
`Believed::Unknown` is "it has never come up", `Believed::Absent` is "someone
said there is none".

A decision with no scope recorded reads as applying everywhere. Nothing
disappears the day this ships. New writes cannot be unscoped, so the unscoped
population is closed and shrinks as decisions are re-decided under a scope.

No backfill command. Guessing reach from a title prefix would be the origin
model wearing the reach model's clothes: it would tag the machine-wide rules
`goldenmatch` because that is where they were written, which is the exact error
this design exists to avoid.

## Surfaces

- **CLI** — `rmem decide --scope <s>` (required); `rmem decisions [--scope <s>]
  [--all]`; `rmem decision <title> [--scope <s>] [--all]`.
- **MCP** — `scope` required on `decide`; optional `scope` and `all` on
  `decisions` and `decision`. `RMEM_SCOPE` read once at startup, beside
  `RMEM_CONFIG` and `RMEM_TOOLS`.
- **README** — the decision-log section, the `RMEM_TOOLS` cost table, and the
  shared-store configuration example.

## Out of scope

- **`recall` and `about` stay unscoped.** `recall` is the more interesting case
  and it is a different axis — a similarity search that also filters by
  applicability is a retrieval-quality question, and folding it in would put
  scoring back on the table and double this spec.
- **No backfill.** Argued above.
- **No cross-scope access control.** Reach is about relevance, not permission.
  Everything remains readable with `--all`; nothing here is a boundary and the
  design must not be mistaken for one.
