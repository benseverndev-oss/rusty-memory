# Scoped Recall Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `recall` the applicability rule the decision reads already honour, so a session sees one consistent world however it asks.

**Architecture:** The scope rule moves from `rm-host` down to `rm-core`, where `rm-engine` can reach it, with `rm-host` keeping a re-export so no caller changes. `Query` gains a `position`, filtered inside the index scan alongside `entity`, `source` and `session`, so `k` keeps meaning "k results that apply". A `recall applicability` row in `rm-conform` measures the claim rather than exemplifying it.

**Tech Stack:** Rust (pinned in `rust-toolchain.toml`), no new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-25-scoped-recall-design.md`

## Global Constraints

- **`rm-core` is `0.1`, so the move must be additive only** — new module, re-export at the old path, no signature changes. That is what the version promised.
- **Filter inside the scan, never after.** Post-filtering silently shrinks every result set and makes `k` a lie. `in_scope` is already a predicate passed to `index.search_adjusted`; the new clause joins it there.
- **`Query::new` defaults every optional field**, so adding `position: None` breaks none of its 27 call sites. Do not change `Query::new`'s signature.
- **`rm-conform`'s `applicability.rs` must never import the code it judges.** The ban list gains `rm_core::scope`; without it a bare `use rm_core::scope;` slips through and the differential becomes a tautology while its own test still passes.
- **`position` stays in `rm-host`.** It normalises a configured value — empty or whitespace `RMEM_SCOPE` is no position — which is about reading configuration, not about the rule.
- **`about` stays unscoped.** Out of scope by decision, not oversight.
- **CI commands, as CI spells them:** `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features`.
- **Verify with exit codes, not by grepping output.** `cargo clippy … | grep error` exits 0 when it *finds* errors; a `&&` chain after it runs on a red crate.
- **Baseline:** 763 tests pass on `main`. Every task must leave that at or above where it started.
- **Commit style:** a title line in plain words, a body explaining why. No conventional-commit prefixes.
- **The working copy is CRLF.** Preserve line endings when editing.

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/rm-core/src/scope.rs` | The rule: `applies_at`, `validate`, `UNIVERSAL` | **Create** (moved) |
| `crates/rm-core/src/lib.rs` | Module list | Add `pub mod scope;` |
| `crates/rm-host/src/scope.rs` | Re-export, plus `position` which does not move | Rewrite |
| `crates/rm-conform/src/applicability.rs` | The import ban | One string |
| `crates/rm-engine/src/read.rs` | `Query::position`, `Query::at`, the `in_scope` clause | Modify |
| `crates/rm-host/src/command.rs` | `plan_recall`/`recall` carry a position | Modify |
| `crates/rm-cli/src/args.rs`, `run.rs` | `--scope` / `--all` on `recall` | Modify |
| `crates/rm-mcp/src/tools.rs`, `serve.rs` | Two parameters on the `recall` tool | Modify |
| `crates/rm-conform/src/applicability.rs` | The `recall applicability` measurement | Modify |
| `crates/rm-conform/src/report.rs`, `README.md` | The new row | Modify |
| `README.md` | `rmem recall` flags, the `RMEM_TOOLS` table | Modify |

---

### Task 1: The rule moves down, and the guard follows it

**Files:**
- Create: `crates/rm-core/src/scope.rs`
- Modify: `crates/rm-core/src/lib.rs`, `crates/rm-host/src/scope.rs`, `crates/rm-conform/src/applicability.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `rm_core::scope::{applies_at, validate, UNIVERSAL}` — identical signatures to their `rm-host` originals
  - `rm_host::scope` continues to export all four names: the three re-exported, plus `position` which stays put

- [ ] **Step 1: Move the file, keeping its tests with it**

`git mv crates/rm-host/src/scope.rs crates/rm-core/src/scope.rs`

Then delete from the moved file the `position` function and its two tests (`an_empty_position_is_no_position_at_all`, `an_empty_string_would_otherwise_be_the_root_position`) — they go back to `rm-host` in Step 3. Everything else, including `applies_at`, `validate`, `UNIVERSAL` and their five tests, stays.

Add to `crates/rm-core/src/lib.rs`, keeping the module list alphabetical:

```rust
pub mod scope;
```

- [ ] **Step 2: Run the moved tests where they now live**

Run: `cargo test -p rm-core --all-features scope::`
Expected: PASS, 5 tests. They moved with the code and should not need editing.

- [ ] **Step 3: Rebuild `rm-host`'s module as a re-export plus `position`**

Create `crates/rm-host/src/scope.rs`:

```rust
//! Where a memory reaches, and where a host stands.
//!
//! The rule itself lives in [`rm_core::scope`] and is re-exported here. It
//! moved down so `rm_engine::Query` could use it: `Query` lives in `rm-engine`,
//! which depends on `rm-core` and not on this crate, and a second
//! implementation of ancestor-or-self in the engine is exactly the drift this
//! project keeps finding.
//!
//! [`position`] did not move. It normalises a *configured* value, which is a
//! fact about how a host learns a position rather than about what a scope
//! means.

