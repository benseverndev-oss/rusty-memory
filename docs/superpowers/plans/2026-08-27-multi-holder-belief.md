# Multi-Holder Belief Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an assertion say whose view it is, so a shared store keeps two people's disagreement instead of settling it by arrival order.

**Architecture:** `Version` gains an optional holder and survivorship partitions a slot's log by it. The attribute map is **not** re-keyed — the log stays `BTreeMap<String, Vec<Version>>`, so snapshots keep their shape and old ones round-trip byte for byte.

**Tech Stack:** Rust, `serde`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-27-multi-holder-belief-design.md`

## Global Constraints

- **A holder-less read returns only holder-less assertions.** This is the entire compatibility story. Nothing written before this feature changes meaning; the 327 entities in the live store answer exactly as they do now. Views and facts do not mix, in either direction.
- **A holder is a `StableId`, never a string.** A holder is somebody the store already knows, so two spellings of one person cannot become two holders.
- **No `Contested` variant.** `Believed` keeps three states. Adding a fourth would change what every existing read can return.
- **No ranking, no trust weighting, no seniority.** A store that ranked holders would be fabricating consensus with extra steps.
- **No inferring a holder from the author.** `provenance.source_ref` says who *wrote* it; that is not whose view it is, and conflating them would make every historical assertion retroactively somebody's opinion.
- Land the tiering plan first. Both touch `crates/rm-engine/src/read.rs`; that one is additive and this one changes survivorship.

## The implementation the spec did not specify

The spec says the survivorship key becomes `(entity, attribute, according_to)`.
That is the right idea and the wrong data structure. The slot is:

```rust
pub struct Entity {
    /// Attribute name to its append-only version log. `BTreeMap` so snapshots
    /// are byte-stable and diffable across runs.
    pub attributes: BTreeMap<String, Vec<Version>>,
}
```

Re-keying that map to a composite would change every snapshot on disk and make
them unreadable and undiffable. **Partition the log instead**: `Version` gains
`according_to`, and survivorship groups by it before resolving.

`Version` already shows how to add a field without breaking a snapshot — the
`supersession` field carries `#[serde(default, skip_serializing_if = ...)]` and
its comment says "written only when it says something, so a snapshot from
before the field existed round-trips byte for byte". Follow it exactly.

---

### Task 1: A version can say whose view it is

**Files:**
- Modify: `crates/rm-store/src/lib.rs`

**Interfaces:**
- Produces, for Tasks 2–4: `Version.according_to: Option<StableId>`.

- [ ] **Step 1: Write the failing test**

```rust
/// A snapshot written before holders existed round-trips byte for byte.
///
/// The same promise `supersession` makes, for the same reason: a store's whole
/// value is that it stays reconstructible, and a field that rewrites every
/// existing snapshot on upgrade costs more than it is worth.
#[test]
fn a_holder_less_version_serialises_exactly_as_it_did_before() {
    let v = Version {
        value: Some("Circulation".into()),
        valid: Interval::since(100),
        provenance: Provenance::new(Source::UserAssertion, 100, "s"),
        supersession: Supersession::Unstated,
        according_to: None,
    };
    let json = serde_json::to_string(&v).unwrap();
    assert!(
        !json.contains("according_to"),
        "a holder-less version wrote the field: {json}"
    );

    // ...and a snapshot that predates the field still reads.
    let old = r#"{"value":"Circulation","valid":{"from":100},"provenance":{"source":"UserAssertion","observed_at":100,"source_ref":"s"}}"#;
    let back: Version = serde_json::from_str(old).unwrap();
    assert_eq!(back.according_to, None);
}
```

