# Two Clocks on the Decision Reads — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `rmem decisions`, `rmem decision "<title>"` and the MCP tools of the same names both time axes, so the decision log — the only surface this store has a real user for — can be asked what it said at a past instant.

**Architecture:** A single `At { valid, tx }` value threads from the CLI and MCP argument parsers down through `command::decisions` / `command::decision` / `chain` into the edge reads that already take both clocks and currently pin them to `Timestamp::MAX`. The attribute reads, which today bypass both clocks *and* survivorship, route through one new `visible()` helper that filters the raw version log by both axes. Valid time is cut natively over the version log rather than through `about_under`, so nothing here depends on policy configuration.

**Tech Stack:** Rust (pinned in `rust-toolchain.toml`), no new dependencies. `serde_json` in `rm-mcp` only, already present.

**Spec:** `docs/superpowers/specs/2026-08-25-decision-log-clocks-design.md`

## Global Constraints

- **No new dependencies.** Every library crate's third-party deps come from `serde` and `serde_json` alone; `rm-host` adds `toml`, `rm-providers` adds `ureq`. Nothing here changes that.
- **Every test runs offline.** No socket, no spawned process, no API key.
- **CI commands, spelled as CI spells them:** `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features`.
- **Clock argument order is `(valid_t, tx_t)`** everywhere in `rm-engine` — `about`, `about_under`, `edges_from`, `edges_into`. `At`'s fields are named so this cannot be got wrong, but any bare call must keep that order.
- **`At::latest()` is `MAX`/`MAX`, never `now`.** Using `now` would silently drop a decision recorded with a future `--at`, which is a behaviour change nobody asked for.
- **Baseline before starting:** 692 tests pass on `main` (`cargo test --workspace --all-features`). Every task must leave that number at or above where it started.
- **Commit style:** a title line that says what changed in plain words, a body explaining why. No conventional-commit prefixes — see `git log --oneline` for the house style.

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/rm-host/src/time.rs` | Day parsing and formatting; now also the two-clock pair | Add `At` |
| `crates/rm-host/src/command.rs` | The decision reads | Add `visible`/`held`, `Found`; `decisions`/`decision`/`chain` take `At` |
| `crates/rm-cli/src/args.rs` | Flag parsing | `--as-of` / `--valid-at` on two commands |
| `crates/rm-cli/src/run.rs` | Command dispatch | Build `At`, pass it |
| `crates/rm-cli/src/format.rs` | Rendering | `Found` arms; time-relative wording |
| `crates/rm-mcp/src/tools.rs` | Tool schemas and argument parsing | Two optional params on two tools; `optional_instant` |
| `crates/rm-mcp/src/serve.rs` | Dispatch to `command::` | Pass `At` |
| `crates/rm-conform/src/decisions.rs` | Measures the decision layer | `time_coverage` becomes a measurement |
| `README.md` | The decision-log section | Document the flags |

---

### Task 1: `At`, and one read that honours it

**Files:**
- Modify: `crates/rm-host/src/time.rs` (append `At` and its tests)
- Modify: `crates/rm-host/src/command.rs` (add `visible` and `held` beside the existing decision code, around `:788`)

**Interfaces:**
- Consumes: `rm_engine::Timestamp`, `rm_engine::Engine::store_history`, `rm_store::Version`
- Produces:
  - `rm_host::time::At { pub valid: Timestamp, pub tx: Timestamp }`, `At::latest() -> At`, deriving `Clone, Copy, Debug, PartialEq, Eq`
  - `fn visible<'a>(engine: &'a Engine, id: StableId, attr: &str, at: At) -> Vec<&'a rm_store::Version>` — private to `command.rs`
  - `fn held(engine: &Engine, id: StableId, attr: &str, at: At) -> Option<String>` — private to `command.rs`

- [ ] **Step 1: Write the failing test for `At`**

Append to the `tests` module in `crates/rm-host/src/time.rs`:

```rust
    #[test]
    fn latest_is_the_end_of_both_axes_and_not_the_current_time() {
        let at = At::latest();
        assert_eq!(at.valid, Timestamp::MAX);
        assert_eq!(at.tx, Timestamp::MAX);
    }
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p rm-host --all-features latest_is_the_end_of_both_axes`
Expected: FAIL — `cannot find type/value 'At' in this scope`.

- [ ] **Step 3: Implement `At`**

Add to `crates/rm-host/src/time.rs`, above the `tests` module:

```rust
/// The two clocks a read is answered under: what held at `valid`, as known by
/// `tx`.
///
/// One value rather than two parameters because both are `Timestamp` and they
/// pass through three layers -- `decisions` to `chain` to `edges_into` -- where
/// swapping them compiles and returns a plausible wrong answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct At {
    /// When in the world. Filters on `Version::valid.from`.
    pub valid: Timestamp,
    /// When the store learned it. Filters on `provenance.observed_at`.
    pub tx: Timestamp,
}

impl At {
    /// Everything the store holds.
    ///
    /// Deliberately not a `Default` impl. `Engine::edges_from` makes the
    /// argument: "an edge read without a `tx_t` is a claim about now that
    /// quietly stops being reproducible" -- so every call site names what it is
    /// asking rather than inheriting it.
    ///
    /// `MAX` rather than the current time, because that is what the decision
    /// reads did before they took an `At`. `now` would silently drop a decision
    /// recorded with a future `--at`.
    pub fn latest() -> Self {
        At {
            valid: Timestamp::MAX,
            tx: Timestamp::MAX,
        }
    }
}
```

- [ ] **Step 4: Run it to make sure it passes**

Run: `cargo test -p rm-host --all-features latest_is_the_end_of_both_axes`
Expected: PASS.

- [ ] **Step 5: Write the failing tests for `visible` and `held`**

Add to the `tests` module in `crates/rm-host/src/command.rs`, in the `---- decisions ----` section:

```rust
    /// Both axes bite, and a tombstone is an answer rather than something to
    /// skip past.
    #[test]
    fn a_visible_version_is_one_both_clocks_admit() {
        const MARCH: Timestamp = 1_772_236_800_000; // 2026-02-28
        const AUGUST: Timestamp = 1_787_532_411_419; // 2026-08-24
        let mut e = engine();
        let stub = StubProvider::new(vec![]);

        // Decided in March, recorded in March.
        decide(
            &mut e, "Pin the compiler", "first choice", None, None, None, None,
            Some(MARCH), MARCH, "t", &stub,
        )
        .unwrap();
        // Re-decided in August under the same title.
        decide(
            &mut e, "Pin the compiler", "second choice", None, None, None, None,
            Some(AUGUST), AUGUST, "t", &stub,
        )
        .unwrap();

        let id = find_decision(&e, "Pin the compiler").expect("recorded");

        // As of March the store had heard only the first.
        assert_eq!(
            held(&e, id, "choice", At { valid: Timestamp::MAX, tx: MARCH }),
            Some("first choice".to_string())
        );
        // As of now it has both, and the later one is the answer.
        assert_eq!(
            held(&e, id, "choice", At::latest()),
            Some("second choice".to_string())
        );
        // Valid time alone: in March the second had not begun to hold.
        assert_eq!(
            held(&e, id, "choice", At { valid: MARCH, tx: Timestamp::MAX }),
            Some("first choice".to_string())
        );
        // Before either clock, nothing at all.
        assert_eq!(held(&e, id, "choice", At { valid: 1, tx: 1 }), None);
        assert!(visible(&e, id, "choice", At { valid: 1, tx: 1 }).is_empty());
        assert_eq!(visible(&e, id, "choice", At::latest()).len(), 2);
    }
```

- [ ] **Step 6: Run them to make sure they fail**

Run: `cargo test -p rm-host --all-features a_visible_version_is_one_both_clocks_admit`
Expected: FAIL — `cannot find function 'held'`.

- [ ] **Step 7: Implement `visible` and `held`**

Add to `crates/rm-host/src/command.rs`, immediately above `pub fn decisions`:

```rust
/// The versions of one attribute both clocks admit, oldest first.
///
/// The raw version log filtered rather than `Engine::about`, deliberately. A
/// decision's timeline is built here from the versions themselves, so a
/// valid-time question is answered without a survivorship strategy -- and
/// therefore without depending on `[policy]`, where the shipped default is
/// `most_recent` and a valid time has nothing to index into.
fn visible<'a>(
    engine: &'a Engine,
    id: StableId,
    attr: &str,
    at: At,
) -> Vec<&'a rm_store::Version> {
    engine
        .store_history(id, attr)
        .iter()
        .filter(|v| v.provenance.observed_at <= at.tx && v.valid.from <= at.valid)
        .collect()
}

/// The value standing at `at`, or `None` if there is none.
///
/// Replaces two `latest()` closures that disagreed. `decisions` read the last
/// *non-tombstone* version and `decision` read the last version and then its
/// value, so a tombstoned `choice` showed the old choice in the list and an
/// empty one in the detail. This is `decision`'s reading: a tombstone asserts
/// the attribute has no value, and stepping past it to report a superseded one
/// would answer with something the store has been told is no longer true.
fn held(engine: &Engine, id: StableId, attr: &str, at: At) -> Option<String> {
    visible(engine, id, attr, at)
        .last()
        .and_then(|v| v.value.clone())
}
```

Add `use crate::time::At;` to the imports at the top of `command.rs` if it is not already reachable.

- [ ] **Step 8: Run them to make sure they pass**

Run: `cargo test -p rm-host --all-features a_visible_version_is_one_both_clocks_admit`
Expected: PASS.

- [ ] **Step 9: Full crate, fmt and clippy**

Run: `cargo test -p rm-host --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: all pass. `visible` and `held` are not yet called by anything, so clippy may warn `dead_code` — if it does, that is expected and is resolved by Task 2; add `#[allow(dead_code)]` on both **and remove it in Task 2 Step 7**.

- [ ] **Step 10: Commit**

```bash
git add crates/rm-host/src/time.rs crates/rm-host/src/command.rs
git commit -m "A pair of clocks, and one read that honours them"
```

---

### Task 2: `decisions` answers at a time

**Files:**
- Modify: `crates/rm-host/src/command.rs:787-851` (`decisions`) and every test call site of `decisions(`

**Interfaces:**
- Consumes: `At`, `visible`, `held` from Task 1
- Produces: `pub fn decisions(engine: &Engine, only: Option<&str>, at: At) -> Result<Outcome, HostError>`

- [ ] **Step 1: Write the failing test**

Add to the `---- decisions ----` section of `crates/rm-host/src/command.rs`:

```rust
    /// A decision the store had not yet heard of is not in the list, and the
    /// count of revisions is the count it had then.
    #[test]
    fn the_list_is_answered_as_of_a_transaction_time() {
        const MARCH: Timestamp = 1_772_236_800_000;
        const AUGUST: Timestamp = 1_787_532_411_419;
        let mut e = engine();
        let stub = StubProvider::new(vec![]);

        decide(&mut e, "Early", "chosen in March", None, None, None, None,
               Some(MARCH), MARCH, "t", &stub).unwrap();
        decide(&mut e, "Late", "chosen in August", None, None, None, None,
               Some(AUGUST), AUGUST, "t", &stub).unwrap();
        decide(&mut e, "Early", "revised in August", None, None, None, None,
               Some(AUGUST), AUGUST, "t", &stub).unwrap();

        let titles = |at: At| {
            let Outcome::Decisions(ds) = decisions(&e, None, at).unwrap() else {
                panic!("decisions did not return decisions")
            };
            ds.into_iter()
                .map(|d| (d.title, d.choice, d.revisions))
                .collect::<Vec<_>>()
        };

        assert_eq!(
            titles(At { valid: Timestamp::MAX, tx: MARCH }),
            vec![("Early".to_string(), "chosen in March".to_string(), 1)],
            "August had not happened yet"
        );

        let now = titles(At::latest());
        assert_eq!(now.len(), 2, "both decisions exist now");
        let early = now.iter().find(|(t, ..)| t == "Early").unwrap();
        assert_eq!(early.1, "revised in August");
        assert_eq!(early.2, 2, "revised once, so two choices");
    }
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p rm-host --all-features the_list_is_answered_as_of_a_transaction_time`
Expected: FAIL — `this function takes 2 arguments but 3 arguments were supplied`.

