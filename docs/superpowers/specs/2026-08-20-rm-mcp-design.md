# rm-mcp — design

Status: approved design, pre-implementation.

`rmem-mcp`, the second executable in the workspace, and `rm-host`, the crate
that stops it being a copy of the first one.

## What this is for

The store, the resolver and the engine exist so an *agent* can remember things,
and the only thing that can currently drive them is a person at a prompt.
`rm-cli` answered the three questions a host has to answer — where a store
lives, where configuration comes from, who calls a model — and the rm-cli
design said out loud what that was for:

> An MCP server would need the same three answers *plus* a protocol, so solving
> them here first means the server is a thin layer rather than the place three
> unsolved problems land together.

This is that thin layer, and the bill for that sentence comes due in the first
section below: "the same three answers" has to mean *the same code*, not the
same code typed twice.

## The one structural change: `rm-host`

`rm-cli` is 1,600 lines of code, and 1,046 of them are not about a command
line:

| Module | Code | About a command line? |
|---|---:|---|
| `config` | 723 | No. `rmem.toml`, the ruleset, the policy, the provider |
| `command` | 200 | No. remember / recall / about / review, returning data |
| `store` | 123 | No. Reading and writing `memory.json` safely |
| `args` | 167 | Yes |
| `format` | 135 | Yes |
| `run` | 122 | Yes |

The rm-cli design already refused the two bad answers to this, in the section
that argued `rm-providers` should be its own crate rather than a module:

> The alternative — putting them in `rm-cli` — leaves the server either
> duplicating them or depending on a binary crate.

The same argument lands on `config`, `command` and `store` now that the second
consumer is real. So they move to **`rm-host`**, which both binaries depend on:
everything a host of this library has to decide, and nothing about how it is
driven. `rm-cli` keeps `args`, `format`, `run` and `main` — the three modules
that are genuinely about a terminal, plus the glue.

This is a move, not a rewrite. Every line and every test goes across unchanged
except for the error type, below. If the diff shows behaviour changing, the
diff is wrong.

### The error type splits, because one of its variants was never shared

`CliError` has five variants and four of them are host concerns: `Config`,
`MissingKey`, `Store`, `Refused`. The fifth, `Usage(String)`, carries the
usage text a bad command line produces — there is no such thing in a server,
which has a schema instead.

So `rm_host::HostError` takes the four, and `rm_cli::CliError` becomes
`Host(HostError) | Usage(String)` with a `From` impl, which is all `?` needs.
`MissingKey`'s doc comment — the one explaining why it carries nothing —
travels with it, because the reasoning is about a config file and not about a
terminal.

### `testing` stops being `cfg(test)`

`rm-cli`'s `TempDir` and `StubProvider` are 82 lines behind `#[cfg(test)]`, so
`rm-mcp`'s tests cannot see them, and a stub provider is exactly what a second
consumer needs in order to test the whole path without a socket. In `rm-host`
the module is public and unconditional.

That is a real cost — stubs compiled into a release build, however dead — and
the alternative, a `testing` feature, is worse here: Cargo unifies features
across normal and dev dependencies, so a binary that dev-depends on
`rm-host/testing` gets the stubs compiled in anyway, and CI already runs
`--all-features`. Paying it plainly beats paying it behind a flag that does not
work.

## The protocol changed, and it changed a lot

MCP's current revision is **`2026-07-28`**, and it is not the protocol most
implementations were written against. The `initialize` handshake is gone.
There is no session, no negotiated state, and no lifecycle:

- Every request declares its own protocol version in
  `params._meta["io.modelcontextprotocol/protocolVersion"]`, and the server
  accepts or rejects **each request independently**.
- `params._meta["io.modelcontextprotocol/clientCapabilities"]` is likewise
  required on every request. A request missing either is malformed: `-32602`.
- An unsupported version is `-32022`, carrying
  `data: {supported: [...], requested: "..."}` so the client can retry.
