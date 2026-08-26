# Executable Claims Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the derived numbers this codebase asserts in prose recompute themselves, so moving an input fails a test instead of quietly falsifying a comment.

**Architecture:** A `claims` test module in each crate that carries derived figures. The comments stay hand-written — they are the specification; the tests check the arithmetic in them still holds. Claims that measure the outside world get a provenance line instead of a test, because asserting a hard-coded constant against itself is the vacuous test this project already has a lesson about.

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-26-executable-claims-design.md`

## Global Constraints

- **Tier three is not to be faked.** LoCoMo's 382/112 split, the ANN bake-off timings, "96.9% of the file was vectors" and `u = 0.38` measured across four stores cannot be recomputed here. They get provenance, never an assertion.
- Every tolerance carries a comment naming the rounding it permits. The config rounds thresholds to four places and says so; a tighter tolerance turns a deliberate choice into a failure.
- Each test must be **seen to fail** when its claim is falsified — perturb the input, watch it go red, restore. Record the observed failure in the commit message.
- The chars-per-token ratio is stated in exactly **one** place in the workspace. This plan and `2026-08-26-tool-table-cost.md` both need it; whichever lands first defines it, and the second consumes it rather than restating it.

---

### Task 1: The thresholds recompute from `kind`'s agreement weight

**Files:**
- Modify: `crates/rm-host/src/config.rs` (new `mod claims` in the test module)

**Interfaces:**
- Consumes: `TEMPLATE`, `FieldRule::agreement_weight`, `Comparator`.
- Produces: nothing. Tests only.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod claims {
    use super::*;

    /// The thresholds are the one-field figures plus what `kind` contributes.
    ///
    /// The comment above `review_at` states this derivation: they were 4.0 and
    /// 6.0 when `name` was the only field, and adding `kind` adds
    /// log2(0.9/0.38) to every pair whose kinds agree, so both rose by that
    /// much. Nothing recomputed it, so moving `u` would have left the comment
    /// false, both thresholds miscalibrated, and every test green.
    #[test]
    fn the_thresholds_are_the_one_field_figures_plus_kinds_agreement_weight() {
        let config = Config::from_template();
        let kind = config
            .resolution
            .field
            .iter()
            .find(|f| f.field == "kind")
            .expect("the template resolves on kind");

        let shift = (kind.m / kind.u).log2();
        assert!(
            (shift - 1.2439256).abs() < 5e-8,
            "the comment says 1.2439256, the fields give {shift}"
        );

        // 4.0 and 6.0 are the thresholds from before `kind` was a field.
        // The tolerance is 1e-4 because the config rounds to four places and
        // says so: "written to four places, which leaves each boundary
        // 0.000026 bits below the exact figure". A tighter bound would fail
        // on rounding the author chose deliberately.
        assert!((config.resolution.review_at - (4.0 + shift)).abs() < 1e-4);
        assert!((config.resolution.match_at - (6.0 + shift)).abs() < 1e-4);
    }
}
```

Field names (`resolution.field`, `.m`, `.u`) must be checked against the real
structs before writing — read the `Deserialize` definitions rather than
trusting these.

- [ ] **Step 2: Run it and watch it fail, if it does**

Run: `cargo test -p rm-host --lib claims`
Expected: PASS if the comment is accurate. That is a real result, not a
non-event: it is the first time the arithmetic has been checked.

- [ ] **Step 3: Prove it can fail**

Change `u = 0.38` to `0.40` in `TEMPLATE`, run, confirm the test fails naming
both figures, restore. Paste the failure into the commit message.

- [ ] **Step 4: Commit**

```bash
git add crates/rm-host/src/config.rs
git commit -m "The thresholds recompute from the fields they were calibrated against"
```

---

### Task 2: The name ceiling and the kind veto

**Files:**
- Modify: `crates/rm-engine/src/lib.rs` (test module) or `crates/rm-resolve/src/lib.rs`, wherever `Ruleset` is reachable from a test.

**Interfaces:**
- Consumes: `Ruleset`, `FieldRule`, `Decision`.

- [ ] **Step 1: Write the failing tests**

`log2(0.9/0.01) ≈ 6.49` is asserted in three doc comments and one test comment,
and computed by none of them. The kind veto is a claim about *behaviour* and
gets a behavioural test as well as an arithmetic one.

```rust
/// A name can contribute at most log2(0.9/0.01) bits, and three comments in
/// this workspace say so in prose.
#[test]
fn a_name_can_contribute_at_most_six_point_four_nine_bits() {
    let name = FieldRule::new("name", Comparator::PossessiveAware, 0.9, 0.01);
    let ceiling = name.agreement_weight();
    assert!((ceiling - 6.49).abs() < 5e-3, "{ceiling}");
}

/// A kind disagreement is final, not merely expensive.
///
/// The config states the arithmetic: a name contributes at most 6.49, a kind
/// disagreement costs 2.63, and 6.49 - 2.63 = 3.86 is below `review_at` -- so
/// two entities whose kinds differ are never even asked about, however
/// identical their names. That is a threshold policy rather than something
/// the probabilities imply, which is exactly why it needs a test: lower the
/// thresholds and the veto silently becomes a penalty again.
#[test]
fn two_things_of_different_kinds_are_never_asked_about_however_alike_their_names() {
    let rules = shipped_ruleset();
    let a = Record::new().with("name", "Paris").with("kind", "place");
    let b = Record::new().with("name", "Paris").with("kind", "person");
    assert_eq!(rules.decide(rules.score(&a, &b)), Decision::Different);
}
```