- [ ] **Step 3: Change the signature and thread `at` through**

In `crates/rm-host/src/command.rs`, change the signature at `:788`:

```rust
pub fn decisions(engine: &Engine, only: Option<&str>, at: At) -> Result<Outcome, HostError> {
```

Delete the local `latest` closure (`:811-818`). Replace the three reads that used it, the edge read, and the revision count:

```rust
        let Some(choice) = held(engine, id, "choice", at) else {
            continue;
        };
        let status = held(engine, id, "status", at).unwrap_or_else(|| DEFAULT_STATUS.into());
        // Read once: it decides the mark and it is shown on the line.
        let superseded_by = engine
            .edges_into(id, at.valid, at.tx)
            .iter()
            .find(|e| e.predicate == SUPERSEDES)
            .map(|e| (e.subject, title_of(engine, e.subject)));
```

and, in the `DecisionLine` construction:

```rust
            revisions: visible(engine, id, "choice", at).len(),
```

and:

```rust
            because: held(engine, id, "because", at),
```

An entity whose `choice` is not visible at `at` is skipped by the existing `else { continue }`, which is exactly the "absent from the list" behaviour the spec calls for — no extra branch.

- [ ] **Step 4: Update every existing call site**

Run: `rg -n 'decisions\((&?mut )?e,' crates/rm-host/src/command.rs`

Every one is a test. Append `, At::latest()` to each — behaviour is unchanged by construction. The known sites are `:1458`, `:1738`, `:1987`. Do the same for the `recorded` helper at `:1457`.

- [ ] **Step 5: Run the crate's tests**

Run: `cargo test -p rm-host --all-features`
Expected: PASS, including the new test.

- [ ] **Step 6: Fix the other crates that call it**

Run: `cargo build --workspace --all-features 2>&1 | rg 'error\[E0061\]' -A3`

Add `, rm_host::time::At::latest()` at each site. Expect `crates/rm-cli/src/run.rs` and `crates/rm-mcp/src/serve.rs:543`; Tasks 4 and 5 replace those with real values.

- [ ] **Step 7: Remove the `#[allow(dead_code)]` if Task 1 added them**

`visible` and `held` now have callers.

- [ ] **Step 8: Whole workspace, fmt and clippy**