- `server/discover` is mandatory, and returns supported versions, capabilities
  and identity in one call.
- Every result carries a `resultType`, `"complete"` for an ordinary answer.

The spec calls these **modern** versions, and `2025-11-25` and earlier
**legacy**.

### Dual-era, and what it costs

A modern-only server is a two-line decision with a three-word consequence: the
spec's own compatibility matrix says *legacy client, modern server — **Fails***,
and legacy clients have no fall-forward mechanism. The revision that removed
the handshake is weeks old. A memory server that no shipping client can open is
not a memory server.

So `rmem-mcp` is **dual-era**, which the spec explicitly permits and describes
how to do. The routing rule is one `if`, and it comes from the spec rather than
from taste:

- `method == "initialize"` → legacy. Nothing modern sends it.
- otherwise, `_meta` carries a protocol version → modern.
- otherwise → legacy operation, from a client that has already handshaked.

That is per-request and stateless in both directions: we never record that a
handshake happened, because in the modern era there is nothing to record and in
the legacy era the alternative is to reject a client's first `tools/call` for
paperwork reasons.

Versions served: `2026-07-28` modern; `2025-11-25`, `2025-06-18`, `2025-03-26`
and `2024-11-05` legacy. The legacy list is not generosity — it is four
revisions whose `tools/list` and `tools/call` shapes we already emit correctly.
The one real difference across them is `structuredContent`, which arrived in
`2025-06-18`, so it is emitted only when the negotiated version is at least
that. Revisions are `YYYY-MM-DD` strings chosen so that they sort, so the
comparison is `>=` on the string and the test says so.

A legacy client asking for a version outside that list gets `2025-11-25` back —
the spec's instruction is to answer with a version we *do* support and let the
client decide — and a modern client asking for an unknown version gets `-32022`
naming all five.

## The tool surface

Five tools, and deliberately exactly the five operations `rm-cli` already
performs.

| Tool | Arguments | What it is |
|---|---|---|
| `remember` | `text`, `session?` | Extract a turn and apply it |
| `recall` | `query`, `k?` | Search for assertions near a query |
| `about` | `entity`, `attribute`, `valid_at?`, `as_of?` | What the store believes |
| `reviews` | — | The open questions |
| `resolve_review` | `id`, `same` | Answer one |

`rm_host::command` therefore moves across **unchanged**, and the whole of
`rm-mcp` is protocol: framing, negotiation, schemas, and rendering an `Outcome`
as JSON. That is the point of shipping this slice first — the two halves of the
diff can be reviewed separately, and a bug in either is not hidden by the other.

What that leaves out, named rather than forgotten: `forget` and `erase`;
`neighborhood`, which is `rm-graph`'s k-hop retrieval and the most obviously
missing tool; `store_history`, which would show an attribute's timeline
directly; and `recall`'s filters — `as_of`, `entity`, `session`, `source` — all
of which `Query` already supports and `command::recall` does not pass through.
Every one of those is engine surface `rm-cli` does not expose either, so adding
them here would mean designing a second, wider API in the same change that
introduces the protocol. They are the next slice.

`about` is the exception, and it is not new surface: `command::about` already
takes both time axes and the CLI simply passes `now` twice. Exposing them is
what makes the crate's central claim reachable by an agent, so `valid_at` and
`as_of` are optional arguments defaulting to now. Both are epoch milliseconds,
which is what `Timestamp` is; a friendlier format would mean a date parser and
a guess about time zones.

## `Believed` has three states and the wire has to keep all three

This is the thesis at its last possible step, and the step where every other
system flattens it. `Believed::Absent` — "they have no employer, and someone
said so" — is not `Believed::Unknown` — "nobody has ever mentioned an employer".
JSON's `null` cannot tell them apart and neither can an empty string.

So `structuredContent` is tagged:

```json
{"believed": "value", "value": "Globex"}
{"believed": "absent"}
{"believed": "unknown"}
```

and the text block, which is what most clients actually put in front of a model,
says which one it is in words rather than printing a bare value that reads as
absence when it is ignorance.

## A refusal is a result, not a transport failure

MCP splits errors exactly the way `rm-cli`'s `exit_code` already does, for the
same reason, and the mapping is one-to-one:

- **Protocol errors** (`-32600`, `-32601`, `-32602`, `-32603`, `-32022`) are
  for a request that is wrong as a request: an unknown tool, a missing required
  argument, an unparseable line, an unsupported version.
- **Tool execution errors** are `isError: true` on an ordinary result, carrying
  text, and the spec says clients **SHOULD** hand them to the model so it can
  self-correct.

Every `HostError` is the second kind. Survivorship declining under the
configured strategy, a review id that does not exist, the model returning
something that is not an extraction — these are all answers a capable agent can
act on, and the words this workspace writes into its refusals are the part
worth carrying. They go in the text block verbatim.

`Believed::Unknown` is not an error at all, and gets `isError: false`, for
precisely the reason `rmem about` exits 0: the store was asked and has no
opinion, which is an answer.

## stdout is the transport

The stdio binding is blunt about it: *the server **MUST NOT** write anything to
its stdout that is not a valid MCP message*, messages are newline-delimited and
**MUST NOT** contain embedded newlines, and stderr is free for anything.

`rmem` prints to stdout. `rmem-mcp` must never do so, which makes it a rule the
compiler cannot enforce and a review has to: nothing below `main` may print. So
the serve loop is written over `BufRead` and `Write` rather than over the real
streams, `main` supplies `stdin`/`stdout` and a clock, and every protocol test
drives it with a `&[u8]` and a `Vec<u8>`. A test asserting that no response line
contains a newline is cheap and pins the framing rule directly.

Serialisation is `serde_json::to_string`, which never emits a bare newline
inside a JSON string — it escapes to `\n` — so the framing rule holds by
construction rather than by sanitising afterwards.

## Shutdown, and the two limits worth naming

The server exits when stdin reaches EOF. The spec calls that the primary
graceful-shutdown signal and the only portable one.

**One writer.** The engine is held in memory for the life of the process and
written back after any tool that changed it, through the same
write-temp-then-rename `rm_host::store::save` the CLI uses. Two servers, or a
server and a `rmem` invocation, against one `memory.json` will lose writes: the
second to save overwrites what the first learned, and neither notices. `rm-cli`
already records that there is no lock file; this makes the same limit easier to
hit, so it is stated here and in the README rather than discovered.

Reloading the store on every call would narrow it and was considered. It does
not close it — two processes can still interleave read-modify-write — and it
pays a full snapshot parse and index rebuild per tool call for a race it only
makes less likely. A lock file is the fix, and it is a separate change.

**No cancellation.** `notifications/cancelled` is accepted and ignored: the loop
handles one request at a time, so by the time a cancellation is read the request
it names has already been answered. That is a correct implementation of "stop
work as soon as practical" for a serial server, and it stops being correct the
moment anything here becomes concurrent.

## Dependencies

None that are new. `rm-mcp` takes `rm-host`, `rm-engine`, `rm-providers`,
`serde` and `serde_json` — every one of them already in the workspace, and
`serde_json` already in eight crates. There is no JSON-RPC framework, no MCP
SDK, and no async runtime, because a stdio server handling one line at a time
needs none of the three and the SDK would be by a wide margin the largest
dependency in the tree.

Hand-writing the protocol is the same trade this workspace has now made four
times — exact search over an ANN index, ports over an HTTP client, a hand-rolled
argument parser over `clap`, and now this — and the cost is stated the same way:
the tool schemas are JSON written by hand and can drift from the code that reads
the arguments, so each one gets a test that calls the tool with exactly what its
schema advertises.
