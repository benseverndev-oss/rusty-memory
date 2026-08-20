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

Five tools — `remember`, `recall`, `about`, `reviews`, `resolve_review` — which
are `rmem`'s five commands over shared code rather than a second implementation
of them. `about` is the one that differs: it takes both time axes, so an agent
can ask what was true in May and, separately, what was known last Tuesday.

The answer keeps its three states across the wire. `{"believed": "absent"}` is
"someone said there is none" and `{"believed": "unknown"}` is "it has never come
up", and the text block says which in words too, because that is the half most
clients put in front of a model.

One process at a time per store. There is no lock file, so a server and a
`rmem` invocation against one `memory.json` will lose each other's writes.

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

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

## Licence

MIT. See [LICENSE](LICENSE).
