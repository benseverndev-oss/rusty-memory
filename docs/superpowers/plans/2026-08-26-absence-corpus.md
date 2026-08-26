# Absence Corpus Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Measure whether the store returns the right one of three answers — value, asserted absent, never mentioned — on a corpus where all three are labelled, and report it as a 3×3 confusion matrix.

**Architecture:** A labelled corpus and a test in `crates/rm-engine/tests/`, scoring `Engine::about` against hand-written truth. No network, no embedder beyond the offline one, no LLM judge. The comparison against other systems is a separate, manual, two-scenario demonstration written up in the README — not a harness.

**Tech Stack:** Rust, `serde_json`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-26-proving-the-claim-design.md`

## Global Constraints

- **The corpus is synthetic and this repository is public.** No real person, employer or address.
- Ground truth is hand-written per case and never derived from a run.
- The metric is a **3×3 confusion matrix**, never a single accuracy figure. Two of the nine cells are fabrications and one of those is the failure this project exists to prevent; an accuracy percentage hides which cell moved.
- Scoring reads `Believed` directly from `Engine::about`. No `recall`, no vector threshold, no `weak_below` — the claim under test is structural and must not be measured through a probabilistic path.
- Absences must carry **consequence**. The positioning document names the domains: people, money, records, compliance. "Has no pets" is a toy; "has no prescribing authority" is the shape that makes the failure legible.

---

### Task 1: The corpus, and the guard that it stays hard

**Files:**
- Create: `crates/rm-engine/tests/absence/cases.json`
- Create: `crates/rm-engine/tests/absence.rs`

**Interfaces:**
- Produces, for Tasks 2 and 3:
  ```rust
  #[derive(serde::Deserialize)]
  struct Corpus { cases: Vec<Case> }

  /// One subject, the facts stated about them, and what each attribute's
  /// correct answer is. `truth` is hand-written and covers attributes the
  /// `states` list deliberately does not mention.
  #[derive(serde::Deserialize)]
  struct Case {
      subject: String,
      /// Attribute -> value, or null to state "there is none".
      states: std::collections::BTreeMap<String, Option<String>>,
      /// Attribute -> "value" | "absent" | "unknown".
      truth: std::collections::BTreeMap<String, String>,
      why: String,
  }

  fn load() -> Corpus;
  ```

- [ ] **Step 1: Write the corpus**

Each case states some attributes, leaves others unmentioned, and labels all
three outcomes. The `why` line says what acting on a wrong answer would cost —
it is what makes the case worth having and what gets quoted in the README.

```json
{
  "cases": [
    {
      "subject": "Case A",
      "states": { "prescribing_authority": null, "specialty": "cardiology" },
      "truth": {
        "specialty": "value",
        "prescribing_authority": "absent",
        "npi_number": "unknown"
      },
      "why": "A system that answers absent for npi_number states this clinician has no NPI because nobody supplied one. Acting on it means filing a claim that will be rejected."
    },
    {
      "subject": "Case B",
      "states": { "outstanding_balance": null },
      "truth": { "outstanding_balance": "absent", "payment_method": "unknown" },
      "why": "Absent and unknown lead to opposite actions here: one closes the account, the other asks."
    },
    {
      "subject": "Case C",
      "states": { "employer": "Northwind Analytics" },
      "truth": { "employer": "value", "former_employer": "unknown" },
      "why": "A current employer says nothing about whether there was a previous one. Inferring absence from a single stated value is the most tempting fabrication of the three."
    }
  ]
}
```

Extend to at least eight cases. Every case must contain **all three truths** —
a case labelled only `value` and `unknown` cannot exercise the cell that
matters.

- [ ] **Step 2: Write the failing guard**

```rust
/// The corpus cannot drift into only the easy half.
///
/// A corpus without `absent` cases measures nothing about this project: any
/// two-state system scores perfectly on value-versus-unknown. The whole claim
/// lives in the cases where something was stated to be missing.
#[test]
fn every_case_labels_all_three_outcomes() {
    for case in load().cases {
        let kinds: std::collections::BTreeSet<&str> =
            case.truth.values().map(String::as_str).collect();
        for want in ["value", "absent", "unknown"] {
            assert!(
                kinds.contains(want),
                "{} has no {want} case -- it cannot exercise the distinction",
                case.subject
            );
        }
        assert!(
            !case.why.is_empty(),
            "{} does not say what a wrong answer would cost",
            case.subject
        );
    }
}
```

- [ ] **Step 3: Run it and watch it fail, then pass**

Run: `cargo test -p rm-engine --test absence`
Expected: FAIL (no `load`), then PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/rm-engine/tests/absence/ crates/rm-engine/tests/absence.rs
git commit -m "A corpus where all three answers are labelled and each one costs something"
```

---

### Task 2: Score the store as a 3×3 matrix

**Files:**
- Modify: `crates/rm-engine/tests/absence.rs`

**Interfaces:**
- Consumes: `Engine::about`, `Believed`, `Corpus`.
- Produces, for Task 3:
  ```rust
  /// Rows are truth, columns are what the store answered.
  #[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
  struct Matrix { cells: std::collections::BTreeMap<String, usize> } // "truth->answered"
  fn score(engine: &Engine, corpus: &Corpus) -> Matrix;
  ```
  Keyed by name rather than indexed, so a baseline diff reads `absent->value: 0 -> 1` instead of a moved number.