The `old` literal must match this crate's actual serialised shape. Produce it by
serialising a holder-less `Version` **before** adding the field and pasting the
output; do not hand-write it from the struct definition.

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rusty-memory-store --lib holder_less_version`
Expected: FAIL — `Version` has no field `according_to`.

- [ ] **Step 3: Implement**

```rust
    /// Whose view this is, when it is a view rather than a fact.
    ///
    /// An entity, not a label: a holder is somebody the store already knows,
    /// so `about(holder, "role")` works on them like anyone else and two
    /// spellings of one person cannot become two holders.
    ///
    /// `None` is the store's own assertion, which is what every version
    /// written before this field existed is. Survivorship partitions a slot by
    /// this before resolving, so one holder correcting themselves is a
    /// correction and two holders differing is not.
    ///
    /// Written only when it says something, so a snapshot from before the
    /// field existed round-trips byte for byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub according_to: Option<StableId>,
```

Every construction site of `Version` in the workspace now needs the field. Add
`according_to: None` at each; `cargo check` lists them.

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace`
Expected: PASS, unchanged count.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-store/src/lib.rs
git commit -m "A version can say whose view it is, without rewriting any snapshot"
```

---

### Task 2: Survivorship partitions a slot by holder

**Files:**
- Modify: `crates/rm-store/src/lib.rs` (`MemoryStore::as_of` and its callers)
- Modify: `crates/rm-engine/src/read.rs`

**Interfaces:**
- Consumes: `Version.according_to` from Task 1.
- Produces, for Task 3: reads filtered by holder, with `None` meaning holder-less only.

- [ ] **Step 1: Write the failing tests**

```rust
/// Two people differing is not one person correcting themselves.
///
/// This is the whole feature. Before it, these two assertions landed in one
/// slot and survivorship picked a winner by arrival — reporting a correction
/// where nothing was corrected and a change where nothing changed.
#[test]
fn two_holders_differing_is_not_a_correction() {
    let mut e = engine();
    let jon = 300;
    let divya = 301;
    let subject = seed_subject(&mut e);

    remember_view(&mut e, subject, "team", "Circulation", jon, 100);
    remember_view(&mut e, subject, "team", "R&A", divya, 200);

    assert_eq!(
        e.about_according_to(subject, "team", jon, Timestamp::MAX, Timestamp::MAX).unwrap(),
        Believed::Value("Circulation".into()),
        "Divya's later view overwrote Jon's"
    );
    assert_eq!(
        e.about_according_to(subject, "team", divya, Timestamp::MAX, Timestamp::MAX).unwrap(),
        Believed::Value("R&A".into())
    );
}

/// A holder correcting themselves still corrects.
///
/// The guard that partitioning did not simply disable survivorship.
#[test]
fn one_holder_correcting_themselves_is_still_a_correction() {
    let mut e = engine();
    let jon = 300;
    let subject = seed_subject(&mut e);

    remember_view(&mut e, subject, "team", "Circulation", jon, 100);
    remember_view(&mut e, subject, "team", "Circ Ops", jon, 200);

    assert_eq!(
        e.about_according_to(subject, "team", jon, Timestamp::MAX, Timestamp::MAX).unwrap(),
        Believed::Value("Circ Ops".into())
    );
}

/// A holder-less read never sees a view, and a holder's read never sees a fact.
///
/// The compatibility promise, stated as a test in both directions. Without the
/// second half, the 327 entities in the live store would start answering
/// differently the moment anybody recorded an opinion about them.
#[test]
fn facts_and_views_do_not_mix_in_either_direction() {
    let mut e = engine();
    let jon = 300;
    let subject = seed_subject(&mut e);

    remember_fact(&mut e, subject, "team", "Circulation", 100);
    remember_view(&mut e, subject, "team", "R&A", jon, 200);

    assert_eq!(
        e.about(subject, "team", Timestamp::MAX, Timestamp::MAX).unwrap(),
        Believed::Value("Circulation".into()),
        "an opinion reached a holder-less read"
    );
    assert_eq!(
        e.about_according_to(subject, "team", jon, Timestamp::MAX, Timestamp::MAX).unwrap(),
        Believed::Value("R&A".into()),
        "a fact reached a holder's read"
    );
}
```

Write `engine()`, `seed_subject`, `remember_fact` and `remember_view` as helpers
in the same file, following the pattern in `crates/rm-engine/tests/absence.rs`.

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p rusty-memory-engine --test holders`
Expected: FAIL — `no method named about_according_to`.

