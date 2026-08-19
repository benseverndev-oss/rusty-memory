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
use rm_core::{Provenance, Source};
use rm_survivor::{merge, Candidate, Strategy};

let march = Provenance::new(Source::UserAssertion, 1_710_000_000_000, "session-1");
let july = Provenance::new(Source::UserAssertion, 1_720_000_000_000, "session-9");

let outcome = merge(
    &[
        Candidate::new(Some("Acme"), &march),
        Candidate::new(Some("Globex"), &july),
    ],
    &Strategy::ValidInterval,
)
.unwrap();

assert_eq!(outcome.as_of(1_715_000_000_000), Some("Acme"));   // in May
assert_eq!(outcome.as_of(1_725_000_000_000), Some("Globex")); // in August
```

(That example runs as a test: `crates/rm-survivor/tests/readme.rs`.)

Both facts are kept, with disjoint validity ranges, and the store answers by
time. Contradiction resolution is a query, not a lossy write.

## Crates

| Crate | Status | Role |
|---|---|---|
| `rm-core` | in progress | Provenance and the bi-temporal model |
| `rm-survivor` | in progress | Survivorship strategies |
| `rm-store` | in progress | Bi-temporal record store with attribute history |
| `rm-graph` | in progress | Entity graph, k-hop retrieval |
| `rm-resolve` | in progress | Probabilistic entity resolution, with a review band |
| `rm-index` | in progress | Exact vector search: deletion, filtering, persistence |
| `rm-extract` | planned | Turn → mentions/edges. The only networked crate. |
| `rm-engine` | in progress | `remember()` / `recall()` / `forget()` |
| `rm-mcp` / `rm-cli` | planned | MCP server and `rmem` binary |

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