- [ ] **Step 1: Write the failing test**

```rust
/// The store answers all three, and never fabricates.
///
/// The two fabrication cells are named individually. A single accuracy figure
/// would let one of them grow while another shrank and report no change,
/// which is exactly the kind of number this project keeps finding.
#[test]
fn the_store_distinguishes_all_three_and_fabricates_nothing() {
    let corpus = load();
    let matrix = score(&seeded_engine(&corpus), &corpus);

    assert_eq!(matrix.get("unknown->absent"), 0,
        "stated there is none, when nobody had said anything");
    assert_eq!(matrix.get("unknown->value"), 0, "invented a value");
    assert_eq!(matrix.get("absent->value"), 0, "invented a value over a stated absence");

    // And the distinction is actually being exercised, not vacuously passed
    // by a store that answers `unknown` to everything.
    assert!(matrix.get("value->value") > 0);
    assert!(matrix.get("absent->absent") > 0);
    assert!(matrix.get("unknown->unknown") > 0);
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rm-engine --test absence the_store_distinguishes`
Expected: FAIL with "cannot find function `score`".

- [ ] **Step 3: Seed and score**

`seeded_engine` writes each case's `states` through the ordinary write path —
a `null` value becomes an asserted absence, exactly as `rmem note --absent`
does — and never writes an attribute whose truth is `unknown`. Use the offline
embedder; no network.

```rust
let answered = match engine.about(entity, attribute, Timestamp::MAX, Timestamp::MAX)? {
    Believed::Value(_) => "value",
    Believed::Absent => "absent",
    Believed::Unknown => "unknown",
};
*matrix.cells.entry(format!("{truth}->{answered}")).or_default() += 1;
```

- [ ] **Step 4: Run the test**

Expected: PASS.

- [ ] **Step 5: Prove it can fail**

Not optional. Temporarily seed one `unknown` attribute as an asserted absence,
confirm `unknown->absent` becomes 1 and the test names that cell, then restore.
Record the failure output in the commit message. A confusion matrix that has
never been seen with a non-zero fabrication cell might be counting nothing.

- [ ] **Step 6: Commit**

```bash
git add crates/rm-engine/tests/absence.rs
git commit -m "Score the three-way answer, and name the fabrication cells separately"
```

---

### Task 3: The write-up, including what makes it unfair

**Files:**
- Modify: `README.md`
- Create: `docs/absence-benchmark.md`

- [ ] **Step 1: Write up the result**

`docs/absence-benchmark.md` carries the matrix, the corpus description, and —
required, not optional — **the paragraph saying the benchmark was designed
around a distinction only this system makes.** The spec's reasoning: a reader
who works that out unaided discounts everything, and one told up front can
weigh it.

It must also cite the LoCoMo refusal analysis as the negative result that
motivated this. A score-based refusal was tried across six signals and rejected
on the evidence at J = 0.494; that is a stronger argument for a structural
distinction than any assertion about one.

- [ ] **Step 2: The competitive demonstration**

Two scenarios, run by hand against each system compared, recorded verbatim:

- **A:** the conversation states "I'm single."
- **B:** partners are never mentioned.
- Both asked: does this person have a partner?

Record what each system answered in both. If a system answers identically, that
is the finding. **If a system distinguishes them, say so plainly and revise the
positioning** — the claim that competitors cannot is an inference from their
architecture, not a measurement, and this is the step that tests it.

- [ ] **Step 3: Lead the README with it**

The positioning document's step 3 is to lead with an `Unknown` that saves the
reader rather than with Acme to Globex. The three-line output is the example:

```
spouse    Alex
employer  no value — asserted to have none
pets      nothing known — this was never discussed
```

Keep the Acme/Globex example — it demonstrates bi-temporality and it runs as a
test — but it stops being the headline.

- [ ] **Step 4: Record the decision**

From a script file, never inline:

```bash
rmem decide "The three-way answer is proved on a purpose-built corpus, not on LoCoMo" \
  "score value, absent and unknown as a 3x3 matrix through about; keep LoCoMo for recall comparability only" \
  --context "benches/locomo already scored six refusal signals against its 382/112 label, best J 0.494, and keeping 90 percent of answerable questions refuses only 36.6 percent of unanswerable. weak_below ships off because of it" \
  --because "that measures whether a similarity score separates answerable from unanswerable, which is not the claim. The claim is that an assertion either exists or does not, and LoCoMo has no category whose correct answer is that the conversation established there is none. A benchmark designed around a distinction only one system makes has to say so up front, so the write-up states who designed the unfair axis and reports recall on the fair one alongside" \
  --scope "*"
```

- [ ] **Step 5: Run the gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
```

---

## Finishing

Use `superpowers:finishing-a-development-branch`. Base branch is `main`.

The pull request carries the 3×3 matrix, states that the corpus is synthetic
and purpose-built, and states in its own words that competitors cannot be
scored on this axis rather than that they score badly on it.

If Task 3 Step 2 finds a competitor that *does* distinguish the two cases, the
pull request leads with that instead. It is the more valuable result, and
finding it cheaply is why that step is manual and comes before any harness.
