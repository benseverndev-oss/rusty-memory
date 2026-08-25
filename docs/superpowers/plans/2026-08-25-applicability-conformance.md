# Applicability Conformance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `rm-conform` an applicability axis, so the scope rule — which decides what every session is shown — is a measured claim rather than a described one.

**Architecture:** One new module, `rm_conform::applicability`, holding an independently-derived oracle (`reaches`), a scope-tree generator whose alphabet deliberately contains prefix-colliding names, and three measurements. `report.rs` gains three rows computed from the module's own `pub` grids, so the README and the suite cannot disagree about what was measured.

**Tech Stack:** Rust (pinned in `rust-toolchain.toml`), no new dependencies. Existing `rm_conform::rng::Rng` (SplitMix64) for generation.

**Spec:** `docs/superpowers/specs/2026-08-25-applicability-conformance-design.md`

## Global Constraints

- **`applicability.rs` must never import `rm_host::scope`** — not `applies_at`, not `validate`, not `UNIVERSAL`. The oracle spells `"*"` out, so a change to the constant surfaces as a disagreement rather than being tracked silently. This is the constraint the crate's findings rest on.
- **No new dependencies.** Library crates take `serde`/`serde_json` only; `rm-host` adds `toml`, `rm-providers` adds `ureq`.
- **Every test runs offline.** `rm-conform` uses `rm_embed::Hashed`, which opens no socket and needs no key.
- **Every row is a bug if it is not 1.000.**
- **CI commands, as CI spells them:** `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features`.
- **Runtime budget:** under **1s** added in release, under **3s** in debug tests. The existing 500-seed sweep computes in 0.27s release. If exceeded, cut seeds rather than coverage and say in the README what was cut.
- **Baseline:** 727 tests pass on `main`. Every task must leave that at or above where it started.
- **Commit style:** a title line in plain words, a body explaining why. No conventional-commit prefixes.
- **The working copy is CRLF.** Preserve line endings when editing.

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/rm-conform/src/applicability.rs` | The oracle, the generator, three measurements, their grids | **Create** |
| `crates/rm-conform/src/lib.rs` | Module list | Add `pub mod applicability;` |
| `crates/rm-conform/src/report.rs` | The headline table | Three rows |
| `crates/rm-conform/README.md` | The table and what it claims | Rows and prose |

`decisions.rs` is untouched. Its `engine()`/`ruleset()` helpers are private to it; `applicability.rs` gets its own rather than making them `pub(crate)`, because a shared fixture that two measurements can silently reconfigure is the sort of coupling this crate exists to avoid.

---

### Task 1: The oracle, and the collision it must not share

**Files:**
- Create: `crates/rm-conform/src/applicability.rs`
- Modify: `crates/rm-conform/src/lib.rs`

**Interfaces:**
- Consumes: nothing. This task's code imports no other crate.
- Produces: `pub fn reaches(scope: &str, position: &str) -> bool`

- [ ] **Step 1: Write the failing tests**

Create `crates/rm-conform/src/applicability.rs` with only this for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_reaches_its_own_position_and_everything_below() {
        assert!(reaches("work", "work"));
        assert!(reaches("work", "work/goldenmatch"));
        assert!(reaches("work/goldenmatch", "work/goldenmatch/fs"));
        assert!(reaches("*", "anything/at/all"));
        assert!(reaches("*", "*"));
    }

    #[test]
    fn a_scope_reaches_neither_sideways_nor_upwards() {
        assert!(!reaches("work/goldenmatch/fs", "work/goldenmatch/er"));
        assert!(!reaches("personal", "work"));
        assert!(!reaches("work/goldenmatch", "work"), "narrower than the asker");
        assert!(!reaches("work", "*"), "the root, where only * reaches");
    }

    /// The mistake the whole rule exists to prevent, and therefore the one an
    /// oracle must not share by construction. A bare `starts_with` says true
    /// to every line here.
    #[test]
    fn a_segment_boundary_is_not_a_string_prefix() {
        assert!(!reaches("prod", "production"));
        assert!(!reaches("work", "workshop"));
        assert!(!reaches("work", "workshop/thing"));
        assert!(reaches("prod", "prod/deploy"));
    }
}
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p rm-conform --all-features applicability::`
Expected: FAIL — the module is not declared, so the crate does not compile.

- [ ] **Step 3: Declare the module**

In `crates/rm-conform/src/lib.rs`, beside the other `pub mod` lines, add:

```rust
pub mod applicability;
```

Keep the list alphabetical: `applicability` goes first, before `decisions`.

- [ ] **Step 4: Implement the oracle**

Insert above the `#[cfg(test)]` block:

```rust
//! Whether a memory reaches where it was asked from, measured.
//!
//! One rule governs every decision read:
//!
//! > A memory applies where its scope is an **ancestor-or-self** of the asker's
//! > position.
//!
//! It decides what a session is shown, and the live store has hundreds of
//! records under it. The headline table had five rows and none of them was
//! this.
//!
//! # This module never imports `rm_host::scope`
//!
//! Not [`applies_at`], not `validate`, not `UNIVERSAL`. An oracle derived from
//! the code it judges is not an oracle. Scopes reach the store through
//! `command::decide` and `command::plan_rescope` like any other caller, so the
//! store is exercised normally; only the *expectation* is computed here.
//!
//! [`applies_at`]: https://docs.rs/rm-host

/// Whether a memory scoped `scope` reaches an asker standing at `position`.
///
/// The oracle. Derived from the rule as written rather than from
/// `rm_host::scope::applies_at`, and derived *differently*: this is
/// separator-anchored string work where the implementation zips segment
/// iterators. Two ways to the same claim is the whole value of a differential.
///
/// `"*"` is spelled out rather than imported. Importing the constant would make
/// this track a change to it silently; spelling it means a change surfaces as a
/// disagreement, which is the point.
pub fn reaches(scope: &str, position: &str) -> bool {
    scope == "*"
        || position == scope
        // The trailing separator is what stops `prod` reaching `production`.
        || position.starts_with(&format!("{scope}/"))
}
```

- [ ] **Step 5: Run them to make sure they pass**

Run: `cargo test -p rm-conform --all-features applicability::`
Expected: PASS, 3 tests.

- [ ] **Step 6: Prove the oracle is not the implementation wearing a hat**

Run: `rg -n 'rm_host::scope|use rm_host' crates/rm-conform/src/applicability.rs`
Expected: **no output.** If this ever prints, the differential has become a tautology and the row is worthless. Add it as a test so the compiler enforces what a grep cannot:

```rust
    /// The constraint the whole module rests on, asserted rather than trusted
    /// to review. `include_str!` reads this file at compile time, so an import
    /// added later fails the suite rather than quietly voiding the measurement.
    #[test]
    fn this_module_does_not_import_the_code_it_judges() {
        let me = include_str!("applicability.rs");
        for banned in ["rm_host::scope", "scope::applies_at", "scope::UNIVERSAL"] {
            // Skip this test's own list, which necessarily contains them.
            let uses = me
                .lines()
                .filter(|l| l.trim_start().starts_with("use "))
                .filter(|l| l.contains(banned))
                .count();
            assert_eq!(uses, 0, "applicability imports {banned}, so it judges itself");
        }
    }
```

- [ ] **Step 7: Run the crate, fmt and clippy**

Run: `cargo test -p rm-conform --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: all clean. `reaches` has no caller yet, so clippy may warn `dead_code` — it is `pub`, so it should not; if it does, that is a signal the module was not declared `pub`.

- [ ] **Step 8: Commit**

```bash
git add crates/rm-conform/src/applicability.rs crates/rm-conform/src/lib.rs
git commit -m "A second way to the same rule"
```

---

### Task 2: A world with names that nearly collide

**Files:**
- Modify: `crates/rm-conform/src/applicability.rs`

**Interfaces:**
- Consumes: `rm_conform::rng::Rng` — `Rng::new(seed: u64)`, `rng.below(n: u64) -> u64`
- Produces:
  - `pub struct World { pub scopes: Vec<String>, pub positions: Vec<String>, pub decisions: Vec<(String, String)> }` — `decisions` is `(title, scope)`
  - `pub struct Params { pub depth: usize, pub branching: usize, pub decisions: usize, pub universal_pct: u64 }`, with a `Default`
  - `pub fn world(seed: u64, params: &Params) -> World`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/rm-conform/src/applicability.rs`:

```rust
    /// The alphabet has to contain names that are string prefixes of each
    /// other, or `a_segment_boundary_is_not_a_string_prefix` is the only place
    /// that property is ever exercised and the sweep tests it zero times.
    #[test]
    fn generated_trees_contain_names_that_nearly_collide() {
        let params = Params::default();
        let mut found = false;
        for seed in 0..40 {
            let w = world(seed, &params);
            for a in &w.scopes {
                for b in &w.scopes {
                    if a != b {
                        for (x, y) in [(a, b), (b, a)] {
                            let (x, y) = (last_segment(x), last_segment(y));
                            if x != y && y.starts_with(x) {
                                found = true;
                            }
                        }
                    }
                }
            }
        }
        assert!(
            found,
            "no generated tree had one segment name as a string prefix of \
             another, so the property the oracle exists to check is untested"
        );
    }

    #[test]
    fn a_world_is_deterministic_and_well_formed() {
        let params = Params::default();
        let a = world(7, &params);
        let b = world(7, &params);
        assert_eq!(a.scopes, b.scopes, "same seed, same world");
        assert_eq!(a.decisions, b.decisions);
        assert_ne!(world(8, &params).scopes, a.scopes, "different seed, different world");

        assert_eq!(a.decisions.len(), params.decisions);
        let titles: std::collections::HashSet<&String> =
            a.decisions.iter().map(|(t, _)| t).collect();
        assert_eq!(titles.len(), a.decisions.len(), "titles must be unique");
        for (_, s) in &a.decisions {
            assert!(s == "*" || a.scopes.contains(s), "{s:?} is not in the tree");
        }
        assert!(!a.positions.is_empty());
    }

    /// Both halves matter. All-universal makes exclusion untestable; none
    /// makes the row that must appear everywhere untestable.
    #[test]
    fn some_decisions_are_universal_and_some_are_not() {
        let params = Params::default();
        let mut universal = 0usize;
        let mut narrow = 0usize;
        for seed in 0..40 {
            for (_, s) in world(seed, &params).decisions {
                if s == "*" {
                    universal += 1;
                } else {
                    narrow += 1;
                }
            }
        }
        assert!(universal > 0, "nothing was scoped everywhere");
        assert!(narrow > 0, "everything was scoped everywhere");
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p rm-conform --all-features applicability::`
Expected: FAIL — `cannot find function 'world'`.

- [ ] **Step 3: Implement the generator**

Add above the `#[cfg(test)]` block:

```rust
use crate::rng::Rng;

/// Segment names, chosen so some are string prefixes of others.
///
/// `prod`/`production` and `work`/`workshop` are the point of this list, not
/// decoration. Without a pair like them the sweep never builds a tree where a
/// bare `starts_with` would differ from a segment comparison, and
/// `applicability agreement` would report 1.000 having tested the one mistake
/// the rule exists to prevent exactly zero times.
const NAMES: [&str; 8] = [
    "prod",
    "production",
    "work",
    "workshop",
    "personal",
    "fs",
    "er",
    "arrow",
];

/// The last `/`-separated part of a path.
fn last_segment(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// How much world to build.
#[derive(Clone, Debug)]
pub struct Params {
    /// How deep the scope tree goes. 1 is a flat list of top-level names.
    pub depth: usize,
    /// How many children each node gets.
    pub branching: usize,
    /// How many decisions to place in it.
    pub decisions: usize,
    /// Percent of decisions scoped `*`.
    pub universal_pct: u64,
}

impl Default for Params {
    fn default() -> Self {
        // Three levels and two-way branching gives a tree small enough to
        // compare exhaustively and deep enough that ancestor-or-self has more
        // than one ancestor to get wrong.
        Params {
            depth: 3,
            branching: 2,
            decisions: 12,
            universal_pct: 20,
        }
    }
}

/// A generated scope tree, the positions to ask from, and the decisions in it.
pub struct World {
    /// Every path in the tree, shallowest first. Never contains `*`.
    pub scopes: Vec<String>,
    /// Where to stand. Drawn from three places -- see [`world`].
    pub positions: Vec<String>,
    /// `(title, scope)`. Scope is a member of `scopes`, or `*`.
    pub decisions: Vec<(String, String)>,
}

/// Build one world from a seed.
///
/// Positions come from three places because they fail differently: a node in
/// the tree (the ordinary case), a node *below* the deepest scope (where only
/// ancestors reach), and a path sharing a segment prefix with a real scope
/// without being under it (`production` beside `prod`) -- which is the case a
/// string-prefix rule gets wrong and a segment rule does not.
pub fn world(seed: u64, params: &Params) -> World {
    let mut rng = Rng::new(seed);

    let mut scopes: Vec<String> = Vec::new();
    let mut frontier: Vec<String> = Vec::new();
    for _ in 0..params.branching {
        let name = NAMES[rng.below(NAMES.len() as u64) as usize];
        if !frontier.iter().any(|f| f == name) {
            frontier.push(name.to_string());
        }
    }
    scopes.extend(frontier.iter().cloned());
    for _ in 1..params.depth {
        let mut next = Vec::new();
        for parent in &frontier {
            for _ in 0..params.branching {
                let name = NAMES[rng.below(NAMES.len() as u64) as usize];
                let child = format!("{parent}/{name}");
                if !scopes.contains(&child) {
                    scopes.push(child.clone());
                    next.push(child);
                }
            }
        }
        frontier = next;
    }

    let mut decisions = Vec::new();
    for i in 0..params.decisions {
        let scope = if rng.below(100) < params.universal_pct {
            "*".to_string()
        } else {
            scopes[rng.below(scopes.len() as u64) as usize].clone()
        };
        decisions.push((format!("decision {i}"), scope));
    }

    let mut positions: Vec<String> = scopes.clone();
    // Below the deepest scope: only ancestors reach here.
    if let Some(deepest) = scopes.last() {
        positions.push(format!("{deepest}/deeper"));
    }
    // A sibling that shares a string prefix without being under anything.
    positions.push("production".to_string());
    positions.push("prod".to_string());
    // The root, where only `*` reaches.
    positions.push("*".to_string());

    World {
        scopes,
        positions,
        decisions,
    }
}
```

