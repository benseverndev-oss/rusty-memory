# rm-conform Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a conformance harness that scores this store on contradiction, supersession and time against ground truth computed independently of the code under test.

**Architecture:** A reference implementation of survivorship — small enough to read end to end — shadows `rm_survivor::merge` with an identical signature. A seeded generator produces histories; the harness compares the two implementations and shrinks any disagreement to a minimal case. Metamorphic invariants that need no oracle run on the same histories at the engine level.

**Tech Stack:** Rust 2021, workspace crate, no new external dependencies.

**Spec:** `docs/superpowers/specs/2026-08-24-rm-conform-design.md`

## Global Constraints

- **No new external dependencies.** The workspace's only third-party crates are `serde`, `serde_json`, `ureq`, `rustls`, `rustls-pemfile`, `webpki-roots`, `toml`. `rand` and `proptest` are NOT available and must not be added — the PRNG in Task 4 is written by hand for this reason.
- **Rust edition 2021, `rust-version = "1.89"`**, matching `[workspace.package]`.
- **Toolchain is pinned** by `rust-toolchain.toml`. Do not add a toolchain action or change the pin.
- **`cargo fmt` and `cargo clippy` must be clean** before every commit. CI runs both.
- **Seeds are fixed and printed.** A failure that cannot be reproduced from its seed is not a finding.
- **No LLM judge anywhere.** The reference model is the only oracle.
- **The reference model is written from the documented semantics** (the doc comments on `Strategy`), *not* by reading `rm_survivor`'s implementation. Reading the implementation to write its oracle destroys the experiment.
- Commit messages follow the repo convention: a prose title, no `feat:`/`fix:` prefixes. Trailers:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01Kc9AvVxcAwEa63HsQxz6pn
  ```

### Deviation from the spec's build order — deliberate

The spec lists build order 1–7 with `Engine::about` scoring at step 3. This plan reorders so that Tasks 1–6 target `rm_survivor::merge`, which is a **pure function** (`&[Candidate], &Strategy -> Result<Outcome, Refused>`) needing no engine, no index, no embedder and no store. The engine only arrives at Task 7.

Same content, cheaper first. The stopping point the spec asks for (still valuable if work halts) moves from Task 4 to **Task 6**: at that point the project has differential agreement and refusal correctness across all nine strategies.

### Types this plan uses, verified present

| Item | Path | Shape |
|---|---|---|
| `merge` | `rm_survivor` | `fn(&[Candidate<'_>], &Strategy) -> Result<Outcome, Refused>` |
| `Candidate<'a>` | `rm_survivor` | `{ value: Asserted<'a>, provenance: &'a Provenance, valid: Interval }` |
| `Asserted<'a>` | `rm_survivor` | `Value(&'a str) \| Absent \| Silent` |
| `Held` | `rm_survivor` | `Value(String) \| Absent` |
| `Fact` | `rm_survivor` | `{ value: Held, valid: Interval }` |
| `Outcome` | `rm_survivor` | `Survivor(Option<Held>) \| Timeline(Vec<Fact>)` |
| `Refused` | `rm_survivor` | `Refused(pub String)` |
| `Strategy` | `rm_survivor` | 9 variants, listed in Task 3 |
| `Interval` | `rm_core` | `{ from: Timestamp, to: Option<Timestamp> }`, half-open `[from, to)` |
| `Provenance` | `rm_core` | `{ source: Source, observed_at: Timestamp, source_ref: String }` |
| `Source` | `rm_core` | `UserAssertion \| ToolOutput \| AgentInference \| External(String)` |
| `Supersession` | `rm_core` | `Corrects \| Joins \| Unstated` |
| `Timestamp` | `rm_core` | `i64` |
| `Engine::about` | `rm_engine` | `fn(&self, StableId, &str, Timestamp, Timestamp) -> Result<Believed, EngineError>` |
| `Believed` | `rm_engine` | `Value(String) \| Absent \| Unknown` |
| `Standing` | `rm_engine` | `Latest \| Joined \| Corrected \| Unsettled`, plus `still_stands()` |

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/rm-conform/Cargo.toml` | Manifest. Depends on `rm-core`, `rm-survivor`, `rm-engine`, `rm-host`, `rm-index`, `rm-resolve`. |
| `crates/rm-conform/src/lib.rs` | Module wiring and the crate's own docs. |
| `crates/rm-conform/src/history.rs` | `Assertion` and `History` — the owned generated corpus. |
| `crates/rm-conform/src/reference.rs` | The oracle. Shadows `merge` with an identical signature. |
| `crates/rm-conform/src/rng.rs` | SplitMix64. Deterministic, seeded, ~20 lines. |
| `crates/rm-conform/src/generate.rs` | `Params` and `Generator`. |
| `crates/rm-conform/src/differential.rs` | Compare engine vs reference; shrink failures. |
| `crates/rm-conform/src/engine_harness.rs` | Build an `Engine` from a `History`; probe `about`. |
| `crates/rm-conform/src/invariants.rs` | Metamorphic properties. |
| `crates/rm-conform/src/decisions.rs` | Decision-layer chain and standing scoring. |
| `crates/rm-conform/src/report.rs` | The headline table. |
| `crates/rm-conform/src/main.rs` | `--report` binary. |

---

## Task 1: The crate, the history representation, and `MostRecent`

**Files:**
- Create: `crates/rm-conform/Cargo.toml`
- Create: `crates/rm-conform/src/lib.rs`
- Create: `crates/rm-conform/src/history.rs`
- Create: `crates/rm-conform/src/reference.rs`
- Modify: `Cargo.toml` (workspace `members`)

**Interfaces:**
- Consumes: `rm_core::{Interval, Provenance, Source, Supersession, Timestamp}`, `rm_survivor::{Asserted, Candidate, Held, Outcome, Refused, Strategy}`
- Produces: `rm_conform::history::Assertion`, `rm_conform::reference::merge`

- [ ] **Step 1: Create the manifest and register the crate**

`crates/rm-conform/Cargo.toml`:

```toml
[package]
name = "rm-conform"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
publish = false
description = "Scores the store on contradiction, supersession and time against an independent reference model"

[dependencies]
rm-core.workspace = true
rm-survivor.workspace = true
rm-engine.workspace = true
rm-host.workspace = true
rm-index.workspace = true
rm-resolve.workspace = true
```

In the root `Cargo.toml`, add `"crates/rm-conform",` to `[workspace.members]` after `"crates/rm-mcp",`, and add to `[workspace.dependencies]`:

```toml
rm-conform = { path = "crates/rm-conform" }
```

- [ ] **Step 2: Write the failing test for the owned assertion type**

`crates/rm-conform/src/history.rs`:

```rust
//! The generated corpus: assertions that own their data.
//!
//! `rm_survivor::Candidate` borrows its value and its provenance, which is
//! right for a merge that runs inside one function and wrong for a corpus that
//! has to outlive the call. This owns both and lends a `Candidate` on demand.

use rm_core::{Interval, Provenance, Source, Supersession, Timestamp};
use rm_survivor::Candidate;

/// One thing said about one attribute, at one time, about one span of time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assertion {
    /// `None` is a tombstone: the source said the attribute has no value.
    pub value: Option<String>,
    pub valid: Interval,
    pub provenance: Provenance,
    pub supersession: Supersession,
}

impl Assertion {
    /// A value asserted at `observed_at`, valid from `valid_from` onward.
    pub fn new(value: &str, valid_from: Timestamp, observed_at: Timestamp) -> Self {
        Assertion {
            value: Some(value.to_string()),
            valid: Interval::since(valid_from),
            provenance: Provenance::new(Source::UserAssertion, observed_at, "conform"),
            supersession: Supersession::Unstated,
        }
    }

    /// The borrowed form `rm_survivor::merge` takes.
    pub fn candidate(&self) -> Candidate<'_> {
        match &self.value {
            Some(v) => Candidate::new(Some(v.as_str()), &self.provenance).over(self.valid),
            None => Candidate::absent(&self.provenance).over(self.valid),
        }
    }
}
```

Add to `crates/rm-conform/src/lib.rs`:

```rust
//! Scores this store on what it claims to do.
//!
//! See `docs/superpowers/specs/2026-08-24-rm-conform-design.md`. The short
//! version: recall@10 is the wrong *kind* of metric for a correctness claim, so
//! the headline here is a claim to hold rather than a score to raise.

