# Resolution Corpus and Baseline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A synthetic ground-truthed corpus and a test that scores the shipped resolution config against a committed baseline, so a change that creates a silent miss or a wrong merge fails CI.

**Architecture:** Three files under `crates/rm-resolve/tests/`: the corpus with ground truth, the baseline it currently produces, and the test that joins them. Scoring lives in the test, not in the library — nothing in the shipped crate needs it, and putting it behind a public API would invite it being used to decide things at runtime.

**Tech Stack:** Rust, `serde_json` (already a dev-dependency of the workspace — confirm in `crates/rm-resolve/Cargo.toml` and add under `[dev-dependencies]` if absent).

**Spec:** `docs/superpowers/specs/2026-08-26-resolution-corpus-design.md`

## Global Constraints

- **The corpus is synthetic. This repository is public.** No real name, address or employer goes in these files. The corpus that produced the original measurements was 27 real colleagues and must not be committed.
- Ground truth is hand-written in the corpus file and never derived from a resolver result. A test that supplies the thing it checks cannot fail.
- The three counts — questions, silent misses, wrong merges — are reported and asserted **separately** and never summed. One wrong merge is worse than any number of questions, and a weighted total lets one hide behind the other.
- Scoring uses `Ruleset` directly. It does not build an `Engine`, write a store, or need an embedder: resolution never sees a vector, which was confirmed by reading `rm-resolve` for any reference to one and finding none.

---

### Task 1: The corpus file and its shape guard

**Files:**
- Create: `crates/rm-resolve/tests/corpus/people.json`
- Create: `crates/rm-resolve/tests/corpus.rs`

**Interfaces:**
- Produces, for Tasks 2 and 3:
  ```rust
  #[derive(serde::Deserialize)]
  struct Corpus { people: Vec<Person>, mentions: Vec<Mention> }

  #[derive(serde::Deserialize)]
  struct Person { id: String, name: String, kind: String, email: Option<String> }

  /// `is` names the person this mention is really about, or is absent when the
  /// mention is a person the corpus has not seen before.
  #[derive(serde::Deserialize)]
  struct Mention { name: String, kind: String, email: Option<String>, is: Option<String>, shape: String }

  fn load() -> Corpus;
  ```

- [ ] **Step 1: Write the corpus**

Every entry carries a `shape`, which is what the guard in Step 3 counts. Names
are invented; the structures are the ones that decided the original outcome.

```json
{
  "people": [
    { "id": "p1", "name": "Jonathan Merrick", "kind": "person", "email": "jmerrick@northwind.example" },
    { "id": "p2", "name": "Christopher Vale", "kind": "person", "email": null },
    { "id": "p3", "name": "Priyanka Raghunathan", "kind": "person", "email": "praghunathan@northwind.example" },
    { "id": "p4", "name": "Rosalind Okafor", "kind": "person", "email": "rokafor@northwind.example" },
    { "id": "p5", "name": "Rosalind Ashby", "kind": "person", "email": "rashby@northwind.example" },
    { "id": "p6", "name": "Delia Okafor", "kind": "person", "email": "dokafor@westgate.example" },
    { "id": "p7", "name": "Marguerite Sandoval", "kind": "person", "email": "msandoval@northwind.example" },
    { "id": "p8", "name": "Northwind Analytics", "kind": "organisation", "email": null }
  ],
  "mentions": [
    { "name": "Jonathan Merrick", "kind": "person", "email": "jmerrick@northwind.example", "is": "p1", "shape": "exact-repeat" },
    { "name": "Jon Merrick", "kind": "person", "email": null, "is": "p1", "shape": "nickname" },
    { "name": "Christopher Vale", "kind": "person", "email": null, "is": "p2", "shape": "exact-repeat" },
    { "name": "Chris Vale", "kind": "person", "email": null, "is": "p2", "shape": "nickname" },
    { "name": "Priyanka", "kind": "person", "email": null, "is": "p3", "shape": "given-name-alone" },
    { "name": "Merrick", "kind": "person", "email": null, "is": "p1", "shape": "surname-alone" },
    { "name": "Rosalind Ashby", "kind": "person", "email": "rokafor@westgate.example", "is": "p4", "shape": "changed-surname-stable-local-part" },
    { "name": "Delia Okafor", "kind": "person", "email": "dokafor@westgate.example", "is": "p6", "shape": "shared-surname-different-people" },
    { "name": "Rosalind Ashby", "kind": "person", "email": "rashby@northwind.example", "is": "p5", "shape": "shared-given-name-shared-domain" },
    { "name": "Northwind Analytics", "kind": "organisation", "email": null, "is": "p8", "shape": "kind-disagreement-guard" },
    { "name": "Northwind Analytics", "kind": "person", "email": null, "is": null, "shape": "kind-disagreement-guard" }
  ]
}
```

