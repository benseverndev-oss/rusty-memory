# Who said it, and who needs telling

Two small changes with one cause: the store cannot say who wrote a record, and
`*` has quietly acquired a second meaning it was never designed to carry.

Neither needs a schema change. That is the finding, and it is why this spec is
short.

## Authorship: the slot exists and one caller throws it away

`rm-core/src/lib.rs:102` already has the field, documented for exactly this:

```rust
/// The session, turn, or document this came from. Opaque to this crate;
/// the host decides its shape.
pub source_ref: String,
```

It is plumbed end to end — `rm-host/src/command.rs` threads a `session: &str`
into every `Provenance::new`. The **MCP host uses it properly**: it writes the
client name from the handshake, and `rm-mcp/tests/session.rs` asserts on that,
under a comment saying `source_ref` is "provenance's name for it [...] the host
is what decides it holds a client name."

The CLI passes the literal string `"cli"` — `rm-cli/src/run.rs:221`, `:253`,
`:262`.

Every record in the live store went through that path. So the store looks
authorless not because provenance lacks a field but because **one caller writes
a constant into it**, and that is the caller everything has been written
through.

### What it should carry

`<agent>@<host>/<session>` — `RM@bsev-002/b149f85e`.

Three parts because three different questions went unanswered today: *which
agent* found it, *which machine* it happened on, and *which run* — five sessions
were called `Print` and four were called `Circ`, so a name alone collides.

### Where it comes from

A new environment variable, `RMEM_SESSION`, read by the CLI. This is not a new
mechanism: `RMEM_CONFIG`, `RMEM_SCOPE` and `RMEM_TOOLS` are already how a host
tells `rmem` about its context, and this is the same shape.

**When it is unset, the fallback is `cli@<hostname>`,** not `"cli"`. That is
honest about what is known — the machine is knowable, the session is not — and
it is strictly more than the store has today.

**Populating it per session has no clean answer right now, and the spec says so
rather than implying one.** A static `env` block in `.claude.json` cannot carry
a per-session id, and an environment variable set by a `SessionStart` hook does
not propagate to later tool calls. The MCP path is better off — it has the
handshake client name already — and should gain the host. The CLI path gets the
variable and the fallback, and whoever can populate it, will. A field that is
usually `cli@bsev-002` and sometimes `RM@bsev-002/b149f85e` is still a field
that can answer the question when it matters, which is not true of a constant.

### One thing this does not do

**It does not backfill.** The 256 existing records stay as they are. Their
authorship is genuinely unknown — it lives in session memory that is about to
evaporate, and inventing it from adjacency is precisely the error that made this
worth fixing: a session attributed ten records to me today on the grounds that I
was writing at the time, and was wrong about all ten.

## Selectivity: cost is not the constraint

64 titles is 1,092 tokens. 300 would be roughly 5,100 — still **once per
session**, against the ~1,130 **per turn** the MCP tools cost. There is no size
at which this breaks on cost.

What breaks is attention. A wall of 300 titles gets skimmed, which is the same
failure as injecting nothing, only more expensive. So the fix is not a cleverer
filter over a growing pile. It is a smaller pile.

### `*` means two things and should mean one

Since the injection hook shipped, `*` means both:

1. *true of every project on this machine* — its designed meaning, reach
2. *shown to every session at startup* — acquired by accident

Those are different claims. A lesson can be universally true and not worth
interrupting anyone with. **`*` becomes the second, and carries the first by
implication:** it is the injected set, by definition, and the bar is "true
everywhere *and* worth telling everyone unprompted."

### Pruning uses mechanisms that already exist

The triage pass over the current 64 has exactly two dispositions, and neither is
a new feature:

- **Keep** — it clears the bar. Stays `*`, stays `accepted`, keeps being
  injected.
- **Demote** — either `rescope` it to the position it is actually about, if it
  was over-scoped, or `decide --status deprecated` if it is true but not worth
  announcing. A deprecated record still exists, is still found by `decision` and
  `recall`, and stops being shouted.

**The hook then injects `*` records whose status is `accepted`.** It already
parses the status out of `rmem decisions` output and discards it; it starts
filtering on it instead. That is the entire code change, and it is one line.

### Why not a salience axis

It was considered. Reach and salience genuinely are different things, and a
second field would say so cleanly.

Turned down because it is a schema addition to a `0.1` crate, needs a default
for 256 existing records, and — decisively — **gives every writer a second dial
to get wrong.** Scope alone was mis-set twelve times today, in three distinct
ways: a glob that ate it, a topic prefix in a position slot, and a repair that
inferred it from configuration. A store that cannot reliably fill one field does
not need two.

## Testing

**Authorship** — `rm-cli` gains a test that a decision written with
`RMEM_SESSION` set carries it in `source_ref`, and one that with it unset the
value is `cli@<something>` rather than the bare constant. Both read the stored
`Provenance` rather than the CLI's own output, per entity 255: `rmem decision`
prints no provenance at all, so its readback cannot see this field.

**Selectivity** — the hook is PowerShell and outside `cargo test`, so it is
exercised by piping a synthetic `rmem decisions` listing containing one
`[accepted]` and one `[deprecated]` line and asserting only the first survives.

## Out of scope

- **No backfill of existing authorship**, per above.
- **No new field, and no salience axis.**
- **No change to what `decide` requires.** It still refuses without an explicit
  `--scope`; `RMEM_SESSION` is read, never demanded, and its absence degrades to
  a worse answer rather than a refusal. Provenance is not the kind of thing to
  block a write on.
- **No cross-machine identity.** `<host>` is whatever the machine calls itself.
  Making that globally meaningful is a different problem and nobody has it yet.
