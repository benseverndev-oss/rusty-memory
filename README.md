# rusty-memory

A memory that knows what it doesn't know.

```
spouse    Alex
employer  no value — asserted to have none
pets      nothing known — this was never discussed
```

Three answers, not two — and the third is *stated*, not inferred.

Run the same two conversations past mem0: one where the speaker says they have
no partner, one where partners never come up. Ask both whether this person has
a partner. It keeps the difference — the first returns the stated negation at
0.64, the second returns unrelated memories at 0.19. What it does not do is say
**which**. The caller is handed content and a score, and has to decide for
themselves what 0.19 means.

That decision is the problem. Measured on LoCoMo, keeping 90% of answerable
questions means refusing only 36.6% of unanswerable ones, and a cutoff tuned on
one corpus marked a perfect answer as a miss on another. Scores do not travel.

So this store answers instead of scoring: `Absent` where somebody said there is
none, `Unknown` where nobody has said anything.

Measured on a corpus where all three answers are labelled by hand: 8/8/8, no
fabrications. See [`docs/absence-benchmark.md`](docs/absence-benchmark.md),
including why a benchmark built around a distinction only one system makes is
reported alongside recall on a corpus others use.

The same refusal runs underneath: conflicting facts are kept rather than
settled at write time, and two entities that score too close to call are filed
as a question rather than merged. Entity resolution and survivorship — solved
problems in master data management — applied to agent memory.

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
rmem init --local               # same, without asking: no key, no socket
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
code rather than a second implementation of them. Three take both time axes, so
an agent can ask what was true in May and, separately, what was known last
Tuesday: `about`, and the two decision reads `decisions` and `decision`. Their
`as_of` and `valid_at` accept a `YYYY-MM-DD` string or a millisecond instant.

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

### A fact you already know

`rmem note` records a fact you already know. One embedding, no completion --
the same bargain `decide` makes, and the reason it exists is that the only
other way in charges a completion model per fact, which is why this store held
265 decisions, zero facts and an empty review queue.

```sh
rmem note "Jon Severn" role "leads circ"
rmem note "Jon Severn" team "circulation" --field email=jon@example.com
rmem note "Jon Severn" reports --absent      # asserted to have none
rmem note "Jon Severn" role "ran print ops" --valid-from 2019-01-01
```

`--absent` is the whole point of the three-way answer reaching the write side.
Leaving an attribute unrecorded reads as `unknown` -- nobody has said. `--absent`
reads as `absent` -- someone asserted there is none. "Has no direct reports" and
"nobody has been asked" are different answers, and this is the only way to record
the first.

It resolves by name. If the store cannot tell whether that is someone it already
knows, the fact is recorded and the identity question is queued: `rmem review`
lists it. A wrong merge is silent and permanent; an open question is neither.

It does not extract. `remember` reads prose and finds facts in it; `note`
receives one someone decided to record. Both are useful and they have different
failure modes.

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

`--valid-at` needs an attribute whose policy keeps a timeline, and **refuses
when it does not**. Survivorship runs first, and most strategies collapse a
history to one winner — a winner has no timeline, so there is nothing for a
valid time to index into. Only an attribute under `valid_interval` can be
asked, which is `employer` in the template and whatever else you configure.

The refusal names the attribute, the strategy in force, and the line that would
change it:

```
"choice" is resolved by MostRecent, which picks one winner rather than keeping
a timeline, so there is no moment to ask about -- every date would answer the
same. Set `choice = "valid_interval"` under [policy.attribute] in rmem.toml to
keep one, or drop --valid-at to read what stands.
```

It used to be accepted and ignored. The flag went in, survivorship collapsed
the history, `held_at` was handed a time it had no use for, and the same answer
came back for every date — on every attribute but `employer`. Nothing said so,
and a test asserted the wrong behaviour was right. Refusing rather than warning
is the same choice this store makes everywhere else: a warning on stderr is a
wrong answer with a note attached.