- [ ] **Step 3: Implement**

Add a holder filter to the read path. `MemoryStore::as_of` gains a
`according_to: Option<StableId>` parameter and filters the slot to versions
whose `according_to` equals it — including `None` matching only `None`.

```rust
// `None` matches only `None`. A holder-less read returning views would make
// every existing entity start answering differently the moment somebody
// recorded an opinion about it, and a holder's read returning facts would
// attribute the store's own assertions to a person who never made them.
let slot: Vec<&Version> = versions
    .iter()
    .filter(|v| v.according_to == according_to)
    .collect();
```

`Engine::about` passes `None`; `Engine::about_according_to(entity, attribute,
holder, valid_t, tx_t)` passes `Some(holder)`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace`
Expected: PASS. The absence corpus and the resolution corpus must both be
unchanged — every one of their assertions is holder-less, so every one of their
answers must be identical. If either moves, partitioning has leaked.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-store/src/lib.rs crates/rm-engine/src/read.rs crates/rm-engine/tests/holders.rs
git commit -m "Survivorship partitions a slot by holder, so differing is not correcting"
```

---

### Task 3: Writing a view, and asking who holds one

**Files:**
- Modify: `crates/rm-engine/src/lib.rs` (`Observation`)
- Modify: `crates/rm-engine/src/read.rs` (`holders_of`)

**Interfaces:**
- Produces, for Task 4:
  ```rust
  // on Observation
  pub according_to: Option<StableId>,
  // on Engine
  pub fn holders_of(&self, entity: StableId, attribute: &str) -> Vec<StableId>;
  ```

- [ ] **Step 1: Write the failing test**

```rust
/// Disagreement is recorded, and a caller who wants to see it asks.
///
/// Deliberately a separate call rather than a fourth `Believed` variant: a
/// `Contested` answer would change what every existing read can return, and
/// the compatibility promise is worth more than the convenience.
#[test]
fn holders_of_names_everyone_with_a_view_and_nobody_else() {
    let mut e = engine();
    let (jon, divya) = (300, 301);
    let subject = seed_subject(&mut e);

    remember_fact(&mut e, subject, "team", "Circulation", 100);
    remember_view(&mut e, subject, "team", "Circulation", jon, 110);
    remember_view(&mut e, subject, "team", "R&A", divya, 120);
    remember_view(&mut e, subject, "role", "peer", jon, 130);

    let mut holders = e.holders_of(subject, "team");
    holders.sort_unstable();
    assert_eq!(holders, vec![jon, divya].tap_sorted());

    assert_eq!(
        e.holders_of(subject, "employer"),
        Vec::<StableId>::new(),
        "an attribute nobody holds a view on has no holders"
    );
}
```

Replace `tap_sorted()` with a plain sorted `vec![300, 301]` — it is written here
only to make the intent obvious, and a helper that exists in no crate is a plan
failure.

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rusty-memory-engine --test holders holders_of`
Expected: FAIL — `no method named holders_of`.

- [ ] **Step 3: Implement**

`Observation` gains `according_to: Option<StableId>`, carried through
`Engine::remember` onto the `Version` it writes. `holders_of` collects the
distinct `Some` values from a slot, sorted, so the result is stable across runs
and diffable.

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-engine/src/
git commit -m "Record whose view a fact is, and name everyone who holds one"
```

---

### Task 4: The `note` surface, and saying what the default asserts

**Files:**
- Modify: `crates/rm-host/src/command.rs`
- Modify: `crates/rm-cli/src/args.rs`, `crates/rm-cli/src/format.rs`
- Modify: `crates/rm-mcp/src/tools.rs`

- [ ] **Step 1: Write the failing test**