Two entries need care and a comment in the file's own README (Step 4):

- `changed-surname-stable-local-part` is a **true match** whose surname changed
  and whose local part did not. It is the case an exact email comparator turns
  into a silent miss.
- `shared-given-name-shared-domain` is a **true non-match** that shares a given
  name and a mail domain with a real person. It is the case a fuzzy email
  comparator merges outright.

They pull in opposite directions on purpose. Any configuration that gets both
right has earned it.

- [ ] **Step 2: Write the failing shape guard**

```rust
/// The corpus cannot be trimmed to the easy cases and still pass.
///
/// A corpus of true matches measures nothing: a configuration that merges
/// everything scores perfectly on it. The negative shapes are the ones that
/// decided every real comparison, so their presence is asserted rather than
/// assumed.
#[test]
fn the_corpus_still_contains_every_shape_that_has_ever_decided_anything() {
    let corpus = load();
    let shapes: std::collections::BTreeSet<&str> =
        corpus.mentions.iter().map(|m| m.shape.as_str()).collect();
    for required in [
        "exact-repeat",
        "nickname",
        "given-name-alone",
        "surname-alone",
        "changed-surname-stable-local-part",
        "shared-surname-different-people",
        "shared-given-name-shared-domain",
        "kind-disagreement-guard",
    ] {
        assert!(shapes.contains(required), "the corpus lost its {required} case");
    }
    assert!(
        corpus.mentions.iter().any(|m| m.is.is_none()),
        "a corpus with no strangers in it cannot detect a wrong merge"
    );
}
```

- [ ] **Step 3: Run it and watch it fail, then pass**

Run: `cargo test -p rm-resolve --test corpus`
Expected: FAIL first (no `load`), then PASS once `load` reads the JSON.

- [ ] **Step 4: Write `crates/rm-resolve/tests/corpus/README.md`**

