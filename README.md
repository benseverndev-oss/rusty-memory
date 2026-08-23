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

Seven tools — `remember`, `recall`, `about`, `reviews`, `resolve_review`,
`decide`, `decisions` — which are `rmem`'s own commands over shared code rather
than a second implementation of them. `about` is the one that differs: it takes
both time axes, so an agent can ask what was true in May and, separately, what
was known last Tuesday.

`decide` and `decisions` are the ADR pair. A decision is an entity with four
attributes — `status`, `choice`, `because`, `context` — written under a title
you would search for, and re-deciding under the same title supersedes the old
one rather than sitting beside it, so `decisions` can say which still stand.
Unlike `remember`, `decide` never reaches a completion model: the shape is
known, so the fields go in directly under names that stay findable. That
matters because extraction invents a fresh attribute name most of the time, and
a record nobody can name twice is not a record.

The answer keeps its three states across the wire. `{"believed": "absent"}` is
"someone said there is none" and `{"believed": "unknown"}` is "it has never come
up", and the text block says which in words too, because that is the half most
clients put in front of a model.

A server and a `rmem` invocation can share one `memory.json`. They take turns
on an advisory lock held beside it, spanning each read-modify-write so neither
can save over a snapshot the other has already changed. The wait is bounded at
five seconds and then refuses rather than blocking forever, because every
holder is doing one operation and a longer wait means the other side is wedged
rather than busy.

## Not leaving it to the agent

An agent that *chooses* to remember forgets. `hooks/rmem-hook.sh` wires
`UserPromptSubmit` to both directions of the store: every prompt is answered
against what is already there before the model reads it, and every prompt is
queued for extraction whether or not the agent thought to record it. It is not
wired by default — it spends a completion per prompt, and that is not a choice
to make on someone's behalf by their cloning a repo. See
[`hooks/README.md`](hooks/README.md).

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