**The decision reads are the exception.** `rmem decisions` and `rmem decision`
take the same two flags, and `--valid-at` works on them whatever `[policy]`
says, because they do not go through survivorship at all: a decision's timeline
is the versions of its own `choice`, so "what stood in March" is a cut over that
list rather than a question for a strategy.

```sh
rmem decision "Pin the compiler" --as-of 2026-03-01   # what the log said then
rmem decisions --as-of 2026-03-01                     # the whole log, then
```

A decision recorded after the date asked about is not missing, and does not read
as one. It says so and names both days — the day it arrived and the day it holds
from — because either clock can be the one that excluded it, and "no decision by
that title" would send you looking for a spelling mistake instead.

A decision that stood then and does not now reads as *stood as of*, not *still
stands*. The walk to whatever replaced it is made at the same clock, so a
supersession recorded in August does not retire anything in March.

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

### How far a decision reaches

`decide` requires a `--scope`, and it is the one argument with no default.

A scope is not a label of where a decision was made. It is a statement of where
it *applies*. "Never run scale benchmarks on this laptop" gets written while
working on one project and is true of every project on the machine; tagged with
where it was written, it would disappear the moment you started something else.
So the question is not "what was I working on" but "where would this still be
true".

There is one rule:

> A decision applies where its scope is an ancestor-or-self of the asker's
> position.

A session at `work/goldenmatch/fs` sees decisions scoped `work/goldenmatch/fs`,
`work/goldenmatch`, `work` and `*`. It does not see `work/goldenmatch/er`, and
it does not see `personal`. Segments are compared one at a time, so `prod` never
matches `production`, and the store never interprets the names — depth and
naming are yours.

```sh
rmem decide "Never benchmark on the laptop" "run heavy compute in CI" --scope '*'
rmem decide "Route scorers by class" "dispatch on the class, not the mass"   --scope work/goldenmatch/fs

RMEM_SCOPE=work/goldenmatch/fs rmem decisions   # both of the above
RMEM_SCOPE=personal rmem decisions              # only the first
rmem decisions --all                            # everything, reach ignored
```

`RMEM_SCOPE` says where a session stands and is **read-side only**. It is never
a write default, because reach varies per decision and only the writer knows
it — which is also why `decide` refuses rather than guessing.

Asking for a title that exists but does not reach you is not the same as asking
for one that does not exist, and does not read like it: you are told where it
does apply. A decision recorded before scopes existed carries none and reaches
everywhere, so nothing disappeared when this arrived.

`rmem recall` takes the same two flags, and for the same reason: a session that
lists 78 of 219 decisions and then searches all 219 has two views of one store.
The filter runs inside the index scan rather than over a fetched page, so `-k 5`
still means five results that apply rather than five candidates of which some
survive.

`about` deliberately does not take them. It reads an entity you named by id,
which is a deliberate act rather than a search — scope decides what you are
*shown*, not what you are allowed to *name*.

Reach is about relevance, not permission. `--all` shows everything; none of this
is a boundary.

### Correcting a reach without re-deciding

```sh
rmem rescope "Pin the compiler" --scope '*'
```

`decide` takes the title and the choice positionally, so attaching a scope
through it writes a second `choice`. Nothing about the decision changed, but
`revisions` counts choice versions, so the entry then reads *revised 2 times*
and the log has a revision in it that never happened. Over a backfill of a few
hundred records that is the whole log falsified.

`rescope` writes the scope and nothing else. It refuses a title it cannot find
rather than creating one, because a decision holding a reach and no choice is
not a decision — and during a backfill an unresolved title is overwhelmingly a
typo, which is when a silent create is least visible and most expensive.

It reports what the decision reached before, and the three cases are different
things to have done: it had none, it reached somewhere else, or it already
reached exactly this and nothing was written.

