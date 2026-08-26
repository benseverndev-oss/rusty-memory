# Direct Assert Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rmem note` — record a fact you already know, with one embedding and no completion model.

**Architecture:** A `plan_note`/`commit_note` pair in `rm-host` mirroring `plan_decide`/`commit_decide`: the plan takes the embedder and does the one embedding, the commit takes the engine and the lock. `commit_note` calls `Engine::remember` (the resolving path) rather than `remember_as`, which is the entire point — it is what makes the resolver run for the first time.

**Tech Stack:** Rust, pinned toolchain 1.98.0. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-26-direct-assert-design.md`

## Global Constraints

- **Toolchain is pinned at 1.98.0.** Every task ends green under `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --workspace`.
- **Verify with exit codes.** `cargo test --workspace && git commit`. A count is a cross-check, never the check — entity 219.
- **Baseline is 804 passing, 0 failing** on `main` at `99ff99e`.
- **`plan_note` must take no `Completer` parameter.** That is the deliverable, stated as a type rather than as a test asserting a negative: if the signature cannot name a completer, no completion can happen.
- **`commit_note` calls `Engine::remember`, never `remember_as`.** `remember_as` takes an entity the caller already identified, which is what `decide` uses and why the resolver has never run.
- **Never changes existing behaviour.** New command, new tool. `decide`, `remember` and every read are untouched.
- **No ruleset change.** The shipped thresholds (`review_at = 5.2439`, `match_at = 7.2439`) stay exactly as they are; changing them deserves its own evidence.
- **Commit trailers**, on every commit:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
  ```

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/rm-host/src/command.rs` | `NotePlan`, `plan_note`, `commit_note`, `Outcome::Noted`. The whole rule lives here; both hosts are thin. |
| `crates/rm-cli/src/args.rs` | `Command::Note` and its flags. |
| `crates/rm-cli/src/run.rs` | Wiring: embedder, plan, commit. |
| `crates/rm-cli/src/format.rs` | Rendering the three outcomes. |
| `crates/rm-mcp/src/tools.rs` | The `note` tool's schema and `Call::Note`. |
| `crates/rm-mcp/src/serve.rs` | Wiring, mirroring the `decide` arm. |
| `crates/rm-mcp/src/render.rs` | Rendering for MCP. |
| `README.md` | The command, and what it costs. |

Four tasks. Task 1 is the substance and is independently testable without either host; Tasks 2 and 3 are the two surfaces; Task 4 is the record.

---

### Task 1: `plan_note` and `commit_note`

**Files:**
- Modify: `crates/rm-host/src/command.rs`

**Interfaces:**
- Produces, for Tasks 2 and 3:
  - `pub struct NotePlan` (opaque; built by `plan_note`, consumed by `commit_note`)
  - ```rust
    pub fn plan_note(
        who: &str,
        kind: &str,
        attribute: &str,
        value: Option<&str>,
        fields: &[(String, String)],
        valid_from: Option<Timestamp>,
        observed_at: Timestamp,
        session: &str,
        scope: Option<&str>,
        embedder: &impl Embedder,
    ) -> Result<NotePlan, HostError>
    ```
  - `pub fn commit_note(engine: &mut Engine, plan: NotePlan) -> Result<Outcome, HostError>`
  - `Outcome::Noted { entity: StableId, attribute: String, absent: bool, merged: bool, review: Option<PendingReview> }`
- Consumes: `Engine::remember`, `Remembered::{Merged, Created, CreatedPendingReview}`, `rm_engine::PendingReview { id, a, b, score }`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/rm-host/src/command.rs`'s first test module (the one with `TempDir`, beside the `init` tests — **not** `rescope_tests`):