pub mod history;
pub mod reference;
```

Test, appended to `history.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rm_survivor::Asserted;

    #[test]
    fn an_assertion_lends_a_candidate_that_keeps_its_valid_span() {
        let a = Assertion::new("fly.io", 100, 500);
        let c = a.candidate();
        assert_eq!(c.value, Asserted::Value("fly.io"));
        assert_eq!(c.valid, Interval::since(100));
        assert_eq!(c.provenance.observed_at, 500);
    }

    #[test]
    fn a_tombstone_lends_an_absent_candidate_not_a_silent_one() {
        let a = Assertion {
            value: None,
            valid: Interval::since(100),
            provenance: Provenance::new(Source::UserAssertion, 500, "conform"),
            supersession: Supersession::Unstated,
        };
        assert_eq!(a.candidate().value, Asserted::Absent);
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p rm-conform`
Expected: FAIL — the crate does not compile yet if `lib.rs` is missing a module, or the assertions fail.

- [ ] **Step 4: Make them pass**

The code in Step 2 is the implementation. Fix compilation errors only; do not change the assertions.

- [ ] **Step 5: Write the failing test for the reference model's `MostRecent`**

`crates/rm-conform/src/reference.rs`. **Write this from the doc comments on `Strategy`, not from `rm_survivor`'s implementation.** The documented rule for `MostRecent` is:

> The most recently observed value. Refuses when the latest observation is a tie between different values: simultaneous contradictory assertions have no "most recent".

```rust
//! The oracle: survivorship implemented for auditability, not performance.
//!
//! Identical signature to `rm_survivor::merge`, so a differential test is one
//! comparison. Written from the documented semantics of each `Strategy`, never
//! from that crate's implementation — an oracle derived from the code it judges
//! is not an oracle.
//!
//! It reuses `rm_survivor`'s *data* types (`Outcome`, `Held`, `Interval`) while
//! implementing the *logic* independently. A bug in what `Interval` means would
//! therefore be shared; the metamorphic invariants in `invariants.rs` exist
//! partly to cover that.

use rm_core::Interval;
use rm_survivor::{Asserted, Candidate, Held, Outcome, Refused, Strategy};

/// What `rm_survivor::merge` should have returned.
pub fn merge(candidates: &[Candidate<'_>], strategy: &Strategy) -> Result<Outcome, Refused> {
    match strategy {
        Strategy::MostRecent => most_recent(candidates),
        _ => unimplemented!("later tasks"),
    }
}

/// Assertions only. Silence is not a claim and never competes.
fn claims<'a, 'b>(candidates: &'b [Candidate<'a>]) -> Vec<&'b Candidate<'a>> {
    candidates
        .iter()
        .filter(|c| c.value.is_assertion())
        .collect()
}

/// The owned form of what a candidate asserted. `Silent` never reaches here.
fn held(c: &Candidate<'_>) -> Held {
    match c.value {
        Asserted::Value(v) => Held::Value(v.to_string()),
        Asserted::Absent => Held::Absent,
        Asserted::Silent => unreachable!("filtered by claims()"),
    }
}

fn most_recent(candidates: &[Candidate<'_>]) -> Result<Outcome, Refused> {
    let claims = claims(candidates);
    if claims.is_empty() {
        return Ok(Outcome::Survivor(None));
    }
    let latest = claims
        .iter()
        .map(|c| c.provenance.observed_at)
        .max()
        .expect("non-empty");
    let mut at_latest: Vec<Held> = claims
        .iter()
        .filter(|c| c.provenance.observed_at == latest)
        .map(|c| held(c))
        .collect();
    at_latest.dedup_by(|a, b| a == b);
    let mut distinct = at_latest.clone();
    distinct.sort_by_key(|h| h.value().unwrap_or("\u{0}").to_string());
    distinct.dedup();
    if distinct.len() > 1 {
        return Err(Refused(
            "simultaneous contradictory assertions have no most recent".to_string(),
        ));
    }
    Ok(Outcome::Survivor(Some(distinct.remove(0))))
}
```

Add `pub mod reference;` to `lib.rs` (already in Step 2).

Tests, appended to `reference.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rm_core::{Provenance, Source};

    fn prov(at: i64) -> Provenance {
        Provenance::new(Source::UserAssertion, at, "t")
    }

    #[test]
    fn nothing_asserted_survives_as_nothing() {
        let out = merge(&[], &Strategy::MostRecent).unwrap();
        assert_eq!(out, Outcome::Survivor(None));
    }

    #[test]
    fn the_latest_observation_wins() {
        let (p1, p2) = (prov(100), prov(200));
        let cs = [
            Candidate::new(Some("fly.io"), &p1),
            Candidate::new(Some("render"), &p2),
        ];
        let out = merge(&cs, &Strategy::MostRecent).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Value("render".into()))));
    }

    #[test]
    fn a_tie_between_different_values_at_the_same_instant_refuses() {
        let (p1, p2) = (prov(200), prov(200));
        let cs = [
            Candidate::new(Some("fly.io"), &p1),
            Candidate::new(Some("render"), &p2),
        ];
        assert!(merge(&cs, &Strategy::MostRecent).is_err());
    }

    #[test]
    fn a_tie_on_the_same_value_is_not_a_contradiction() {
        let (p1, p2) = (prov(200), prov(200));
        let cs = [
            Candidate::new(Some("render"), &p1),
            Candidate::new(Some("render"), &p2),
        ];
        let out = merge(&cs, &Strategy::MostRecent).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Value("render".into()))));
    }

    #[test]
    fn silence_never_wins_however_late_it_arrives() {
        let (p1, p2) = (prov(100), prov(900));
        let cs = [
            Candidate::new(Some("fly.io"), &p1),
            Candidate::new(None, &p2), // Silent
        ];
        let out = merge(&cs, &Strategy::MostRecent).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Value("fly.io".into()))));
    }

    #[test]
    fn a_tombstone_is_a_claim_and_can_win() {
        let (p1, p2) = (prov(100), prov(900));
        let cs = [
            Candidate::new(Some("fly.io"), &p1),
            Candidate::absent(&p2),
        ];
        let out = merge(&cs, &Strategy::MostRecent).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Absent)));
    }
}
```

- [ ] **Step 6: Run the tests to verify they fail**

Run: `cargo test -p rm-conform reference`
Expected: FAIL — `unimplemented!` for anything not `MostRecent`, or assertion failures.

- [ ] **Step 7: Make them pass, then fmt and clippy**

Run:
```
cargo test -p rm-conform
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add crates/rm-conform Cargo.toml
git commit -F - <<'EOF'
An oracle small enough to believe

The reference model is the whole of this harness's claim to measure
anything: "known by construction" means nothing unless the expected
answer is computed without asking the code under test.