Run: `cargo test --workspace --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: all pass, test count at 693 or above.

- [ ] **Step 9: Commit**

```bash
git add crates/rm-host crates/rm-cli crates/rm-mcp
git commit -m "The list, answered as of a time"
```

---

### Task 3: `decision` gains a third answer

**Files:**
- Modify: `crates/rm-host/src/command.rs` — the `Outcome` enum's `Decision` variant, `decision` (`:967-1000`), `chain` (`:1021-1042`)
- Modify: `crates/rm-conform/src/decisions.rs:65-70` (`detail`)
- Modify: `crates/rm-cli/src/format.rs:197-200` (compile only; wording is Task 4)

**Interfaces:**
- Consumes: `At`, `visible`, `held` from Task 1
- Produces:
  - `pub enum Found { Decision(Box<DecisionDetail>), NotYetRecorded { title: String, first_recorded: Timestamp }, Unknown }` deriving `Debug, PartialEq`
  - `Outcome::Decision(Found)` — replaces `Outcome::Decision(Option<Box<DecisionDetail>>)`
  - `pub fn decision(engine: &Engine, title: &str, at: At) -> Result<Outcome, HostError>`

- [ ] **Step 1: Write the failing test**

Add to `crates/rm-host/src/command.rs`'s test module:

```rust
    /// Three answers, not two. A title that resolves but was recorded later is
    /// its own case: reporting "no such decision" would read as a typo.
    #[test]
    fn a_decision_not_yet_recorded_is_distinguished_from_one_that_does_not_exist() {
        const MARCH: Timestamp = 1_772_236_800_000;
        const AUGUST: Timestamp = 1_787_532_411_419;
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(&mut e, "Pin the compiler", "a choice", None, None, None, None,
               Some(MARCH), AUGUST, "t", &stub).unwrap();

        // Backdated to March but recorded in August: as of March the store
        // knew nothing, even though the decision claims to have held then.
        let Outcome::Decision(found) =
            decision(&e, "Pin the compiler", At { valid: Timestamp::MAX, tx: MARCH }).unwrap()
        else {
            panic!("not a decision outcome")
        };
        assert_eq!(
            found,
            Found::NotYetRecorded {
                title: "Pin the compiler".to_string(),
                first_recorded: AUGUST,
                first_held: MARCH,
            },
            "both clocks are reported: it was typed up in August and claims March"
        );

        // The other axis excludes it too, and for a different reason. Asking
        // what held in January, with everything the store knows, still finds
        // nothing -- and the two days below are what tell those cases apart.
        assert!(matches!(
            decision(&e, "Pin the compiler", At { valid: 1, tx: Timestamp::MAX }).unwrap(),
            Outcome::Decision(Found::NotYetRecorded { .. })
        ));

        // A title nobody ever used is a different answer.
        assert_eq!(
            decision(&e, "Never decided", At::latest()).unwrap(),
            Outcome::Decision(Found::Unknown)
        );

        // And now, it is there.
        let Outcome::Decision(Found::Decision(d)) =
            decision(&e, "Pin the compiler", At::latest()).unwrap()
        else {
            panic!("expected a decision")
        };
        assert_eq!(d.choice, "a choice");
    }

    /// The chain is walked at the clock too: a supersession recorded later
    /// does not retire a decision in the past.
    #[test]
    fn a_later_supersession_does_not_reach_back() {
        const MARCH: Timestamp = 1_772_236_800_000;
        const AUGUST: Timestamp = 1_787_532_411_419;
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(&mut e, "First", "the old way", None, None, None, None,
               None, MARCH, "t", &stub).unwrap();
        decide(&mut e, "Second", "the new way", None, None, None, Some("First"),
               None, AUGUST, "t", &stub).unwrap();

        let stands = |at: At| {
            let Outcome::Decision(Found::Decision(d)) = decision(&e, "First", at).unwrap() else {
                panic!("expected a decision")
            };
            (d.still_stands, d.superseded_by.len())
        };

        assert_eq!(
            stands(At { valid: Timestamp::MAX, tx: MARCH }),
            (true, 0),
            "in March nothing had replaced it"
        );
        assert_eq!(stands(At::latest()), (false, 1), "August replaced it");
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p rm-host --all-features a_decision_not_yet_recorded a_later_supersession_does_not_reach_back`
Expected: FAIL — `cannot find type 'Found'`.

- [ ] **Step 3: Add the `Found` enum**

Add to `crates/rm-host/src/command.rs`, immediately above `pub struct DecisionDetail`:

```rust
/// What looking for one decision found.
///
/// Three answers rather than two, because the store holds the difference and
/// collapsing it loses information a reader needs. `find_decision` matches on
/// the identity record's `name`, which is not versioned, so a decision recorded
/// after `at.tx` still resolves by title -- and answering "no such decision"
/// for it would read as a spelling mistake and send the reader looking for one.
///
/// The same distinction `Believed` draws between `Absent` ("someone said there
/// is none") and `Unknown` ("it has never come up").
#[derive(Debug, PartialEq)]
pub enum Found {
    /// The decision, as it stood at the time asked about.
    Decision(Box<DecisionDetail>),
    /// The title resolves, but nothing of it stood at `at`.
    ///
    /// Both days are carried because either clock can be the one that excluded
    /// it and they are not the same question. A decision backdated to March and
    /// typed up in August is invisible before March on the valid axis and
    /// before August on the transaction axis, and a reader told only "first
    /// recorded August" would not understand why asking about April also came
    /// back empty.
    NotYetRecorded {
        title: String,
        /// The first moment the store heard of this decision.
        first_recorded: Timestamp,
        /// The first moment it claims to have held.
        first_held: Timestamp,
    },
    /// No decision by that title.
    Unknown,
}
```

Change the `Outcome` variant from `Decision(Option<Box<DecisionDetail>>)` to `Decision(Found)`.

- [ ] **Step 4: Rewrite `decision`**

Replace the body of `decision` at `:967`:

```rust
pub fn decision(engine: &Engine, title: &str, at: At) -> Result<Outcome, HostError> {
    let Some(id) = find_decision(engine, title) else {
        return Ok(Outcome::Decision(Found::Unknown));
    };
    // `status` is always written by `commit_decide`, so its absence at `at`
    // means the store had not heard of this decision at all -- not that a
    // field is missing. Same fact the `Outcome::Decided` construction relies
    // on further up.
    if held(engine, id, "status", at).is_none() {
        let versions = engine.store_history(id, "status");
        let first_recorded = versions
            .iter()
            .map(|v| v.provenance.observed_at)
            .min()
            .ok_or_else(|| {
                HostError::Refused(
                    "a decision recorded no status, which should not be reachable".into(),
                )
            })?;
        let first_held = versions
            .iter()
            .map(|v| v.valid.from)
            .min()
            .unwrap_or(first_recorded);
        return Ok(Outcome::Decision(Found::NotYetRecorded {
            title: title.to_string(),
            first_recorded,
            first_held,
        }));
    }
    let history: Vec<(Timestamp, String)> = visible(engine, id, "choice", at)
        .iter()
        // The day it was decided, not the day the store was told. They are
        // the same unless the decision was backdated, and when they differ the
        // decided day is what a log is a log of -- "we chose this in March" is
        // the entry, and "we typed it up in August" is not.
        .filter_map(|v| Some((v.valid.from, v.value.clone()?)))
        .collect();
    let status = held(engine, id, "status", at).unwrap_or_else(|| DEFAULT_STATUS.into());
    let superseded_by = chain(engine, id, Direction::Forward, at);

    Ok(Outcome::Decision(Found::Decision(Box::new(DecisionDetail {
        entity: id,
        title: title.to_string(),
        choice: held(engine, id, "choice", at).unwrap_or_default(),
        because: held(engine, id, "because", at),
        context: held(engine, id, "context", at),
        still_stands: superseded_by.is_empty() && status == DEFAULT_STATUS,
        status,
        supersedes: chain(engine, id, Direction::Back, at),
        superseded_by,
        history,
    }))))
}
```

- [ ] **Step 5: Thread `at` into `chain`**

Change the signature and the two edge reads:

```rust
fn chain(engine: &Engine, start: StableId, dir: Direction, at: At) -> Vec<(StableId, String)> {
```

```rust
        let edges = match dir {
            Direction::Back => engine.edges_from(at_id, at.valid, at.tx),
            Direction::Forward => engine.edges_into(at_id, at.valid, at.tx),
        };
```

Note: the loop's cursor variable is currently named `at`, which now collides with the parameter. Rename the cursor to `at_id` throughout `chain` — it is a `StableId`, so the new name is also the more accurate one.

- [ ] **Step 6: Update every call site of `decision(`**

Run: `rg -n 'decision\(&e,|Outcome::Decision\(' crates/ -g '*.rs'`

- In `command.rs` tests: append `, At::latest()` and change `Outcome::Decision(Some(d))` to `Outcome::Decision(Found::Decision(d))`. The one `Outcome::Decision(None)` assertion at `:2094` becomes `Outcome::Decision(Found::Unknown)`.
- In `crates/rm-conform/src/decisions.rs:65-70`, `detail` becomes:

```rust
pub fn detail(e: &Engine, title: &str) -> DecisionDetail {
    match command::decision(e, title, At::latest()).expect("a recorded title resolves") {
        Outcome::Decision(Found::Decision(d)) => *d,
        _ => panic!("expected a decision for {title:?}"),
    }
}
```

with `use rm_host::command::Found;` and `use rm_host::time::At;` added to its imports.
- In `crates/rm-cli/src/format.rs`, change the two arms so the crate compiles — `Outcome::Decision(Found::Unknown)` keeps the existing not-found string, `Outcome::Decision(Found::Decision(d))` keeps the existing body, and add a `Found::NotYetRecorded { .. }` arm returning `String::new()` for now. **Task 4 Step 5 replaces that placeholder**; it exists only so this task compiles.

- [ ] **Step 7: Run the workspace**

Run: `cargo test --workspace --all-features`
Expected: PASS, count at 695 or above.

- [ ] **Step 8: fmt and clippy**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add crates/rm-host crates/rm-conform crates/rm-cli
git commit -m "A decision that had not been recorded yet is its own answer"
```

---

### Task 4: The flags, and words that match the clock

**Files:**
- Modify: `crates/rm-cli/src/args.rs` — the `Decisions` and `Decision` variants, their parsing, and the usage text at `:43`
- Modify: `crates/rm-cli/src/run.rs` — build `At` and pass it
- Modify: `crates/rm-cli/src/format.rs` — the `Found` arms

**Interfaces:**
- Consumes: `Found`, `At`, the new `decisions`/`decision` signatures
- Produces: `Command::Decisions { status, valid_at, as_of }` and `Command::Decision { title, valid_at, as_of }`, both `Option<Timestamp>`

- [ ] **Step 1: Write the failing parse test**

Add to the `tests` module in `crates/rm-cli/src/args.rs`:

```rust
    #[test]
    fn the_decision_reads_take_both_clocks() {
        let Command::Decision { valid_at, as_of, title } =
            parse_args(&["decision", "Pin the compiler",
                         "--valid-at", "2026-03-01", "--as-of", "2026-08-24"]).unwrap()
        else {
            panic!("not a decision command")
        };
        assert_eq!(title, "Pin the compiler");
        // End of the named day, same as `about`.
        assert_eq!(valid_at, Some(1_772_323_200_000 + 86_399_999));
        assert_eq!(as_of, Some(1_787_529_600_000 + 86_399_999));

        let Command::Decisions { valid_at, as_of, .. } =
            parse_args(&["decisions", "--as-of", "2026-08-24"]).unwrap()
        else {
            panic!("not a decisions command")
        };
        assert_eq!(valid_at, None);
        assert_eq!(as_of, Some(1_787_529_600_000 + 86_399_999));

        assert!(
            parse_args(&["decision", "X", "--as-of", "not-a-date"]).is_err(),
            "a date that is not one must be refused"
        );
    }
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p rm-cli --all-features the_decision_reads_take_both_clocks`
Expected: FAIL — the variants have no such fields.

- [ ] **Step 3: Add the fields and parse them**

In `crates/rm-cli/src/args.rs`, add to both variants:

```rust
        valid_at: Option<Timestamp>,
        as_of: Option<Timestamp>,
```

and in each variant's construction, alongside the existing fields, mirroring `about` at `:255-256`:

```rust
                valid_at: day("--valid-at")?,
                as_of: day("--as-of")?,
```

Update the usage text near `:43` to add both lines:

```
    rmem decisions [--status <s>] [--valid-at YYYY-MM-DD] [--as-of YYYY-MM-DD]
    rmem decision <title> [--valid-at YYYY-MM-DD] [--as-of YYYY-MM-DD]
```

Fix any other construction of these variants in the existing tests by adding `valid_at: None, as_of: None`.

- [ ] **Step 4: Run the parse test**

Run: `cargo test -p rm-cli --all-features the_decision_reads_take_both_clocks`
Expected: PASS.

- [ ] **Step 5: Build the `At` in `run.rs` and render the third answer**

In `crates/rm-cli/src/run.rs`, at the two dispatch arms:

```rust
        Command::Decisions { status, valid_at, as_of } => command::decisions(
            engine,
            status.as_deref(),
            At {
                valid: valid_at.unwrap_or(Timestamp::MAX),
                tx: as_of.unwrap_or(Timestamp::MAX),
            },
        ),
        Command::Decision { title, valid_at, as_of } => command::decision(
            engine,
            &title,
            At {
                valid: valid_at.unwrap_or(Timestamp::MAX),
                tx: as_of.unwrap_or(Timestamp::MAX),
            },
        ),
```

`Timestamp::MAX` rather than `now`, matching `At::latest()` — see the Global Constraints.

In `crates/rm-cli/src/format.rs`, replace the placeholder arm from Task 3 Step 6. Add `use rm_host::time::format_day;` to the imports at `:8` — `format.rs` does not import it today.

```rust
        Outcome::Decision(Found::NotYetRecorded {
            title,
            first_recorded,
            first_held,
        }) => format!(
            "{title:?} is on record, but nothing of it stood at the time you asked.\n\
             \n\
             It was first recorded {} and holds from {}.\n\
             Ask on or after both of those, or drop the flags for what stands now.\n",
            format_day(*first_recorded),
            format_day(*first_held),
        ),
```

Both days, because either clock can be the one that excluded it — see the doc comment on `Found::NotYetRecorded`.

- [ ] **Step 6: Make `still_stands` say the right tense**

`still_stands` is present tense and is now evaluated at `at`. In the `Found::Decision(d)` arm, the line `out.push_str("\nthis is what stands.\n")` is the one that lies under a past clock.

`format.rs` does not currently receive the clock: its entry point is `pub fn render(outcome: &Outcome) -> String` at `:10`. Change it to:

```rust
pub fn render(outcome: &Outcome, as_of: Option<Timestamp>) -> String {
```

`None` means no `--as-of` was given. Every other arm ignores it. Update `render`'s call site in `crates/rm-cli/src/run.rs` to pass the `as_of` it already parsed, and `None` for every command that has no such flag. Then render:

```rust
            if d.still_stands {
                match as_of {
                    None => out.push_str("\nthis is what stands.\n"),
                    Some(t) => out.push_str(&format!(
                        "\nthis is what stood as of {}.\n",
                        format_day(t)
                    )),
                }
            } else if d.superseded_by.is_empty() {
```

Thread the value from `run.rs`, which already has it.

- [ ] **Step 7: Write the failing render test**

Add to `crates/rm-cli/src/format.rs`'s test module:

```rust
    #[test]
    fn a_past_clock_is_not_described_in_the_present_tense() {
        const AUGUST: Timestamp = 1_787_529_600_000;
        let d = DecisionDetail {
            entity: 1,
            title: "Pin the compiler".into(),
            choice: "a choice".into(),
            because: None,
            context: None,
            still_stands: true,
            status: "accepted".into(),
            supersedes: vec![],
            superseded_by: vec![],
            history: vec![],
        };
        let now = render(&Outcome::Decision(Found::Decision(Box::new(d.clone()))), None);
        assert!(now.contains("this is what stands."), "{now}");

        let then = render(&Outcome::Decision(Found::Decision(Box::new(d))), Some(AUGUST));
        assert!(then.contains("stood as of 2026-08-24"), "{then}");
        assert!(
            !then.contains("this is what stands."),
            "present tense under a past clock: {then}"
        );
    }

    #[test]
    fn a_decision_the_store_had_not_heard_of_says_when_it_arrived() {
        const AUGUST: Timestamp = 1_787_529_600_000;
        const MARCH: Timestamp = 1_772_236_800_000;
        let out = render(
            &Outcome::Decision(Found::NotYetRecorded {
                title: "Pin the compiler".into(),
                first_recorded: AUGUST,
                first_held: MARCH,
            }),
            Some(1),
        );
        assert!(out.contains("2026-08-24"), "the day it arrived: {out}");
        assert!(out.contains("2026-02-28"), "the day it holds from: {out}");
        assert!(
            !out.contains("no decision by that title"),
            "must not read as a typo: {out}"
        );
    }
```

`DecisionDetail` needs `#[derive(Clone)]` for the first test — add it in `command.rs` if absent.

- [ ] **Step 8: Run the crate's tests**

Run: `cargo test -p rm-cli --all-features`
Expected: PASS.

- [ ] **Step 9: Try it against the live store, read-only**

Run:
```bash
RMEM_CONFIG=D:/memory/rmem.toml cargo run -q -p rm-cli -- decisions --as-of 2026-08-01 | head -20
```
Expected: a shorter list than `decisions` with no flag, on a store of 219 decisions. This is a read; it writes nothing. If the embedder is unreachable it does not matter — these commands reach no model.

- [ ] **Step 10: fmt, clippy, workspace**

Run: `cargo test --workspace --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean, count at 698 or above.

- [ ] **Step 11: Commit**

```bash
git add crates/rm-cli
git commit -m "Flags on the reads, and a tense that matches the clock"
```

---

### Task 5: The same two questions over MCP

**Files:**
- Modify: `crates/rm-mcp/src/tools.rs` — the two schemas (`:195-227`), the `Call` variants (`:287-293`), the parse arms (`:385-390`), and a new `optional_instant` helper
- Modify: `crates/rm-mcp/src/serve.rs:543-544`

**Interfaces:**
- Consumes: `At`, `Found`, the new command signatures
- Produces: `Call::Decisions { status, valid_at, as_of }`, `Call::Decision { title, valid_at, as_of }`, all clocks `Option<Timestamp>`; `fn optional_instant(arguments: &Value, key: &str) -> Result<Option<Timestamp>, ToolError>`

**A deviation from the spec, decided here.** The spec said "string parameters". `tools.rs:381-382` shows `about` takes `valid_at`/`as_of` as **integers** via `optional_integer`, while `decide` takes `decided_at` as a **date string**. One parameter name meaning two types across tools is a real footgun for a model caller, and a raw epoch is error-prone to emit. `optional_instant` therefore accepts **either**: a JSON number is a millisecond timestamp, a JSON string is `YYYY-MM-DD` read as the end of that day. That makes these tools a superset of both existing conventions and breaks nothing.

- [ ] **Step 1: Write the failing test**

Add to `crates/rm-mcp/src/tools.rs`'s test module:

```rust
    #[test]
    fn the_decision_reads_take_either_a_date_or_an_instant() {
        let Call::Decision { valid_at, as_of, .. } = read(
            "decision",
            json!({"title": "Pin the compiler", "as_of": "2026-08-24"}),
        )
        .unwrap() else {
            panic!("not a decision call")
        };
        assert_eq!(as_of, Some(1_787_529_600_000 + 86_399_999));
        assert_eq!(valid_at, None);

        let Call::Decisions { as_of, .. } =
            read("decisions", json!({"as_of": 1_787_529_600_000i64})).unwrap()
        else {
            panic!("not a decisions call")
        };
        assert_eq!(as_of, Some(1_787_529_600_000));

        assert!(
            read("decision", json!({"title": "X", "as_of": "not-a-date"})).is_err(),
            "a date that is not one must be refused"
        );
        assert!(
            read("decision", json!({"title": "X", "as_of": true})).is_err(),
            "a boolean is neither"
        );
    }
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p rm-mcp --all-features the_decision_reads_take_either_a_date_or_an_instant`
Expected: FAIL — the variants have no such fields.

- [ ] **Step 3: Add `optional_instant`**

Beside `optional_integer` in `crates/rm-mcp/src/tools.rs`:

```rust
/// A point in time, given either way.
///
/// A JSON number is milliseconds, matching `about`'s `valid_at`/`as_of`. A
/// string is `YYYY-MM-DD` read as the end of that day, matching `decide`'s
/// `decided_at` and the CLI's flags. Those two conventions already both exist
/// in this file; accepting either means the same parameter name does not mean
/// two different types depending on which tool it is on.
fn optional_instant(args: &Value, field: &str) -> Result<Option<Timestamp>, Unreadable> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(_)) => optional_string(args, field)?
            .map(|d| rm_host::time::parse_day_end(&d))
            .transpose(),
        Some(Value::Number(_)) => optional_integer(args, field),
        Some(_) => Err(format!(
            "{field} must be a date as YYYY-MM-DD or a time in milliseconds"
        )),
    }
}
```

The error type is `Unreadable` and its values are built with a bare `format!` — see `optional_integer` at `:464-476`, which this deliberately mirrors. `parse_day_end` already returns `Result<Timestamp, String>`, so `transpose()` lines up without a conversion; if `Unreadable` turns out not to be a `String` alias, add `.map_err(Into::into)`.

- [ ] **Step 4: Add the fields, parse them, and dispatch**

In the `Call` enum:

```rust
    Decisions {
        status: Option<String>,
        valid_at: Option<Timestamp>,
        as_of: Option<Timestamp>,
    },
    Decision {
        title: String,
        valid_at: Option<Timestamp>,
        as_of: Option<Timestamp>,
    },