```rust
    // ---- a fact you already know -----------------------------------------

    /// A name nobody has mentioned creates an entity.
    #[test]
    fn a_note_about_someone_new_creates_them() {
        let mut e = decision_engine();
        let plan = plan_note(
            "Jon Severn", "person", "role", Some("leads circ"),
            &[], None, 100, "test", None, &Hashed::new(3),
        )
        .unwrap();
        let Outcome::Noted { entity, merged, review, .. } = commit_note(&mut e, plan).unwrap()
        else {
            panic!("expected Noted")
        };
        assert!(!merged, "nothing was there to merge onto");
        assert!(review.is_none());
        assert_eq!(
            e.about(entity, "role", Timestamp::MAX, Timestamp::MAX).unwrap(),
            Believed::Value("leads circ".into())
        );
    }

    /// The same name again lands on the same entity rather than a second one.
    ///
    /// This is the resolver doing its job, and it is the first time anything in
    /// this store has asked it to.
    #[test]
    fn a_second_note_about_the_same_name_joins_the_first() {
        let mut e = decision_engine();
        let first = plan_note(
            "Jon Severn", "person", "role", Some("leads circ"),
            &[], None, 100, "test", None, &Hashed::new(3),
        )
        .unwrap();
        let Outcome::Noted { entity: a, .. } = commit_note(&mut e, first).unwrap() else {
            panic!("expected Noted")
        };

        let second = plan_note(
            "Jon Severn", "person", "team", Some("circulation"),
            &[], None, 200, "test", None, &Hashed::new(3),
        )
        .unwrap();
        let Outcome::Noted { entity: b, merged, .. } = commit_note(&mut e, second).unwrap()
        else {
            panic!("expected Noted")
        };

        assert_eq!(a, b, "the same person twice is one entity");
        assert!(merged, "and the second write should say so");
        assert_eq!(e.entity_count(), 1);
    }

    /// `--absent` asserts there is no value, which is not the same as never
    /// having been asked. The store's own instructions open with this
    /// distinction and no write path could express it before.
    #[test]
    fn an_absence_is_asserted_rather_than_left_unknown() {
        let mut e = decision_engine();
        let plan = plan_note(
            "Jon Severn", "person", "reports", None,
            &[], None, 100, "test", None, &Hashed::new(3),
        )
        .unwrap();
        let Outcome::Noted { entity, absent, .. } = commit_note(&mut e, plan).unwrap() else {
            panic!("expected Noted")
        };
        assert!(absent);

        // Asserted absence, and an attribute nobody mentioned. Two different
        // answers, and collapsing them is the failure this guards.
        assert_eq!(
            e.about(entity, "reports", Timestamp::MAX, Timestamp::MAX).unwrap(),
            Believed::Absent
        );
        assert_eq!(
            e.about(entity, "spouse", Timestamp::MAX, Timestamp::MAX).unwrap(),
            Believed::Unknown
        );
    }

    /// `--valid-from` is valid time and only that: the store learned it now,
    /// and it was true earlier.
    #[test]
    fn a_backdated_note_is_true_from_when_it_started_being_true() {
        let mut e = decision_engine();
        let plan = plan_note(
            "Jon Severn", "person", "role", Some("leads circ"),
            &[], Some(50), 100, "test", None, &Hashed::new(3),
        )
        .unwrap();
        let Outcome::Noted { entity, .. } = commit_note(&mut e, plan).unwrap() else {
            panic!("expected Noted")
        };
        // True at 60, which is before the store was told at 100.
        assert_eq!(
            e.about(entity, "role", 60, Timestamp::MAX).unwrap(),
            Believed::Value("leads circ".into())
        );
    }

    /// Mention fields reach the identity record, so a later ruleset can compare
    /// them without every record being rewritten.
    #[test]
    fn a_mention_field_lands_on_the_identity_not_the_attributes() {
        let mut e = decision_engine();
        let plan = plan_note(
            "Jon Severn", "person", "role", Some("leads circ"),
            &[("email".to_string(), "j@example.com".to_string())],
            None, 100, "test", None, &Hashed::new(3),
        )
        .unwrap();
        let Outcome::Noted { entity, .. } = commit_note(&mut e, plan).unwrap() else {
            panic!("expected Noted")
        };
        let identity = e.identity_of(entity).expect("a noted entity has an identity");
        assert_eq!(identity.get("email"), Some("j@example.com"));
        assert_eq!(identity.get("name"), Some("Jon Severn"));
        // And it is not an attribute: `email` was never noted as one.
        assert_eq!(
            e.about(entity, "email", Timestamp::MAX, Timestamp::MAX).unwrap(),
            Believed::Unknown
        );
    }

    /// An empty name is refused before the embedder, so a typo costs nothing --
    /// the same bargain `plan_decide` makes.
    #[test]
    fn a_note_about_nobody_is_refused_before_it_costs_an_embedding() {
        let err = plan_note(
            "   ", "person", "role", Some("x"),
            &[], None, 100, "test", None, &Hashed::new(3),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("who"), "{err}");
    }
```

