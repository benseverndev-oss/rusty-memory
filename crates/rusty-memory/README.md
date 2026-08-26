# rusty-memory

A memory that knows what it doesn't know.

```text
spouse    Alex                                      Believed::Value
employer  no value — asserted to have none          Believed::Absent
pets      nothing known — this was never discussed  Believed::Unknown
```

Three answers, not two. Memory systems for agents dedupe by embedding
similarity and settle conflicts by asking a model to re-summarise; both
operations are lossy and neither reports that it happened. Ask one whether
someone has an employer and it will tell you. Ask this one and it can say it
does not know — a different answer from "they have none", and the difference is
the point. Treating the second as the first is how a memory comes to state as
fact that somebody is unemployed because their job never came up.

```toml
[dependencies]
rusty-memory = "0.1"
```

**Depend on this crate.** The `rusty-memory-*` crates underneath it are how it
is built, are published only because Cargo requires a published crate's
dependencies to be published too, and change without notice.

The same refusal runs underneath. Contradicting facts are both kept and
resolved when asked rather than settled at write time, so one stored history
reads as a single winner under `Strategy::MostRecent` and as a timeline under
`Strategy::ValidInterval`, with nothing rewritten in between. Two entities that
score too close to call are filed as an open question rather than merged — a
wrong merge is silent and permanent, an open question is neither.

Full documentation, the measured benchmarks, and the CLI and MCP server live in
the [repository](https://github.com/benseverndev-oss/rusty-memory).
