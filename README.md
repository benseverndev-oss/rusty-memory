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
| `rm-cli` | in progress | `rmem`, the command line |
| `rm-mcp` | planned | MCP server |

No *library* crate touches the network, and every library crate's third-party
dependencies come from `serde` and `serde_json` alone. The two things that need
a remote service — completion and embedding — are ports (`rm_extract::Completer`,
`rm_engine::Embedder`) the host implements, so the whole library builds, tests
and audits offline.

Exactly two crates at the edge have more: `rm-providers` adds `ureq` for HTTP,
and `rm-cli` adds `toml` for its config. Both are binaries-adjacent by design,
and every test in the workspace still runs offline.

The target is a single static binary and an embeddable library: no Python
runtime, no CMake, no compose file.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

## Licence

MIT. See [LICENSE](LICENSE).