Add whatever imports the test module needs: `rm_embed::Hashed`, `rm_engine::Believed`, `Timestamp`. Check whether `decision_engine()` exists as a helper in that module; if it does not, add one that builds an `Engine` with a 3-dimensional index, the test ruleset and `Policy::new(Strategy::MostRecent)` — mirroring what the `init` tests already do.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p rm-host --lib a_note_about_someone_new
```

Expected: FAIL to compile — `cannot find function 'plan_note'`.

- [ ] **Step 3: Write the plan type and `plan_note`**

Add beside `DecidePlan` in `crates/rm-host/src/command.rs`:

```rust
/// Everything [`commit_note`] will need from the embedder, and nothing else.
///
/// The same split `DecidePlan` makes and for the same reason: the embedding
/// happens before the store's exclusive lock is taken, so a slow or failing
/// embedder never holds it.
pub struct NotePlan {
    who: String,
    kind: String,
    attribute: String,
    /// `None` is a tombstone -- an asserted absence, which is a claim and not
    /// a gap. `rm_store` keeps the two apart and so does this.
    value: Option<String>,
    /// Extra mention fields. They reach the identity record, which is what the
    /// resolver compares, and are written once per entity.
    fields: Vec<(String, String)>,
    valid_from: Timestamp,
    observed_at: Timestamp,
    session: String,
    /// The scope and its embedding together, because a scope is a second
    /// attribute and therefore a second vector -- and it is taken here so the
    /// store's exclusive lock is never held while an embedder is called.
    scope: Option<(String, Vec<f32>)>,
    embedding: Vec<f32>,
}

/// Record a fact someone already knows.
///
/// # No completer, stated as a type
///
/// This signature cannot name a `Completer`, which is the whole point:
/// `plan_remember` takes one, so every fact in this store would have cost a
/// completion call, and the cheapest way to record something you already knew
/// was to write prose about it and pay a model to read the prose back.
/// `plan_decide` made the opposite bargain for a decision; this makes it for a
/// fact.
///
/// # `scope` is optional here and required by `plan_decide`
///
/// Not an inconsistency. An entity with no `scope` attribute already reaches
/// every position, so omitting it is the correct answer rather than an unset
/// field -- and a fact about a person is usually true whichever project the
/// asker is standing in. `plan_decide` refuses without one because a
/// decision's reach genuinely varies.
#[allow(clippy::too_many_arguments)]
pub fn plan_note(
    who: &str,
    kind: &str,
    attribute: &str,
    value: Option<&str>,
    fields: &[(String, String)],
    valid_from: Option<Timestamp>,
    observed_at: Timestamp,
    session: &str,
    scope: Option<&str>,
    embedder: &impl Embedder,
) -> Result<NotePlan, HostError> {
    if who.trim().is_empty() {
        return Err(HostError::Refused(
            "a note needs to say who or what it is about: that name is how the store decides whether this is someone it already knows".into(),
        ));
    }
    if attribute.trim().is_empty() {
        return Err(HostError::Refused(
            "a note needs an attribute: the name of the thing being recorded, so it can be asked about later".into(),
        ));
    }

    // Before the embedder, so a typo costs nothing.
    if let Some(scope) = scope {
        crate::scope::validate(scope).map_err(HostError::Refused)?;
    }

    // One embedding, in the same shape `plan_decide` uses for a field.
    let text = match value {
        Some(v) => format!("{who}: {attribute} is {v}"),
        None => format!("{who}: {attribute} is not set"),
    };
    let embedding = embedder
        .embed(&text)
        .map_err(|e| HostError::Refused(e.to_string()))?;

    // A scope is a second attribute, so it needs its own vector -- taken here
    // with the first, before the lock, rather than inside `commit_note`.
    let scope = match scope {
        Some(s) => {
            let v = embedder
                .embed(&format!("{who}: scope is {s}"))
                .map_err(|e| HostError::Refused(e.to_string()))?;
            Some((s.to_string(), v))
        }
        None => None,
    };

    Ok(NotePlan {
        who: who.trim().to_string(),
        kind: kind.trim().to_string(),
        attribute: attribute.trim().to_string(),
        value: value.map(str::to_string),
        fields: fields.to_vec(),
        // Valid time defaults to when the store was told, which is the honest
        // answer when nobody said otherwise.
        valid_from: valid_from.unwrap_or(observed_at),
        observed_at,
        session: session.to_string(),
        scope,
        embedding,
    })
}
```

- [ ] **Step 4: Add the outcome variant**

In `crates/rm-host/src/command.rs`'s `Outcome` enum:

```rust
    /// A fact recorded about someone the store may or may not have known.
    Noted {
        entity: StableId,
        attribute: String,
        /// The fact asserted there is no value.
        absent: bool,
        /// It landed on an entity that already existed.
        merged: bool,
        /// It scored inside the review band against something known. The fact
        /// is recorded either way; what is open is only whose it is.
        review: Option<rm_engine::PendingReview>,
    },