pub use rm_core::scope::{applies_at, validate, UNIVERSAL};

/// A position, from a source that can hand back an empty value.
///
/// An unset `RMEM_SCOPE` and one set to the empty string look identical in a
/// shell and in a JSON `env` block, and they used to behave nothing alike:
/// unset suspends the applicability rule, while empty was read as a position
/// and split into one empty segment -- the root, where only [`UNIVERSAL`]
/// reaches. Measured on a 219-decision store, `RMEM_SCOPE=` returned 32
/// records where unset returned all 219.
///
/// That is the worst shape a defect can take here: a configuration that looks
/// unconfigured, hiding most of the store, reporting nothing. Whitespace is
/// trimmed for the same reason -- `RMEM_SCOPE=" "` is a typo, not a position.
pub fn position(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty value is not a position. It arrives that way from a shell that
    /// says `RMEM_SCOPE=` and from a JSON `env` block with an empty string,
    /// both of which read as "not configured" to whoever wrote them.
    #[test]
    fn an_empty_position_is_no_position_at_all() {
        assert_eq!(position(None), None);
        assert_eq!(position(Some(String::new())), None);
        assert_eq!(position(Some("   ".into())), None, "whitespace is a typo");
        assert_eq!(position(Some("\t\n".into())), None);

        assert_eq!(position(Some("work".into())), Some("work".into()));
        assert_eq!(
            position(Some("  work/goldenmatch  ".into())),
            Some("work/goldenmatch".into()),
            "trimmed, because a stray space is never meant"
        );
        // `*` is a real position -- the root -- and must survive.
        assert_eq!(position(Some(UNIVERSAL.into())), Some(UNIVERSAL.into()));
    }

    /// The bug this exists to prevent, stated as the two behaviours it kept
    /// apart. Without the filter above, `""` splits into one empty segment and
    /// nothing but `*` reaches it.
    #[test]
    fn an_empty_string_would_otherwise_be_the_root_position() {
        assert!(!applies_at("work", ""), "this is what made it dangerous");
        assert!(applies_at(UNIVERSAL, ""));
        // ...so the normalisation, not the rule, is what has to catch it.
        assert_eq!(position(Some(String::new())), None);
    }
}
```

- [ ] **Step 4: Extend the import ban before anything can slip through it**

In `crates/rm-conform/src/applicability.rs`, the banned list:

```rust
        for banned in [
            "rm_host::scope",
            "rm_core::scope",
            "scope::applies_at",
            "scope::UNIVERSAL",
        ] {
```

and the module doc's line naming the crate:

```rust
//! Not `applies_at`, not `validate`, not `UNIVERSAL`, from `rm_core::scope` or
//! its `rm_host` re-export. An oracle derived from the code it judges is not an
//! oracle.
```

**This step is not cosmetic.** Before it, a bare `use rm_core::scope;` matches none of the banned strings, and the guard would pass while no longer catching the import it exists to catch.

- [ ] **Step 5: Prove the extended ban catches the new path**

Temporarily add `use rm_core::scope as _judged;` above `pub fn reaches` in `applicability.rs`.

Run: `cargo test -p rm-conform --all-features this_module_does_not_import`
Expected: **FAIL** with `applicability imports rm_core::scope, so it judges itself`.

Remove the import. Re-run; expected PASS. A ban that has never been seen to fire is a comment.

- [ ] **Step 6: Verify the whole workspace with exit codes**

Run: `cargo test --workspace --all-features && cargo clippy --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && echo ALL CLEAN`
Expected: `ALL CLEAN`, count unchanged at 763. Nothing behavioural moved — if the count changed, a test was lost in the move.

- [ ] **Step 7: Commit**

```bash
git add crates/rm-core crates/rm-host crates/rm-conform
git commit -m "The rule moves to where the engine can reach it"
```

---

### Task 2: A query knows where it is asked from

**Files:**
- Modify: `crates/rm-engine/src/read.rs`

**Interfaces:**
- Consumes: `rm_core::scope::applies_at` from Task 1
- Produces:
  - `Query::position: Option<String>` — defaulted to `None` by `Query::new`, so no call site changes
  - `pub fn at(self, position: impl Into<String>) -> Query` — the builder, named for the others (`as_of`, `in_session`, `from_source`, `boosting`)
  - `in_scope` filters on it

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/rm-engine/src/read.rs`:

```rust
    /// A recall answers from where it is asked. The filter runs inside the
    /// scan, so `k` still means "k results that apply" rather than "k
    /// candidates, some of which survive".
    #[test]
    fn a_recall_returns_only_what_reaches_the_position_it_was_asked_from() {
        let mut e = scoped_engine();

        let all = e
            .recall(&Query::new(vec![1.0, 0.0, 0.0], 10))
            .expect("recall");
        assert_eq!(all.len(), 3, "unscoped, everything is a candidate");

        let here = e
            .recall(&Query::new(vec![1.0, 0.0, 0.0], 10).at("work/goldenmatch"))
            .expect("recall");
        let titles: Vec<&str> = here.iter().map(|r| r.attribute.as_str()).collect();
        assert_eq!(
            titles.len(),
            2,
            "the universal one and this project's, not the sibling: {titles:?}"
        );

        let elsewhere = e
            .recall(&Query::new(vec![1.0, 0.0, 0.0], 10).at("personal"))
            .expect("recall");
        assert_eq!(elsewhere.len(), 1, "only the universal one reaches personal");
    }

    /// An entity with no scope recorded reaches everywhere, exactly as in the
    /// decision reads. `remember`'s facts carry none, so a scoped recall must
    /// never hide them.
    #[test]
    fn an_assertion_whose_entity_has_no_scope_is_never_hidden() {
        let mut e = engine();
        let id = remember_at(&mut e, None, "unscoped", "a value", 1_000);
        let _ = id;
        let hits = e
            .recall(&Query::new(vec![1.0, 0.0, 0.0], 10).at("anywhere/at/all"))
            .expect("recall");
        assert_eq!(hits.len(), 1, "no scope recorded means it reaches here");
    }
```

Add the two helpers beside the module's existing fixtures:

```rust
    /// Write one assertion on `attribute`, optionally giving its entity a
    /// scope, and return the entity.
    fn remember_at(
        e: &mut Engine,
        scope: Option<&str>,
        attribute: &str,
        value: &str,
        observed_at: Timestamp,
    ) -> StableId {
        let obs = |attribute: &str, value: &str| Observation {
            kind: "thing".to_string(),
            mention: Record::new().with("name", attribute),
            attribute: attribute.to_string(),
            value: Some(value.to_string()),
            valid: Interval {
                from: observed_at,
                to: None,
            },
            provenance: Provenance {
                source: Source::UserAssertion,
                observed_at,
                source_ref: "test".to_string(),
            },
            supersession: Supersession::Unstated,
            embedding: vec![1.0, 0.0, 0.0],
        };
        let (id, _) = e
            .remember_as(None, obs(attribute, value))
            .expect("pinned write");
        if let Some(scope) = scope {
            e.remember_as(Some(id), obs("scope", scope))
                .expect("pinned write");
        }
        id
    }

    /// Three assertions at three reaches, on one engine.
    fn scoped_engine() -> Engine {
        let mut e = engine();
        remember_at(&mut e, Some("*"), "everywhere", "a value", 1_000);
        remember_at(&mut e, Some("work/goldenmatch"), "here", "a value", 1_100);
        remember_at(&mut e, Some("work/other"), "sibling", "a value", 1_200);
        e
    }
```

If `read.rs`'s test module has no `engine()` fixture, copy the one from `crates/rm-conform/src/applicability.rs` — a `VectorIndex::new(3, Metric::Cosine)`, a one-field `Ruleset` on `name`, and `Policy::new(Strategy::MostRecent)`.

**Note the shape:** `remember_at` writes the scope as an ordinary attribute on the same entity, because that is how `decide` writes it. The recall filter must find it the same way.

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p rm-engine --all-features a_recall_returns_only_what_reaches`
Expected: FAIL — `no method named 'at' found for struct 'Query'`.

- [ ] **Step 3: Add the field and the builder**

In `Query`, after `session`:

```rust
    /// Where the asker stands. `None` suspends the applicability rule.
    ///
    /// A *filter*, unlike [`Query::boost`], and the reason the two differ is
    /// worth keeping straight. `boost` is a boost because turning a name into
    /// an entity is fallible -- measured at J = 0.33 on this corpus -- so
    /// filtering on it would discard the answer every time the guess was wrong.
    /// A position is not a guess: it is a declared string compared to a stored
    /// one, so filtering discards nothing on a bad inference because there is
    /// no inference.
    pub position: Option<String>,
```

In `Query::new`, add `position: None` beside the other defaults. **Do not change the signature** — every one of its 27 call sites relies on it defaulting the optional fields.

Beside the other builders:

```rust
    /// Ask from `position`, returning only what reaches it.
    pub fn at(mut self, position: impl Into<String>) -> Self {
        self.position = Some(position.into());
        self
    }
```

- [ ] **Step 4: Filter inside the scan**

In `in_scope`, after the `session` clause and before the function returns true:

```rust
        if let Some(position) = &q.position {
            // The scope lives on the entity, as an ordinary attribute, because
            // that is how `decide` writes it. Read at the query's own clocks so
            // a scoped recall and a scoped `decisions` agree about the same
            // instant; unqualified, that is the latest of both axes.
            let (valid_t, tx_t) = q.as_of.unwrap_or((Timestamp::MAX, Timestamp::MAX));
            let reach = self
                .store
                .history(entry.entity, "scope")
                .iter()
                .filter(|v| v.provenance.observed_at <= tx_t && v.valid.from <= valid_t)
                .next_back()
                .and_then(|v| v.value.clone());
            // No scope recorded reaches everywhere -- the legacy rule, and what
            // keeps `remember`'s facts from ever being hidden.
            if let Some(reach) = reach {
                if !rm_core::scope::applies_at(&reach, position) {
                    return false;
                }
            }
        }
```

- [ ] **Step 5: Run them**

Run: `cargo test -p rm-engine --all-features`
Expected: PASS, both new tests plus everything already there.

- [ ] **Step 6: Prove the filter is not vacuous**

Temporarily change `applies_at(&reach, position)` to `true`.

Run: `cargo test -p rm-engine --all-features a_recall_returns_only_what_reaches`
Expected: **FAIL** — the scoped recalls return 3 instead of 2 and 1. Restore, re-run, expect PASS. A filter never seen to exclude anything is decoration.

- [ ] **Step 7: Verify with exit codes**

Run: `cargo test --workspace --all-features && cargo clippy --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && echo ALL CLEAN`
Expected: `ALL CLEAN`, count at 765 or above.

- [ ] **Step 8: Commit**

```bash
git add crates/rm-engine
git commit -m "A query knows where it is asked from"
```

---

### Task 3: The two surfaces

**Files:**
- Modify: `crates/rm-host/src/command.rs` (`plan_recall`, `recall`)
- Modify: `crates/rm-cli/src/args.rs`, `crates/rm-cli/src/run.rs`
- Modify: `crates/rm-mcp/src/tools.rs`, `crates/rm-mcp/src/serve.rs`

**Interfaces:**
- Consumes: `Query::at`, `rm_host::scope::position`
- Produces:
  - `pub fn commit_recall(engine: &Engine, embedding: Vec<f32>, k: usize, weak_below: f32, here: Option<&str>) -> Result<Outcome, HostError>` — the position joins the existing arguments. `plan_recall` returns a bare `Vec<f32>`, not a plan struct.
  - `pub fn recall(engine, query: &str, k, embedder, weak_below, here)` — the convenience wrapper over `plan_recall` + `commit_recall`, threaded the same way
  - `Command::Recall { query, k, scope: Option<String>, all: bool }`
  - `Call::Recall { query, k, scope: Option<String>, all: bool }`

- [ ] **Step 1: Write the failing CLI parse test**

Add to `crates/rm-cli/src/args.rs`'s test module:

```rust
    #[test]
    fn recall_takes_a_position_and_a_way_to_ignore_it() {
        let Command::Recall { scope, all, .. } =
            parse_args(&["recall", "a question", "--scope", "personal"]).unwrap()
        else {
            panic!("not a recall command")
        };
        assert_eq!(scope.as_deref(), Some("personal"));
        assert!(!all);

        let Command::Recall { all, scope, .. } =
            parse_args(&["recall", "a question", "--all"]).unwrap()
        else {
            panic!("not a recall command")
        };
        assert!(all, "--all suspends the rule");
        assert_eq!(scope, None);
    }
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p rm-cli --all-features recall_takes_a_position`
Expected: FAIL — `Command::Recall` has no such fields.

- [ ] **Step 3: Add the fields and parse them**

In `Command::Recall`, beside `query` and `k`:

```rust
        /// Ask from this position instead of `RMEM_SCOPE`.
        scope: Option<String>,
        /// Suspend the applicability rule and search everything.
        all: bool,
```

In the `"recall"` arm, beside the existing `k` parsing:

```rust
            scope: flag(&args, "--scope")?,
            all: args.iter().any(|a| a == "--all"),
```

Update the usage text near the other `recall` line:

```
    rmem recall \"<query>\" [-k N] [--scope <s>] [--all]
                                     find assertions near a query (default 5).
                                     --scope asks from a position, --all
                                     searches regardless of reach
```

Add `scope: None, all: false` to any existing test that constructs `Command::Recall` literally.

- [ ] **Step 4: Take a position in `commit_recall`**

`commit_recall` is where the `Query` is built; `recall` is a convenience wrapper over `plan_recall` + `commit_recall`. **`weak_below` is a parameter, not a return value** — it is passed in and handed straight to `Outcome::Recalled`.

```rust
pub fn commit_recall(
    engine: &Engine,
    embedding: Vec<f32>,
    k: usize,
    weak_below: f32,
    here: Option<&str>,
) -> Result<Outcome, HostError> {
    let mut query = Query::new(embedding, k);
    if let Some(here) = here {
        query = query.at(here);
    }
    let hits = engine
        .recall(&query)
        .map_err(|e| HostError::Refused(e.to_string()))?;
    Ok(Outcome::Recalled { hits, weak_below })
}
```

Add `here: Option<&str>` to `recall`'s signature too, passing it through to `commit_recall`, so both entry points carry a position.

- [ ] **Step 5: Wire the CLI arm**

In `crates/rm-cli/src/run.rs`, at the `Command::Recall` dispatch:

```rust
                (
                    Command::Recall {
                        k, scope, all, ..
                    },
                    Some(vector),
                ) => {
                    // `--all` beats `--scope`, which beats the environment.
                    // `None` is no position, which suspends the rule.
                    let here = rm_host::scope::position(if all {
                        None
                    } else {
                        scope.or_else(|| session_scope.clone())
                    });
                    command::recall(engine, vector, k, here.as_deref())
                }
```

- [ ] **Step 6: Wire the MCP tool**

In `crates/rm-mcp/src/tools.rs`, add to the `recall` tool's `properties`:

```json
                    "scope": {
                        "type": "string",
                        "description": "Ask from this position instead of the session's own."
                    },
                    "all": {
                        "type": "boolean",
                        "description": "Ignore reach; search memories scoped elsewhere too."
                    }
```

Add the two fields to `Call::Recall` and parse them:

```rust
                scope: optional_string(arguments, "scope")?,
                all: optional_bool(arguments, "all")?.unwrap_or(false),
```

In `crates/rm-mcp/src/serve.rs`, the `Call::Recall` dispatch uses the existing `position(scope, all)` helper — the same one the decision reads use — and passes `here.as_deref()` to `command::recall`.

- [ ] **Step 7: Run both crates**

Run: `cargo test -p rm-cli -p rm-mcp --all-features`
Expected: PASS. Expect the MCP tool-listing snapshot tests to need the new properties; update them to match.

- [ ] **Step 8: Measure what the schema now costs**

```bash
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"m","version":"1"}}}\n{"jsonrpc":"2.0","id":2,"method":"tools/list"}\n' > /tmp/req.jsonl
RMEM_CONFIG=D:/memory/rmem.toml RMEM_TOOLS=decide,decisions,decision,recall \
  cargo run -q -p rm-mcp --bin rmem-mcp < /tmp/req.jsonl 2>/dev/null | tail -1 | wc -c