```

In the parse arms:

```rust
            "decisions" => Ok(Call::Decisions {
                status: optional_string(arguments, "status")?,
                valid_at: optional_instant(arguments, "valid_at")?,
                as_of: optional_instant(arguments, "as_of")?,
            }),
            "decision" => Ok(Call::Decision {
                title: string(arguments, "title")?,
                valid_at: optional_instant(arguments, "valid_at")?,
                as_of: optional_instant(arguments, "as_of")?,
            }),
```

In `crates/rm-mcp/src/serve.rs:543-544`:

```rust
            (Call::Decisions { status, valid_at, as_of }, _) => command::decisions(
                engine,
                status.as_deref(),
                At { valid: valid_at.unwrap_or(Timestamp::MAX), tx: as_of.unwrap_or(Timestamp::MAX) },
            ),
            (Call::Decision { title, valid_at, as_of }, _) => command::decision(
                engine,
                &title,
                At { valid: valid_at.unwrap_or(Timestamp::MAX), tx: as_of.unwrap_or(Timestamp::MAX) },
            ),
```

- [ ] **Step 5: Add both properties to both schemas**

In the `decisions` schema's `properties`, and again in `decision`'s:

```json
                    "as_of": {
                        "type": ["string", "integer"],
                        "description": "Answer as the store knew things on this date (YYYY-MM-DD), not as it knows them now. Use this to see what was on record when an earlier choice was made. Omit for what is known now."
                    },
                    "valid_at": {
                        "type": ["string", "integer"],
                        "description": "Answer with what held on this date (YYYY-MM-DD) rather than what holds now. A decision backdated with decided_at holds from that day. Omit for what holds now."
                    }