```

- [ ] **Step 5: Write `commit_note`**

Add beside `commit_decide`:

```rust
/// Write the fact, resolving who it is about.
///
/// `Engine::remember`, never `remember_as`. `remember_as` takes an entity the
/// caller has already identified -- which is what `decide` does, and is why
/// this store has 265 entities, an empty review queue and a resolver that has
/// never been asked to judge anything. Naming a person and letting the ruleset
/// decide whether that is someone already known is the whole of what this adds.
pub fn commit_note(engine: &mut Engine, plan: NotePlan) -> Result<Outcome, HostError> {
    let NotePlan {
        who,
        kind,
        attribute,
        value,
        fields,
        valid_from,
        observed_at,
        session,
        scope,
        embedding,
    } = plan;

    let mut mention = Record::new().with("name", who.as_str()).with("kind", kind.as_str());
    for (k, v) in &fields {
        mention = mention.with(k.as_str(), v.as_str());
    }

    let absent = value.is_none();
    let observation = Observation {
        kind: kind.clone(),
        mention,
        attribute: attribute.clone(),
        value,
        valid: Interval::since(valid_from),
        provenance: Provenance::new(Source::UserAssertion, observed_at, session.clone()),
        supersession: Supersession::Corrects,
        embedding,
    };

    let remembered = engine
        .remember(observation)
        .map_err(|e| HostError::Refused(e.to_string()))?;

    let (entity, merged, review) = match remembered {
        rm_engine::Remembered::Merged { entity, .. } => (entity, true, None),
        rm_engine::Remembered::Created { entity, .. } => (entity, false, None),
        rm_engine::Remembered::CreatedPendingReview { entity, review, .. } => {
            (entity, false, Some(review))
        }
    };

    // `remember_as`, not `remember`: the entity was identified by the fact
    // above, and re-resolving the same mention would ask a question already
    // answered -- and could answer it differently.
    if let Some((scope, scope_embedding)) = scope {
        engine
            .remember_as(
                Some(entity),
                Observation {
                    kind: kind.clone(),
                    mention: Record::new()
                        .with("name", who.as_str())
                        .with("kind", kind.as_str()),
                    attribute: "scope".to_string(),
                    value: Some(scope),
                    valid: Interval::since(valid_from),
                    provenance: Provenance::new(Source::UserAssertion, observed_at, session),
                    supersession: Supersession::Corrects,
                    embedding: scope_embedding,
                },
            )
            .map_err(|e| HostError::Refused(e.to_string()))?;
    }
    Ok(Outcome::Noted {
        entity,
        attribute,
        absent,
        merged,
        review,
    })
}
```

**Note the two different write calls, which is the subtle part.** The fact goes
through `remember`, because who it is about is the open question. The scope
goes through `remember_as` against the entity that just came back, because
re-resolving the same mention would ask a question already answered -- and
could answer it differently, leaving a fact on one entity and its scope on
another.

- [ ] **Step 6: Add the scope test**



```rust
    /// A scoped note reaches only where it says, and an unscoped one reaches
    /// everywhere -- the same applicability rule the decision reads use.
    #[test]
    fn a_note_can_be_scoped_and_is_otherwise_everywhere() {
        let mut e = decision_engine();
        let plan = plan_note(
            "Jon Severn", "person", "oncall", Some("tuesdays"),
            &[], None, 100, "test", Some("work/circ-tools"), &Hashed::new(3),
        )
        .unwrap();
        let Outcome::Noted { entity, .. } = commit_note(&mut e, plan).unwrap() else {
            panic!("expected Noted")
        };
        assert_eq!(
            e.about(entity, "scope", Timestamp::MAX, Timestamp::MAX).unwrap(),
            Believed::Value("work/circ-tools".into())
        );
    }