So it is written from the doc comments on Strategy rather than from
rm_survivor, and it is kept short enough to read end to end. It reuses
that crate's data types while implementing the logic independently --
a shared bug in what Interval means is the residual risk, and the
metamorphic invariants are aimed at it.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Kc9AvVxcAwEa63HsQxz6pn
EOF
```

---

## Task 2: The reference model's `ValidInterval` — the bi-temporal one

**Files:**
- Modify: `crates/rm-conform/src/reference.rs`

**Interfaces:**
- Consumes: `Assertion::candidate` from Task 1
- Produces: `reference::merge` handling `Strategy::ValidInterval`

This is the strategy the store is *for*. Its documented rule:

> Do not pick a winner. Emit each distinct value with the validity range over which it stood, inferred from observation order. Refuses when two different values share an observation timestamp: with no order between them there is no way to say which superseded which.

And `Outcome::Timeline` is documented as "a timeline of values with non-overlapping validity, oldest first."

- [ ] **Step 1: Write the failing tests, with the spans worked by hand**

Appended to `reference.rs` tests:

```rust
    #[test]
    fn a_timeline_tiles_valid_time_without_overlap() {
        // Two values, each valid from a stated point. The first is cut where
        // the second begins.
        let (p1, p2) = (prov(500), prov(600));
        let cs = [
            Candidate::new(Some("fly.io"), &p1).over(Interval::since(100)),
            Candidate::new(Some("render"), &p2).over(Interval::since(300)),
        ];
        let out = merge(&cs, &Strategy::ValidInterval).unwrap();
        assert_eq!(
            out,
            Outcome::Timeline(vec![
                Fact { value: Held::Value("fly.io".into()), valid: Interval::between(100, 300) },
                Fact { value: Held::Value("render".into()), valid: Interval::since(300) },
            ])
        );
    }

    #[test]
    fn a_backdated_correction_takes_effect_when_it_happened_not_when_it_was_said() {
        // The store's own motivating example. Told at t=900 that the value
        // changed at t=200, the timeline must say so from 200 -- not from 900.
        let (p1, p2) = (prov(100), prov(900));
        let cs = [
            Candidate::new(Some("fly.io"), &p1).over(Interval::since(100)),
            Candidate::new(Some("render"), &p2).over(Interval::since(200)),
        ];
        let out = merge(&cs, &Strategy::ValidInterval).unwrap();
        assert_eq!(
            out,
            Outcome::Timeline(vec![
                Fact { value: Held::Value("fly.io".into()), valid: Interval::between(100, 200) },
                Fact { value: Held::Value("render".into()), valid: Interval::since(200) },
            ])
        );
    }

    #[test]
    fn two_different_values_sharing_an_observation_instant_refuse() {
        let (p1, p2) = (prov(500), prov(500));
        let cs = [
            Candidate::new(Some("fly.io"), &p1).over(Interval::since(100)),
            Candidate::new(Some("render"), &p2).over(Interval::since(300)),
        ];
        assert!(merge(&cs, &Strategy::ValidInterval).is_err());
    }

    #[test]
    fn nothing_asserted_is_an_empty_timeline_not_a_refusal() {
        let out = merge(&[], &Strategy::ValidInterval).unwrap();
        assert_eq!(out, Outcome::Timeline(vec![]));
    }
```

Add `use rm_survivor::Fact;` to the test module's imports.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rm-conform reference::tests`
Expected: FAIL with `unimplemented!("later tasks")`.

- [ ] **Step 3: Implement `valid_interval`**

Replace the `_ => unimplemented!` arm with `Strategy::ValidInterval => valid_interval(candidates),` and add:

```rust
/// Ordered by when each value began to hold, ties broken by when it was heard.
///
/// Sorting by `valid.from` rather than `observed_at` is the whole difference
/// between a valid-time timeline and a transaction-time one wearing its name.
fn valid_interval(candidates: &[Candidate<'_>]) -> Result<Outcome, Refused> {
    let claims = claims(candidates);
    if claims.is_empty() {
        return Ok(Outcome::Timeline(vec![]));
    }

    // Refusal first: two different values heard at the same instant have no
    // order, so no timeline can say which replaced which.
    for a in &claims {
        for b in &claims {
            if a.provenance.observed_at == b.provenance.observed_at && held(a) != held(b) {
                return Err(Refused(
                    "two different values share an observation timestamp".to_string(),
                ));
            }
        }
    }

    let mut ordered: Vec<&&Candidate<'_>> = claims.iter().collect();
    ordered.sort_by_key(|c| (c.valid.from, c.provenance.observed_at));

    let mut facts: Vec<Fact> = Vec::new();
    for c in ordered {
        let value = held(c);
        // A repeat of the value already standing extends it rather than
        // starting a second span; the timeline holds *distinct* values.
        if facts.last().map(|f| &f.value) == Some(&value) {
            continue;
        }
        // Close the previous span where this one opens.
        if let Some(prev) = facts.last_mut() {
            prev.valid = Interval::between(prev.valid.from, c.valid.from);
        }
        facts.push(Fact { value, valid: Interval::since(c.valid.from) });
    }
    Ok(Outcome::Timeline(facts))
}
```

Add `use rm_survivor::Fact;` to the module's imports.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p rm-conform reference::tests`
Expected: PASS.

If `a_timeline_tiles_valid_time_without_overlap` fails on a zero-length span (two candidates sharing a `valid.from`), that is a genuine ambiguity in the documented semantics. Record it as an open question in the crate docs and pick the behaviour that keeps spans non-empty — do not silently paper over it.

- [ ] **Step 5: fmt, clippy, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p rm-conform
git add crates/rm-conform/src/reference.rs
git commit -F - <<'EOF'
The timeline the store exists to produce

ValidInterval is the strategy this project is for, and it is the one
whose reference implementation is worth writing carefully: sorting by
valid.from rather than observed_at is the entire difference between a
valid-time timeline and a transaction-time one wearing its name.

The backdating test is the store's own motivating example, stated as a
property rather than as a fixture.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Kc9AvVxcAwEa63HsQxz6pn
EOF
```

---

## Task 3: The remaining seven strategies

**Files:**
- Modify: `crates/rm-conform/src/reference.rs`

**Interfaces:**
- Produces: `reference::merge` total over all nine `Strategy` variants.

Documented semantics, copied from `rm_survivor`:

| Variant | Rule |
|---|---|
| `MostComplete` | Longest value wins; ties go to the first seen. |
| `LongestValue` | Alias of `MostComplete`. |
| `MajorityVote` | Most frequently asserted value wins; count ties go to the first seen. |
| `ConfidenceMajority` | Count-majority; no weighted form is implemented. |
| `FirstNonNull` | The first non-null assertion in input order. |
| `UnanimousOrNull` | The value if every non-null assertion agrees, otherwise nothing. |
| `SourcePriority(Vec<Source>)` | Highest-priority source that asserted one. **Refuses when any asserting source is absent from the priority list.** Within the winning source, ties resolve by `MostRecent`. |

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn most_complete_takes_the_longest_value_and_first_on_a_tie() {
        let (p1, p2, p3) = (prov(100), prov(200), prov(300));
        let cs = [
            Candidate::new(Some("aa"), &p1),
            Candidate::new(Some("bbbb"), &p2),
            Candidate::new(Some("cccc"), &p3),
        ];
        let out = merge(&cs, &Strategy::MostComplete).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Value("bbbb".into()))));
    }

    #[test]
    fn longest_value_is_the_same_rule_as_most_complete() {
        let (p1, p2) = (prov(100), prov(200));
        let cs = [Candidate::new(Some("aa"), &p1), Candidate::new(Some("bbb"), &p2)];
        assert_eq!(
            merge(&cs, &Strategy::LongestValue).unwrap(),
            merge(&cs, &Strategy::MostComplete).unwrap()
        );
    }

    #[test]
    fn majority_vote_counts_assertions_not_recency() {
        let (p1, p2, p3) = (prov(100), prov(200), prov(300));
        let cs = [
            Candidate::new(Some("fly.io"), &p1),
            Candidate::new(Some("fly.io"), &p2),
            Candidate::new(Some("render"), &p3),
        ];
        let out = merge(&cs, &Strategy::MajorityVote).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Value("fly.io".into()))));
    }

    #[test]
    fn first_non_null_takes_input_order_not_time_order() {
        let (p1, p2) = (prov(900), prov(100));
        let cs = [Candidate::new(Some("first"), &p1), Candidate::new(Some("second"), &p2)];
        let out = merge(&cs, &Strategy::FirstNonNull).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Value("first".into()))));
    }

    #[test]
    fn unanimous_or_null_yields_nothing_when_sources_disagree() {
        let (p1, p2) = (prov(100), prov(200));
        let cs = [Candidate::new(Some("fly.io"), &p1), Candidate::new(Some("render"), &p2)];
        assert_eq!(merge(&cs, &Strategy::UnanimousOrNull).unwrap(), Outcome::Survivor(None));
    }

    #[test]
    fn unanimous_or_null_yields_the_value_when_they_agree() {
        let (p1, p2) = (prov(100), prov(200));
        let cs = [Candidate::new(Some("render"), &p1), Candidate::new(Some("render"), &p2)];
        assert_eq!(
            merge(&cs, &Strategy::UnanimousOrNull).unwrap(),
            Outcome::Survivor(Some(Held::Value("render".into())))
        );
    }

    #[test]
    fn source_priority_refuses_an_asserting_source_it_was_not_told_how_to_rank() {
        let p1 = Provenance::new(Source::AgentInference, 100, "t");
        let cs = [Candidate::new(Some("guess"), &p1)];
        let ranked = Strategy::SourcePriority(vec![Source::UserAssertion]);
        assert!(merge(&cs, &ranked).is_err());
    }

    #[test]
    fn source_priority_prefers_the_higher_ranked_source_however_old() {
        let p1 = Provenance::new(Source::UserAssertion, 100, "t");
        let p2 = Provenance::new(Source::ToolOutput, 900, "t");
        let cs = [Candidate::new(Some("stated"), &p1), Candidate::new(Some("fetched"), &p2)];
        let ranked = Strategy::SourcePriority(vec![Source::UserAssertion, Source::ToolOutput]);
        assert_eq!(
            merge(&cs, &ranked).unwrap(),
            Outcome::Survivor(Some(Held::Value("stated".into())))
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rm-conform reference::tests`
Expected: FAIL with `unimplemented!`.

