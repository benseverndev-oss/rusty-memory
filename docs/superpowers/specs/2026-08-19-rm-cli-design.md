# rm-cli — design

Status: approved design, pre-implementation.

`rmem`, the first executable in the workspace, and `rm-providers`, the first
crate allowed to open a socket.

## What this is for

Eight crates work and none of them can be run. There is no `main()` anywhere,
and every test supplies its own `Completer`, `Embedder`, `Ruleset` and `Policy`
by hand. That is not an oversight — the ports exist so the *binary* implements
them — but it means three questions have been deferred this whole time and all
three come due at once:

- Where does a store live, and how is it written safely?
- Where does an engine's configuration come from?
- Who actually calls a model?

A CLI answers all three. An MCP server would need the same three answers *plus*
a protocol, so solving them here first means the server is a thin layer rather
than the place three unsolved problems land together.

## The dependency line moves, deliberately and once

Every library crate depends on `serde` and nothing else. That has held for eight
crates and is worth keeping.

It cannot hold here. `Observation.embedding` is required, so a binary that
cannot embed cannot record a memory — which is most of what a binary is for.
Something must make an HTTP request.

So the line moves to exactly one place and is named:

- **`rm-providers`** — implements `rm_extract::Completer` and
  `rm_engine::Embedder` against an OpenAI-compatible API. Takes `ureq` and
  `rustls`. Blocking, not async: a CLI making one request at a time gains
  nothing from a runtime and pays for it in dependencies and complexity.
- **`rm-cli`** — takes `toml` and `serde` for its config.

Nothing else changes. The claim becomes "every library crate is `serde`-only;
two crates at the edge have dependencies, and here is what they are" — which is
checkable, whereas "no third-party dependencies" would now be false.

`rm-providers` is a crate rather than a module inside `rm-cli` because `rm-mcp`
is on the roadmap and will need exactly the same two implementations. The
alternative — putting them in `rm-cli` — leaves the server either duplicating
them or depending on a binary crate. The YAGNI objection is fair and noted: this
is a ninth crate with one consumer today.

### Arguments are parsed by hand

Five subcommands is roughly sixty lines of matching on `std::env::args`, against
a dependency tree that pulls in `syn`, `quote`, `proc-macro2` and several more.
This workspace has twice chosen to write the small thing rather than take the
large dependency — exact search rather than an ANN index, ports rather than an
HTTP client — and both held up.

The cost, stated: no generated `--help`, so the usage text is written and
maintained by hand and can drift from the parser. No automatic validation of
flags. If the surface grows much past this, revisit.

## Configuration

`rmem.toml`, written by `rmem init` with its comments intact:

```toml
[store]
path = "memory.json"

[provider]
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"        # the NAME of the variable, never the key
completion_model = "gpt-4o-mini"
embedding_model = "text-embedding-3-small"
dimension = 1536                       # discovered by init; do not guess
metric = "cosine"

[resolution]
review_at = 4.0                        # below this: different. above match_at: same.
match_at = 6.0                         # between them: asked about, never merged.

[[resolution.field]]
field = "name"                         # which mention field this rule scores
comparator = "jaro_winkler"
m = 0.9                                # P(agrees | same entity)
u = 0.01                               # P(agrees | different entities) — its commonness

[[resolution.blocking]]
kind = "prefix"
field = "name"
n = 3

[policy]
default = "most_recent"

[policy.attribute]
employer = "valid_interval"
```

The ruleset is spelled out rather than hidden behind a named profile. That is
the point: `FieldRule`'s own documentation says the Fellegi–Sunter model "makes
you state *why* a field is informative rather than how much you like it", and a
profile that buries `m` and `u` inside the binary is exactly the hand-tuned
opacity the resolver was built to refuse. `init` writes working values so nobody
starts from nothing, and the values are on disk where they can be read and
changed.

**The API key is never written.** The file holds the *name* of an environment
variable. A config file is a thing people commit.

### `init` discovers the dimension

`VectorIndex::new` needs a dimension up front, it is a property of the embedding
model, and a config where `embedding_model` and `dimension` disagree produces
silently wrong vectors — every distance meaningless, nothing erroring.

So `init` embeds a probe string, reads the length back, and writes what it
found. The two cannot drift because only one of them was ever entered by a
human.

## Persistence

`Engine::snapshot()` to `store.path`, and `Engine::open(snapshot, ruleset,
policy)` to read it back.

Writes go to a temporary file in the same directory and are renamed over the
target. An interrupted write then leaves the previous snapshot intact rather
than a truncated one — the store's whole value is that it is reconstructible,
and a half-written file is the one way to lose that outright.

This is where an earlier decision pays off. `Engine::open` deliberately does not
persist the ruleset or policy; they are supplied fresh by the caller. Here the
caller is the config file, so there is exactly one source for them and a stale
copy inside a snapshot cannot silently override what the file says.

