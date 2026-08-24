# rusty-memory

Agent memory that resolves contradictions deterministically.

Memory systems for agents dedupe by embedding similarity and settle conflicts by
asking a model to re-summarise. `rusty-memory` treats conflicting facts as the
core problem rather than an afterthought, and applies entity resolution and
survivorship — solved problems in master data management — to agent memory.

Status: early. See [`docs/architecture-sketch.md`](docs/architecture-sketch.md)
for the design and what is planned.

## Why it is different

When the store learns the user moved from Acme to Globex, most systems have to
pick a winner. This one does not have to:

```rust
use rm_engine::{Believed, Engine, Policy, Strategy};

let mut engine = Engine::new(index, ruleset, Policy::new(Strategy::ValidInterval));

// March. The first thing we hear about someone is a new entity.
engine.remember(told("Acme", MARCH))?;

// July, months later. Resolution recognises the same person.
engine.remember(told("Globex", JULY))?;

assert_eq!(engine.about(person, "employer", MAY,    NOW)?, Believed::Value("Acme".into()));
assert_eq!(engine.about(person, "employer", AUGUST, NOW)?, Believed::Value("Globex".into()));
```

Neither fact was discarded and nothing was rewritten. Both are stored with
disjoint validity, and the store answers by time — contradiction resolution is a
query, not a lossy write. Ask the same history under `Strategy::MostRecent`
instead and it names one winner at every instant, from the same stored versions:

```rust
engine.about_under(&Policy::new(Strategy::MostRecent), person, "employer", MAY, NOW)?
// => Believed::Value("Globex")
```