- [ ] **Step 3: Implement the remaining arms**

```rust
        Strategy::MostComplete | Strategy::LongestValue => most_complete(candidates),
        Strategy::MajorityVote | Strategy::ConfidenceMajority => majority(candidates),
        Strategy::FirstNonNull => first_non_null(candidates),
        Strategy::UnanimousOrNull => unanimous(candidates),
        Strategy::SourcePriority(order) => source_priority(candidates, order),
```

```rust
fn most_complete(candidates: &[Candidate<'_>]) -> Result<Outcome, Refused> {
    let claims = claims(candidates);
    let mut best: Option<Held> = None;
    for c in &claims {
        let h = held(c);
        let len = h.value().map(str::len).unwrap_or(0);
        let better = match &best {
            None => true,
            Some(b) => len > b.value().map(str::len).unwrap_or(0),
        };
        if better {
            best = Some(h);
        }
    }
    Ok(Outcome::Survivor(best))
}

fn majority(candidates: &[Candidate<'_>]) -> Result<Outcome, Refused> {
    let claims = claims(candidates);
    let mut counts: Vec<(Held, usize)> = Vec::new();
    for c in &claims {
        let h = held(c);
        match counts.iter_mut().find(|(k, _)| *k == h) {
            Some((_, n)) => *n += 1,
            None => counts.push((h, 1)),
        }
    }
    let best = counts.into_iter().max_by_key(|(_, n)| *n).map(|(h, _)| h);
    Ok(Outcome::Survivor(best))
}

fn first_non_null(candidates: &[Candidate<'_>]) -> Result<Outcome, Refused> {
    let first = claims(candidates)
        .into_iter()
        .find(|c| matches!(c.value, Asserted::Value(_)))
        .map(|c| held(c));
    Ok(Outcome::Survivor(first))
}

fn unanimous(candidates: &[Candidate<'_>]) -> Result<Outcome, Refused> {
    let values: Vec<Held> = claims(candidates)
        .into_iter()
        .filter(|c| matches!(c.value, Asserted::Value(_)))
        .map(|c| held(c))
        .collect();
    match values.first() {
        None => Ok(Outcome::Survivor(None)),
        Some(first) if values.iter().all(|v| v == first) => {
            Ok(Outcome::Survivor(Some(first.clone())))
        }
        _ => Ok(Outcome::Survivor(None)),
    }
}

fn source_priority(
    candidates: &[Candidate<'_>],
    order: &[rm_core::Source],
) -> Result<Outcome, Refused> {
    let claims = claims(candidates);
    for c in &claims {
        if !order.contains(&c.provenance.source) {
            return Err(Refused(
                "an asserting source is absent from the priority list".to_string(),
            ));
        }
    }
    for source in order {
        let at_source: Vec<Candidate<'_>> = claims
            .iter()
            .filter(|c| c.provenance.source == *source)
            .map(|c| (*c).clone())
            .collect();
        if !at_source.is_empty() {
            return most_recent(&at_source);
        }
    }
    Ok(Outcome::Survivor(None))
}
```

`max_by_key` returns the *last* maximum on a tie, which contradicts "count ties go to the first seen". Guard it by iterating explicitly if the majority test fails on a tie — write a tie test before assuming.

- [ ] **Step 4: Run, fmt, clippy**

Run: `cargo test -p rm-conform && cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-conform/src/reference.rs
git commit -F - <<'EOF'
The other seven, including the two that refuse

SourcePriority refusing an unranked source is the interesting one: it
is a refusal that exists to stop the store silently preferring the
wrong system of record, and nothing has ever tested that it fires
exactly when it should.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Kc9AvVxcAwEa63HsQxz6pn
EOF
```

---

## Task 4: A seeded generator

**Files:**
- Create: `crates/rm-conform/src/rng.rs`
- Create: `crates/rm-conform/src/generate.rs`
- Modify: `crates/rm-conform/src/lib.rs`

**Interfaces:**
- Consumes: `history::Assertion`
- Produces: `rng::Rng::{new, next_u64, below, chance}`, `generate::{Params, generate}`

- [ ] **Step 1: Write the failing test for the PRNG**

`crates/rm-conform/src/rng.rs`:

```rust
//! SplitMix64. Written here because the workspace takes no `rand` dependency
//! and this needs twenty lines, not a crate.
//!
//! Determinism is the requirement: a failure that cannot be reproduced from
//! its seed is not a finding.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `[0, n)`. Panics on `n == 0`, which is a caller bug.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "below(0) has no answer");
        self.next_u64() % n
    }

    /// True with probability `percent/100`.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_sequence() {
        let a: Vec<u64> = (0..8).map(|_| Rng::new(42).next_u64()).collect();
        let mut r = Rng::new(42);
        let b: Vec<u64> = (0..8).map(|_| { let mut s = Rng::new(42); s.next_u64() }).collect();
        assert_eq!(a, b);
        let first = r.next_u64();
        assert_eq!(first, Rng::new(42).next_u64());
    }

    #[test]
    fn different_seeds_diverge() {
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
    }

    #[test]
    fn below_stays_in_range() {
        let mut r = Rng::new(7);
        for _ in 0..1000 {
            assert!(r.below(5) < 5);
        }
    }
}
```

- [ ] **Step 2: Run to verify it fails, then passes**

Run: `cargo test -p rm-conform rng`
Expected: FAIL (module not declared), then PASS after adding `pub mod rng;` to `lib.rs`.

- [ ] **Step 3: Write the failing test for the generator**

`crates/rm-conform/src/generate.rs`:

```rust
//! Generated histories.
//!
//! Difficulty is a knob rather than a rewrite. Timestamp ties get their own
//! parameter because three strategies are specified to refuse on them, and a
//! generator that never produces one would never reach that code.

use crate::history::Assertion;
use crate::rng::Rng;
use rm_core::{Interval, Provenance, Source, Supersession};

#[derive(Clone, Debug)]
pub struct Params {
    pub len: usize,
    /// How many distinct values compete. Small, so collisions are frequent.
    pub alphabet: u64,
    /// Percent of assertions whose valid time precedes their observation.
    pub backdate_pct: u64,
    /// Percent of assertions that reuse the previous observation timestamp.
    pub tie_pct: u64,
    /// Percent of assertions that are tombstones.
    pub tombstone_pct: u64,
}

impl Default for Params {
    fn default() -> Self {
        Params { len: 12, alphabet: 4, backdate_pct: 30, tie_pct: 15, tombstone_pct: 10 }
    }
}

/// A history of `params.len` assertions, reproducible from `seed`.
pub fn generate(seed: u64, params: &Params) -> Vec<Assertion> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(params.len);
    let mut clock: i64 = 1_000;

    for _ in 0..params.len {
        let tie = rng.chance(params.tie_pct) && !out.is_empty();
        if !tie {
            clock += 1 + rng.below(50) as i64;
        }
        let observed_at = clock;

        let valid_from = if rng.chance(params.backdate_pct) {
            observed_at - 1 - rng.below(500) as i64
        } else {
            observed_at
        };

        let value = if rng.chance(params.tombstone_pct) {
            None
        } else {
            Some(format!("v{}", rng.below(params.alphabet)))
        };

        let supersession = match rng.below(3) {
            0 => Supersession::Corrects,
            1 => Supersession::Joins,
            _ => Supersession::Unstated,
        };

        out.push(Assertion {
            value,
            valid: Interval::since(valid_from),
            provenance: Provenance::new(Source::UserAssertion, observed_at, "conform"),
            supersession,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_history() {
        let p = Params::default();
        assert_eq!(generate(99, &p), generate(99, &p));
    }

    #[test]
    fn different_seeds_give_different_histories() {
        let p = Params::default();
        assert_ne!(generate(1, &p), generate(2, &p));
    }

    #[test]
    fn it_produces_the_requested_length() {
        let p = Params { len: 25, ..Params::default() };
        assert_eq!(generate(5, &p).len(), 25);
    }

    #[test]
    fn ties_actually_occur_at_the_configured_rate() {
        // The refusal paths are unreachable without them, so assert they exist
        // rather than hoping.
        let p = Params { len: 200, tie_pct: 50, ..Params::default() };
        let h = generate(3, &p);
        let mut seen_tie = false;
        for w in h.windows(2) {
            if w[0].provenance.observed_at == w[1].provenance.observed_at {
                seen_tie = true;
            }
        }
        assert!(seen_tie, "no timestamp tie in 200 assertions at tie_pct=50");
    }

    #[test]
    fn backdating_actually_occurs() {
        let p = Params { len: 200, backdate_pct: 50, ..Params::default() };
        let h = generate(4, &p);
        assert!(h.iter().any(|a| a.valid.from < a.provenance.observed_at));
    }
}
```

