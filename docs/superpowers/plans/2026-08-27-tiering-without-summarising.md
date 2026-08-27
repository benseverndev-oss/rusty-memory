# Tiering Without Summarising Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a caller ask for a recall result at three depths — locator, stated, traced — paying only for the depth asked for, with no model call and nothing summarised away.

**Architecture:** Three return types and three methods rather than one type with optional fields. `Engine::recall` keeps its current signature and meaning; `recall_located` and `recall_traced` are added beside it. Every level is derived at query time from what is already stored.

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-27-tiering-without-summarising-design.md`

## Global Constraints

- **Nothing is summarised.** No model call, no generated text. A level omits; it never compresses. The same assertion at `Traced` is byte-identical to what `recall` returns today, plus history.
- **`recall` is unchanged.** Existing callers keep their signature, their type and their bytes. Tiering is opt-in in the direction that saves money.
- Every level is derived at query time. No precomputed layer — a second copy is a thing that can drift, which this repository has now been bitten by twice in one week.
- `CHARS_PER_TOKEN` in `crates/rm-mcp/src/tools.rs` is the one place the ratio lives. Task 3 consumes it rather than restating it.

## A correction to the spec, and why

The spec said `Recalled`'s value-bearing fields "become optional at `Located`".
**They cannot**, and the reason is the project's own central distinction:

```rust
pub struct Recalled {
    /// `None` is a tombstone — this assertion claimed the attribute had no
    /// value. It is never "we have nothing".
    pub value: Option<String>,
```

`value: None` already means *asserted absent*. Reusing it for *omitted at this
depth* would make a tombstone indistinguishable from a field the caller did not
ask for — the exact confusion between `Absent` and `Unknown` that this store
exists to prevent, reintroduced in its own return type.

So each depth gets its own type. This is simpler than the spec's `Depth` field
on `Query`, and `Depth` survives only as the MCP-facing vocabulary in Task 3.

---

### Task 1: `Located`, the cheapest level

**Files:**
- Modify: `crates/rm-engine/src/read.rs`

**Interfaces:**
- Produces, for Tasks 2 and 3:
  ```rust
  pub struct Located {
      pub entity: StableId,
      pub name: Option<String>,
      pub assertion: AssertionId,
      pub attribute: String,
      pub standing: Standing,
      pub score: f32,
  }
  impl Engine {
      pub fn recall_located(&self, q: &Query) -> Result<Vec<Located>, EngineError>;
  }
  ```
- Consumes: `Engine::recall`, `Recalled`, `Standing`.

- [ ] **Step 1: Write the failing test**

Add to `crates/rm-engine/tests/readme.rs`'s sibling — a new file
`crates/rm-engine/tests/tiering.rs`:

```rust
//! Three depths over one stored assertion, and nothing lost between them.

use rm_engine::{
    BlockingKey, Comparator, Engine, FieldRule, Interval, Metric, Observation, Policy,
    Provenance, Query, Record, Ruleset, Source, Strategy, Supersession, VectorIndex,
};

const NOW: i64 = 1_725_000_000_000;

fn ruleset() -> Ruleset {
    Ruleset::new(
        vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
        vec![BlockingKey::Prefix("name".to_string(), 3)],
        4.0,
        6.0,
    )
    .unwrap()
}

fn seeded() -> Engine {
    let mut e = Engine::new(
        VectorIndex::new(3, Metric::Cosine),
        ruleset(),
        Policy::new(Strategy::MostRecent),
    );
    e.remember(Observation {
        kind: "person".into(),
        mention: Record::new().with("name", "Rosalind Okafor"),
        attribute: "role".into(),
        value: Some("owns the Okta setup".into()),
        valid: Interval::since(NOW),
        provenance: Provenance::new(Source::UserAssertion, NOW, "tiering-test"),
        supersession: Supersession::Corrects,
        embedding: vec![1.0, 0.0, 0.0],
    })
    .unwrap();
    e
}

/// The cheapest level says what was found and whether it stands, and carries
/// no assertion text at all.
///
/// It is a distinct type rather than `Recalled` with fields blanked, because
/// `Recalled::value` is already `Option` and `None` there means *asserted
/// absent*. Reusing it for *not asked for* would make a tombstone
/// indistinguishable from an omission, which is the confusion this store
/// exists to prevent.
#[test]
fn located_carries_the_locator_and_nothing_the_caller_did_not_ask_for() {
    let e = seeded();
    let hits = e
        .recall_located(&Query::new(vec![1.0, 0.0, 0.0], 5))
        .unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].attribute, "role");
    assert_eq!(hits[0].name.as_deref(), Some("Rosalind Okafor"));
    assert!(hits[0].score > 0.9);

    // The type itself is the guarantee: there is no field to read the value
    // from, so no caller can accidentally depend on text at this depth.
    let serialised = format!("{:?}", hits[0]);
    assert!(
        !serialised.contains("owns the Okta setup"),
        "the value reached a locator-only hit: {serialised}"
    );
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rusty-memory-engine --test tiering`
Expected: FAIL — `no method named recall_located`.

- [ ] **Step 3: Implement**

In `crates/rm-engine/src/read.rs`, beside `Recalled`:

```rust
/// A hit at its cheapest: what was found, and whether it still stands.
///
/// A separate type rather than `Recalled` with its value-bearing fields
/// blanked. `Recalled::value` is already `Option<String>` and `None` there
/// means the assertion claimed the attribute had no value; reusing it to mean
/// "not fetched at this depth" would make a tombstone indistinguishable from
/// an omission. That is the `Absent`/`Unknown` confusion this store exists to
/// prevent, and it has no business appearing in the store's own return type.
#[derive(Clone, Debug, PartialEq)]
pub struct Located {
    pub entity: StableId,
    pub name: Option<String>,
    pub assertion: AssertionId,
    pub attribute: String,
    pub standing: Standing,
    pub score: f32,
}