**No lock file.** Two `rmem` processes writing at once is documented as
unsupported rather than half-defended. A lock that is not tested under
contention is worse than an honest limitation.

A missing store is not an error. `recall` and `about` against a store that does
not exist yet report that nothing has been remembered, and `remember` creates
one — the first turn is not a special case a user should have to know about.

A missing *config* is a different matter, and is an error: every command but
`init` needs a model, a dimension and a ruleset, and none of them can be
guessed. The message names the file that was looked for and says to run
`rmem init`.

## Commands

```
rmem init                          # probe the model, write rmem.toml
rmem remember "<turn>"             # extract + ingest: the whole pipeline
rmem recall "<query>" [-k N]
rmem about <entity> <attribute>
rmem review [confirm <id> | reject <id>]
```

`<entity>` is a `StableId`. Users get them from `remember`, which names the
entity each mention landed on, and from `recall`, which reports the entity
behind every hit. `<id>` for `review` is a `ReviewId`, printed by `remember`
when it raises one and by `review` with no argument.

`-k` defaults to 5.

```
crates/rm-cli/src/
  lib.rs     — Command, run(), and the Outcome types each command returns
  config.rs  — Config, loading, and the init template
  args.rs    — parsing and the usage text
  format.rs  — rendering an Outcome to text
  main.rs    — parse, run, print, set exit code
```

Command functions return **data**, never strings, and `format.rs` renders it.
That is what lets every command be tested as an ordinary function against a stub
provider, with no process spawning and no network — the same shape `rm-extract`
already uses for its `Completer`.

```rust
pub enum Outcome {
    Initialised { path: PathBuf, dimension: usize },
    Remembered(rm_engine::Ingested, Vec<MentionLanding>),
    Recalled(Vec<rm_engine::Recalled>),
    About(rm_engine::Believed),
    Reviews(Vec<ReviewLine>),
    Confirmed { survivor: rm_engine::StableId },
    Rejected,
}

/// One mention and where it ended up, so `remember` can say "recognised" or
/// "new" rather than only printing a count.
pub struct MentionLanding {
    pub name: String,
    pub entity: rm_engine::StableId,
    pub was_new: bool,
}
```

`Ingested` already carries the entity ids, the assertions, the reviews and the
closures, so `remember`'s output is mostly a rendering of it. `MentionLanding`
exists because `Ingested` does not record which entities were *created* versus
recognised, and that distinction is the most useful thing on the screen.

### `remember` prints what it inferred

The library works hardest to make inferences visible: a closure is provenanced
`AgentInference` precisely so nobody can mistake it for testimony. A CLI that
prints `remembered.` discards that at the last possible step.

```
$ rmem remember "I started at Globex last month"
remembered 2 mentions, 1 fact, 1 relationship
  Ben Severn  → entity 0 (recognised)
  Globex      → entity 7 (new)
inferred, not stated:
  ended: Ben Severn employed_by Acme — "starting a new job ends the previous one"
```

The same applies to reviews. If ingesting raises one, `remember` says so on the
spot with its id — the engine returns reviews rather than logging them on the
argument that a review nobody can reach is the same as no review, and printing
nothing would undo that.

### Errors print the library's own words

Every refusal in this workspace names what was missing. Wrapping those in
`Error: operation failed` would discard the one part that took effort to write,
so the CLI prints them verbatim.

**Exit codes are 0 or 1, and the distinction is not cosmetic.** `about`
returning `Believed::Unknown` is a real answer — the store has no opinion — and
exits 0. A *refusal* is a failure to answer: survivorship declining under the
configured strategy, or extraction rejecting a malformed response. Those exit 1.

## Testing

Every test runs offline against a stub `Completer`/`Embedder`.

- `init_writes_a_config_whose_dimension_came_from_the_model`
- `init_refuses_to_overwrite_an_existing_config`
- `init_writes_the_name_of_the_key_variable_and_never_the_key`
- `remembering_a_turn_reports_what_it_inferred_separately_from_what_was_said`
- `remembering_a_turn_that_raises_a_review_says_so_with_its_id`
- `recall_on_a_store_that_does_not_exist_says_so_rather_than_failing`
- `about_an_attribute_nobody_mentioned_is_not_an_error`
- `a_refusal_prints_what_was_missing_and_exits_nonzero`
- `confirming_a_review_merges_and_names_the_surviving_entity`
- `an_interrupted_write_leaves_the_previous_snapshot_intact`
- `a_config_naming_an_unset_key_variable_says_which_one`

## Out of scope

- **`forget`, `erase`, `erase-edges`, `relate`, `unrelate`, `neighborhood`,
  `edge-history`.** All exist on `Engine`; each is additive once the shape is
  proven.
- **Locking, concurrent invocation.** Documented as unsupported.
- **Shell completion, colour, multiple config profiles, streaming output.**
- **`rm-mcp`.** Its own piece of work, and easier for this existing.
- **Retry and backoff in `rm-providers`.** One request, one answer; a failure is
  reported rather than papered over.