- [ ] **Step 2: Run them**

Run: `cargo test -p rm-resolve claims`
Expected: PASS. If the veto test fails, that is a genuine finding about the
shipped config and stops this plan — report it rather than adjusting the test.

- [ ] **Step 3: Prove each can fail**

Lower `review_at` below 3.86 in the test ruleset and confirm the veto test goes
red. Restore. Record the output.

- [ ] **Step 4: Commit**

```bash
git commit -m "The name ceiling and the kind veto are computed, not asserted in prose"
```

---

### Task 3: The tool table's byte count, and the ratio in one place

**Files:**
- Modify: `crates/rm-mcp/src/tools.rs`

**Interfaces:**
- Produces: `pub(crate) const CHARS_PER_TOKEN: f64` — the single place the ratio lives. `2026-08-26-tool-table-cost.md` consumes this rather than restating it.

- [ ] **Step 1: Write the failing test**

```rust
/// The table's size, pinned, because the figure in `definitions`' comment and
/// the one in the README disagreed for a day in August 2026: the README moved
/// twice, for the clocks and for scope, and the comment did not follow.
///
/// The byte count is the measurement. The token figure is derived from it, so
/// only one of the two can rot.
#[test]
fn the_tool_table_is_the_size_the_documentation_says() {
    let chars = serde_json::to_string(&all_definitions()).unwrap().len();
    assert!(
        (10_000..11_000).contains(&chars),
        "the table is {chars} chars; update the README's row and the comment on `definitions` together"
    );
    let tokens = chars as f64 / CHARS_PER_TOKEN;
    assert!((tokens - 2_600.0).abs() < 150.0, "~{tokens:.0} tokens");
}
```

The band is wide on purpose: a prose edit should not fail this, and a tool
being added or removed should.

- [ ] **Step 2: Run it and watch it fail, then pass**

Run: `cargo test -p rm-mcp --lib the_tool_table_is_the_size`

- [ ] **Step 3: Define the ratio once, with its provenance**

```rust
/// Characters per token for this table's JSON, from the four counted rows in
/// the README: 8,203/2,060, 5,650/1,420, 4,475/1,130 and 2,385/610 give 3.91
/// to 3.98. Stated here so the README's rows and the comment on
/// `definitions` derive from one number rather than three copies of it.
pub(crate) const CHARS_PER_TOKEN: f64 = 3.97;
```

- [ ] **Step 4: Commit**

```bash
git commit -m "The tool table's size is measured in one place and derived everywhere else"
```

---

### Task 4: Provenance for what cannot be recomputed

**Files:**
- Modify: `crates/rm-host/src/config.rs` (the `u = 0.38` comment)
- Modify: wherever the LoCoMo, ANN bake-off and vector-share figures are stated — locate with `rg -n 'measured|Measured' --include=*.rs --include=*.md crates/ README.md`

- [ ] **Step 1: Add provenance lines**

No test. Each figure gains **what was measured, when, and where the harness
is**. `u = 0.38` matters most, because Task 1 builds arithmetic on top of it:

```
# u is high because there are only a handful of kinds and the distribution is
# skewed: 0.38 is the rate at which two entities that share a name prefix --
# the pairs blocking actually compares -- happen to share a kind, measured
# across four stores from a real corpus.
#
# Measured 2026-08-.., harness not committed. This is an input to the
# threshold derivation `the_thresholds_are_the_one_field_figures_plus_kinds_
# agreement_weight` checks, so if it is wrong that test still passes and both
# thresholds are wrong with it. Re-measuring it needs the corpus from
# docs/superpowers/specs/2026-08-26-resolution-corpus-design.md.
```

Fill the date from `git log -S '0.38' -- crates/rm-host/src/config.rs`. Where
it cannot be established, say "date not recorded" rather than guessing — a
wrong date is worse than an absent one.

- [ ] **Step 2: Commit**

```bash
git commit -m "Say where the numbers that cannot be recomputed came from"
```

---

### Task 5: The record

- [ ] **Step 1: Record the decision**

From a script file, never inline:

```bash
rmem decide "A derived number in a comment is a claim, and claims get tests" \
  "recompute derivations from their inputs; give measurements of the outside world provenance instead of an assertion" \
  --context "100 sentences across the crates and README say measured, and two were checked. log2(0.9/0.38) = 1.2439256 lived only in a comment, so moving u would have miscalibrated both thresholds with every test still green" \
  --because "this codebase's distinguishing asset is that it writes down why, which is exactly why a stale number here is worse than elsewhere: it is quoted with the confidence the surrounding prose has earned. A test that asserts a hard-coded constant against itself is not the fix and would be worse than nothing, so the split between what is recomputed and what merely carries provenance is the whole design" \
  --scope "*"
```

- [ ] **Step 2: Run the whole gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
```

---

## Finishing

Use `superpowers:finishing-a-development-branch`. Base branch is `main`.

The pull request states the honest limit: a green suite now means **the numbers
are consistent with each other**, not that they are right. `u = 0.38` could be
wrong about the world and every test here would still pass, because they check
the arithmetic built on it rather than the value. The provenance lines are what
keep that visible.