- [ ] **Step 4: Run them to make sure they pass**

Run: `cargo test -p rm-conform --all-features applicability::`
Expected: PASS, 6 tests.

If `generated_trees_contain_names_that_nearly_collide` fails, `NAMES` lost its colliding pair or `branching` is too low to draw two distinct names — do not weaken the test, fix the generator.

- [ ] **Step 5: fmt, clippy, crate**

Run: `cargo test -p rm-conform --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rm-conform/src/applicability.rs
git commit -m "A tree whose names nearly collide, on purpose"
```

---

### Task 3: Agreement, and the two vacuity guards it needs

**Files:**
- Modify: `crates/rm-conform/src/applicability.rs`

**Interfaces:**
- Consumes: `reaches`, `world`, `Params` from Tasks 1–2; `rm_embed::Hashed`, `rm_engine::{Engine, Policy, Strategy}`, `rm_index::{Metric, VectorIndex}`, `rm_resolve::{BlockingKey, Comparator, FieldRule, Ruleset}`, `rm_host::command::{self, Outcome}`, `rm_host::time::At`
- Produces:
  - `pub fn build(world: &World) -> Engine`
  - `pub fn visible(engine: &Engine, position: &str) -> Vec<String>` — titles, sorted
  - `pub fn expected(world: &World, position: &str) -> Vec<String>` — titles, sorted, from the oracle
  - `pub fn agreement(seeds: std::ops::Range<u64>, params: &Params) -> bool`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
    #[test]
    fn the_read_path_returns_exactly_what_the_oracle_expects() {
        assert!(
            agreement(0..60, &Params::default()),
            "the read path disagreed with the oracle on some (world, position)"
        );
    }

    /// A sweep where every position saw everything, or nothing, would report
    /// perfect agreement having measured neither filtering nor inclusion.
    #[test]
    fn the_sweep_both_includes_and_excludes() {
        let params = Params::default();
        let (mut saw_all, mut saw_some, mut saw_none) = (0, 0, 0);
        for seed in 0..40 {
            let w = world(seed, &params);
            for p in &w.positions {
                let n = expected(&w, p).len();
                if n == 0 {
                    saw_none += 1;
                } else if n == w.decisions.len() {
                    saw_all += 1;
                } else {
                    saw_some += 1;
                }
            }
        }
        assert!(saw_some > 0, "no position ever saw a strict subset");
        assert!(saw_none > 0, "no position ever excluded everything");
        let _ = saw_all;
    }

    /// The guard that makes the agreement row mean something: the oracle must
    /// be capable of disagreeing. A string-prefix rule differs from a segment
    /// rule on exactly the colliding names the generator plants.
    #[test]
    fn a_string_prefix_oracle_would_disagree_with_this_one() {
        fn naive(scope: &str, position: &str) -> bool {
            scope == "*" || position.starts_with(scope)
        }
        let params = Params::default();
        let differ = (0..40).any(|seed| {
            let w = world(seed, &params);
            w.positions.iter().any(|p| {
                w.decisions
                    .iter()
                    .any(|(_, s)| naive(s, p) != reaches(s, p))
            })
        });
        assert!(
            differ,
            "a bare starts_with agreed with the oracle everywhere, so the \
             sweep cannot tell a segment rule from a string one"
        );
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p rm-conform --all-features applicability::`
Expected: FAIL — `cannot find function 'agreement'`.

- [ ] **Step 3: Implement the harness**

Add above the `#[cfg(test)]` block:

```rust
use rm_embed::Hashed;
use rm_engine::{Engine, Policy, Strategy};
use rm_host::command::{self, Outcome};
use rm_host::time::At;
use rm_index::{Metric, VectorIndex};
use rm_resolve::{BlockingKey, Comparator, FieldRule, Ruleset};

/// Its own fixture rather than sharing `decisions.rs`'s.
///
/// A fixture two measurements share is one either can silently reconfigure,
/// which is the coupling this crate exists to avoid rather than introduce.
fn engine() -> Engine {
    let ruleset = Ruleset::new(
        vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
        vec![BlockingKey::Prefix("name".to_string(), 3)],
        4.0,
        8.0,
    )
    .expect("a one-field ruleset is valid");
    Engine::new(
        VectorIndex::new(3, Metric::Cosine),
        ruleset,
        Policy::new(Strategy::MostRecent),
    )
}

/// Record every decision in `world`, each with its stated reach.
pub fn build(world: &World) -> Engine {
    let mut e = engine();
    let embedder = Hashed::new(3);
    let mut observed_at = 1_000;
    for (title, scope) in &world.decisions {
        command::decide(
            &mut e,
            title,
            "the chosen option",
            scope,
            None, // status: accepted
            Some("a stated reason"),
            None, // context
            None, // supersedes
            None, // decided_at: defaults to observed_at
            observed_at,
            "conform",
            &embedder,
        )
        .expect("a decision with a fresh title and a valid scope is recorded");
        observed_at += 10;
    }
    e
}

/// The titles the read path returns from `position`, sorted.
pub fn visible(engine: &Engine, position: &str) -> Vec<String> {
    let Outcome::Decisions(ds) = command::decisions(engine, None, At::latest(), Some(position))
        .expect("listing decisions cannot fail on a store this builds")
    else {
        panic!("decisions did not return decisions")
    };
    let mut out: Vec<String> = ds.into_iter().map(|d| d.title).collect();
    out.sort();
    out
}

/// The titles that *should* be visible from `position`, sorted.
///
/// Computed from what the generator wrote, through [`reaches`] -- never by
/// asking the store.
pub fn expected(world: &World, position: &str) -> Vec<String> {
    let mut out: Vec<String> = world
        .decisions
        .iter()
        .filter(|(_, scope)| reaches(scope, position))
        .map(|(title, _)| title.clone())
        .collect();
    out.sort();
    out
}

/// Whether the read path and the oracle agree on every generated world.
pub fn agreement(seeds: std::ops::Range<u64>, params: &Params) -> bool {
    seeds.into_iter().all(|seed| {
        let w = world(seed, params);
        let e = build(&w);
        w.positions
            .iter()
            .all(|p| visible(&e, p) == expected(&w, p))
    })
}
```

- [ ] **Step 4: Run them**

Run: `cargo test -p rm-conform --all-features applicability::`
Expected: PASS, 9 tests.

**If `the_read_path_returns_exactly_what_the_oracle_expects` fails, stop.** That is a real disagreement between the shipped rule and an independent reading of it — exactly what this row is for. Work out which side is right before touching either; do not adjust the oracle to match. `rm-conform`'s four previous corrections were all in the reference model, but the direction is not guaranteed and assuming it would defeat the point.

- [ ] **Step 5: Check the runtime budget**

Run: `cargo test -p rm-conform --all-features applicability:: 2>&1 | rg 'test result'`

The harness prints `finished in Xs` on that line, so no external timer is needed.
Expected: under **3s** for the module. If over, reduce the seed range in the tests from `0..60` to `0..30` and record the change in the commit message rather than silently.

- [ ] **Step 6: fmt, clippy, workspace**

Run: `cargo test --workspace --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean, count at 733 or above.

- [ ] **Step 7: Commit**

```bash
git add crates/rm-conform/src/applicability.rs
git commit -m "What the read path returns, against what it should"
```

---

### Task 4: Descending only ever adds

**Files:**
- Modify: `crates/rm-conform/src/applicability.rs`

**Interfaces:**
- Consumes: `build`, `visible`, `world`, `Params`
- Produces: `pub fn depth_monotonic(seeds: std::ops::Range<u64>, params: &Params) -> bool`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module:

```rust
    #[test]
    fn a_deeper_position_never_sees_less() {
        assert!(
            depth_monotonic(0..60, &Params::default()),
            "descending removed a decision, which ancestor-or-self forbids"
        );
    }

    /// The companion. If no generated pair were ever nested, the property
    /// above would hold across every seed having compared nothing.
    #[test]
    fn the_monotonicity_check_finds_nested_pairs_to_compare() {
        let params = Params::default();
        let mut pairs = 0usize;
        for seed in 0..40 {
            let w = world(seed, &params);
            for p in &w.positions {
                for q in &w.positions {
                    if p != q && reaches(p, q) {
                        pairs += 1;
                    }
                }
            }
        }
        assert!(
            pairs > 20,
            "only {pairs} nested position pairs across 40 worlds, so the \
             property is close to vacuous"
        );
    }
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p rm-conform --all-features a_deeper_position_never_sees_less`
Expected: FAIL — `cannot find function 'depth_monotonic'`.

- [ ] **Step 3: Implement it**

Add above the `#[cfg(test)]` block:

```rust
/// Whether descending the tree only ever adds.
///
/// A metamorphic property, derived from what ancestor-or-self *means* rather
/// than from either implementation: if a scope reaches `p`, it reaches every
/// position below `p`, so `visible(p)` is a subset of `visible(q)` whenever `q`
/// sits under `p`.
///
/// That derivation is the point. The oracle and the engine were written by the
/// same author against the same mental model and can agree enthusiastically on
/// a shared misunderstanding; this is the cover for that, in the same role
/// `invariants::monotonic_in_transaction_time` plays on the temporal axis.
///
/// Nesting is decided with [`reaches`] rather than by asking the store, so the
/// property does not depend on the thing it is checking.
pub fn depth_monotonic(seeds: std::ops::Range<u64>, params: &Params) -> bool {
    seeds.into_iter().all(|seed| {
        let w = world(seed, params);
        let e = build(&w);
        w.positions.iter().all(|p| {
            let above = visible(&e, p);
            w.positions
                .iter()
                .filter(|q| *q != p && reaches(p, q))
                .all(|q| {
                    let below = visible(&e, q);
                    above.iter().all(|t| below.contains(t))
                })
        })
    })
}
```

- [ ] **Step 4: Run them**

Run: `cargo test -p rm-conform --all-features applicability::`
Expected: PASS, 11 tests.

- [ ] **Step 5: fmt, clippy, workspace**

Run: `cargo test --workspace --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean, count at 735 or above.

- [ ] **Step 6: Commit**

```bash
git add crates/rm-conform/src/applicability.rs
git commit -m "Descending the tree only ever adds"
```

---

### Task 5: What a correction did, and did not, rewrite

**Files:**
- Modify: `crates/rm-conform/src/applicability.rs`

**Interfaces:**
- Consumes: `build`, `world`, `Params`, `visible`; `rm_host::command::{plan_rescope, commit_rescope}`
- Produces:
  - `pub fn rescope_probes() -> Vec<(rm_core::Timestamp, rm_core::Timestamp)>`
  - `pub fn rescope_history(seeds: std::ops::Range<u64>, params: &Params) -> bool`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
    #[test]
    fn a_correction_does_not_rewrite_what_was_true_before_it() {
        assert!(
            rescope_history(0..40, &Params::default()),
            "a corrected reach answered wrongly on some (valid, tx) probe"
        );
    }

    /// This is where the correction branch stops being unexercised. Every one
    /// of the 219 records in the live store was a backfill, so `rescope` with a
    /// previous scope has only ever run in unit tests.
    #[test]
    fn the_correction_branch_actually_fires() {
        let params = Params::default();
        let w = world(3, &params);
        let mut e = build(&w);
        let embedder = Hashed::new(3);
        let (title, original) = w.decisions[0].clone();

        // First write is the backfill case only if there was no scope; these
        // decisions all carry one already, so this is a correction by
        // construction -- which is the case that needed exercising.
        let new_scope = if original == "*" { "work" } else { "*" };
        let plan = command::plan_rescope(&title, new_scope, 9_000, "conform", &embedder)
            .expect("a known title with a valid scope");
        let Outcome::Rescoped { previous, .. } =
            command::commit_rescope(&mut e, plan).expect("the title resolves")
        else {
            panic!("rescope did not report a rescope")
        };
        assert_eq!(
            previous.as_deref(),
            Some(original.as_str()),
            "the correction branch did not see a previous scope"
        );
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p rm-conform --all-features applicability::`
Expected: FAIL — `cannot find function 'rescope_history'`.

- [ ] **Step 3: Implement it**

Add above the `#[cfg(test)]` block:

```rust
use rm_core::Timestamp;

/// The `(valid, tx)` grid the correction is probed on.
///
/// `pub` so `report.rs` and the tests read one grid rather than two that could
/// drift apart. `build` records at 1000 and steps by 10; the correction below
/// lands at 9000, so 5000 is before it on both axes and 12000 is after.
pub fn rescope_probes() -> Vec<(Timestamp, Timestamp)> {
    let mut out = Vec::new();
    for valid_t in [5_000, 12_000] {
        for tx_t in [5_000, 12_000] {
            out.push((valid_t, tx_t));
        }
    }
    out
}

/// Whether correcting a reach leaves the past alone.
///
/// Three claims, and the middle one is the reason this row exists:
///
/// - At a transaction time *before* the correction, the store had not heard of
///   the new reach, so the decision answers under its original one.
/// - At a valid time before the correction but a transaction time after it, the
///   decision answers under its **original** reach -- a correction is dated
///   from now, because the reach genuinely changed today. Dating it from the
///   decision's start would assert it always reached somewhere it did not.
/// - At both after, the new reach applies.
///
/// Checked by asking from the old scope and the new one and seeing which
/// admits the title, rather than by reading the scope attribute back -- the
/// question is what a reader is shown, not what is stored.
pub fn rescope_history(seeds: std::ops::Range<u64>, params: &Params) -> bool {
    const CORRECTED_AT: Timestamp = 9_000;
    let embedder = Hashed::new(3);

    seeds.into_iter().all(|seed| {
        let w = world(seed, params);
        let mut e = build(&w);
        let (title, original) = w.decisions[0].clone();
        // A reach the original does not cover, so the two are distinguishable.
        let new_scope = if original == "*" { "work" } else { "*" };

        let Ok(plan) = command::plan_rescope(&title, new_scope, CORRECTED_AT, "conform", &embedder)
        else {
            return false;
        };
        if command::commit_rescope(&mut e, plan).is_err() {
            return false;
        }

        rescope_probes().into_iter().all(|(valid_t, tx_t)| {
            let at = At {
                valid: valid_t,
                tx: tx_t,
            };
            let under = |scope: &str| {
                let Ok(Outcome::Decisions(ds)) =
                    command::decisions(&e, None, at, Some(position_for(scope)))
                else {
                    return false;
                };
                ds.into_iter().any(|d| d.title == title)
            };
            // Which reach is in force at this instant.
            let corrected = tx_t >= CORRECTED_AT && valid_t >= CORRECTED_AT;
            // `original.as_str()`, not `&original`: both arms of an if-else
            // must be the same type, and `&String` does not coerce here.
            let want = if corrected { new_scope } else { original.as_str() };
            under(want)
        })
    })
}

/// A position from which `scope` reaches, for asking "is it here".
///
/// `*` reaches everywhere, so any position does; for anything else the scope
/// itself is the nearest position that admits it.
fn position_for(scope: &str) -> &str {
    if scope == "*" {
        "anywhere/at/all"
    } else {
        scope
    }
}
```

- [ ] **Step 4: Add the no-op case**

The spec names three cases. Two are reachable here and one is not — see the note below. Add the third that is:

```rust
    /// Re-stating a reach writes nothing. Cheap to get wrong, and the failure
    /// is silent: a second identical scope version would inflate every
    /// backfilled record's history without changing a single answer.
    #[test]
    fn restating_the_same_reach_writes_nothing() {
        let params = Params::default();
        let w = world(5, &params);
        let mut e = build(&w);
        let embedder = Hashed::new(3);
        let (title, original) = w.decisions[0].clone();

        let before = e
            .store_history(
                command::find_decision_id(&e, &title).expect("a recorded title"),
                "scope",
            )
            .len();

        let plan = command::plan_rescope(&title, &original, 9_000, "conform", &embedder)
            .expect("a known title with a valid scope");
        let Outcome::Rescoped { previous, .. } =
            command::commit_rescope(&mut e, plan).expect("the title resolves")
        else {
            panic!("rescope did not report a rescope")
        };
        assert_eq!(previous.as_deref(), Some(original.as_str()));

        let after = e
            .store_history(
                command::find_decision_id(&e, &title).expect("a recorded title"),
                "scope",
            )
            .len();
        assert_eq!(before, after, "re-stating a reach wrote a second version");
    }
```

`find_decision` is private to `rm-host`'s `command` module. If no public way to resolve a title to a `StableId` exists, take the id from `Outcome::Rescoped { entity, .. }`, which carries it — adjust the test to capture `entity` from the first call and use it for both counts, and drop the `find_decision_id` calls. **Do not make `find_decision` public for a test's convenience.**

**The backfill case is not reachable from here, and that is a finding.** The spec assumed all three cases could be swept. They cannot: `decide` refuses without a scope, so every decision this generator writes already carries one, and a rescope over it is a correction by construction. A scopeless decision can only exist as a *legacy* record written before scopes shipped — which no public write path can now produce. Record it in the module doc:

```rust
//! # The backfill case is not reachable here
//!
//! `decide` refuses without a scope, so every decision this module generates
//! carries one and every rescope over it is a correction. A decision with no
//! scope at all can only be a record written before scopes existed, and no
//! public write path produces one any more. That case stays covered by
//! `rm-host`'s own unit tests and by the live-store check in the scope PR
//! (219 records, all unscoped, all still visible), not by this sweep.
```

- [ ] **Step 5: Run them**

Run: `cargo test -p rm-conform --all-features applicability::`
Expected: PASS, 14 tests.

**If `a_correction_does_not_rewrite_what_was_true_before_it` fails, stop and read the failure.** The most likely real cause is that `commit_rescope` dates a correction from the decision's start rather than from now, which would be a genuine defect in the shipped command — not something to accommodate in the probe.

- [ ] **Step 6: fmt, clippy, workspace**

Run: `cargo test --workspace --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean, count at 738 or above.

- [ ] **Step 7: Commit**

```bash
git add crates/rm-conform/src/applicability.rs
git commit -m "A correction dated from today, and a past left alone"
```

---

### Task 6: Three rows in the table

**Files:**
- Modify: `crates/rm-conform/src/report.rs`
- Modify: `crates/rm-conform/README.md`

**Interfaces:**
- Consumes: `agreement`, `depth_monotonic`, `rescope_history`, `applicability::Params`
- Produces: nothing code depends on.

- [ ] **Step 1: Write the failing test**

`report.rs` already has `the_table_reports_every_row_and_no_failures`, which lists the expected rows. Add the three new names to its list:

```rust
        for row in [
            "merge agreement",
            "refusal correctness",
            "transaction-time monotonicity",
            "arrival-order independence",
            "decision-layer time coverage",
            "applicability agreement",
            "depth monotonicity",
            "rescope keeps its history",
        ] {
            assert!(t.contains(row), "row missing from the table: {row}\n{t}");
        }
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p rm-conform --all-features the_table_reports_every_row`
Expected: FAIL — `row missing from the table: applicability agreement`.

- [ ] **Step 3: Add the rows**

In `crates/rm-conform/src/report.rs`, add to the imports:

```rust
use crate::applicability::{self, agreement, depth_monotonic, rescope_history};
```

and after the `decision-layer time coverage` row:

```rust
    // A smaller seed range than the merge sweep: each world builds a real
    // engine and records a dozen decisions through it, where the merge sweep
    // compares two pure functions.
    let scope_params = applicability::Params::default();
    out.push_str(&format!(
        "| applicability agreement | {} |\n",
        verdict(agreement(0..SCOPE_SEEDS, &scope_params))
    ));
    out.push_str(&format!(
        "| depth monotonicity | {} |\n",
        verdict(depth_monotonic(0..SCOPE_SEEDS, &scope_params))
    ));
    out.push_str(&format!(
        "| rescope keeps its history | {} |\n",
        verdict(rescope_history(0..SCOPE_SEEDS, &scope_params))
    ));
```

and beside `SEEDS`:

```rust
/// Seeds swept for the applicability rows.
///
/// Fewer than [`SEEDS`] and printed alongside it, because each one builds an
/// engine and writes a dozen decisions through the real command path rather
/// than comparing two pure functions. A number that was quietly smaller than
/// the one above it would overstate what was measured.
pub const SCOPE_SEEDS: u64 = 60;
```

Extend the header line so both counts are printed:

```rust
    out.push_str(&format!(
        "Seeds `0..{SEEDS}` for the merge sweep and `0..{SCOPE_SEEDS}` for the \
         applicability rows, params `{params:?}`, {} probes per history.\n\n",
        probes.len()
    ));
```

- [ ] **Step 4: Run the report**

Run: `cargo run --release -q -p rm-conform -- --report`
Expected: eight rows, all `1.000`, and the header naming both seed counts.

- [ ] **Step 5: Time it**

Run: `cargo build --release -q -p rm-conform` then time `./target/release/rm-conform --report`.
Expected: under **1.3s** total (0.27s was the previous whole-report time, and the budget is 1s added). If over, drop `SCOPE_SEEDS` to 30, re-time, and say so in the README — a silent cut reads as full coverage.

- [ ] **Step 6: Update the crate README**

`crates/rm-conform/README.md` carries the five-row table and the sentence "Seeds `0..500`, 20 probes per history, 12 assertions each." Replace the table with the eight-row output of Step 4, update that sentence to name both seed counts, and add to the "How ground truth is computed" section:

```markdown
The applicability rows have their own oracle, in `applicability.rs`, and it is
independent in a stricter sense than the survivorship one: it never imports
`rm_host::scope` at all, not even the `"*"` constant. Importing it would make
the oracle track a change to the rule silently. A test reads this module's own
source to assert the import never appears.
```

and to "What it found", since the row is new and green:

```markdown
The applicability rows were added after the fact, to a rule that had shipped
and was already governing a live store. They found nothing — which is the
result to expect from a sweep written after the unit tests rather than before,
and is worth recording precisely because a green row proves the measurement
exists, not that the measurement was hard to pass.
```

- [ ] **Step 7: Spellcheck and final verification**

Run: `typos crates/rm-conform/README.md && cargo test --workspace --all-features && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: all clean, count at 737 or above.

- [ ] **Step 8: Commit**

```bash
git add crates/rm-conform
git commit -m "Three rows for the rule that decides what you see"
```

---

## What this does not do

Carried from the spec so it is not lost between documents:

- **The live store.** Generated data only. The crate is free, deterministic and runs in CI on every push; pointing it at private records on one machine breaks all three.
- **The MCP layer.** The rule lives in `command::`; the MCP surface parses a position and passes it through, covered by its own tests.
- **`recall` and `about`.** Still unscoped, still a separate axis.
- **Fixing anything this finds.** Reported, not closed.