Add `pub mod generate;` to `lib.rs`.

- [ ] **Step 4: Run to verify, then fmt, clippy, commit**

Run: `cargo test -p rm-conform && cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`

```bash
git add crates/rm-conform/src/rng.rs crates/rm-conform/src/generate.rs crates/rm-conform/src/lib.rs
git commit -F - <<'EOF'
Histories, reproducible from a seed

Twenty lines of SplitMix64 rather than a rand dependency, for the same
reason rm-embed exists: the workspace pays for its dependencies
deliberately.

Ties and backdating have assertions of their own. Three strategies are
specified to refuse on a timestamp tie, and a generator that never
produced one would leave that code unreached while reporting a green
suite.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Kc9AvVxcAwEa63HsQxz6pn
EOF
```

---

## Task 5: Differential agreement, with shrinking

**Files:**
- Create: `crates/rm-conform/src/differential.rs`
- Modify: `crates/rm-conform/src/lib.rs`

**Interfaces:**
- Consumes: `reference::merge`, `generate::generate`, `history::Assertion`
- Produces: `differential::{Disagreement, check_history, shrink, sweep}`

- [ ] **Step 1: Write the failing test**

```rust
//! Engine against oracle, and the smallest history that separates them.

use crate::generate::{generate, Params};
use crate::history::Assertion;
use crate::reference;
use rm_survivor::{merge as engine_merge, Strategy};

/// A history on which the two implementations differ.
#[derive(Clone, Debug)]
pub struct Disagreement {
    pub seed: u64,
    pub strategy: String,
    pub history: Vec<Assertion>,
    pub engine: String,
    pub reference: String,
}

/// Whether the two agree on this history. Refusals compare as refusals:
/// the property is "refuses if and only if the reference refuses", never
/// that the two wrote the same sentence.
pub fn agrees(history: &[Assertion], strategy: &Strategy) -> bool {
    let candidates: Vec<_> = history.iter().map(|a| a.candidate()).collect();
    match (engine_merge(&candidates, strategy), reference::merge(&candidates, strategy)) {
        (Ok(a), Ok(b)) => a == b,
        (Err(_), Err(_)) => true,
        _ => false,
    }
}

/// The shortest prefix-and-deletion reduction that still disagrees.
pub fn shrink(history: &[Assertion], strategy: &Strategy) -> Vec<Assertion> {
    let mut best = history.to_vec();
    let mut improved = true;
    while improved {
        improved = false;
        for i in 0..best.len() {
            let mut candidate = best.clone();
            candidate.remove(i);
            if !candidate.is_empty() && !agrees(&candidate, strategy) {
                best = candidate;
                improved = true;
                break;
            }
        }
    }
    best
}

/// Every seed in `seeds`, every strategy in `strategies`.
pub fn sweep(seeds: impl Iterator<Item = u64>, params: &Params, strategies: &[Strategy]) -> Vec<Disagreement> {
    let mut found = Vec::new();
    for seed in seeds {
        let history = generate(seed, params);
        for strategy in strategies {
            if agrees(&history, strategy) {
                continue;
            }
            let minimal = shrink(&history, strategy);
            let candidates: Vec<_> = minimal.iter().map(|a| a.candidate()).collect();
            found.push(Disagreement {
                seed,
                strategy: format!("{strategy:?}"),
                engine: format!("{:?}", engine_merge(&candidates, strategy)),
                reference: format!("{:?}", reference::merge(&candidates, strategy)),
                history: minimal,
            });
        }
    }
    found
}

/// The strategies scored by default. `SourcePriority` is excluded here and
/// covered in Task 6, because it needs a priority list to be meaningful.
pub fn default_strategies() -> Vec<Strategy> {
    vec![
        Strategy::MostRecent,
        Strategy::ValidInterval,
        Strategy::MostComplete,
        Strategy::LongestValue,
        Strategy::MajorityVote,
        Strategy::ConfidenceMajority,
        Strategy::FirstNonNull,
        Strategy::UnanimousOrNull,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_implementations_agree_across_the_fixed_seed_set() {
        let params = Params::default();
        let found = sweep(0..200, &params, &default_strategies());
        assert!(
            found.is_empty(),
            "{} disagreement(s); first: {:#?}",
            found.len(),
            found.first()
        );
    }

    #[test]
    fn shrinking_reduces_a_known_disagreement_to_its_minimum() {
        // A deliberately wrong strategy pairing proves the machinery finds and
        // minimises a difference, rather than the suite passing because
        // nothing is ever compared.
        let history = generate(11, &Params::default());
        let disagreeing = |h: &[Assertion]| {
            let cs: Vec<_> = h.iter().map(|a| a.candidate()).collect();
            engine_merge(&cs, &Strategy::MostRecent) != reference::merge(&cs, &Strategy::FirstNonNull)
        };
        assert!(disagreeing(&history), "seed 11 should separate these two rules");
    }
}
```

Add `pub mod differential;` to `lib.rs`.

- [ ] **Step 2: Run it**

Run: `cargo test -p rm-conform differential -- --nocapture`
Expected: **Either outcome is a result.** PASS means 200 seeds × 8 strategies agree. FAIL prints a minimised history and is the first real finding — investigate which side is wrong before changing either.

- [ ] **Step 3: If a disagreement is found, triage it before fixing**

Do not assume the engine is wrong. Work the minimal case by hand against the doc comment for that strategy and decide which implementation contradicts the documentation. Record the conclusion in `crates/rm-conform/src/reference.rs` docs, and freeze the minimal history as a named `#[test]` in `reference.rs` regardless of which side changed.

- [ ] **Step 4: fmt, clippy, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p rm-conform
git add crates/rm-conform/src/differential.rs crates/rm-conform/src/lib.rs
git commit -F - <<'EOF'
The first correctness number this project has had

Refusals compare as refusals, never by message: the property is that
the two refuse on the same inputs, not that they chose the same
sentence.

Shrinking is what makes a disagreement useful. A random history that
fails tells you nothing; the three assertions that still fail tell you
which rule is wrong.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Kc9AvVxcAwEa63HsQxz6pn
EOF
```

---

## Task 6: Refusal correctness

**Files:**
- Modify: `crates/rm-conform/src/differential.rs`

**Interfaces:**
- Produces: `differential::{refusal_agreement, RefusalScore}`

The property has two failure modes a single number hides: refusing when it should not (useless) and answering when it should refuse (silently wrong). Score them separately.

- [ ] **Step 1: Write the failing test**

```rust
/// How the two implementations' refusals line up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefusalScore {
    pub both_refused: usize,
    pub both_answered: usize,
    /// The engine refused where the reference answered. The store is useless
    /// here: it had an answer available and declined to give it.
    pub engine_only: usize,
    /// The engine answered where the reference refused. The store is silently
    /// wrong here, which is the worse of the two.
    pub reference_only: usize,
}

impl RefusalScore {
    pub fn exact(&self) -> bool {
        self.engine_only == 0 && self.reference_only == 0
    }
}