```

- [ ] **Step 7: Run the tests**

```bash
cargo test -p rm-host --lib note
```

Expected: PASS, 7 tests.

**If `a_second_note_about_the_same_name_joins_the_first` fails with two entities**, do not adjust the test. The ruleset's blocking key is `Prefix("name", 3)` and its thresholds are the shipped ones; two identical names failing to merge means the test engine's ruleset differs from the shipped one, and the engine helper is what to look at.

- [ ] **Step 8: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace -q && git add -A crates/ && git commit -m "$(cat <<'EOF'
A fact you already know

plan_note and commit_note: record a fact without asking a model to find one.
The signature cannot name a Completer, which is the deliverable stated as a
type rather than as a test asserting a negative.

commit_note calls Engine::remember rather than remember_as, and that is the
whole point. remember_as takes an entity the caller already identified,
which is what decide does -- and it is why this store has 265 entities, an
empty review queue and a resolver that has never been asked to judge
anything.

--absent writes a tombstone, so an asserted absence is distinguishable from
an attribute nobody has mentioned. That distinction is what the store's own
instructions open with, and no write path could express it before.

scope is optional here and required by decide, which is not an
inconsistency: an entity with no scope attribute already reaches every
position, so omitting it is the correct answer rather than an unset field.
When given it is a second attribute and therefore a second embedding, taken
in the plan so the store's lock is never held while a vector is fetched --
and written with remember_as, because the entity was identified by the fact
and re-resolving would ask a question already answered.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
EOF
)"
```

---

### Task 2: The CLI surface

**Files:**
- Modify: `crates/rm-cli/src/args.rs`, `crates/rm-cli/src/run.rs`, `crates/rm-cli/src/format.rs`

**Interfaces:**
- Consumes from Task 1: `plan_note`, `commit_note`, `Outcome::Noted { entity, attribute, absent, merged, review }`.

- [ ] **Step 1: Write the failing arg tests**

Append to `crates/rm-cli/src/args.rs`'s test module:

```rust
    /// The shape of a note: who, what, and the value -- with the value
    /// optional, because `--absent` says there is none.
    #[test]
    fn a_note_parses_who_what_and_value() {
        assert_eq!(
            parse_args(&["note", "Jon Severn", "role", "leads circ"]).unwrap(),
            Command::Note {
                who: "Jon Severn".into(),
                kind: "person".into(),
                attribute: "role".into(),
                value: Some("leads circ".into()),
                fields: vec![],
                valid_from: None,
                scope: None,
            }
        );
    }

    /// `--absent` is a claim, so it takes the place of the value rather than
    /// sitting beside one.
    #[test]
    fn absent_replaces_the_value_rather_than_joining_it() {
        assert_eq!(
            parse_args(&["note", "Jon", "reports", "--absent"]).unwrap(),
            Command::Note {
                who: "Jon".into(),
                kind: "person".into(),
                attribute: "reports".into(),
                value: None,
                fields: vec![],
                valid_from: None,
                scope: None,
            }
        );
        let err = parse_args(&["note", "Jon", "reports", "none", "--absent"]).unwrap_err();
        assert!(
            format!("{err}").contains("--absent"),
            "a value and --absent contradict each other: {err}"
        );
    }

    /// `--field` repeats, because a person has more than one identifier.
    #[test]
    fn field_repeats_and_keeps_its_order() {
        let Command::Note { fields, .. } = parse_args(&[
            "note", "Jon", "role", "x",
            "--field", "email=j@example.com",
            "--field", "handle=jsev",
        ])
        .unwrap() else {
            panic!("expected Note")
        };
        assert_eq!(
            fields,
            vec![
                ("email".to_string(), "j@example.com".to_string()),
                ("handle".to_string(), "jsev".to_string())
            ]
        );
    }

    /// A `--field` with no `=` is a typo, and typos are refused rather than
    /// stored as a field named after the whole argument.
    #[test]
    fn a_field_without_a_value_is_refused() {
        let err = parse_args(&["note", "Jon", "role", "x", "--field", "email"]).unwrap_err();
        assert!(format!("{err}").contains("--field"), "{err}");
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p rm-cli --lib a_note_parses_who_what
```

Expected: FAIL to compile — `Command` has no variant `Note`.

- [ ] **Step 3: Add the variant and the parser**

In `crates/rm-cli/src/args.rs`'s `Command` enum:

```rust
    Note {
        who: String,
        /// What sort of thing this is. Defaults to `person`, which is what the
        /// first real dataset is; anything else says so.
        kind: String,
        attribute: String,
        /// `None` when `--absent` was given: an asserted absence.
        value: Option<String>,
        /// Extra mention fields, in the order given.
        fields: Vec<(String, String)>,
        valid_from: Option<Timestamp>,
        scope: Option<String>,
    },
```