```

Both tools already carry `"additionalProperties": false`, so this is the only place a new argument can be accepted from.

- [ ] **Step 6: Run the crate's tests**

Run: `cargo test -p rm-mcp --all-features`
Expected: PASS. Expect the tool-listing snapshot tests near `:520` and `:824` to need the new properties; update them to match.

- [ ] **Step 7: Check what the schema now costs**

Run:
```bash
cargo run -q -p rm-mcp --bin rmem-mcp <<< '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' 2>/dev/null | wc -c
```
Record the byte count in the commit message. The spec budgeted 150–200 tokens; roughly four bytes per token, so an increase beyond ~800 bytes over the previous listing is worth reporting rather than absorbing silently.

- [ ] **Step 8: fmt, clippy, workspace**

Run: `cargo test --workspace --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean, count at 699 or above.

- [ ] **Step 9: Commit**

```bash
git add crates/rm-mcp
git commit -m "The same two questions, over the wire"
```

---

### Task 6: A coverage number that is no longer zero

**Files:**
- Modify: `crates/rm-conform/src/decisions.rs` — `time_coverage` and its tests

**Interfaces:**
- Consumes: `command::decision`, `command::decisions`, `Found`, `At`, the existing `build_chain`
- Produces: `pub fn time_coverage() -> f64` — a measurement; `pub fn coverage_probes() -> Vec<(Timestamp, Timestamp)>`