**The scope's valid time follows the decision, not the clock.** Attaching a
reach to a decision that never had one says *this is how far it always reached*,
so it is dated from the decision's own earliest choice — transaction time stays
now, because the store genuinely only just learned it. Changing a reach that was
already recorded is the other thing: the reach changed today, so that one is
dated from today. Backfilling from now would have the store claim every
decision's reach began the day the backfill ran.

That example is not invented. `docs/seed-decision-log.sh` records this project's
own log — the options tried
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

A relative `[store] path` is resolved against the directory holding the config
file, not against wherever the command was run. That is what lets one
`RMEM_CONFIG` be shared: a path resolved against the caller would name a
different store for every caller, and two stores are not an error, so nothing
would report it.

It matters more than it looks, because a store that is not there is not
refused — it starts empty. So the wrong path does not fail, it answers every
question with "nothing known", which is indistinguishable from a store you
have not written to yet. This once pointed two stores at one file and the tell
was implausibly clean data rather than an error.

`rmem init` writes an absolute path, so the rule only matters for configs
written by hand or before this was true.
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
    "RMEM_SCOPE": "work/goldenmatch",
    "RMEM_TOOLS": "decide,decisions,decision"
  }
}
```

`RMEM_CONFIG` is what makes it one store rather than one per project: without
it each session reads `./rmem.toml` from its own directory, and eventually one
of them points somewhere else -- a divergence nothing reports, because two
stores are not an error.

`RMEM_SCOPE` is what makes one shared store readable by many projects. Without
it every session sees every decision, which is the state that made a flat log of
219 unusable — and it says where a session *stands*, never how far its writes
reach, which each `decide` states for itself.

`RMEM_TOOLS` is what it costs. The tool table is sent on every turn of every
session that has this configured, used or not:

| exposed | tools | tokens per turn |
|---|---|---|
| everything | 9 | ~2,560 |
| `decide,decisions,decision,recall` | 4 | ~1,420 |
| `decide,decisions,decision` | 3 | ~1,130 |
| `decisions,decision` | 2 | ~610 |

These figures are measured, and they have moved twice. The two clocks
(`as_of`, `valid_at`) added about 154 tokens to every row, because `decisions`
and `decision` appear in all of them. Scope added about 162 more to the rows
carrying `decide`, and about 93 to the row that does not — it gains the two read
parameters but not `decide`'s own.

`note` added the ninth tool. Cutting per-property prose that restated the schema then took the table from ~2,600 tokens to ~2,450: the top-level description is only about a third of a tool's bytes, and the rest is the prose inside `inputSchema`. What came out restated a type, a default, or something the tool description had already said; `default` and `format` keys now carry what two of those sentences used to.

The ninth row is derived rather than counted directly -- the serialised table is
10,368 characters against 8,203 at eight tools, and the four rows above imply
3.91 to 3.98 characters per token. Said plainly because it is a weaker
measurement than the rows it sits with.

Scoping `recall` added ~48 more, to the two rows that expose it — the other two
are byte-identical, which is the check that the figure is the flag rather than
something else drifting.

All of it is written down rather than absorbed quietly, and the first drafts of
the clock and scope descriptions both came in over budget and were cut back:
209 tokens for the clocks, 236 for scope.

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

Each request arrives on its own connection, so the handshake and the calls
after it land in different servers. `Mcp-Session-Id` is what carries the
handshake across that gap: the server mints one when `initialize` settles
something, returns it in the response header, and the client echoes it on
everything after. Two things ride on it — which agent made a write, and which
protocol revision that client agreed to. Without it a shared log recorded every
write as `mcp` whoever made it, and a client that handshaked at a revision
older than `structuredContent` was sent the field anyway.

A `DELETE` ends a session. An id this server did not mint gets a **404**, so a
client holding a stale one finds out rather than being quietly served as a
stranger. A request carrying *no* session id is still answered, unattributed —
the same choice this server already makes for a client that never handshakes at
all.

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

### Setting one up

```sh
rmem init --local
```

That writes a config with `embedder = "local"` and asks nothing of any
model. **`rmem init` on its own cannot produce this file**, which is the
defect the flag exists to fix: `init` probes the model for its embedding
dimension, so with no key it exits 1 having written nothing at all. The
documented keyless path had no reachable way to create the config it
recommends, and a session following these steps literally landed exactly
there.

The `[provider]` block still carries `base_url`, `api_key_env`,
`completion_model` and `embedding_model`, and the local embedder dials none
of them. They stay required deliberately: making them optional would weaken
validation on the http path -- the one where a wrong `api_key_env` costs
money -- to tidy four inert lines here.

Left in place they are also worth using. **Point `api_key_env` at a variable
nothing sets**, and an accidental fall back to `http` fails loudly instead
of quietly spending someone's key:

```toml
[provider]
embedder = "local"
api_key_env = "RMEM_OPENAI_API_KEY"   # deliberately unset
```

The dimension `--local` writes is the template's own, 1536. That is not a
number chosen for subword hashing: it is the configuration the recall figure
below was measured under, and writing a different one would leave that
number describing a config `rmem init` does not produce.

Switching is not free — vectors from the two are not comparable — but it is
reversible: `rmem reindex` rebuilds the index under whichever is configured.

## Crates

**`0.1` means the surface will not break without a version bump. `0.0` means it
moves.** Nothing is published to crates.io — sibling dependencies are declared
by path with no version, which cargo refuses to publish — so these are promises
to a reader of this repository, not to a package manager. Publishing is a
separate decision with a release cadence and a deprecation policy behind it.

| Crate | Version | Role |
|---|---|---|
| `rm-core` | 0.1 | Provenance and the bi-temporal model |
| `rm-survivor` | 0.1 | Survivorship strategies |
| `rm-store` | 0.1 | Bi-temporal record store with attribute history |
| `rm-graph` | 0.1 | Entity graph, k-hop retrieval |
| `rm-resolve` | 0.1 | Probabilistic entity resolution, with a review band |
| `rm-index` | 0.1 | Exact vector search: deletion, filtering, persistence |
| `rm-embed` | 0.1 | Subword hashing, for vectors without a service |
| `rm-providers` | 0.0 | `Completer`/`Embedder` over an OpenAI-compatible API |
| `rm-extract` | 0.0 | Turn → mentions/edges, and whether arrival implies departure |
| `rm-engine` | 0.0 | `remember()` / `recall()` / `forget()` |
| `rm-host` | 0.0 | Config, store file, and the operations over them |
| `rm-cli` | 0.0 | `rmem`, the command line |
| `rm-mcp` | 0.0 | `rmem-mcp`, the MCP server |
| `rm-conform` | 0.0 | The conformance suite. Internal; nothing depends on it |

The split is measured rather than felt. Counting source files touched across
the last thirty commits — a run that added two features, a command, a bug fix
and a conformance axis:

| | | | |
|---|---|---|---|
| `rm-embed` | 1 | `rm-extract` | 12 |
| `rm-graph` | 1 | `rm-conform` | 17 |
| `rm-core` | 2 | `rm-engine` | 28 |
| `rm-survivor` | 4 | `rm-host` | 32 |
| `rm-store`, `rm-resolve`, `rm-index` | 5 | `rm-mcp` | 50 |
| `rm-providers` | 5 | `rm-cli` | 54 |

Everything is at or below 5, or at or above 12. The gap is the line.
`rm-survivor`'s only change in that window was a doc comment, and it is the
crate `rm-conform` differentially verifies across 500 generated histories,
which is the difference between calling a surface stable and having checked.

`rm-providers` is the one exception to the measurement: its churn is 5, but its
*behaviour* depends on a third-party API contract this project does not
control, and promising stability for that is a different promise from promising
it for a pure function. It stays 0.0 deliberately.

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