```

The README records that configuration at **~1,210 tokens**, and a token is almost exactly 4.01 bytes by the table's own figures. Record the new number in the commit message; update the README row in Task 5.

- [ ] **Step 9: Verify with exit codes**

Run: `cargo test --workspace --all-features && cargo clippy --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && echo ALL CLEAN`
Expected: `ALL CLEAN`, count at 766 or above.

- [ ] **Step 10: Commit**

```bash
git add crates/rm-host crates/rm-cli crates/rm-mcp
git commit -m "Recall asks from where the session stands"
```

---

### Task 4: A row, not an example

**Files:**
- Modify: `crates/rm-conform/src/applicability.rs`
- Modify: `crates/rm-conform/src/report.rs`

**Interfaces:**
- Consumes: `build`, `world`, `Params`, `reaches`, `expected` — all already in `applicability.rs`
- Produces:
  - `pub fn recall_visible(engine: &Engine, position: &str) -> Vec<String>` — the titles a scoped recall returns, sorted
  - `pub fn recall_agreement(seeds: std::ops::Range<u64>, params: &Params) -> bool`
  - A `recall applicability` row in the table

- [ ] **Step 1: Write the failing tests**

Add to `applicability.rs`'s test module:

```rust
    #[test]
    fn a_scoped_recall_returns_exactly_what_applies() {
        assert!(
            recall_agreement(0..40, &Params::default()),
            "a scoped recall disagreed with the oracle on some (world, position)"
        );
    }

    /// The guard that matters for this row: the filter has to exclude
    /// something similarity would otherwise have returned. A recall row that
    /// never excludes anything reports 1.000 having measured the generator.
    #[test]
    fn the_recall_filter_actually_excludes_something() {
        let params = Params::default();
        let excluded = (0..40).any(|seed| {
            let w = world(seed, &params);
            let e = build(&w);
            w.positions.iter().any(|p| {
                let unscoped = recall_visible(&e, "*");
                let scoped = recall_visible(&e, p);
                scoped.len() < unscoped.len()
            })
        });
        assert!(
            excluded,
            "no position ever narrowed a recall, so the row measures the \
             generator rather than the filter"
        );
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p rm-conform --all-features a_scoped_recall_returns`
Expected: FAIL — `cannot find function 'recall_agreement'`.

- [ ] **Step 3: Implement them**

Add above the `#[cfg(test)]` block in `applicability.rs`:

```rust
/// The titles a scoped recall returns, sorted.
///
/// `k` is the whole decision count, so the measurement is about *which*
/// assertions come back rather than how many fit — a smaller `k` would
/// confound the filter with the cut and make a disagreement unattributable.
pub fn recall_visible(engine: &Engine, position: &str) -> Vec<String> {
    let q = rm_engine::Query::new(vec![1.0, 0.0, 0.0], 1_000).at(position);
    // `Recalled::name` already carries the entity's name -- the field exists
    // because "every caller wants it and the engine already holds it" -- so no
    // second lookup per hit.
    let mut out: Vec<String> = engine
        .recall(&q)
        .expect("recall cannot fail on a store this builds")
        .into_iter()
        .filter(|r| r.attribute == "choice")
        .filter_map(|r| r.name)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Whether a scoped recall and the oracle agree on every generated world.
///
/// The same expectation `applicability agreement` uses, against a different
/// read path. `decisions` filters in the host and `recall` filters inside the
/// index scan, so agreeing on one says nothing about the other.
pub fn recall_agreement(seeds: std::ops::Range<u64>, params: &Params) -> bool {
    seeds.into_iter().all(|seed| {
        let w = world(seed, params);
        let e = build(&w);
        w.positions
            .iter()
            .all(|p| recall_visible(&e, p) == expected(&w, p))
    })
}
```

Every generated decision carries a `choice`, so filtering hits to that attribute gives one row per decision; `dedup` guards the case where a title is re-decided and two `choice` versions both match.

- [ ] **Step 4: Run them**

Run: `cargo test -p rm-conform --all-features applicability::`
Expected: PASS.

**If `a_scoped_recall_returns_exactly_what_applies` fails, stop and read it.** The two read paths filter in different places — the host for `decisions`, the index scan for `recall` — so a disagreement is a real difference between them and not a test to adjust. The most likely cause is the recall filter reading the scope at different clocks than `held` does.

- [ ] **Step 5: Add the row**

In `crates/rm-conform/src/report.rs`, import `recall_agreement` beside the others and add after the `rescope keeps its history` row:

```rust
    out.push_str(&format!(
        "| recall applicability | {} |\n",
        verdict(recall_agreement(0..SCOPE_SEEDS, &scope_params))
    ));
```

and add `"recall applicability"` to the row list in `the_table_reports_every_row_and_no_failures`.

- [ ] **Step 6: Run the report and check the budget**

Run: `cargo run --release -q -p rm-conform -- --report`
Expected: nine rows, all `1.000`.

Time it. The spec's budget for the whole applicability group was under 1s added in release; the report was 0.698s before this row. If the total exceeds ~1.3s, drop `SCOPE_SEEDS` and say so in the commit message rather than silently.

- [ ] **Step 7: Verify with exit codes**

Run: `cargo test --workspace --all-features && cargo clippy --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && echo ALL CLEAN`
Expected: `ALL CLEAN`, count at 768 or above.

- [ ] **Step 8: Commit**

```bash
git add crates/rm-conform
git commit -m "The claim that a recall returns only what applies"
```

---

### Task 5: Saying so

**Files:**
- Modify: `README.md`
- Modify: `crates/rm-conform/README.md`

**Interfaces:**
- Consumes: everything above. Produces nothing code depends on.

- [ ] **Step 1: Document the flags**

In `README.md`'s scope section, after the paragraph explaining that the decision reads take `--scope` and `--all`:

```markdown
`rmem recall` takes them too, and for the same reason: a session that lists 78
of 219 decisions and then searches all 219 has two views of one store. The
filter runs inside the index scan rather than over a fetched page, so `-k 5`
still means five results that apply rather than five candidates of which some
survive.

`about` deliberately does not take them. It reads an entity you named by id,
which is a deliberate act rather than a search — scope decides what you are
*shown*, not what you are allowed to *name*.
```

- [ ] **Step 2: Update the `RMEM_TOOLS` table**

Task 3 Step 8 measured the `decide,decisions,decision,recall` listing. Update that row, and re-measure the other three the same way so the table stays internally consistent:

```bash
for cfg in "" "decide,decisions,decision,recall" "decide,decisions,decision" "decisions,decision"; do
  n=$(RMEM_CONFIG=D:/memory/rmem.toml RMEM_TOOLS="$cfg" \
      cargo run -q -p rm-mcp --bin rmem-mcp < /tmp/req.jsonl 2>/dev/null | tail -1 | wc -c)
  echo "${cfg:-ALL} $n bytes ~$((n * 100 / 401)) tokens"
done
```

Only the rows containing `recall` should move. If another row moves, something else changed and it is worth knowing why before publishing the number.

- [ ] **Step 3: Update the conformance README**

`crates/rm-conform/README.md` carries the eight-row table and the seed sentence. Add the ninth row from Task 4 Step 6's output, and one line to the applicability section:

```markdown
`recall applicability` measures the same claim as `applicability agreement`
against a different read path. `decisions` filters in the host; `recall` filters
inside the index scan, where `k` is applied. Agreeing on one says nothing about
the other, which is why both are rows rather than one.
```

- [ ] **Step 4: Spellcheck and final verification**

Run: `typos README.md crates/rm-conform/README.md && cargo test --workspace --all-features && cargo clippy --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && echo ALL CLEAN`
Expected: `ALL CLEAN`.

- [ ] **Step 5: Commit**

```bash
git add README.md crates/rm-conform/README.md
git commit -m "Two views of one store, reduced to one"
```

---

## What this does not do

Carried from the spec so it is not lost between documents:

- **`about` stays unscoped.** It takes an explicit entity id; refusing something you named by id is unhelpful, and `decision "<title>"` already handles the named case better by saying where it lives.
- **No retrieval-quality claim.** This measures *which* assertions come back, not whether they are the right ones. That is `benches/locomo`'s axis and it costs money.
- **No change to `boost`.** It stays a boost for the reason the code gives: turning a name into an entity is fallible at J = 0.33, and a position is not a guess.