State that the data is synthetic and why (this repository is public), that
`is` is hand-written ground truth, and that the two opposed cases above are
deliberate. Anyone appending a shape should say where they saw it.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-resolve/tests/corpus/ crates/rm-resolve/tests/corpus.rs
git commit -m "A synthetic corpus, carrying the shapes that have decided real comparisons"
```

---

### Task 2: Score a configuration against ground truth

**Files:**
- Modify: `crates/rm-resolve/tests/corpus.rs`

**Interfaces:**
- Consumes: `Corpus` from Task 1; `Ruleset`, `Record`, `Decision` from `rm_resolve`.
- Produces, for Task 3:
  ```rust
  #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
  struct Score {
      questions: Vec<String>,     // "mention -> person" pairs, sorted
      silent_misses: Vec<String>,
      wrong_merges: Vec<String>,
  }
  fn score(rules: &Ruleset, corpus: &Corpus) -> Score;
  ```
  Pairs, not counts: a diff that says *which* pair changed is worth more than one saying a number moved.

- [ ] **Step 1: Write the failing test**

```rust
/// Scoring reports what a configuration does, judged against truth written by
/// hand. The three outcomes are kept apart on purpose: one wrong merge is
/// worse than any number of questions, and a single total would let one hide
/// behind the other.
#[test]
fn scoring_separates_a_question_from_a_miss_from_a_wrong_merge() {
    let corpus = load();
    let score = score(&shipped_ruleset(), &corpus);

    // The shapes that must never regress, named individually so a failure
    // says which one moved.
    assert!(
        score.wrong_merges.is_empty(),
        "a stranger was absorbed: {:?}",
        score.wrong_merges
    );
    assert!(
        !score.questions.is_empty(),
        "a corpus producing no questions is not exercising the review band"
    );
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rm-resolve --test corpus scoring_separates`
Expected: FAIL with "cannot find function `score`".

- [ ] **Step 3: Implement scoring**

For each mention, build a `Record` with `name`, `kind` and `email` where
present, score it against every person's record, take the best, and classify:

```rust
// Classification is against `is`, the hand-written truth, never against
// another resolver result. Deciding correctness from the thing under test is
// how a test comes to assert that the code does what the code does.
let decision = rules.decide(best_score);
match (mention.is.as_deref(), decision) {
    // Truth says this is `who`, and the config merged it onto `who`: right.
    (Some(who), Decision::Match) if who == best_id => {}
    // Truth says this is `who`, and the config merged it onto someone else,
    // or truth says stranger and the config merged it at all: wrong, silent,
    // permanent.
    (_, Decision::Match) => out.wrong_merges.push(pair),
    // Asked about. Cheap and recoverable whichever way it is answered.
    (_, Decision::Review) => out.questions.push(pair),
    // Truth says this is someone we have, and the config did not notice.
    (Some(_), Decision::Different) => out.silent_misses.push(pair),
    // Truth says stranger and the config agreed.
    (None, Decision::Different) => {}
}
```

`shipped_ruleset()` must build from the crate's own `TEMPLATE` config rather
than from hand-written numbers, or the test measures a configuration nobody
ships.

- [ ] **Step 4: Run the test**

Run: `cargo test -p rm-resolve --test corpus`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-resolve/tests/corpus.rs
git commit -m "Score a configuration against truth, keeping the three outcomes apart"
```

---

### Task 3: The committed baseline

**Files:**
- Create: `crates/rm-resolve/tests/corpus/baseline.json`
- Modify: `crates/rm-resolve/tests/corpus.rs`

- [ ] **Step 1: Write the failing test**

```rust
/// The shipped configuration's behaviour, pinned.
///
/// A change that turns a caught match into a silent duplicate, or a stranger
/// into a merge, fails here. Updating `baseline.json` is how you say a change
/// was meant -- a deliberate act with a diff a reviewer can read, rather than
/// a number nobody watches.
#[test]
fn the_shipped_configuration_still_scores_what_the_baseline_says() {
    let score = score(&shipped_ruleset(), &load());
    let baseline: Score =
        serde_json::from_str(include_str!("corpus/baseline.json")).unwrap();
    assert_eq!(score, baseline, "resolution behaviour moved -- see the diff");
}
```

- [ ] **Step 2: Run it and watch it fail**

Expected: FAIL — no baseline file yet.

- [ ] **Step 3: Generate the baseline, then read it before committing it**

Print the `Score` and write it to `baseline.json`. **Do not accept it
blind.** Every entry in `silent_misses` is a duplicate this configuration will
create in production. Read each one and decide whether it is tolerable.

`surname-alone` is expected to sit there permanently: in the original data the
equivalent pair scored 4.80 while a true *non*-match scored 4.73, 0.07 bits
apart, so no threshold separates them and no field the mention carries can
break the tie. Record that in the baseline file's own comment field so nobody
later reads it as an unfixed bug and tunes the corpus to make it disappear.

- [ ] **Step 4: Run the test**

Expected: PASS.

- [ ] **Step 5: Add the anti-vacuity guard**

```rust
/// A baseline of all-zeros would pass forever and mean nothing.
///
/// It is what an empty corpus, a broken loader, or a ruleset that compares
/// nothing all look like, and each of those is silent.
#[test]
fn the_baseline_is_not_a_configuration_that_does_nothing() {
    let baseline: Score =
        serde_json::from_str(include_str!("corpus/baseline.json")).unwrap();
    assert!(
        !baseline.questions.is_empty() || !baseline.silent_misses.is_empty(),
        "a baseline with no questions and no misses is not measuring anything"
    );
}
```

- [ ] **Step 6: Prove the test can fail**

Not optional, and not a formality. Temporarily set `review_at` in the test's
ruleset well below its shipped value, confirm `wrong_merges` becomes non-empty
and the baseline test fails naming the pair, then restore it. A baseline test
that has never been seen red might be comparing a file against itself.

Record the observed failure output in the commit message.

- [ ] **Step 7: Commit**

```bash
git add crates/rm-resolve/tests/corpus/baseline.json crates/rm-resolve/tests/corpus.rs
git commit -m "Pin what the shipped configuration does, and show the pin can fail"
```

---

### Task 4: The record

**Files:**
- Modify: `README.md`
- The store

- [ ] **Step 1: Document it**

Add to `README.md` near the resolution section: what the corpus is, that it is
synthetic and why, and that changing resolution config means updating the
baseline deliberately rather than incidentally.

- [ ] **Step 2: Record the decision**

From a script file, never inline — inline `--scope "*"` globs before the
command sees it, which once sent twelve records to the wrong scope.

```bash
rmem decide "Resolution changes are measured against a committed corpus" \
  "score questions, silent misses and wrong merges separately against hand-written truth, and fail CI when the baseline moves" \
  --context "added after answering one question -- should email be a resolution field -- took four configurations and a throwaway corpus, reversed the recommendation twice, and ended in a negative result no existing test could have caught" \
  --because "the thresholds cite a corpus that is not in the repository, so the calibration cannot be re-measured or checked, and the interaction that decided the answer was between a field and the blocking key rather than in either alone" \
  --scope "*"
```

Read it back with `rmem recall`, not `rmem decision` — the latter prints no
scope, so it cannot see the field most likely to be wrong.

- [ ] **Step 3: Run the whole gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
git add README.md
git commit -m "What the corpus is for, and that it is synthetic on purpose"
```

---

## Finishing

Use `superpowers:finishing-a-development-branch`. Base branch is `main`.

The pull request reports the baseline's three counts, names the shapes the
corpus carries, and states plainly that **the corpus is synthetic and the
figures are therefore not a measurement of any real population** — it measures
whether a configuration handles known-hard structures, which is a different and
smaller claim than "resolution works".