- [ ] **Step 1: Write the failing test**

Replace `the_decision_layer_answers_no_temporal_probe` in `crates/rm-conform/src/decisions.rs` with:

```rust
    #[test]
    fn every_temporal_probe_is_answered_correctly() {
        assert_eq!(
            time_coverage(),
            1.0,
            "the decision layer disagreed with the expectation on some probe"
        );
    }

    /// The companion. A coverage figure measured over a grid where every probe
    /// is trivially "now" would read 1.000 having tested nothing -- the same
    /// vacuity the differential suite guards against.
    #[test]
    fn the_probe_grid_straddles_the_chain_rather_than_sitting_after_it() {
        let probes = coverage_probes();
        let before = probes.iter().filter(|(_, tx)| *tx < 1_000).count();
        let inside = probes
            .iter()
            .filter(|(_, tx)| (1_000..1_300).contains(tx))
            .count();
        let after = probes.iter().filter(|(_, tx)| *tx >= 1_300).count();
        assert!(before > 0, "no probe predates the chain");
        assert!(inside > 0, "no probe lands mid-chain");
        assert!(after > 0, "no probe sees the whole chain");
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p rm-conform --all-features every_temporal_probe_is_answered_correctly`
Expected: FAIL — `time_coverage()` still returns `0.0`, and `coverage_probes` does not exist.

- [ ] **Step 3: Implement the measurement**

`build_chain` records at `observed_at` starting at `1_000` and stepping `100`, with `decided_at` defaulting to `observed_at` — so a three-link chain occupies 1000, 1100, 1200. Replace `time_coverage`:

```rust
/// The grid the decision layer is probed on.
///
/// `pub` so the report and the vacuity test read the same grid rather than two
/// that could drift apart. Chosen against `build_chain`'s clock: it records at
/// 1000, 1100, 1200, so 900 predates the chain and 1500 follows it.
pub fn coverage_probes() -> Vec<(Timestamp, Timestamp)> {
    let mut out = Vec::new();
    for valid_t in [900, 1_050, 1_150, 1_500] {
        for tx_t in [900, 1_050, 1_150, 1_500] {
            out.push((valid_t, tx_t));
        }
    }
    out
}

/// The fraction of a bi-temporal probe set the decision API answers correctly.
///
/// Was a hardcoded `0.0`: `command::decisions` and `command::decision` took no
/// time parameters, so there was no probe they could answer. They take an `At`
/// now, and this measures whether the answers are right rather than whether an
/// answer came back.
///
/// The expectation is computed here from what `build_chain` wrote, not from
/// what the command returns -- an oracle derived from the code it judges is not
/// an oracle, which is the rule the rest of this crate is built on.
pub fn time_coverage() -> f64 {
    const TITLES: [&str; 3] = ["adopt sqlite", "prefer postgres", "switch to duckdb"];
    let recorded_at: [Timestamp; 3] = [1_000, 1_100, 1_200];
    let e = build_chain(&TITLES);

    let probes = coverage_probes();
    let mut right = 0usize;
    for (valid_t, tx_t) in &probes {
        let at = At {
            valid: *valid_t,
            tx: *tx_t,
        };
        // What should be true of the first link at this instant, worked out
        // from the timestamps `build_chain` used.
        let known = recorded_at[0] <= *tx_t && recorded_at[0] <= *valid_t;
        // It is retired once the second link exists on both axes, because that
        // is the one carrying the `supersedes` edge into it.
        let retired = recorded_at[1] <= *tx_t && recorded_at[1] <= *valid_t;

        let got = command::decision(&e, TITLES[0], at).expect("a recorded title resolves");
        let ok = match got {
            Outcome::Decision(Found::NotYetRecorded { .. }) => !known,
            Outcome::Decision(Found::Decision(d)) => known && d.still_stands == !retired,
            _ => false,
        };
        if ok {
            right += 1;
        }
    }
    right as f64 / probes.len() as f64
}
```

Add `use rm_host::command::Found;` and `use rm_host::time::At;` to the module's imports, and `use rm_core::Timestamp;` if `Timestamp` is not already in scope.

- [ ] **Step 4: Run them**

Run: `cargo test -p rm-conform --all-features`
Expected: PASS. If `time_coverage()` is not 1.000, that is a real disagreement between the expectation and the implementation — **do not adjust the expectation to match.** Work out which is right first; #36's four corrections were all in the reference model, but the direction is not guaranteed.

- [ ] **Step 5: Recompute the report**

Run: `cargo run --release -q -p rm-conform -- --report`
Expected: the `decision-layer time coverage` row now reads `1.000`. `report.rs` calls `time_coverage()` directly, so no change is needed there.

- [ ] **Step 6: Update the two prose claims that are now stale**

`crates/rm-conform/README.md` carries the table with `| decision-layer time coverage | 0.000 |`. Update that row to the measured value, and revise the surrounding text: the README currently says every row but the last is a bug if it is not 1.000, which is no longer the right framing for a row that now measures correctness like the others.

The doc comment on `time_coverage` in Step 3 already replaces the old one describing why it was zero.

- [ ] **Step 7: fmt, clippy, workspace**

Run: `cargo test --workspace --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean, count at 700 or above.

- [ ] **Step 8: Commit**

```bash
git add crates/rm-conform
git commit -m "The number that was zero because nothing could be asked"
```

---

### Task 7: Say so in the README

**Files:**
- Modify: `README.md` — the decision-log section, around the `--as-of` / `--valid-at` paragraphs

**Interfaces:**
- Consumes: everything above. Produces nothing code depends on.

- [ ] **Step 1: Document the flags on the decision reads**

The README's existing paragraph reads:

> `--valid-at` needs an attribute whose policy keeps a timeline. Survivorship runs first, and most strategies collapse a history to one winner [...] Only an attribute under `valid_interval` can be asked, which is `employer` in the template and whatever else you configure.

That is still true of `rmem about` and must stay. Add, in the decision-log section, that the decision reads are the exception and why:

```markdown
`rmem decisions` and `rmem decision` take the same two flags, and `--valid-at`
works on them whatever `[policy]` says. They do not go through survivorship:
a decision's timeline is the versions of its own `choice`, so "what stood in
March" is a cut over that list rather than a question for a strategy.

```sh
rmem decision "Pin the compiler" --as-of 2026-03-01   # what the log said then
rmem decisions --as-of 2026-03-01                     # the whole log, then
```

A decision recorded after the date asked about is not missing — it says so, and
names the day it arrived, because "no decision by that title" would read as a
typo and send you looking for a spelling mistake.

A decision that stood then and does not now reads as *stood as of*, not *still
stands*: the walk to whatever replaced it is made at the same clock, so a
supersession recorded in August does not retire anything in March.
```

- [ ] **Step 2: Add the two parameters to the MCP tool description**

The README lists the eight tools and says `about` "is the one that differs: it takes both time axes". That is no longer the only one. Amend it to name `decisions` and `decision` as also taking both, and note that they accept a `YYYY-MM-DD` string or a millisecond instant.

- [ ] **Step 3: Check the token table is still honest**

The README's `RMEM_TOOLS` table gives ~810 tokens for `decide,decisions,decision`. Task 5 Step 7 measured the new listing. If the figure moved by more than ~50 tokens, update the table row and say what it bought.

- [ ] **Step 4: Spellcheck**

Run: `typos README.md crates/rm-conform/README.md`
Expected: clean.

- [ ] **Step 5: Final full verification**

Run: `cargo test --workspace --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: all clean.

- [ ] **Step 6: Commit**

```bash
git add README.md
git commit -m "The flags, and the one place valid time does not need a policy"
```

---

## What this does not do

Carried from the spec so it is not lost between documents:

- **It does not fix finding #2.** `rmem about --valid-at` stays inert under `Strategy::MostRecent`, which the template ships for every attribute but `employer`. The decision layer sidesteps that by building its own timeline; it does not close it. Task 7 Step 1 says so in the README rather than letting a green coverage row imply otherwise.
- **No change to `decide`.** It already has `--at`. This is the read half.
- **No change to survivorship, resolution, or the index.**
