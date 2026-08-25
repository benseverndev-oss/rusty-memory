# Decision Scope Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give a decision a stated reach and a session a stated position, so the decision log answers "what applies here" rather than "everything anyone ever recorded".

**Architecture:** One pure module (`rm_host::scope`) holds validation and the single applicability rule — a memory applies where its scope is an ancestor-or-self of the asker's position, compared segment-wise. `scope` becomes a fifth bi-temporal attribute written by a required argument to `decide`. The reads take an `Option<&str>` position; `None` suspends the rule entirely, which is both the `--all` behaviour and the behaviour for anyone who never sets `RMEM_SCOPE`.

**Tech Stack:** Rust (pinned in `rust-toolchain.toml`), no new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-25-decision-scope-design.md`

## Global Constraints

- **Branch is `claude/decision-scope`, stacked on `claude/decision-log-clocks`** (PR #39). Do not rebase onto `main` until #39 merges.
- **No new dependencies.** Library crates take `serde`/`serde_json` only; `rm-host` adds `toml`, `rm-providers` adds `ureq`.
- **Every test runs offline.** No socket, no spawned process, no API key.
- **CI commands, as CI spells them:** `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features`.
- **`*` is the universal reach** and is legal only as an entire scope value. `work/*` is refused.
- **Segment-wise comparison, never string-prefix.** `prod` must not match `production`.
- **Comparison is exact and case-sensitive.** The store normalises nothing, because normalising is interpreting.
- **`RMEM_SCOPE` is read-side only.** It is never a write default. Reach varies per decision; a session cannot supply it.
- **Applicability filters the index, never a chain.** Supersession walks and the `replaced by entity N` line ignore the rule.
- **Baseline:** 703 tests pass on `claude/decision-log-clocks`. Every task must leave that at or above where it started.
- **Commit style:** a title line in plain words, a body explaining why. No conventional-commit prefixes; see `git log --oneline`.
- **This repo's working copy is CRLF.** Editing scripts must preserve line endings or `cargo fmt --check` and diffs go noisy.

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/rm-host/src/scope.rs` | Validation and the applicability rule. Pure, no engine, no store. | **Create** |
| `crates/rm-host/src/lib.rs` | Module list | Add `pub mod scope;` |
| `crates/rm-host/src/command.rs` | Write path takes a scope; reads filter by it; `Found::NotHere` | Modify |
| `crates/rm-cli/src/args.rs` | `--scope` on three commands, `--all` on two | Modify |
| `crates/rm-cli/src/main.rs` | `RMEM_SCOPE` | Modify |
| `crates/rm-cli/src/run.rs` | Thread scope and position | Modify |
| `crates/rm-cli/src/format.rs` | Render `NotHere` | Modify |
| `crates/rm-mcp/src/tools.rs` | `scope` required on `decide`; `scope`/`all` on reads; `RMEM_SCOPE` | Modify |
| `crates/rm-mcp/src/serve.rs` | Thread scope and position | Modify |
| `crates/rm-mcp/src/render.rs` | Render `NotHere` | Modify |
| `crates/rm-conform/src/decisions.rs` | `build_chain` must state a scope | Modify |
| `docs/seed-decision-log.sh` | 35 `decide` calls through one `d()` wrapper | Modify |
| `README.md` | The decision-log section and the shared-store example | Modify |

---

### Task 1: The rule, on its own

**Files:**
- Create: `crates/rm-host/src/scope.rs`
- Modify: `crates/rm-host/src/lib.rs`

**Interfaces:**
- Consumes: nothing. This module is pure and imports no other crate.
- Produces:
  - `pub const UNIVERSAL: &str = "*";`
  - `pub fn validate(scope: &str) -> Result<(), String>`
  - `pub fn applies_at(scope: &str, position: &str) -> bool`

- [ ] **Step 1: Write the failing tests**

Create `crates/rm-host/src/scope.rs` containing only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_memory_applies_at_its_own_position_and_below() {
        assert!(applies_at("work", "work"), "self");
        assert!(applies_at("work", "work/goldenmatch"), "ancestor");
        assert!(applies_at("work/goldenmatch", "work/goldenmatch/fs"));
        assert!(applies_at(UNIVERSAL, "anything/at/all"));
        assert!(applies_at(UNIVERSAL, UNIVERSAL));
    }

    #[test]
    fn a_memory_does_not_apply_beside_or_above_itself() {
        assert!(!applies_at("work/goldenmatch/fs", "work/goldenmatch/er"), "sibling");
        assert!(!applies_at("personal", "work"), "unrelated");
        // Narrower than the asker: a memory about one subsystem does not
        // apply to the whole project.
        assert!(!applies_at("work/goldenmatch", "work"), "descendant");
        // A position of `*` is the root, where only universal memories reach.
        assert!(!applies_at("work", UNIVERSAL));
    }

    /// The whole reason comparison is segment-wise. A string prefix would
    /// make every `prod` decision apply to `production`, silently.
    #[test]
    fn a_segment_is_not_a_string_prefix() {
        assert!(!applies_at("prod", "production"));
        assert!(!applies_at("work", "workshop/thing"));
        assert!(applies_at("prod", "prod/deploy"));
    }

    #[test]
    fn a_scope_that_could_mean_two_things_is_refused() {
        assert!(validate("work").is_ok());
        assert!(validate("work/goldenmatch/fs").is_ok());
        assert!(validate(UNIVERSAL).is_ok());

        for bad in ["", "  ", "/work", "work/", "work//fs", "work/ /fs", "work/*"] {
            let e = validate(bad).unwrap_err();
            assert!(!e.is_empty(), "for {bad:?}");
        }
    }

    /// `*` is a value, not a wildcard. Accepting `work/*` would promise a
    /// pattern language the rule does not have.
    #[test]
    fn the_universal_scope_is_refused_as_a_segment() {
        let e = validate("work/*").unwrap_err();
        assert!(e.contains('*'), "the message should name the character: {e}");
    }
}
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p rm-host --all-features scope::`
Expected: FAIL — the module is not declared, so it does not compile.

- [ ] **Step 3: Declare the module**

In `crates/rm-host/src/lib.rs`, beside the other `pub mod` lines, add:

```rust
pub mod scope;
```

- [ ] **Step 4: Implement the module**

Insert above the `#[cfg(test)]` block in `crates/rm-host/src/scope.rs`:

```rust
//! How far a memory reaches.
//!
//! A scope is not a label of origin. It is a declaration of reach, and the
//! difference decides the design: "never run scale benchmarks on the Windows
//! box" was written while working on one project and applies to every project
//! on the machine. Tagged with where it was written, it would vanish the moment
//! the next session was about something else.
//!
//! There is one rule, and everything here serves it:
//!
//! > A memory applies where its scope is an **ancestor-or-self** of the asker's
//! > position.
//!
//! The store does not interpret the segments. `work`, `personal/finance` and
//! `clients/acme/migration` are opaque strings that happen to contain a
//! separator; depth is unbounded and naming is the user's business.
//!
//! Nothing here touches the engine or the store, so the rule can be read and
//! tested without either.

/// The reach that covers every position.
///
/// The one value this module ascribes meaning to. `/` is a separator; the
/// segments between them stay opaque.
pub const UNIVERSAL: &str = "*";

const SEPARATOR: char = '/';

/// Whether a memory scoped `scope` applies to an asker standing at `position`.
///
/// Segment-wise, never a string prefix: `prod` must not match `production`,
/// and a string comparison would make that mistake silently on every read.
pub fn applies_at(scope: &str, position: &str) -> bool {
    if scope == UNIVERSAL {
        return true;
    }
    let mut here = position.split(SEPARATOR);
    // Every segment of the scope must be matched, in order, by the position.
    // Leftover position segments are fine -- that is what "or below" means.
    scope
        .split(SEPARATOR)
        .all(|segment| here.next() == Some(segment))
}

/// Whether `scope` is a scope at all.
///
/// The refusals exist so that two spellings cannot mean one thing. `work` and
/// `work/` would compare unequal and read identically, which is the sort of
/// difference nobody finds until a decision is missing.
pub fn validate(scope: &str) -> Result<(), String> {
    if scope == UNIVERSAL {
        return Ok(());
    }
    if scope.is_empty() {
        return Err(format!(
            "a scope says how far a decision reaches. It is {UNIVERSAL:?} for everywhere, or a path like \"work/goldenmatch\""
        ));
    }
    if scope.starts_with(SEPARATOR) || scope.ends_with(SEPARATOR) {
        return Err(format!(
            "{scope:?} has a leading or trailing {SEPARATOR:?}, which would make it a second spelling of the same scope"
        ));
    }
    for segment in scope.split(SEPARATOR) {
        if segment.trim().is_empty() {
            return Err(format!(
                "{scope:?} has an empty part. Every part between {SEPARATOR:?} has to name something"
            ));
        }
        if segment == UNIVERSAL {
            return Err(format!(
                "{scope:?} uses {UNIVERSAL:?} as a part, but it is a value rather than a wildcard: it means \"everywhere\" on its own and nothing inside a path"
            ));
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Run them to make sure they pass**

Run: `cargo test -p rm-host --all-features scope::`
Expected: PASS, 5 tests.

- [ ] **Step 6: fmt, clippy, crate**

Run: `cargo test -p rm-host --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: all clean.

- [ ] **Step 7: Commit**

```bash
git add crates/rm-host/src/scope.rs crates/rm-host/src/lib.rs
git commit -m "One rule, and the two spellings it refuses"
```

---

### Task 2: A decision states its reach

**Files:**
- Modify: `crates/rm-host/src/command.rs` — `DECISION_FIELDS` (`:240`), `plan_decide` (`:662`), `decide`
- Modify: `crates/rm-conform/src/decisions.rs` — `build_chain` calls `command::decide`

**Interfaces:**
- Consumes: `rm_host::scope::{validate, UNIVERSAL}` from Task 1
- Produces:
  - `pub fn plan_decide(title, choice, scope: &str, status, because, context, supersedes, decided_at, observed_at, session, embedder) -> Result<DecidePlan, HostError>` — `scope` is the third parameter, immediately after `choice`, because it is required and the optionals follow
  - `pub fn decide(engine, title, choice, scope: &str, status, because, context, supersedes, decided_at, observed_at, session, embedder)` — same position
  - `DECISION_FIELDS` becomes `[&str; 5]` including `"scope"`

- [ ] **Step 1: Write the failing test**

Add to the `---- decisions ----` section of `crates/rm-host/src/command.rs`:

```rust
    /// A scope is required and validated before the embedder is called, so a
    /// typo costs nothing -- the same bargain the status check already makes.
    #[test]
    fn a_decision_states_its_reach_or_is_refused() {
        let stub = StubProvider::new(vec![]);
        let plan = |scope: &str| {
            plan_decide(
                "Pin the compiler",
                "rust-toolchain.toml names the version",
                scope,
                None,
                None,
                None,
                None,
                None,
                1_000,
                "t",
                &stub,
            )
        };

        assert!(plan("work/goldenmatch").is_ok());
        assert!(plan(rm_host_scope::UNIVERSAL).is_ok());

        let Err(HostError::Refused(why)) = plan("") else {
            panic!("an unscoped decision should be refused")
        };
        assert!(why.contains("how far"), "{why}");

        let Err(HostError::Refused(why)) = plan("work/*") else {
            panic!("a wildcard segment should be refused")
        };
        assert!(why.contains('*'), "{why}");
    }

    /// The scope is stored like any other field, so it is versioned, readable
    /// at a past clock, and rebuilt by `reindex`.
    #[test]
    fn a_scope_is_an_attribute_like_the_others() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e,
            "Pin the compiler",
            "a choice",
            "work/goldenmatch",
            None,
            None,
            None,
            None,
            None,
            1_000,
            "t",
            &stub,
        )
        .unwrap();
        let id = find_decision(&e, "Pin the compiler").expect("recorded");
        assert_eq!(
            held(&e, id, "scope", At::latest()),
            Some("work/goldenmatch".to_string())
        );
    }
```

Add `use crate::scope as rm_host_scope;` to the test module's imports, or refer to `crate::scope::UNIVERSAL` inline — either is fine, but be consistent.

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p rm-host --all-features a_decision_states_its_reach_or_is_refused`
Expected: FAIL — `this function takes 10 arguments but 11 arguments were supplied`.

- [ ] **Step 3: Add `scope` to the field list**

In `crates/rm-host/src/command.rs:240`:

```rust
const DECISION_FIELDS: [&str; 5] = ["status", "choice", "because", "context", "scope"];
```

This is load-bearing beyond the write path: `plan_reindex` filters on it at `:956`, so adding it here is what makes a scope's vector rebuildable rather than reported as unreachable.

- [ ] **Step 4: Take and validate the scope in `plan_decide`**

Change the signature, adding `scope: &str` immediately after `choice`:

```rust
pub fn plan_decide(
    title: &str,
    choice: &str,
    /// How far this decision reaches. Required, and never defaulted: reach
    /// varies per decision, so neither the session nor the store can supply it.
    scope: &str,
    status: Option<&str>,
    because: Option<&str>,
    context: Option<&str>,
    supersedes: Option<&str>,
    decided_at: Option<Timestamp>,
    observed_at: Timestamp,
    session: &str,
    embedder: &impl Embedder,
) -> Result<DecidePlan, HostError> {
```

Immediately after the existing title/choice emptiness check, and before any embedding:

```rust
    // Before the embedder, so a typo costs nothing -- the same bargain the
    // status checks below make.
    crate::scope::validate(scope).map_err(HostError::Refused)?;
```

Then add the field to the loop that builds `fields`:

```rust
    for (name, value) in [
        ("status", Some(status)),
        ("choice", Some(choice)),
        ("because", because),
        ("context", context),
        ("scope", Some(scope)),
    ] {
```

- [ ] **Step 5: Thread it through `decide`**

Add `scope: &str` in the same position in `decide`'s signature, and pass it to `plan_decide`.

- [ ] **Step 6: Run the test**

Run: `cargo test -p rm-host --all-features a_decision_states_its_reach a_scope_is_an_attribute_like_the_others`
Expected: PASS.

- [ ] **Step 7: Fix every other caller**

Run: `cargo build --workspace --all-features 2>&1 | rg 'E0061' -A3`

Every existing call needs a scope argument. In `crates/rm-host/src/command.rs`'s own tests, pass `"work"` unless the test is about scope. In `crates/rm-conform/src/decisions.rs`, `build_chain` calls `command::decide` — give it `"conform"`, matching the `"conform"` it already passes as the session:

```rust
        command::decide(
            &mut e,
            title,
            "the chosen option",
            "conform",
            None, // status: defaults to accepted
            Some("a stated reason"),
            None, // context
            previous,
            None, // decided_at: defaults to observed_at
            observed_at,
            "conform",
            &embedder,
        )
```

`crates/rm-cli/src/run.rs` and `crates/rm-mcp/src/serve.rs` also call it; pass `"*"` at both for now — **Tasks 4 and 5 replace those with the real argument**, and a temporary universal reach keeps behaviour identical in the meantime.

- [ ] **Step 8: Check `reindex` still round-trips**

Run: `cargo test -p rm-host --all-features a_rebuilt_decision_vector_matches_the_one_decide_wrote`
Expected: PASS. That test is what holds `embed_field` and `plan_reindex` to the same composition; a scope field that reindex could not rebuild would show up here.

- [ ] **Step 9: Workspace, fmt, clippy**

Run: `cargo test --workspace --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean, count at 705 or above.

- [ ] **Step 10: Commit**

```bash
git add crates/rm-host crates/rm-conform crates/rm-cli crates/rm-mcp
git commit -m "A decision states how far it reaches"
```

---

### Task 3: The reads ask from somewhere

**Files:**
- Modify: `crates/rm-host/src/command.rs` — `Found`, `decisions`, `decision`

**Interfaces:**
- Consumes: `rm_host::scope::applies_at`, `held`, `At`
- Produces:
  - `Found::NotHere { title: String, scope: String, asked_from: String }`
  - `pub fn decisions(engine, only: Option<&str>, at: At, here: Option<&str>) -> Result<Outcome, HostError>`
  - `pub fn decision(engine, title: &str, at: At, here: Option<&str>) -> Result<Outcome, HostError>`
  - In both, `here == None` suspends the rule. That is one behaviour serving two callers: `--all`, and anyone who never set `RMEM_SCOPE`.

- [ ] **Step 1: Write the failing test**

Add to `crates/rm-host/src/command.rs`'s test module:

```rust
    /// Three decisions at three reaches, asked from one position.
    #[test]
    fn a_read_returns_what_applies_where_it_is_asked_from() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        let mut t = 1_000;
        for (title, scope) in [
            ("Machine wide", "*"),
            ("Work wide", "work"),
            ("This project", "work/goldenmatch"),
            ("A sibling", "work/other"),
            ("Personal", "personal"),
        ] {
            decide(
                &mut e, title, "a choice", scope, None, None, None, None, None, t, "t", &stub,
            )
            .unwrap();
            t += 10;
        }

        let titles = |here: Option<&str>| {
            let Outcome::Decisions(ds) = decisions(&e, None, At::latest(), here).unwrap() else {
                panic!("decisions did not return decisions")
            };
            let mut out: Vec<String> = ds.into_iter().map(|d| d.title).collect();
            out.sort();
            out
        };

        assert_eq!(
            titles(Some("work/goldenmatch")),
            vec![
                "Machine wide".to_string(),
                "This project".to_string(),
                "Work wide".to_string()
            ],
            "ancestor-or-self, and nothing beside it"
        );
        assert_eq!(
            titles(Some("personal")),
            vec!["Machine wide".to_string(), "Personal".to_string()]
        );
        // No position, no filtering. This is `--all`, and it is also every
        // caller that never set RMEM_SCOPE.
        assert_eq!(titles(None).len(), 5);
    }

    /// An exact title that exists but does not reach here is its own answer,
    /// for the same reason `NotYetRecorded` is: the title resolved, so "no
    /// decision by that title" would read as a spelling mistake.
    #[test]
    fn a_title_out_of_reach_says_where_it_lives() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e,
            "A sibling",
            "a choice",
            "work/other",
            None,
            None,
            None,
            None,
            None,
            1_000,
            "t",
            &stub,
        )
        .unwrap();

        assert_eq!(
            decision(&e, "A sibling", At::latest(), Some("work/goldenmatch")).unwrap(),
            Outcome::Decision(Found::NotHere {
                title: "A sibling".to_string(),
                scope: "work/other".to_string(),
                asked_from: "work/goldenmatch".to_string(),
            })
        );

        // Asked from where it lives, or from nowhere, it is just a decision.
        for here in [Some("work/other"), None] {
            assert!(matches!(
                decision(&e, "A sibling", At::latest(), here).unwrap(),
                Outcome::Decision(Found::Decision(_))
            ));
        }
    }

    /// A decision written before scopes existed carries none, which is not the
    /// same as one that declined to state one -- and new writes cannot decline.
    /// It reaches everywhere, so nothing vanishes the day this ships.
    #[test]
    fn a_decision_with_no_scope_recorded_reaches_everywhere() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e, "Legacy", "a choice", "work", None, None, None, None, None, 1_000, "t", &stub,
        )
        .unwrap();
        let id = find_decision(&e, "Legacy").expect("recorded");
        // Stand in for a pre-scope record by reading at a clock before the
        // scope was written -- the attribute is absent there in exactly the
        // way it is absent from the 219 already in the shared store.
        let before = At {
            valid: Timestamp::MAX,
            tx: 999,
        };
        assert_eq!(held(&e, id, "scope", before), None, "no scope at that clock");
        let Outcome::Decisions(ds) = decisions(&e, None, before, Some("personal")).unwrap() else {
            panic!("decisions did not return decisions")
        };
        assert!(
            ds.is_empty(),
            "nothing was recorded by then, so this proves nothing yet"
        );
    }
```

**Note on that third test:** reading before the write proves the clock filter, not the legacy rule, and its own assertion says so. The legacy rule is exercised properly in Task 6 against the real store, where records genuinely have no `scope` attribute. Keep the test — it pins that `held` returns `None` for an unwritten scope, which is the precondition the rule rests on.

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p rm-host --all-features a_read_returns_what_applies a_title_out_of_reach`
Expected: FAIL — `this function takes 3 arguments but 4 arguments were supplied`.

- [ ] **Step 3: Add the `NotHere` variant**

In the `Found` enum:

```rust
    /// The title resolves, and the decision does not reach where it was asked
    /// from.
    ///
    /// Distinct from `Unknown` for the same reason `NotYetRecorded` is: the
    /// title matched, so "no decision by that title" would read as a spelling
    /// mistake. You named it exactly, so you are told where it lives.
    NotHere {
        title: String,
        /// The reach the decision states.
        scope: String,
        /// The position it was asked from.
        asked_from: String,
    },
```

- [ ] **Step 4: Filter in `decisions`**

Change the signature:

```rust
pub fn decisions(
    engine: &Engine,
    only: Option<&str>,
    at: At,
    here: Option<&str>,
) -> Result<Outcome, HostError> {
```

Inside the entity loop, immediately after the `kind == "decision"` check and before anything expensive:

```rust
        // A decision with no scope recorded reaches everywhere. That is not a
        // default for new writes -- those are refused without one -- it is how
        // records written before scopes existed read, so nothing vanishes.
        if let (Some(here), Some(reach)) = (here, held(engine, id, "scope", at)) {
            if !crate::scope::applies_at(&reach, here) {
                continue;
            }
        }
```

Leave the `superseded_by` edge read exactly as it is. It names the successor whether or not that successor applies here: a line saying a decision is retired while withholding what retired it is the state the rule exists to prevent.

- [ ] **Step 5: Filter in `decision`**

Change the signature:

```rust
pub fn decision(
    engine: &Engine,
    title: &str,
    at: At,
    here: Option<&str>,
) -> Result<Outcome, HostError> {
```

After the `NotYetRecorded` block and before `history` is built:

```rust
    // Existence first, reach second. A decision the store had not heard of yet
    // is a different answer from one it has heard of and that does not apply.
    if let (Some(here), Some(reach)) = (here, held(engine, id, "scope", at)) {
        if !crate::scope::applies_at(&reach, here) {
            return Ok(Outcome::Decision(Found::NotHere {
                title: title.to_string(),
                scope: reach,
                asked_from: here.to_string(),
            }));
        }
    }
```

Both `chain(..)` calls stay unfiltered — see the Global Constraints.

- [ ] **Step 6: Fix every call site**

Run: `rg -n 'decisions\(&?e,|decision\(&e,|command::decisions|command::decision\b' crates/ -g '*.rs'`

Append `, None` to each — no position means no filtering, so every existing test keeps its meaning. `crates/rm-cli/src/run.rs`, `crates/rm-mcp/src/serve.rs` and `crates/rm-conform/src/decisions.rs` get `, None` too; Tasks 4 and 5 replace the first two.

In `crates/rm-cli/src/format.rs` and `crates/rm-mcp/src/render.rs`, add a `Found::NotHere { .. } => String::new()` (respectively a `Rendered` with empty text) arm so the crates compile. **Tasks 4 and 5 replace both placeholders.**

- [ ] **Step 7: Run the workspace**

Run: `cargo test --workspace --all-features`
Expected: PASS, count at 708 or above.

- [ ] **Step 8: fmt and clippy**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add crates/
git commit -m "What applies here, and what lives somewhere else"
```

---

### Task 4: The command line

**Files:**
- Modify: `crates/rm-cli/src/main.rs` — a `SCOPE_ENV` constant beside `CONFIG_ENV` (`:20`)
- Modify: `crates/rm-cli/src/args.rs` — `--scope` on `decide`/`decisions`/`decision`, `--all` on the two reads
- Modify: `crates/rm-cli/src/run.rs` — pass scope and position
- Modify: `crates/rm-cli/src/format.rs` — render `NotHere`

**Interfaces:**
- Consumes: `Found::NotHere`, the new `decide`/`decisions`/`decision` signatures
- Produces:
  - `Command::Decide { .., scope: String, .. }` — required, so `String` not `Option`
  - `Command::Decisions { status, valid_at, as_of, scope: Option<String>, all: bool }`
  - `Command::Decision { title, valid_at, as_of, scope: Option<String>, all: bool }`
  - `pub const SCOPE_ENV: &str = "RMEM_SCOPE";`

- [ ] **Step 1: Write the failing parse test**

Add to `crates/rm-cli/src/args.rs`'s test module:

```rust
    #[test]
    fn decide_requires_a_scope_and_the_reads_take_a_position() {
        let Command::Decide { scope, .. } = parse_args(&[
            "decide",
            "Pin the compiler",
            "rust-toolchain.toml names the version",
            "--scope",
            "work/goldenmatch",
        ])
        .unwrap() else {
            panic!("not a decide command")
        };
        assert_eq!(scope, "work/goldenmatch");

        let e = parse_args(&["decide", "A title", "A choice"]).unwrap_err();
        assert!(
            format!("{e}").contains("--scope"),
            "the refusal should name the flag: {e}"
        );

        let Command::Decisions { scope, all, .. } =
            parse_args(&["decisions", "--scope", "personal"]).unwrap()
        else {
            panic!("not a decisions command")
        };
        assert_eq!(scope.as_deref(), Some("personal"));
        assert!(!all);

        let Command::Decision { all, scope, .. } =
            parse_args(&["decision", "A title", "--all"]).unwrap()
        else {
            panic!("not a decision command")
        };
        assert!(all, "--all suspends the rule");
        assert_eq!(scope, None);
    }
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p rm-cli --all-features decide_requires_a_scope`
Expected: FAIL — the variants have no such fields.

- [ ] **Step 3: Add the fields and parse them**

Add to `Command::Decide`:

```rust
        /// How far this decision reaches. Required: reach varies per decision,
        /// so no session default can be right.
        scope: String,
```

Add to both `Command::Decisions` and `Command::Decision`:

```rust
        /// Ask from this position instead of `RMEM_SCOPE`.
        scope: Option<String>,
        /// Suspend the applicability rule and show everything.
        all: bool,
```

In the `"decide"` arm, using the existing `flag` helper:

```rust
            let Some(scope) = flag(&args, "--scope")? else {
                return Err(CliError::Usage(format!(
                    "decide needs --scope: how far this decision reaches. {UNIVERSAL:?} for everywhere, or a path like \"work/goldenmatch\"\n\n{USAGE}"
                )));
            };
```

with `use rm_host::scope::UNIVERSAL;` at the top of the file.

In the two read arms:

```rust
            scope: flag(&args, "--scope")?,
            all: args.iter().any(|a| a == "--all"),
```

Update the three usage lines near `:43-56` to show the new flags, and add `scope`/`all` to every existing test that constructs these variants literally.

- [ ] **Step 4: Run the parse test**

Run: `cargo test -p rm-cli --all-features decide_requires_a_scope`
Expected: PASS.

- [ ] **Step 5: Read `RMEM_SCOPE` and pass the position**

In `crates/rm-cli/src/main.rs`, beside `CONFIG_ENV`:

```rust
/// Where this session stands, for deciding what applies to it.
///
/// Read-side only. It is never a write default: reach varies per decision, and
/// the caller is the only one who knows it.
const SCOPE_ENV: &str = "RMEM_SCOPE";
```

Read it once in `main` and pass it into `run` beside `now`, so nothing below reads the environment on its own — the same discipline the clock already follows. `run`'s signature gains a parameter:

```rust
pub fn run(
    args: impl Iterator<Item = String>,
    config: &Path,
    now: Timestamp,
    session_scope: Option<String>,
) -> Result<Outcome, CliError>
```

Every existing test caller of `run` passes `None`, which is no position and therefore no filtering — exactly today's behaviour.

In `crates/rm-cli/src/run.rs`, at the two read arms, compute the position:

```rust
                // `--all` beats `--scope`, which beats the environment. None
                // means no position, which suspends the rule.
                let here = if all { None } else { scope.or(session_scope.clone()) };
```

and pass `here.as_deref()` to `command::decisions` / `command::decision`. At the `decide` arm, pass `&scope` — the required one — in place of the temporary `"*"` from Task 2.

- [ ] **Step 6: Render `NotHere`**

Replace the placeholder arm in `crates/rm-cli/src/format.rs`:

```rust
        Outcome::Decision(Found::NotHere {
            title,
            scope,
            asked_from,
        }) => format!(
            "{title:?} is on record, but it does not apply here.\n\n  \
             it reaches   {scope}\n  you asked from {asked_from}\n\n\
             Use --scope {scope} to ask from there, or --all to ignore reach.\n"
        ),
```

- [ ] **Step 7: Write the failing render test**

Add to `crates/rm-cli/src/format.rs`'s test module:

```rust
    #[test]
    fn a_decision_out_of_reach_names_both_places() {
        let out = render(&Outcome::Decision(Found::NotHere {
            title: "A sibling".into(),
            scope: "work/other".into(),
            asked_from: "work/goldenmatch".into(),
        }));
        assert!(out.contains("work/other"), "{out}");
        assert!(out.contains("work/goldenmatch"), "{out}");
        assert!(
            !out.contains("no decision by that title"),
            "must not read as a typo: {out}"
        );
    }
```

- [ ] **Step 8: Run the crate's tests**

Run: `cargo test -p rm-cli --all-features`
Expected: PASS.

- [ ] **Step 9: Exercise it against the live store, read-only**

Run:
```bash
RMEM_CONFIG=D:/memory/rmem.toml cargo run -q -p rm-cli -- decisions | grep -c '^[ ~] entity '
RMEM_SCOPE=personal RMEM_CONFIG=D:/memory/rmem.toml cargo run -q -p rm-cli -- decisions | grep -c '^[ ~] entity '
```
Expected: **219 both times.** Every record there predates scopes, so all of them reach everywhere and a position changes nothing. That is the migration promise, checked rather than asserted. A number below 219 on the second line means the legacy rule is wrong — stop and fix it before going on.

- [ ] **Step 10: fmt, clippy, workspace**

Run: `cargo test --workspace --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean, count at 710 or above.

- [ ] **Step 11: Commit**

```bash
git add crates/rm-cli
git commit -m "Where this session stands, and where a decision does not reach"
```

---

### Task 5: The same, for agents

**Files:**
- Modify: `crates/rm-mcp/src/tools.rs` — `SCOPE_ENV`, `decide`'s schema and `required`, the two read schemas, the `Call` variants and parse arms
- Modify: `crates/rm-mcp/src/serve.rs` — thread scope and position
- Modify: `crates/rm-mcp/src/render.rs` — render `NotHere`

**Interfaces:**
- Consumes: everything from Tasks 1–3
- Produces:
  - `pub const SCOPE_ENV: &str = "RMEM_SCOPE";` in `tools.rs`, beside `TOOLS_ENV` (`:38`)
  - `Call::Decide { .., scope: String, .. }`
  - `Call::Decisions { status, valid_at, as_of, scope: Option<String>, all: bool }`
  - `Call::Decision { title, valid_at, as_of, scope: Option<String>, all: bool }`

- [ ] **Step 1: Write the failing test**

Add to `crates/rm-mcp/src/tools.rs`'s test module:

```rust
    #[test]
    fn an_agent_cannot_record_a_decision_without_stating_its_reach() {
        assert!(
            read("decide", json!({"title": "A title", "choice": "A choice"})).is_err(),
            "scope is required"
        );

        let Call::Decide { scope, .. } = read(
            "decide",
            json!({"title": "A title", "choice": "A choice", "scope": "work/goldenmatch"}),
        )
        .unwrap() else {
            panic!("not a decide call")
        };
        assert_eq!(scope, "work/goldenmatch");
    }

    /// The schema has to say it too, or a model never learns the argument
    /// exists and every call fails at the parse instead.
    #[test]
    fn the_decide_schema_marks_scope_required() {
        let decide = all_definitions()
            .into_iter()
            .find(|t| t["name"] == "decide")
            .expect("decide is defined");
        let required = decide["inputSchema"]["required"]
            .as_array()
            .expect("a required list");
        assert!(
            required.iter().any(|v| v == "scope"),
            "scope must be required: {required:?}"
        );
        assert!(decide["inputSchema"]["properties"]["scope"].is_object());
    }

    #[test]
    fn the_reads_take_a_position_and_a_way_to_ignore_it() {
        let Call::Decisions { scope, all, .. } =
            read("decisions", json!({"scope": "personal"})).unwrap()
        else {
            panic!("not a decisions call")
        };
        assert_eq!(scope.as_deref(), Some("personal"));
        assert!(!all);

        let Call::Decision { all, .. } =
            read("decision", json!({"title": "A title", "all": true})).unwrap()
        else {
            panic!("not a decision call")
        };
        assert!(all);
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p rm-mcp --all-features an_agent_cannot_record the_decide_schema_marks the_reads_take_a_position`
Expected: FAIL — the variants have no such fields.

- [ ] **Step 3: Add the constant**

In `crates/rm-mcp/src/tools.rs`, beside `TOOLS_ENV`:

```rust
/// Where this session stands, for deciding what applies to it.
///
/// Read-side only, and deliberately: reach varies per decision, so a session
/// value would answer -- silently and usually wrongly -- the one question the
/// writer is uniquely placed to answer.
pub const SCOPE_ENV: &str = "RMEM_SCOPE";
```

- [ ] **Step 4: Add `scope` to the `decide` schema**

In the `decide` tool's `properties`, after `"choice"`:

```json
                    "scope": {
                        "type": "string",
                        "description": "How far this decision reaches, as a path from broad to narrow -- \"work/goldenmatch/fs\". A decision that applies to every project regardless of what you are working on is \"*\". Ask yourself where this would still be true, not where you happened to learn it: a rule about this machine is \"*\" even if you found it in one project."
                    },
```

and change `"required"`:

```json
                "required": ["title", "choice", "scope"],
```

- [ ] **Step 5: Add both properties to the two read schemas**

In `decisions` and again in `decision`:

```json
                    "scope": {
                        "type": "string",
                        "description": "Ask from this position instead of the session's own. A decision applies here when its reach covers this path."
                    },
                    "all": {
                        "type": "boolean",
                        "description": "Ignore reach and show everything, including decisions scoped elsewhere. Omit for what applies here."
                    }
```

- [ ] **Step 6: Add the fields, parse them, and dispatch**

`Call::Decide` gains `scope: String`; both read variants gain `scope: Option<String>` and `all: bool`.

```rust
            "decide" => Ok(Call::Decide {
                title: string(arguments, "title")?,
                choice: string(arguments, "choice")?,
                scope: string(arguments, "scope")?,
```

and in each read arm:

```rust
                scope: optional_string(arguments, "scope")?,
                all: optional_bool(arguments, "all")?.unwrap_or(false),
```

If `optional_bool` does not exist in `tools.rs`, add it beside `optional_string`, mirroring its shape exactly:

```rust
fn optional_bool(args: &Value, field: &str) -> Result<Option<bool>, Unreadable> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("{field} must be true or false")),
    }
}
```

In `crates/rm-mcp/src/serve.rs`, pass `scope` to `command::plan_decide` in place of the temporary `"*"` from Task 2, and compute the position at both read arms.

Read at the point of use rather than threaded from startup, which is not sloppiness: `tools::definitions()` already reads `TOOLS_ENV` the same way, at call time. The CLI threads its value from `main` because it also threads the clock; this binary has no such spine to hang it on, and one long-lived process's environment does not change under it.

```rust
                let here = if all {
                    None
                } else {
                    scope.clone().or_else(|| std::env::var(SCOPE_ENV).ok())
                };
```

- [ ] **Step 7: Render `NotHere`**

Replace the placeholder arm in `crates/rm-mcp/src/render.rs`:

```rust
        Outcome::Decision(Found::NotHere {
            title,
            scope,
            asked_from,
        }) => Rendered {
            text: format!(
                "{title:?} is on record, but it does not apply here. It reaches {scope:?} \
                 and you asked from {asked_from:?}. Pass scope={scope:?} to ask from there, \
                 or all=true to ignore reach."
            ),
            structured: json!({
                "found": true,
                "applies_here": false,
                "scope": scope,
                "asked_from": asked_from,
            }),
        },
```

`{"found": true, "applies_here": false}` rather than `{"found": false}`: the title is real, and telling a model otherwise is telling it something untrue.

- [ ] **Step 8: Run the crate's tests**

Run: `cargo test -p rm-mcp --all-features`
Expected: PASS. Tool-listing snapshot tests near `:520` and `:824` may need the new properties; update them to match.

- [ ] **Step 9: Measure what the schema now costs**

```bash
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"m","version":"1"}}}\n{"jsonrpc":"2.0","id":2,"method":"tools/list"}\n' > /tmp/req.jsonl
RMEM_CONFIG=D:/memory/rmem.toml RMEM_TOOLS=decide,decisions,decision \
  cargo run -q -p rm-mcp --bin rmem-mcp < /tmp/req.jsonl 2>/dev/null | tail -1 | wc -c
```

The previous change measured this listing at **3,868 bytes**, and the README's own figures put a token at almost exactly 4.01 bytes. Record the new number in the commit message. If the increase exceeds ~800 bytes, trim the descriptions rather than absorbing it quietly — that is what the last change did when its first draft came in over budget.

- [ ] **Step 10: fmt, clippy, workspace**

Run: `cargo test --workspace --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean, count at 713 or above.

- [ ] **Step 11: Commit**

```bash
git add crates/rm-mcp
git commit -m "An agent says how far what it learned reaches"
```

---

### Task 6: The callers that break, and saying so

**Files:**
- Modify: `docs/seed-decision-log.sh` — the `d()` wrapper at `:32`
- Modify: `README.md` — the decision-log section, the shared-store example, the `RMEM_TOOLS` table

**Interfaces:**
- Consumes: everything above. Produces nothing code depends on.

- [ ] **Step 1: Scope the seed log**

`docs/seed-decision-log.sh` calls `decide` 35 times through one wrapper at `:32`:

```sh
d() { "$R" decide "$@" >/dev/null; printf '.'; }
```

A required `--scope` breaks all 35. Fix the wrapper rather than 35 call sites, and let the environment override it so the script can seed a store under any position:

```sh
# Every decision here is about this project, so they share a reach. Overridable
# because the same script seeds demo stores that may sit somewhere else.
SEED_SCOPE="${SEED_SCOPE:-rusty-memory}"
d() { "$R" decide --scope "$SEED_SCOPE" "$@" >/dev/null; printf '.'; }
```

Preserve the file's existing line endings when editing.

- [ ] **Step 2: Check the script still parses**

Run: `shellcheck docs/seed-decision-log.sh`
Expected: no new findings relative to before the edit. Run it on the committed version first if you want the baseline.

- [ ] **Step 3: Run it against a throwaway store**

```bash
cargo build -p rm-cli
RMEM=$(pwd)/target/debug/rmem
TMP=$(mktemp -d)
( cd "$TMP" && "$RMEM" init --embedder local )
( cd "$TMP" && RMEM_CONFIG="$TMP/rmem.toml" bash "$OLDPWD/docs/seed-decision-log.sh" )
RMEM_CONFIG="$TMP/rmem.toml" "$RMEM" decisions | grep -c '^[ ~] entity '
RMEM_SCOPE=somewhere/else RMEM_CONFIG="$TMP/rmem.toml" "$RMEM" decisions | grep -c '^[ ~] entity '
rm -rf "$TMP"
```

Expected: the script prints its dots and refuses nothing; the first count is the number of decisions it seeded; the second is **0**, because they are all scoped `rusty-memory` and `somewhere/else` is not below it. That second number is the first end-to-end proof the rule actually filters — every earlier check was either a unit test or a store whose records predate scopes.

`--embedder local` so it opens no socket and needs no key. Check the seed script's own variable for the binary path (`$R` at the top) and set it if it does not pick up `$RMEM`.

- [ ] **Step 4: Document the model in the README**

In the decision-log section, after the paragraph introducing `decide`:

```markdown
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
rmem decide "Route scorers by class" "dispatch on the class, not the mass" \
  --scope work/goldenmatch/fs

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

Reach is about relevance, not permission. `--all` shows everything; none of this
is a boundary.
```

- [ ] **Step 5: Add `RMEM_SCOPE` to the shared-store example**

The README's `"rmem"` MCP configuration block lists `RMEM_CONFIG` and `RMEM_TOOLS`. Add the third:

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

and a sentence: `RMEM_SCOPE` is what makes one shared store readable by many projects — without it every session sees every decision, which is the state that made the flat log unusable at 219.

- [ ] **Step 6: Update the token table**

Task 5 Step 9 measured the new `decide,decisions,decision` listing. The table currently reads `~1,850 / ~1,210 / ~960 / ~510`. All four rows contain `decisions` and `decision`, and three contain `decide`, so recompute each from a measurement rather than scaling one row:

```bash
for cfg in "" "decide,decisions,decision,recall" "decide,decisions,decision" "decisions,decision"; do
  n=$(RMEM_CONFIG=D:/memory/rmem.toml RMEM_TOOLS="$cfg" \
      cargo run -q -p rm-mcp --bin rmem-mcp < /tmp/req.jsonl 2>/dev/null | tail -1 | wc -c)
  echo "${cfg:-ALL} $n bytes ~$((n * 100 / 401)) tokens"
done
```

- [ ] **Step 7: Spellcheck**

Run: `typos README.md docs/seed-decision-log.sh`
Expected: clean.

- [ ] **Step 8: Final full verification**

Run: `cargo test --workspace --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: all clean.

- [ ] **Step 9: Commit**

```bash
git add README.md docs/seed-decision-log.sh
git commit -m "Where a decision would still be true"
```

---

## What this does not do

Carried from the spec so it is not lost between documents:

- **`recall` and `about` stay unscoped.** `recall` is the interesting case and a different axis: a similarity search that also filters by applicability is a retrieval-quality question, and folding it in would put scoring back on the table.
- **No backfill of the 219.** Guessing reach from a title prefix is the origin model wearing the reach model's clothes — it would tag the machine-wide rules `goldenmatch` because that is where they were written, which is the exact error this design exists to avoid.
- **No access control.** Reach is relevance. Everything stays readable with `--all`, and nothing here is a security boundary.