/// Refusal agreement over `seeds`, with ties turned up so the refusal paths
/// are actually reached.
pub fn refusal_agreement(seeds: impl Iterator<Item = u64>, strategies: &[Strategy]) -> RefusalScore {
    let params = Params { len: 10, alphabet: 3, tie_pct: 60, ..Params::default() };
    let mut score = RefusalScore::default();
    for seed in seeds {
        let history = generate(seed, &params);
        let candidates: Vec<_> = history.iter().map(|a| a.candidate()).collect();
        for strategy in strategies {
            let e = engine_merge(&candidates, strategy).is_err();
            let r = reference::merge(&candidates, strategy).is_err();
            match (e, r) {
                (true, true) => score.both_refused += 1,
                (false, false) => score.both_answered += 1,
                (true, false) => score.engine_only += 1,
                (false, true) => score.reference_only += 1,
            }
        }
    }
    score
}
```

Tests:

```rust
    #[test]
    fn refusals_line_up_exactly() {
        let score = refusal_agreement(0..300, &default_strategies());
        assert!(score.exact(), "{score:?}");
    }

    #[test]
    fn the_refusal_paths_are_actually_reached() {
        // A score of all-answered would make the test above vacuous.
        let score = refusal_agreement(0..300, &default_strategies());
        assert!(score.both_refused > 0, "no refusal in 300 seeds: {score:?}");
    }

    #[test]
    fn source_priority_refuses_together_on_unranked_sources() {
        let params = Params::default();
        let ranked = Strategy::SourcePriority(vec![rm_core::Source::UserAssertion]);
        let score = refusal_agreement(0..100, &[ranked]);
        assert!(score.exact(), "{score:?}");
    }
```

- [ ] **Step 2: Run**

Run: `cargo test -p rm-conform differential -- --nocapture`
Expected: PASS, with `both_refused > 0`.

`the_refusal_paths_are_actually_reached` is the important one. Without it, a suite where nothing ever refuses reports perfect refusal correctness while testing nothing.

- [ ] **Step 3: fmt, clippy, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p rm-conform
git add crates/rm-conform/src/differential.rs
git commit -F - <<'EOF'
Refusing exactly when it should, and a test that the path is reached

Two failure modes, counted separately, because they are not equally
bad: refusing where an answer existed makes the store useless, and
answering where it should have refused makes it silently wrong.

The second test is the one that matters. A suite in which nothing ever
refuses reports perfect refusal correctness and has measured nothing.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Kc9AvVxcAwEa63HsQxz6pn
EOF
```

**This is the stopping point.** If work halts here the project has differential agreement and refusal correctness across all nine strategies — the first correctness numbers it has ever had.

---

## Task 7: The engine harness

**Files:**
- Create: `crates/rm-conform/src/engine_harness.rs`
- Modify: `crates/rm-conform/src/lib.rs`

**Interfaces:**
- Consumes: `history::Assertion`
- Produces: `engine_harness::{build, probe_agreement}`

Everything above tested `rm_survivor` alone. This tests the read path on top of it: history assembly and `as_of` filtering.

Two facts that shape the code:
- `Engine::remember_as(Some(entity), obs)` pins the entity, bypassing resolution entirely. The spec puts resolution out of scope, and this is how it stays out.
- Embeddings are irrelevant to survivorship, so every observation gets the same fixed 3-vector.

- [ ] **Step 1: Write the failing test**

```rust
//! The read path, not just the merge underneath it.

use crate::history::Assertion;
use rm_engine::{Believed, Engine, Observation, Policy, Strategy};
use rm_index::{Metric, VectorIndex};
use rm_resolve::{BlockingKey, Comparator, FieldRule, Record, Ruleset};
use rm_core::Timestamp;

/// A ruleset that resolves nothing interesting. Entities are pinned by id, so
/// this exists only because `Engine::new` requires one.
fn ruleset() -> Ruleset {
    Ruleset::new(
        vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
        vec![BlockingKey::Prefix("name".to_string(), 3)],
        4.0,
        8.0,
    )
    .expect("a one-field ruleset is valid")
}

/// An engine holding `history` on one attribute of one entity.
///
/// Every assertion is pinned to the same entity with `remember_as`, so
/// resolution never runs — it is out of scope for this harness and generated
/// names would measure the generator, not the resolver.
pub fn build(history: &[Assertion], attribute: &str, strategy: Strategy) -> (Engine, rm_engine::StableId) {
    let mut engine = Engine::new(VectorIndex::new(3, Metric::Cosine), ruleset(), Policy::new(strategy));
    let mut entity = None;
    for a in history {
        let obs = Observation {
            kind: "thing".to_string(),
            mention: Record::new().with("name", "subject"),
            attribute: attribute.to_string(),
            value: a.value.clone(),
            valid: a.valid,
            provenance: a.provenance.clone(),
            supersession: a.supersession,
            embedding: vec![1.0, 0.0, 0.0],
        };
        let (id, _) = engine.remember_as(entity, obs).expect("pinned write");
        entity = Some(id);
    }
    (engine, entity.expect("history is non-empty"))
}

/// `Believed` compared with what the reference says held at `valid_t`, given
/// only what was observed at or before `tx_t`.
pub fn probe_agreement(history: &[Assertion], valid_t: Timestamp, tx_t: Timestamp) -> bool {
    let (engine, id) = build(history, "attr", Strategy::MostRecent);
    let believed = engine.about(id, "attr", valid_t, tx_t).expect("known entity");

    let visible: Vec<Assertion> = history
        .iter()
        .filter(|a| a.provenance.observed_at <= tx_t && a.valid.contains(valid_t))
        .cloned()
        .collect();
    let candidates: Vec<_> = visible.iter().map(|a| a.candidate()).collect();

    let expected = match crate::reference::merge(&candidates, &Strategy::MostRecent) {
        Ok(rm_survivor::Outcome::Survivor(Some(rm_survivor::Held::Value(v)))) => Believed::Value(v),
        Ok(rm_survivor::Outcome::Survivor(Some(rm_survivor::Held::Absent))) => Believed::Absent,
        Ok(_) => Believed::Unknown,
        Err(_) => return true, // a refusal is scored in Task 6, not here
    };
    believed == expected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::{generate, Params};

    #[test]
    fn a_backdated_fact_is_true_from_when_it_happened() {
        let history = vec![
            Assertion::new("fly.io", 100, 100),
            Assertion::new("render", 200, 900),
        ];
        let (engine, id) = build(&history, "attr", Strategy::MostRecent);
        // Asked about t=250 knowing everything: the backdated correction holds.
        assert_eq!(
            engine.about(id, "attr", 250, 1000).unwrap(),
            Believed::Value("render".to_string())
        );
        // Asked about t=250 knowing only what was said by t=500: not yet heard.
        assert_eq!(
            engine.about(id, "attr", 250, 500).unwrap(),
            Believed::Value("fly.io".to_string())
        );
    }

    #[test]
    fn the_read_path_agrees_with_the_reference_across_probes() {
        let params = Params::default();
        for seed in 0..50 {
            let history = generate(seed, &params);
            for valid_t in [900, 1_050, 1_200, 1_500, 3_000] {
                for tx_t in [1_000, 1_100, 1_400, 5_000] {
                    assert!(
                        probe_agreement(&history, valid_t, tx_t),
                        "seed {seed} valid_t {valid_t} tx_t {tx_t}"
                    );
                }
            }
        }
    }
}
```

Add `pub mod engine_harness;` to `lib.rs`.

- [ ] **Step 2: Run**

Run: `cargo test -p rm-conform engine_harness -- --nocapture`
Expected: PASS, or a printed `(seed, valid_t, tx_t)` triple to investigate.

`Interval::contains` is used above — confirm it exists on `rm_core::Interval` and takes a `Timestamp`. If it does not, filter with `a.valid.from <= valid_t && a.valid.to.map_or(true, |to| valid_t < to)`, which is the half-open rule `[from, to)` stated in the type's own docs.

- [ ] **Step 3: fmt, clippy, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p rm-conform
git add crates/rm-conform/src/engine_harness.rs crates/rm-conform/src/lib.rs
git commit -F - <<'EOF'
The read path, not just the merge underneath it