(The full version of the first example runs as a test:
`crates/rm-engine/tests/readme.rs`. `Believed` has three states, not two —
`Value`, `Absent`, and `Unknown` — because "they have no employer" and "we have
never discussed it" are different answers.)

## Trying it

```sh
cargo install --path crates/rm-cli
export OPENAI_API_KEY=...
rmem init                       # asks the model its embedding size, writes rmem.toml
rmem remember "I just started at Globex"
rmem recall "where do I work"
```

`rmem init` writes a config with the resolver's thresholds and probabilities
spelled out rather than hidden, because they are decisions and you should be
able to see them.

## Giving it to an agent

`rmem-mcp` is an MCP server over the same store and the same `rmem.toml`. It
speaks stdio, and it speaks both eras of the protocol: `2026-07-28`, which
replaced the `initialize` handshake with a per-request `_meta` envelope and a
mandatory `server/discover`, and the handshake revisions back to `2024-11-05`.

```sh
cargo install --path crates/rm-mcp
rmem-mcp                        # reads ./rmem.toml, serves stdin
```

Eight tools — `remember`, `recall`, `about`, `reviews`, `resolve_review`,
`decide`, `decisions`, `decision` — which are `rmem`'s own commands over shared
code rather than a second implementation of them. `about` is the one that differs: it takes
both time axes, so an agent can ask what was true in May and, separately, what
was known last Tuesday.

`decide`, `decisions` and `decision` are the decision log. A decision is an
entity with four attributes — `status`, `choice`, `because`, `context` — written
under a title you would search for. Unlike `remember`, `decide` never reaches a
completion model: the shape is known, so the fields go in directly under names
that stay findable, and the title is matched exactly rather than resolved.
That matters because extraction invents a fresh attribute name most of the
time, and a record nobody can name twice is not a record.

Superseding writes an edge, not a flag. `decide X --supersedes Y` draws
`X -supersedes-> Y`, so the link is a fact about the pair and both ends can read
it: `decisions` names the successor beside every retired decision, and
`decision "<title>"` walks the chain to whatever stands now.

That walk is the point of the whole thing. An agent that searches its memory
lands on whatever matches the words, and a superseded decision matches just as
well as the one that replaced it — the `choice` line of a retired decision looks
exactly as authoritative as a live one. So the answer to "what did we decide
about X" has to carry its own correction:

```
Store snapshots as one JSON file [superseded]
  write the whole store on every change
  because simplest thing that survives a restart

DO NOT ACT ON THE CHOICE ABOVE -- IT WAS REPLACED.
What stands now is entity 1, "Store snapshots in SQLite". Read that one.
```

Re-deciding under the same title is the other way a decision changes, and it is
a different thing: the title keeps its entity, the new choice is what stands,
and `decision` shows every choice it has held with the day each was decided.

`--at YYYY-MM-DD` records a decision that was made earlier — reconstructing a
log from history, or writing up a choice made last week. It moves the **valid**
time and leaves the transaction time alone, which is the difference the store's
two axes exist for: the decision held from March, and the store learned it in
August. Moving both would say it knew in March, and every answer it gave in
between would become retroactively wrong — you could no longer tell a stale
answer from a bug.

```sh
rmem decide "Pin the compiler" "rust-toolchain.toml names the version" \
  --at 2026-02-28 \
  --because "CI took whatever stable had become that week"

rmem decision "Pin the compiler"   # its history reads 2026-02-28, not today
rmem decisions --status rejected   # what was tried and turned down
```

### The embedder is a choice you can now change your mind about

The store keeps a value, an interval and a provenance. The text that was
*embedded* is not among them — it goes to the embedder and is dropped — so the
vectors are the only surviving representation of it. That made choosing an
embedding model a **one-way door**: a different model, or the same model at a
different dimension, strands every vector already written, and there is no way
back short of re-ingesting from source.

`rmem reindex` is the way back, where the text can be worked out again. A
decision's is `"decision {title}: {attribute} is {value}"`, and title, attribute
and value are all in the store, so a decision log can be re-embedded by anything
at any time — a different provider, a local model, a different dimension.

It refuses on a store holding anything else. A fact that came from a
conversation was embedded on a sentence the extractor wrote, and that sentence
is not kept; re-embedding around it would leave two models' output in one index,
where the distances between them are not wrong but meaningless. That is the
failure this project refuses everywhere else it appears, so it is refused here
too, naming what it found.

Reading back along either axis is an `about` with a date on it:

```sh
rmem about 30 choice --as-of 2026-03-01      # what the store knew then
rmem about 30 choice --valid-at 2026-03-01   # what was true then
```

A date names a whole day and both flags read it as the *end* of that day, so a
query naming today sees what was recorded this morning.

`--as-of` works on anything: transaction time is filtered before survivorship
runs, so asking what the store knew before it knew anything answers `nothing
known`.

`--valid-at` needs an attribute whose policy keeps a timeline. Survivorship runs
first, and most strategies collapse a history to one winner — a winner has no
timeline, so there is nothing for a valid time to index into. Only an attribute
under `valid_interval` can be asked, which is `employer` in the template and
whatever else you configure.

That timeline used to be cut at **observation** times rather than valid ones, so
the case this store's design opens with — told in September that a job changed
in July, asked what held in August — answered with the old employer.
`rm_survivor::Candidate` carried a value and a provenance and no interval at
all, so the strategy named for the valid interval could not see one. It carries
its validity now, and the timeline is cut where the values actually changed.

A decision's `status` is one of `proposed`, `accepted`, `rejected` or
`deprecated` — a closed vocabulary, because the point of a status is that a
reader can branch on it, and an open one lets `rejected`, `Rejected` and
`declined` mean one thing to a person and three to a program. `superseded` is
not settable: it claims a *specific* other decision replaced this one, and
written alone it produces the state the edge exists to prevent, so it is
refused with a pointer to `--supersedes`.

The status that earns the feature is `rejected`. An option considered and turned
down, with the reason it lost, is the entry that stops a settled question being
reopened — and it is exactly what could not be recorded before, since every
decision was accepted and the only way to write a rejection was to accept the
word "no".

```sh
rmem decide "Retrieval reranking" "a cross-encoder over the top 200" \
  --status rejected \
  --because "the k-curve is still 0.926 at k=200 -- there is nothing to rerank into"
```

That example is not invented. `docs/seed-decision-log.sh` records this project's
own log — thirty decisions from eleven merged pull requests, the options tried
and turned down with the numbers that killed them, and the three supersession
chains that actually happened. Run it against an empty store to see what the
thing reads like holding real history rather than a demo.

The answer keeps its three states across the wire. `{"believed": "absent"}` is
"someone said there is none" and `{"believed": "unknown"}` is "it has never come
up", and the text block says which in words too, because that is the half most
clients put in front of a model.

Several servers and `rmem` invocations can share one `memory.json`. They take
turns on an advisory lock held beside it, spanning each read-modify-write so
neither can save over a snapshot the other has already changed. The wait is
bounded at five seconds and then refuses rather than blocking forever.

That bound used to be the ceiling. Every model call happened inside the lock —
an extraction and a set of embeddings, seconds each across a network — so
measured on a live store the fourth concurrent writer was refused outright.
Nothing about those calls needed the store: an extraction is a function of the
turn, an embedding of its text. They now happen before the lock is taken, and
the lock covers resolution and a save. Same measurement afterwards: twelve
concurrent writers, twelve distinct decisions in the store, no lost updates.

The split is held by the signatures rather than by care. `commit_remember` and
`commit_decide` take no completer and no embedder, so nothing reachable from
inside the lock can call one.

## Not leaving it to the agent

An agent that *chooses* to remember forgets. `hooks/rmem-hook.sh` wires
`UserPromptSubmit` to both directions of the store: every prompt is answered
against what is already there before the model reads it, and every prompt is
queued for extraction whether or not the agent thought to record it. It is not
wired by default — it spends a completion per prompt, and that is not a choice
to make on someone's behalf by their cloning a repo. See
[`hooks/README.md`](hooks/README.md).

## Where a store lives

Two files. `memory.json` holds everything the store remembers — assertions,
identities, the review queue — and `memory.vec` holds the vectors, as a flat run
of little-endian `f32` rows.

They were one file, and measured on a real store the vectors were **96.9%** of
it: 1536 floats per assertion, written as JSON numbers at roughly thirteen bytes
for each four-byte float, and all of them rewritten to record one decision.
Splitting them took the part that is parsed and re-serialised on every write
from 918 KB to 70 KB on a 33-decision log — **13× smaller** — and the vectors
became 702 KB of bytes rather than 848 KB of text.

The shape is Qdrant's dense storage: same-sized rows, and a map from id to row.
The design only — Qdrant is Apache 2.0 and this is MIT, and a few hundred lines
is not worth carrying somebody else's licence for.

The vectors are written first and the snapshot second, because the snapshot's
rename is the commit. A crash between them leaves rows nothing points at, which
the next open neither reads nor minds; the other order would leave a snapshot
naming rows that are not there.

**A store written before the split still opens.** Its snapshot carries its own
vectors, there is no `.vec` beside it, and the next save writes both. Refusing
it would lose an existing store rather than move it.

**What this does not yet do** is write incrementally. Both files are still
replaced whole, so a save is O(store) rather than O(one row) — the win is that
the expensive half, encoding and parsing two million JSON floats, is gone. Row
writes in place need the engine to track which rows changed, and that is a
separate piece of work.

## Giving it to the agents you already have

On one machine, several sessions share a store with nothing running between
them. Each spawns its own `rmem-mcp`, and they take turns on the advisory lock
beside the store -- measured here at eight concurrent writers from eight
separate processes, all eight landing.

```json
"rmem": {
  "command": "rmem-mcp",
  "env": {
    "RMEM_CONFIG": "D:/memory/rmem.toml",
    "RMEM_TOOLS": "decide,decisions,decision"
  }
}
```

`RMEM_CONFIG` is what makes it one store rather than one per project: without
it each session reads `./rmem.toml` from its own directory, and eventually one
of them points somewhere else -- a divergence nothing reports, because two
stores are not an error.

`RMEM_TOOLS` is what it costs. The tool table is sent on every turn of every
session that has this configured, used or not:

| exposed | tools | tokens per turn |
|---|---|---|
| everything | 8 | ~1,700 |
| `decide,decisions,decision,recall` | 4 | ~1,060 |
| `decide,decisions,decision` | 3 | ~810 |
| `decisions,decision` | 2 | ~360 |

For comparison, a thirty-decision log is about 1,850 tokens to read in full. A
project that only ever consults decisions should not pay most of that again,
every turn, to advertise five tools it will never call.

Writes are attributed to whoever made them. A client names itself in the MCP
handshake and that goes on everything it writes, so a shared log says which
agent decided what without any of them having to remember to say. A `session`
argument, when given, is appended rather than replacing it.

## Many agents, one store

```sh
rmem-mcp                          # stdio: one client, one machine
RMEM_TOKEN=... rmem-mcp --http 0.0.0.0:8899
```

stdio serves one client on one box. The store is a file and the lock is a
`flock` on a sidecar beside it, so "many agents" has meant many processes on one
filesystem. A memory several agents *share* — where one records a decision and
another is corrected by it — needs a socket.

`--http` is MCP's Streamable HTTP transport. There is no SSE: the stream exists
for servers that send messages of their own, and this one never does, so every
response is a single JSON object. Each connection gets its own server, because
the protocol version is negotiated per client.

It is safe by default and refuses rather than warns:

- a request carrying `Origin` gets **403** — that is a browser, this server has
  no browser clients, and the specification requires the check by name because a
  page on any site can point at a loopback port
- binding anything but loopback without `RMEM_TOKEN` **refuses to start**, since
  a memory on an open port with no credential is not a deployment anyone chooses
  on purpose
- `GET` gets **405**: the stream it would open carries server-initiated
  messages, and there are none

**No TLS and no OAuth.** The specification's authorization chapter describes an
OAuth 2.1 resource server, which a bearer token is not. Anything facing a
hostile network wants a reverse proxy in front that terminates TLS and does the
real thing.

## Vectors without a service

`[provider] embedder = "local"` computes vectors here instead of asking a
remote model. `rm_embed` is subword hashing — about a hundred lines of
arithmetic, no dependency, no model file, deterministic across machines and
releases.

With it, the whole decision path is offline: `decide` reaches no completion
model by design, so `decide`, `recall`, `decisions` and `reindex` need no API
key and open no socket.

It costs recall, and the number is in `benches/locomo/README.md`. On this
project's own decision log, asked in words other than the title's own, it
places the right decision first **6 times in 12** where
`text-embedding-3-small` manages **10**. It has morphology and no semantics:
nothing lexical connects *talking* to *speaking*. Asked by exact title both are
perfect, which is why that is the wrong test to judge it by.

Switching is not free — vectors from the two are not comparable — but it is
reversible: `rmem reindex` rebuilds the index under whichever is configured.

## Crates

| Crate | Status | Role |
|---|---|---|
| `rm-core` | in progress | Provenance and the bi-temporal model |
| `rm-survivor` | in progress | Survivorship strategies |
| `rm-store` | in progress | Bi-temporal record store with attribute history |
| `rm-graph` | in progress | Entity graph, k-hop retrieval |
| `rm-resolve` | in progress | Probabilistic entity resolution, with a review band |
| `rm-index` | in progress | Exact vector search: deletion, filtering, persistence |
| `rm-extract` | in progress | Turn → mentions/edges, and whether arrival implies departure |
| `rm-engine` | in progress | `remember()` / `recall()` / `forget()` |
| `rm-providers` | in progress | `Completer`/`Embedder` over an OpenAI-compatible API |
| `rm-host` | in progress | Config, store file, and the operations over them |
| `rm-cli` | in progress | `rmem`, the command line |
| `rm-mcp` | in progress | `rmem-mcp`, the MCP server |

No *library* crate touches the network, and every library crate's third-party
dependencies come from `serde` and `serde_json` alone. The two things that need
a remote service — completion and embedding — are ports (`rm_extract::Completer`,
`rm_engine::Embedder`) the host implements, so the whole library builds, tests
and audits offline.

Exactly two crates have more, both of them about hosting rather than about
memory: `rm-providers` adds `ureq` for HTTP, and `rm-host` adds `toml` for
`rmem.toml`. Neither binary adds anything of its own — `rmem-mcp` implements
the protocol by hand rather than taking an MCP SDK and an async runtime, which
is the same trade as exact search over an ANN index and a hand-written argument
parser over `clap`. Every test in the workspace still runs offline, the server
included: it is driven through a byte slice and a stub provider, so there is no
process to spawn and no socket to open.

The target is a static binary and an embeddable library: no Python
runtime, no CMake, no compose file.

## Development

These are the three commands CI runs, spelled the way CI spells them. They
were not, once: the README asked for clippy without `--all-features` while CI
asked for it with, and a local run could be clean against a check the branch
would fail.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

The compiler is pinned in `rust-toolchain.toml` and rustup installs it on the
first cargo invocation, so there is no setup step and no version to agree on.
CI reads the same file rather than naming a channel of its own. Bumping it is
a deliberate commit — see the note in that file for why, and for what the pin
costs.

## Licence

MIT. See [LICENSE](LICENSE).