The spec's headline risk is that `note X role Y` and `note X role Y
--according-to Z` differ by one flag and produce records that never meet, so a
forgotten flag silently promotes an opinion to a fact.

```rust
/// `--according-to` takes an entity id, not a name.
///
/// Resolving a holder's name would put a resolution failure — and possibly a
/// review — in the middle of a write. The host resolves first; this parses an
/// id or refuses.
#[test]
fn according_to_takes_an_id_and_refuses_a_name() {
    let Command::Note { according_to, .. } =
        parse_args(&["note", "Jon", "team", "Circulation", "--according-to", "300"]).unwrap()
    else {
        panic!("expected Note")
    };
    assert_eq!(according_to, Some(300));

    let err = parse_args(&["note", "Jon", "team", "Circulation", "--according-to", "Divya"])
        .unwrap_err();
    assert!(format!("{err}").contains("entity id"), "{err}");
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rusty-memory-cli --lib according_to_takes_an_id`
Expected: FAIL — `Command::Note` has no `according_to`.

- [ ] **Step 3: Implement, and make the default say something**

Thread `according_to` through `Command::Note` → `plan_note` → `Observation`.
The output must distinguish the two, because the flag's absence is the risk:

```
team recorded on entity 305, new                        # a fact
team recorded on entity 305 as entity 300's view, new   # a view
```

The MCP property description names the consequence rather than the field:

```
"according_to": {
    "type": "integer",
    "description": "The entity whose view this is, if it is a view rather than a fact. Omit it and the assertion is the store's own — a fact nobody is named as holding. Two people differing are both kept; neither overwrites the other."
}
```

- [ ] **Step 4: Run the tests and re-measure the table**

```bash
cargo test --workspace
cargo test -p rusty-memory-mcp --lib report_sizes -- --ignored --nocapture
```

`note` gains bytes. Update `EXPECTED_BYTES`, the README's row and
`definitions`' comment in this commit.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-host/src/command.rs crates/rm-cli/src/ crates/rm-mcp/src/ README.md
git commit -m "note can say whose view it is, and says so when it does"
```

---

### Task 5: The record, and the version

- [ ] **Step 1: Check the conformance suite**

`rm-conform` checks the engine against an independently written reference
model. Partitioning survivorship is exactly the kind of change it exists to
catch, so run it and read the result rather than assuming the workspace suite
covered it:

```bash
cargo test -p rusty-memory-conform
```

- [ ] **Step 2: Decide the version**

`rm-core` and `rm-survivor` are described as 0.1 and additive-only. `Version`
gaining an optional field is additive; survivorship partitioning a slot is a
behaviour change for any caller who was relying on views and facts sharing one
log — which is nobody, because views did not exist. Record which reading was
taken and why in the commit, and bump the workspace version if the answer is
that it is not additive.

- [ ] **Step 3: Record the decision**

From a script file, never inline:

```bash
rmem decide "Two people disagreeing is not a contradiction to resolve" \
  "an assertion can name whose view it is; survivorship partitions a slot by holder, and a holder-less read sees only holder-less assertions" \
  --context "the third case of a shape handled twice already: disagreement across time is kept and resolved at read, identities too close to call are filed rather than merged, and holders were being settled by arrival order" \
  --because "nothing was corrected and nothing changed -- two people simply differ, and survivorship reported a correction anyway. A holder is an entity rather than a label so two spellings cannot become two holders. No Contested variant, because it would change what every existing read can return; no ranking of holders, because that is fabricating consensus with extra steps; and no inferring a holder from whoever wrote the record, because every historical assertion would retroactively become somebody's opinion" \
  --scope "*"
```

- [ ] **Step 4: Run the whole gate**

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
```

---

## Finishing

Use `superpowers:finishing-a-development-branch`. Base branch is `main`.

The pull request states that no existing assertion changed meaning, that the
absence and resolution corpora are unchanged and why that is the test that
matters, and which version reading was taken for `rm-survivor`.

It also states what was deliberately not built: no `Contested` answer, no
ranking of holders, no inferred holders, and no per-holder access control —
holders say whose view a fact is, not who may read it.