Entities are pinned with remember_as rather than resolved, so
resolution stays out of scope exactly as the design said it should:
generated names would measure the generator's name distribution and
call it a resolver score.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Kc9AvVxcAwEa63HsQxz6pn
EOF
```

---

## Task 8: Metamorphic invariants

**Files:**
- Create: `crates/rm-conform/src/invariants.rs`
- Modify: `crates/rm-conform/src/lib.rs`

**Interfaces:**
- Consumes: `engine_harness::build`, `generate::generate`
- Produces: `invariants::{monotonic_in_transaction_time, order_independent}`

These need no oracle. They are derived from what bi-temporality *means*, which is why they can catch a bug the reference model shares with the engine.

- [ ] **Step 1: Write the failing tests**

```rust
//! Properties that hold whatever the right answer is.
//!
//! The reference model and the engine were written by the same author against
//! the same mental model, so they can agree enthusiastically on a shared
//! misunderstanding. These are derived from the meaning of bi-temporality
//! rather than from either implementation, which is the mitigation.

use crate::engine_harness::build;
use crate::generate::{generate, Params};
use crate::history::Assertion;
use rm_engine::Strategy;
use rm_core::Timestamp;

/// Learning something today must not change what you believed last Tuesday.
///
/// The defining property of the transaction axis, and nothing else asserts it.
pub fn monotonic_in_transaction_time(history: &[Assertion], cut: usize, probes: &[(Timestamp, Timestamp)]) -> bool {
    if cut >= history.len() {
        return true;
    }
    let prefix = &history[..cut];
    if prefix.is_empty() {
        return true;
    }
    let (before, id_b) = build(prefix, "attr", Strategy::MostRecent);
    let (after, id_a) = build(history, "attr", Strategy::MostRecent);

    // Only ask about transaction times at or before the cut: what was known
    // then cannot be changed by what arrived later.
    let cutoff = prefix
        .iter()
        .map(|a| a.provenance.observed_at)
        .max()
        .expect("non-empty prefix");

    probes.iter().filter(|(_, tx)| *tx <= cutoff).all(|(valid_t, tx_t)| {
        before.about(id_b, "attr", *valid_t, *tx_t).ok()
            == after.about(id_a, "attr", *valid_t, *tx_t).ok()
    })
}

/// Ingestion order must not change belief when observation times are fixed.
pub fn order_independent(history: &[Assertion], probes: &[(Timestamp, Timestamp)]) -> bool {
    let mut reversed = history.to_vec();
    reversed.reverse();
    let (a, id_a) = build(history, "attr", Strategy::MostRecent);
    let (b, id_b) = build(&reversed, "attr", Strategy::MostRecent);
    probes.iter().all(|(valid_t, tx_t)| {
        a.about(id_a, "attr", *valid_t, *tx_t).ok() == b.about(id_b, "attr", *valid_t, *tx_t).ok()
    })
}