impl Engine {
    /// Recall, without the assertion text.
    ///
    /// For a caller that wants to know what exists before deciding what to
    /// read. Derived from the same query path as [`Engine::recall`] and then
    /// narrowed — nothing is precomputed, so no level can drift from another.
    pub fn recall_located(&self, q: &Query) -> Result<Vec<Located>, EngineError> {
        Ok(self
            .recall(q)?
            .into_iter()
            .map(|r| Located {
                entity: r.entity,
                name: r.name,
                assertion: r.assertion,
                attribute: r.attribute,
                standing: r.standing,
                score: r.score,
            })
            .collect())
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rusty-memory-engine`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-engine/src/read.rs crates/rm-engine/tests/tiering.rs
git commit -m "A recall level that says what exists without saying what it says"
```

---

### Task 2: `Traced`, and why a hit came back

**Files:**
- Modify: `crates/rm-engine/src/read.rs`
- Modify: `crates/rm-engine/tests/tiering.rs`

**Interfaces:**
- Consumes: `Located` and `recall_located` from Task 1; `Engine::store_history`.
- Produces, for Task 3:
  ```rust
  pub struct Traced {
      pub recalled: Recalled,
      /// Every version in this assertion's slot, oldest first.
      pub history: Vec<Version>,
  }
  impl Engine {
      pub fn recall_traced(&self, q: &Query) -> Result<Vec<Traced>, EngineError>;
  }
  ```

- [ ] **Step 1: Write the failing test**

```rust
/// The deepest level answers what an answer rests on.
///
/// Not why the vector matched — that is a cosine score and this does not
/// pretend otherwise. What it gives is the part a caller can act on: who
/// asserted it, and what it stands against.
#[test]
fn traced_carries_what_the_answer_rests_on() {
    let mut e = seeded();
    // A correction, so the slot has something to stand against.
    e.remember(Observation {
        kind: "person".into(),
        mention: Record::new().with("name", "Rosalind Okafor"),
        attribute: "role".into(),
        value: Some("owns Okta and the SSO rollout".into()),
        valid: Interval::since(NOW + 1),
        provenance: Provenance::new(Source::UserAssertion, NOW + 1, "tiering-test-2"),
        supersession: Supersession::Corrects,
        embedding: vec![1.0, 0.0, 0.0],
    })
    .unwrap();

    let hits = e
        .recall_traced(&Query::new(vec![1.0, 0.0, 0.0], 5))
        .unwrap();
    let hit = hits.first().expect("something came back");

    assert_eq!(hit.history.len(), 2, "both versions, neither overwritten");
    assert_eq!(hit.recalled.provenance.source_ref, "tiering-test-2");
}

/// Nothing is lost between levels.
///
/// The guarantee that separates this from summarisation: a deeper level is a
/// superset, byte for byte, not a re-rendering.
#[test]
fn a_deeper_level_is_a_superset_and_not_a_rewrite() {
    let e = seeded();
    let q = Query::new(vec![1.0, 0.0, 0.0], 5);

    let located = e.recall_located(&q).unwrap();
    let stated = e.recall(&q).unwrap();
    let traced = e.recall_traced(&q).unwrap();

    assert_eq!(located.len(), stated.len());
    assert_eq!(stated.len(), traced.len());
    for ((l, s), t) in located.iter().zip(&stated).zip(&traced) {
        assert_eq!(l.assertion, s.assertion);
        assert_eq!(&t.recalled, s, "Traced re-rendered the assertion");
        assert_eq!(l.standing, s.standing);
    }
}
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p rusty-memory-engine --test tiering`
Expected: FAIL — `no method named recall_traced`.

- [ ] **Step 3: Implement**

```rust
/// A hit with what it rests on: who asserted it, and the versions it stands
/// against.
///
/// `recalled` is the ordinary `Recalled`, unchanged and not re-rendered —
/// a deeper level is a superset of a shallower one, which is the property
/// that separates this from summarising.
#[derive(Clone, Debug, PartialEq)]
pub struct Traced {
    pub recalled: Recalled,
    pub history: Vec<Version>,
}

impl Engine {
    /// Recall, with each hit's slot history.
    ///
    /// This does not explain why the vector matched; that is a cosine score
    /// and no amount of history makes it an explanation. It answers the
    /// question a caller can act on — what the answer rests on — in one call
    /// instead of one per hit.
    pub fn recall_traced(&self, q: &Query) -> Result<Vec<Traced>, EngineError> {
        self.recall(q)?
            .into_iter()
            .map(|r| {
                let history = self.store_history(r.entity, &r.attribute).to_vec();
                Ok(Traced { recalled: r, history })
            })
            .collect()
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rusty-memory-engine`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-engine/src/read.rs crates/rm-engine/tests/tiering.rs
git commit -m "A recall level that carries what the answer rests on"
```

---

### Task 3: Measure it, and decide whether `Located` ships

**Files:**
- Create: `crates/rm-engine/tests/tiering_cost.rs`
- Modify: `docs/absence-benchmark.md` is **not** touched; create `docs/tiering-cost.md`

**Interfaces:**
- Consumes: `Located`, `Recalled`, `Traced` from Tasks 1 and 2.

- [ ] **Step 1: Write the measurement**

The spec makes this decisive rather than decorative: *"If the measured saving
is small, the honest outcome is that only `Traced` ships and `Located` does
not."* So the test reports, and a human reads it.

```rust
//! What each depth costs, over a fixed corpus.
//!
//! Ignored: it asserts a floor, not a target, and the number it prints is for
//! `docs/tiering-cost.md`. Run with
//! `cargo test -p rusty-memory-engine --test tiering_cost -- --ignored --nocapture`.

#[test]
#[ignore]
fn report_bytes_per_hit_at_each_depth() {
    let e = seeded_with(20);
    let q = Query::new(vec![1.0, 0.0, 0.0], 20);

    let located = format!("{:?}", e.recall_located(&q).unwrap()).len();
    let stated = format!("{:?}", e.recall(&q).unwrap()).len();
    let traced = format!("{:?}", e.recall_traced(&q).unwrap()).len();

    println!("located {located} chars, stated {stated}, traced {traced}");
    println!("located saves {:.0}% against stated", 100.0 * (1.0 - located as f64 / stated as f64));
}
```

`Debug` formatting is a stand-in for wire bytes and will overstate both sides
similarly; say so in the doc rather than implying it is the JSON size. The MCP
serialisation is what a client actually pays, and Task 4 of the plan for
`docs/superpowers/specs/2026-08-26-tool-table-cost-design.md` already owns that
surface.

- [ ] **Step 2: Assert a floor, not a target**

```rust
/// `Located` has to be meaningfully cheaper or it is not worth a second call.
///
/// A floor rather than a target: the spec's decision rule is that a small
/// saving means `Located` does not ship, and this is what makes that decision
/// from a number instead of an impression.
#[test]
fn located_is_at_least_a_third_cheaper_than_stated() {
    let e = seeded_with(20);
    let q = Query::new(vec![1.0, 0.0, 0.0], 20);
    let located = format!("{:?}", e.recall_located(&q).unwrap()).len();
    let stated = format!("{:?}", e.recall(&q).unwrap()).len();
    assert!(
        (located as f64) < 0.67 * stated as f64,
        "located {located} against stated {stated} -- the spec says a small \
         saving means this level should not ship"
    );
}
```

- [ ] **Step 3: Run it**

Run: `cargo test -p rusty-memory-engine --test tiering_cost`
Expected: PASS. **If it fails, stop and report** — that is the spec's decision
rule firing, not a test to be adjusted.

- [ ] **Step 4: Write `docs/tiering-cost.md`**

Record the three figures, that `Debug` length is a proxy for wire bytes and
overstates both sides, and the round-trip risk the spec names: for an MCP client
a second call is a turn, and a turn can cost more than the bytes saved. State
that the saving only pays where a query returns several hits and the caller
wants text from one.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-engine/tests/tiering_cost.rs docs/tiering-cost.md
git commit -m "What each depth costs, and the rule for whether the cheapest one ships"
```

---

### Task 4: The MCP surface

**Files:**
- Modify: `crates/rm-mcp/src/tools.rs`
- Modify: `crates/rm-mcp/src/serve.rs`
- Modify: `crates/rm-mcp/src/render.rs`

- [ ] **Step 1: Write the failing test**

`Depth` survives here and only here, as the vocabulary a model sees.

```rust
/// `recall` takes a depth, and the default is what it has always returned.
///
/// Opt-in in the direction that saves money: a caller who does not know about
/// depths pays exactly today's price, never more.
#[test]
fn recall_reads_a_depth_and_defaults_to_stated() {
    let Call::Recall { depth, .. } =
        Call::read("recall", &json!({"query": "who owns Okta"}), Some("RM")).unwrap()
    else {
        panic!("expected Recall")
    };
    assert_eq!(depth, Depth::Stated, "the default must not cost more than today");

    let Call::Recall { depth, .. } = Call::read(
        "recall",
        &json!({"query": "who owns Okta", "depth": "located"}),
        Some("RM"),
    )
    .unwrap() else {
        panic!("expected Recall")
    };
    assert_eq!(depth, Depth::Located);
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rusty-memory-mcp --lib recall_reads_a_depth`
Expected: FAIL — `Call::Recall` has no `depth`.

- [ ] **Step 3: Implement**

Add `depth: Depth` to `Call::Recall`, read it from an optional `"depth"` string
(`"located"` / `"stated"` / `"traced"`, defaulting to `Stated`), dispatch in
`serve.rs` to the matching engine method, and render each in `render.rs`.

The property description must say what each level is **for**, not what it
contains — the schema already says what it contains:

```
"depth": {
    "enum": ["located", "stated", "traced"],
    "default": "stated",
    "description": "How much of each hit to return. \"located\" gives what was found without the text, for deciding what to read. \"traced\" adds who asserted it and what it replaced. Nothing is summarised at any level: a deeper one is a superset."
}
```

- [ ] **Step 4: Run the tests, and re-measure the table**

```bash
cargo test -p rusty-memory-mcp
cargo test -p rusty-memory-mcp --lib report_sizes -- --ignored --nocapture
```

`recall` gains bytes here. Update `EXPECTED_BYTES`, the README's cost table row
and `definitions`' comment **in this commit** — those three disagreed for a day
in August 2026 because one moved and the others did not.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-mcp/src/ README.md
git commit -m "A depth on recall, and the table's cost updated with it"
```

---

### Task 5: The record

- [ ] **Step 1: Record the decision**

From a script file, never inline — an inline `--scope "*"` globs before the
command sees it.

```bash
rmem decide "Tiering omits, it does not summarise" \
  "three return types at three depths, derived at query time, default unchanged" \
  --context "OpenViking writes model-generated L0/L1/L2 summaries on every write. The layers here already existed structurally: about was L0-shaped and store_history was L2, and nothing exposed them as levels" \
  --because "a summarised layer costs a completion per write, which undoes the door note was built to open, and it is lossy re-summarisation, the operation this store defines itself against. Separate types rather than optional fields because Recalled::value is already Option and None there means asserted absent -- reusing it for not-fetched would make a tombstone indistinguishable from an omission, which is the confusion this store exists to prevent, in its own return type" \
  --scope "*"
```

- [ ] **Step 2: Run the whole gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
```

---

## Finishing

Use `superpowers:finishing-a-development-branch`. Base branch is `main`.

The pull request reports the three byte figures, states that `Debug` length is a
proxy that overstates both sides, and names the round-trip risk: for an MCP
client a second call is a turn, and a turn can cost more than the bytes saved.

**Sequencing:** land this before the multi-holder plan. Both touch
`crates/rm-engine/src/read.rs`, this one is additive and that one changes the
survivorship key, and resolving that conflict in the other order is harder.