In the `match first.as_str()` block, beside `"decide"`:

```rust
        "note" => {
            let mut positional: Vec<String> = Vec::new();
            let mut fields: Vec<(String, String)> = Vec::new();
            let (mut absent, mut valid_from, mut scope, mut kind) =
                (false, None, None, "person".to_string());
            let mut rest = args[1..].iter();
            while let Some(a) = rest.next() {
                match a.as_str() {
                    "--absent" => absent = true,
                    "--field" => {
                        let pair = rest.next().ok_or_else(|| {
                            CliError::Usage("--field takes name=value".into())
                        })?;
                        let (k, v) = pair.split_once('=').ok_or_else(|| {
                            CliError::Usage(format!(
                                "--field takes name=value, and {pair:?} has no '=' -- without one there is nothing to compare it against"
                            ))
                        })?;
                        fields.push((k.to_string(), v.to_string()));
                    }
                    "--valid-from" => {
                        let v = rest.next().ok_or_else(|| {
                            CliError::Usage("--valid-from takes a date".into())
                        })?;
                        valid_from = Some(crate::args::date(v)?);
                    }
                    "--scope" => {
                        scope = Some(rest.next().cloned().ok_or_else(|| {
                            CliError::Usage("--scope takes a reach".into())
                        })?);
                    }
                    "--kind" => {
                        kind = rest.next().cloned().ok_or_else(|| {
                            CliError::Usage("--kind takes a sort of thing".into())
                        })?;
                    }
                    other if other.starts_with("--") => {
                        return Err(CliError::Usage(format!("note does not take {other:?}")));
                    }
                    other => positional.push(other.to_string()),
                }
            }
            let (who, attribute, value) = match (positional.len(), absent) {
                (3, false) => (
                    positional[0].clone(),
                    positional[1].clone(),
                    Some(positional[2].clone()),
                ),
                (2, true) => (positional[0].clone(), positional[1].clone(), None),
                (3, true) => {
                    return Err(CliError::Usage(
                        "a value and --absent contradict each other: --absent says there is no value, so do not also give one".into(),
                    ))
                }
                _ => {
                    return Err(CliError::Usage(format!(
                        "note takes <who> <attribute> <value>, or <who> <attribute> --absent

{USAGE}"
                    )))
                }
            };
            Ok(Command::Note {
                who,
                kind,
                attribute,
                value,
                fields,
                valid_from,
                scope,
            })
        }
```

**Check how `decide` parses `--at` before writing `date(v)`** — reuse whatever it already uses rather than adding a second date parser:

```bash
rg -n 'fn date|--at' crates/rm-cli/src/args.rs | head -5
```

If the existing helper has a different name, use that name; if `--at` parses inline, extract it so both call one parser rather than two that can disagree.

Add the usage line beside the others:

```
    rmem note <who> <attr> <value>   record a fact; --absent for an asserted absence
```

- [ ] **Step 4: Wire it in `run.rs`**

In the `Planned` match, beside `Command::Decide`:

```rust
            Command::Note {
                who,
                kind,
                attribute,
                value,
                fields,
                valid_from,
                scope,
            } => {
                // An embedder, never a provider. A fact has a known shape, so
                // this costs one embedding and no completion at all.
                let embedder = config.embedder()?;
                Some(Planned::Note(command::plan_note(
                    who,
                    kind,
                    attribute,
                    value.as_deref(),
                    fields,
                    *valid_from,
                    now,
                    &attribution::cli(),
                    scope.as_deref(),
                    &embedder,
                )?))
            }
```

Add `Note(command::NotePlan)` to the `Planned` enum and a `Planned::Note(p) => command::commit_note(engine, p)` arm wherever `Planned::Decide` is committed.

- [ ] **Step 5: Render the three outcomes**

In `crates/rm-cli/src/format.rs`:

```rust
        Outcome::Noted {
            entity,
            attribute,
            absent,
            merged,
            review,
        } => {
            let what = if *absent {
                format!("{attribute} recorded as absent")
            } else {
                format!("{attribute} recorded")
            };
            let who = if *merged {
                format!("on entity {entity}, which the store already knew")
            } else {
                format!("on entity {entity}, new")
            };
            // The review is reported rather than swallowed. An open question
            // nobody is told about is one nobody settles, and the engine's
            // position is that the fact is kept either way -- what is uncertain
            // is only whose it is.
            match review {
                None => format!("{what} {who}"),
                Some(r) => format!(
                    "{what} {who}\n\nopen question: this scored {:.2} against entity {}, inside the review band. Both are kept, and the fact above is recorded either way -- what is open is only whose it is.\n`rmem review` lists it; `rmem review --confirm {}` says they are the same, `--reject {}` says they are not.",
                    r.score, if r.a == *entity { r.b } else { r.a }, r.id, r.id
                ),
            }
        }
```