fn probe_grid() -> Vec<(Timestamp, Timestamp)> {
    let mut out = Vec::new();
    for valid_t in [900, 1_050, 1_200, 1_600, 4_000] {
        for tx_t in [1_000, 1_100, 1_500, 6_000] {
            out.push((valid_t, tx_t));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_arrivals_never_change_earlier_belief() {
        let params = Params::default();
        let probes = probe_grid();
        for seed in 0..60 {
            let history = generate(seed, &params);
            for cut in [1, 3, 6, 9] {
                assert!(
                    monotonic_in_transaction_time(&history, cut, &probes),
                    "seed {seed} cut {cut}"
                );
            }
        }
    }

    #[test]
    fn reversing_ingestion_order_changes_nothing() {
        let params = Params::default();
        let probes = probe_grid();
        for seed in 0..60 {
            let history = generate(seed, &params);
            assert!(order_independent(&history, &probes), "seed {seed}");
        }
    }
}
```

Add `pub mod invariants;` to `lib.rs`.

- [ ] **Step 2: Run**

Run: `cargo test -p rm-conform invariants -- --nocapture`
Expected: PASS, or a seed to investigate.

`reversing_ingestion_order_changes_nothing` is the one most likely to find something — order dependence is the property most easily broken by an optimisation. If it fails, that is a real finding about the store, not about this harness.

- [ ] **Step 3: fmt, clippy, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p rm-conform
git add crates/rm-conform/src/invariants.rs crates/rm-conform/src/lib.rs
git commit -F - <<'EOF'
Two properties that hold whatever the right answer is

The reference model and the engine share an author and a mental model,
so they can agree enthusiastically on the same misunderstanding. These
two are derived from what bi-temporality means rather than from either
implementation, which is the only cover for that.

Learning something today must not change what you believed last
Tuesday. Ingestion order must not change belief when observation times
are fixed.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Kc9AvVxcAwEa63HsQxz6pn
EOF
```

---

## Task 9: Decision-layer chains and standing

**Files:**
- Create: `crates/rm-conform/src/decisions.rs`
- Modify: `crates/rm-conform/src/lib.rs`

**Interfaces:**
- Consumes: `rm_host::command::{decide, decision}`, `rm_host::command::DecisionDetail`
- Produces: `decisions::{build_chain, time_coverage}`

Two facts verified against the source, both of which make this task simpler than it looks:

- **`find_decision` (`command.rs:1055`) is an exact string match** on the identity record's `name` field — no vector search. So the 3-dimension index from Task 7 works here and the chain test cannot be retrieval-flaky.
- **`DecisionDetail` exposes `still_stands: bool`** directly, plus `supersedes: Vec<(StableId, String)>` and `superseded_by: Vec<(StableId, String)>`. No `Standing` accessor is needed.

`decide` needs an `&impl Embedder`; use `rm_embed::Hashed::new(3)` to match the index. Add `rm-embed.workspace = true` to `crates/rm-conform/Cargo.toml`.

- [ ] **Step 1: Add the dependency**

In `crates/rm-conform/Cargo.toml`, add to `[dependencies]`:

```toml
rm-embed.workspace = true
```

- [ ] **Step 2: Write the failing tests**

`crates/rm-conform/src/decisions.rs`:

```rust
//! The decision layer: chains, standing, and the temporal question it cannot
//! be asked.

use rm_embed::Hashed;
use rm_engine::{Engine, Policy, Strategy};
use rm_host::command::{self, DecisionDetail, Outcome};
use rm_index::{Metric, VectorIndex};
use rm_resolve::{BlockingKey, Comparator, FieldRule, Ruleset};

fn ruleset() -> Ruleset {
    Ruleset::new(
        vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
        vec![BlockingKey::Prefix("name".to_string(), 3)],
        4.0,
        8.0,
    )
    .expect("a one-field ruleset is valid")
}

fn engine() -> Engine {
    Engine::new(
        VectorIndex::new(3, Metric::Cosine),
        ruleset(),
        Policy::new(Strategy::MostRecent),
    )
}

/// Record `titles` as a chain, each superseding the one before it.
pub fn build_chain(titles: &[&str]) -> Engine {
    let mut e = engine();
    let embedder = Hashed::new(3);
    let mut observed_at = 1_000;
    let mut previous: Option<&str> = None;
    for title in titles {
        command::decide(
            &mut e,
            title,
            "the chosen option",
            None,                // status: defaults to accepted
            Some("a stated reason"),
            None,                // context
            previous,            // supersedes
            None,                // decided_at: defaults to observed_at
            observed_at,
            "conform",
            &embedder,
        )
        .expect("a decision with a fresh title is recorded");
        previous = Some(title);
        observed_at += 100;
    }
    e
}

fn detail(e: &Engine, title: &str) -> DecisionDetail {
    match command::decision(e, title).expect("a recorded title resolves") {
        Outcome::Decision(Some(d)) => d,
        other => panic!("expected a decision for {title:?}, got {other:?}"),
    }
}

/// The fraction of a bi-temporal probe set the decision API can answer.
///
/// Zero, and stated as a computed number rather than as prose: `decisions` and
/// `decision` take no time parameters, so there is no probe they can answer.
/// This changes visibly the day somebody adds `--valid-at` / `--as-of`.
pub fn time_coverage() -> f64 {
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    // Titles chosen to be far apart under jaro_winkler: the decide-title fuzzy
    // match is a separate concern, and titles that merged would measure the
    // resolver instead of the chain.
    const TITLES: [&str; 3] = ["adopt sqlite", "prefer postgres", "switch to duckdb"];

    #[test]
    fn a_chain_of_three_is_recovered_in_order() {
        let e = build_chain(&TITLES);

        let first = detail(&e, TITLES[0]);
        let titles_after: Vec<&str> =
            first.superseded_by.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(titles_after, vec![TITLES[1], TITLES[2]]);

        let last = detail(&e, TITLES[2]);
        let titles_before: Vec<&str> =
            last.supersedes.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(titles_before, vec![TITLES[1], TITLES[0]]);
    }

    #[test]
    fn only_the_end_of_the_chain_still_stands() {
        let e = build_chain(&TITLES);
        assert!(!detail(&e, TITLES[0]).still_stands);
        assert!(!detail(&e, TITLES[1]).still_stands);
        assert!(detail(&e, TITLES[2]).still_stands);
    }

    #[test]
    fn a_decision_that_supersedes_nothing_has_an_empty_chain_behind_it() {
        let e = build_chain(&TITLES);
        assert!(detail(&e, TITLES[0]).supersedes.is_empty());
        assert!(detail(&e, TITLES[2]).superseded_by.is_empty());
    }

    #[test]
    fn the_decision_layer_answers_no_temporal_probe() {
        assert_eq!(time_coverage(), 0.0);
    }
}
```

Add `pub mod decisions;` to `lib.rs`.

- [ ] **Step 3: Run to verify they fail, then pass**

Run: `cargo test -p rm-conform decisions -- --nocapture`
Expected: FAIL first (module not declared), then PASS.

If `superseded_by` returns the chain in the opposite order, take the order from `DecisionDetail`'s own doc comment — "what replaced this decision, and what replaced that, ending at whatever stands now" — and fix the test, not the source.

- [ ] **Step 4: Run, fmt, clippy, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p rm-conform
git add crates/rm-conform/src/decisions.rs crates/rm-conform/src/lib.rs
git commit -F - <<'EOF'
Chains, standing, and a coverage number that is currently zero

decisions and decision take no time parameters, so the product surface
cannot be asked the question the store exists to answer. Asserted as
0.0 rather than described in prose, so it changes visibly the day
somebody adds --valid-at.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Kc9AvVxcAwEa63HsQxz6pn
EOF
```

---

## Task 10: The report, and CI

**Files:**
- Create: `crates/rm-conform/src/report.rs`
- Create: `crates/rm-conform/src/main.rs`
- Create: `crates/rm-conform/README.md`
- Modify: `.github/workflows/` — the `check` job

**Interfaces:**
- Consumes: everything above
- Produces: `rm-conform` binary with `--report`

- [ ] **Step 1: Write the report table**

`report.rs` emits the spec's table computed rather than typed. Every number is
produced by the same code the tests run, so the README cannot drift from the
suite.

```rust
//! The headline table, computed.
//!
//! Every figure here comes from the same functions the tests call. A README
//! number that was typed by hand is a number that goes stale silently.

use crate::differential::{default_strategies, refusal_agreement, sweep};
use crate::generate::{generate, Params};
use crate::invariants::{monotonic_in_transaction_time, order_independent, probe_grid};

const SEEDS: u64 = 500;

fn tick(passed: bool) -> &'static str {
    if passed { "1.000" } else { "FAILED" }
}

pub fn table() -> String {
    let params = Params::default();

    let disagreements = sweep(0..SEEDS, &params, &default_strategies());
    let refusals = refusal_agreement(0..SEEDS, &default_strategies());

    let probes = probe_grid();
    let monotonic = (0..SEEDS).all(|s| {
        let h = generate(s, &params);
        [1, 3, 6, 9]
            .iter()
            .all(|c| monotonic_in_transaction_time(&h, *c, &probes))
    });
    let ordered = (0..SEEDS).all(|s| order_independent(&generate(s, &params), &probes));

    let mut out = String::new();
    out.push_str(&format!(
        "Seeds 0..{SEEDS}, params {params:?}.\n\n\
         | property | result |\n|---|---|\n"
    ));
    out.push_str(&format!(
        "| merge agreement, 8 strategies | {} |\n",
        tick(disagreements.is_empty())
    ));
    out.push_str(&format!(
        "| refusal correctness | {} ({} refusals reached) |\n",
        tick(refusals.exact()),
        refusals.both_refused
    ));
    out.push_str(&format!(
        "| transaction-time monotonicity | {} |\n",
        tick(monotonic)
    ));
    out.push_str(&format!(
        "| arrival-order independence | {} |\n",
        tick(ordered)
    ));
    out.push_str(&format!(
        "| decision-layer time coverage | {:.3} |\n",
        crate::decisions::time_coverage()
    ));

    if !disagreements.is_empty() {
        out.push_str(&format!(
            "\n{} disagreement(s). First, minimised:\n\n```\n{:#?}\n```\n",
            disagreements.len(),
            disagreements[0]
        ));
    }
    out
}
```

`probe_grid` is currently private in `invariants.rs`. Make it `pub` as part of
this step rather than duplicating the grid — two grids that drift apart would
make the README and the tests disagree about what was measured.

- [ ] **Step 2: The binary**

```rust
fn main() {
    if std::env::args().any(|a| a == "--report") {
        println!("{}", rm_conform::report::table());
    } else {
        eprintln!("rm-conform --report    run the sweep and print the headline table");
        std::process::exit(2);
    }
}
```

- [ ] **Step 3: Confirm CI already covers it**

`cargo test --workspace` picks the crate up automatically once it is a workspace member. Verify the `check` job runs `--workspace` and not a list of crate names; if it names crates, add `rm-conform`.

Run: `cargo test --workspace` and confirm `rm-conform` tests appear.

- [ ] **Step 4: Write the README with the measured table**

Include the seed counts, the parameters, and the honest limits from the spec's Risks section — particularly that a green suite proves the engine agrees with a small model on generated inputs, and does not prove the store is useful.

- [ ] **Step 5: fmt, clippy, full suite, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/rm-conform .github
git commit -F - <<'EOF'
A number that reappears on every push

LoCoMo lives outside the workspace because it costs money and minutes,
and the consequence is on the record: four findings measured once and
shipped switched off, because re-measuring was expensive enough that
nobody did. This one is free, so it runs in CI and goes stale the day
it starts failing rather than quietly.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Kc9AvVxcAwEa63HsQxz6pn
EOF
```

---

## Self-review notes

**Spec coverage.** Every spec section maps to a task: reference model → 1–3; generator → 4; bi-temporal agreement → 5 and 7; refusal correctness → 6; metamorphic invariants → 8; supersession/standing and the time-coverage number → 9; report and CI → 10. The spec's "no scenario catalogue up front" is honoured — Task 5 Step 3 freezes regressions only from failures actually found.

**Placeholder scan.** Two were found on review and both are now closed. Task 9
originally pointed at `command.rs:557` instead of transcribing `decide`; the
signature is now written out in full, verified against the source, along with
the two facts that simplify the task (`find_decision` is an exact string match,
so the 3-dimension index is fine and the test cannot be retrieval-flaky; and
`DecisionDetail` exposes `still_stands`, `supersedes` and `superseded_by`
directly, so no `Standing` accessor is needed). Task 10 originally carried a
`todo!()` for the report composition; it is now written.

**Type consistency.** `Params`, `Rng`, `Assertion`, `Disagreement`,
`RefusalScore`, `probe_grid` and `time_coverage` are each defined once and used
under the same names throughout. `probe_grid` is private in Task 8 and made
`pub` in Task 10, which is called out in that step rather than left to be
discovered.

**Known gaps, stated rather than hidden.**

- **`Interval::contains` is used in Task 7 without being verified.** The
  half-open fallback is given inline in that step.
- **`max_by_key` tie direction in Task 3** returns the *last* maximum, which
  contradicts "count ties go to the first seen". Flagged in the task; write the
  tie test before assuming either way.
- **`rm_host::command::Outcome` is assumed to derive `Debug`**, for the panic
  message in Task 9's `detail` helper. If it does not, match without the
  formatted payload rather than adding a derive to `rm-host` for a test's
  convenience.
- **Task 5's `shrinking_reduces_a_known_disagreement_to_its_minimum` asserts
  that seed 11 separates `MostRecent` from `FirstNonNull`.** That is a claim
  about a specific seed and it has not been run. If it does not hold, find a
  seed that does rather than deleting the test — its job is to prove the
  differential machinery can detect a difference at all, and without it a green
  suite could mean nothing is being compared.