- [ ] **Step 6: Run everything and prove it end to end**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace -q
```

Then against a throwaway local store, which needs no key:

```bash
T=/d/Temp/rmem-note-test; R=/d/show_case/rusty-memory/target/release/rmem.exe
cargo build --release -q
rm -rf "$T"; mkdir -p "$T"
cd "$T" && "$R" init --local >/dev/null
cd "$T" && RMEM_CONFIG="$T/rmem.toml" "$R" note "Jon Severn" role "leads circ" --field email=j@example.com
cd "$T" && RMEM_CONFIG="$T/rmem.toml" "$R" note "Jon Severn" team "circulation"
cd "$T" && RMEM_CONFIG="$T/rmem.toml" "$R" note "Jon Severn" reports --absent
cd "$T" && RMEM_CONFIG="$T/rmem.toml" "$R" about 0 role
cd "$T" && RMEM_CONFIG="$T/rmem.toml" "$R" about 0 reports
cd "$T" && RMEM_CONFIG="$T/rmem.toml" "$R" about 0 spouse
```

Expected: the second note says the entity was already known; `role` answers `leads circ`; `reports` answers as an asserted absence; `spouse` answers unknown. **The last two must differ** — that is the distinction this exists for.

- [ ] **Step 7: Commit**

```bash
cargo test --workspace -q && git add -A crates/ && git commit -m "$(cat <<'EOF'
rmem note

<who> <attribute> <value>, or <who> <attribute> --absent. --field repeats,
because a person has more than one identifier and the mention is written
once -- without it every identity would be name-only, permanently, and no
later ruleset change could use an email without a migration.

A value and --absent together are refused rather than resolved by
precedence: they contradict each other, and guessing which the writer meant
is how an asserted absence silently becomes a value.

The review-band outcome is rendered rather than swallowed, and names the
other entity and the score. An open question nobody is told about is one
nobody settles.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
EOF
)"
```

---

### Task 3: The MCP surface

**Files:**
- Modify: `crates/rm-mcp/src/tools.rs`, `crates/rm-mcp/src/serve.rs`, `crates/rm-mcp/src/render.rs`

**Interfaces:**
- Consumes from Task 1: `plan_note`, `commit_note`, `Outcome::Noted`.

- [ ] **Step 1: Write the failing test**

Append to `crates/rm-mcp/src/tools.rs`'s test module:

```rust
    /// The note tool reads its arguments, and an absent value is a claim it
    /// can express.
    #[test]
    fn the_note_tool_reads_who_what_and_an_absence() {
        let Call::Note { who, attribute, value, .. } = Call::read(
            "note",
            &json!({"who": "Jon Severn", "attribute": "role", "value": "leads circ"}),
            Some("RM"),
        )
        .unwrap() else {
            panic!("expected Note")
        };
        assert_eq!(who, "Jon Severn");
        assert_eq!(attribute, "role");
        assert_eq!(value.as_deref(), Some("leads circ"));

        let Call::Note { value, .. } = Call::read(
            "note",
            &json!({"who": "Jon", "attribute": "reports", "absent": true}),
            Some("RM"),
        )
        .unwrap() else {
            panic!("expected Note")
        };
        assert_eq!(value, None, "absent is an asserted absence, not a gap");
    }

    /// A value and `absent` together contradict each other, and are refused
    /// rather than resolved by precedence.
    #[test]
    fn the_note_tool_refuses_a_value_and_an_absence_together() {
        let err = Call::read(
            "note",
            &json!({"who": "Jon", "attribute": "reports", "value": "none", "absent": true}),
            Some("RM"),
        )
        .unwrap_err();
        assert!(format!("{err:?}").contains("absent"), "{err:?}");
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p rm-mcp --lib the_note_tool_reads
```

Expected: FAIL to compile — no `Call::Note`.

- [ ] **Step 3: Add the tool**

Add `Call::Note` to the `Call` enum with the same fields as the CLI's `Command::Note` plus `session: String`, add a `"note"` arm to `Call::read` using `Call::attributed(arguments, client)?` for the session exactly as `decide` does, and add the tool's schema beside `decide`'s with this description:

```
Record a fact you already know about someone or something. Costs one
embedding and no completion. `who` is a name, and the store decides whether
that is someone it already knows -- if it cannot tell, the fact is still
recorded and the identity question is queued for a person. Set `absent` to
assert there is no value, which is different from never having been asked.
```

Add it to the `RMEM_TOOLS` vocabulary wherever the other tool names are listed, so a session can expose `note` without exposing `remember`.

- [ ] **Step 4: Wire and render**

In `serve.rs`, add a `Call::Note { .. }` arm mirroring the `Call::Decide` arm — `Self::vectors(config, provider)?` for the embedder, then `command::plan_note(...)`, then `Planned::Note`.

In `render.rs`, add an `n::Noted { .. }` arm carrying the same three outcomes as the CLI, with the review named when there is one.

- [ ] **Step 5: Run and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace -q && git add -A crates/ && git commit -m "$(cat <<'EOF'
A note tool, and a way to expose it without remember

Same shape as the CLI: who, attribute, value or absent, optional fields and
scope. The description says what it costs, because the reason facts never
reached this store is that the only door charged a completion model.

Listed in the RMEM_TOOLS vocabulary separately from remember, so a session
can record facts it already knows without also turning on extraction.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
EOF
)"
```

---

### Task 4: The record

**Files:**
- Modify: `README.md`
- The store

- [ ] **Step 1: Document the command**

Add to the README beside `rmem decide`, and to the `RMEM_TOOLS` cost table — **measure the per-turn token figure rather than estimating it**, the way the existing rows were measured, and say so.

The paragraph must say what the command is for and what it deliberately does not do:

```markdown
`rmem note` records a fact you already know. One embedding, no completion —
the same bargain `decide` makes, and the reason it exists is that the only
other way in charges a completion model per fact, which is why this store
held 265 decisions, zero facts and an empty review queue.

It resolves by name. If the store cannot tell whether that is someone it
already knows, the fact is recorded and the identity question is queued:
`rmem review` lists it. A wrong merge is silent and permanent; an open
question is neither.

It does not extract. `remember` reads prose and finds facts in it; `note`
receives one someone decided to record. Both are useful and they have
different failure modes.
```

- [ ] **Step 2: Record the decision**

From a script file, never inline — inline is what mis-scoped twelve records:

```bash
cat > /tmp/note-decision.sh <<'SH'
#!/usr/bin/env bash
set -euo pipefail
export RMEM_CONFIG=D:/memory/rmem.toml
rmem decide "A fact you already know needs a door that costs no completion" \
  "rmem note: one embedding, resolves by name, --absent for an asserted absence" \
  --context "the store held 265 entities, all decisions, five attribute names, zero edges and an empty review queue" \
  --because "plan_remember takes a Completer, so every fact cost a completion call and the cheapest way to record something known was to write prose and pay a model to read it back. That, not the absence of a use case, is why rm-resolve, rm-extract and rm-graph never touched real data. decide already made the opposite bargain for a decision; this makes it for a fact, and deliberately does not wake extraction -- harvested facts have a different quality bar from deliberate ones" \
  --scope "*"
SH
doppler run --scope "D:\personal" --project local-tooling --config dev -- bash /tmp/note-decision.sh
```

Read it back with `rmem recall`, not `rmem decision` — the latter prints no scope, so it cannot see the field most likely to be wrong. Entity 255.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "$(cat <<'EOF'
What note is for, and what it deliberately is not

The README gains the command and its bargain, with the per-turn tool cost
measured rather than estimated.

It says plainly that note does not extract. remember reads prose and finds
facts in it; note receives one someone decided to record. Both are useful
and they fail differently, and the store's quality today rests on every
record being deliberate.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
EOF
)"
```

---

## Finishing

After Task 4, use `superpowers:finishing-a-development-branch`. Base branch is `main`.

The pull request reports the workspace test count going 804 → 817 (seven in Task 1, four in Task 2, two in Task 3), and the end-to-end run from Task 2 Step 6 showing an asserted absence and an unknown answering differently.

**It does not report a resolver result, because none has been produced yet.** Running `note` over the first real dataset is the next piece of work and its own argument — the thresholds were calibrated on generated names, and what they do with real ones is a finding rather than a step in this plan.
