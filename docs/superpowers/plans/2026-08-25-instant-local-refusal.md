# Instant-Local Refusal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `Strategy::ValidInterval` refuse only the instants that are genuinely ambiguous, instead of refusing every read because one pair of writes collided somewhere in the history.

**Architecture:** `rm_survivor::Fact` stops carrying a bare `Held` and starts carrying a `Span`, which is either `Held` or `Contested`. `merge` stops returning `Err` for a `ValidInterval` collision and instead builds a timeline that names its holes; `Outcome::held_at` becomes fallible and refuses only when the instant asked about lands in one. The read path (`rm-engine`) indexes in; the write path (`rm-store`) still refuses the whole resolution, because a contested span has no representation in storage.

**Tech Stack:** Rust, pinned toolchain 1.98.0. Workspace crates only — no new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-25-instant-local-refusal-design.md`

## Global Constraints

- **Toolchain is pinned at 1.98.0.** Every task ends green under `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --workspace`.
- **Never commit on a red tree.** Check exit codes directly (`cmd && git commit`), never `cmd | grep -E '^error' && git commit` — `grep` exits 0 when it *finds* errors, which is how a red crate got committed in `73cb4b3`.
- **Baseline is 771 passing, 0 failing** on `main` at `1a86307`. Every task states its expected new total.
- **`rm-core` and `rm-survivor` are both `0.1.0`.** `rm_survivor::Outcome` never crosses the MCP wire (the hosts match on `Believed`; see `crates/rm-mcp/src/render.rs`) and the store persists `Version`s rather than outcomes, so type changes here are compile-time breaks inside the workspace only. No store-format migration, no protocol change.
- **The prose changes before the reference model, and the reference model before the engine.** `rm-conform`'s reference model is evidence only because it is written from the documentation independently of the engine. Written in the other order it is a transcription and the differential sweep is a tautology. Task 2's step order is not a matter of taste.
- **Refusals compare as refusals, never by message.** `crates/rm-conform/src/differential.rs:20` states this; the reference model's refusal strings deliberately do not match the engine's.
- **`Interval` is half-open `[from, to)`**, `to: Option<Timestamp>` where `None` means open-ended. `Interval::since(from)`, `Interval::between(from, to)`, `Interval::contains(t)` — `crates/rm-core/src/lib.rs:60-80`.
- **Commit trailers**, on every commit:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
  ```

---

## File Structure

| File | Responsibility in this change |
|---|---|
| `crates/rm-survivor/src/lib.rs` | Owns the rule. Gains `Span`, `Fact.span`, a fallible `held_at`/`as_of`, a rewritten `timeline`, and the doc comment that the reference model is written from. |
| `crates/rm-store/src/lib.rs` | Write path. Scans for a contested span and refuses the whole resolution before writing anything. |
| `crates/rm-engine/src/read.rs` | Read path. The `?` moves from the `merge` call to the `held_at` call — that relocation is the fix. |
| `crates/rm-conform/src/reference.rs` | The independent model. `valid_interval` and `held_at` rewritten from the changed prose. |
| `crates/rm-conform/src/engine_harness.rs` | Consumes `reference::held_at`, so it absorbs the `Result`. |
| `crates/rm-conform/src/differential.rs` | Gains `instant_agreement` — the row that measures the new property, with its own anti-vacuity guard. |
| `crates/rm-conform/src/report.rs` | Prints the new row. |
| `crates/rm-contrast/src/surface.rs` | Holds the assertion that pins the defect and must invert to pin the fix. |
| `crates/rm-conform/README.md`, `crates/rm-contrast/README.md`, `docs/seed-decision-log.sh` | The record: the new row, the corrected claims, and the re-decision. |

Four tasks. Task 1 is a pure refactor that changes no behaviour; Task 2 is the behaviour change and is atomic (a reviewer cannot accept the engine without the store guard); Task 3 adds the measurement; Task 4 writes the record.

---

### Task 1: The vocabulary

Introduce `Span` and make the accessors fallible **without changing any behaviour**. `timeline` still refuses history-wide at the end of this task, so it never constructs a `Span::Contested`. Every existing test passes unchanged.

This task exists separately because it is a large mechanical diff across five files, and a reviewer can meaningfully approve "the types changed and nothing else did" before being asked about the rule.

**Files:**
- Modify: `crates/rm-survivor/src/lib.rs`
- Modify: `crates/rm-store/src/lib.rs:465-495`
- Modify: `crates/rm-engine/src/read.rs:309-318`
- Modify: `crates/rm-conform/src/reference.rs:39-49`
- Modify: `crates/rm-conform/src/engine_harness.rs:93-97`

**Interfaces:**
- Produces, for Tasks 2–4:
  - `pub enum Span { Held(Held), Contested { values: Vec<Held>, observed_at: Timestamp } }`
  - `pub struct Fact { pub span: Span, pub valid: Interval }` — note the field is `span`, not `value`
  - `Outcome::held_at(&self, t: Timestamp) -> Result<Option<&Held>, Refused>`
  - `Outcome::as_of(&self, t: Timestamp) -> Result<Option<&str>, Refused>`
  - `reference::held_at(outcome: &Outcome, t: Timestamp) -> Result<Option<&Held>, Refused>`

- [ ] **Step 1: Add `Span` and change `Fact`**

In `crates/rm-survivor/src/lib.rs`, immediately above the existing `Fact` struct, add `Span` and replace `Fact`:

```rust
/// What a span of valid time holds.
///
/// Two shapes, because a timeline over contradictory writes has regions where
/// no single value can be said to have stood. Naming those regions is what
/// lets a read refuse the instant it was asked about rather than the whole
/// history: a timeline with an unnamed hole cannot be indexed into, and one
/// whose holes are named can.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Span {
    /// One value stood here.
    Held(Held),
    /// Two or more values opened here sharing an `observed_at`, so nothing
    /// orders them and none of them can be said to have held.
    ///
    /// `observed_at` is carried because it is what a refusal hands back to
    /// whoever has to fix it: the timestamp naming which writes to separate.
    Contested {
        values: Vec<Held>,
        observed_at: Timestamp,
    },
}

/// A span of valid time and what stood over it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub span: Span,
    pub valid: Interval,
}
```

Delete the old `Fact` definition (the one with `pub value: Held`).

- [ ] **Step 2: Make `held_at` and `as_of` fallible**

Replace `Outcome::as_of` and `Outcome::held_at` in the same file. Keep the surrounding doc comments; the bodies and signatures change:

```rust
    /// The value in force at `t`.
    ///
    /// Reports an asserted absence as `None`, the same as no coverage at all.
    /// Use [`Outcome::held_at`] where the difference matters — it does to a
    /// memory store, and this method is the convenience, not the precise answer.
    ///
    /// Fallible along with `held_at` rather than collapsing a contested span
    /// into `None`: `None` here already means *no coverage*, and flattening
    /// "two values and nothing orders them" into it is the same collapse the
    /// `Absent`/`Unknown` distinction exists to prevent.
    pub fn as_of(&self, t: Timestamp) -> Result<Option<&str>, Refused> {
        Ok(self.held_at(t)?.and_then(Held::value))
    }

    /// What held at `t`, distinguishing an asserted absence from no coverage.
    ///
    /// Refuses only when `t` lands in a contested span. A [`Outcome::Survivor`]
    /// never refuses: it has no time dimension — that is what
    /// [`Strategy::keeps_a_timeline`] reports — so it is `Ok` at every instant,
    /// and the `Result` is a shape the timeline arm needs rather than a
    /// behaviour every strategy acquires.
    pub fn held_at(&self, t: Timestamp) -> Result<Option<&Held>, Refused> {
        match self {
            Outcome::Survivor(v) => Ok(v.as_ref()),
            Outcome::Timeline(facts) => match facts.iter().find(|f| f.valid.contains(t)) {
                None => Ok(None),
                Some(Fact {
                    span: Span::Held(v),
                    ..
                }) => Ok(Some(v)),
                Some(Fact {
                    span:
                        Span::Contested {
                            values,
                            observed_at,
                        },
                    valid,
                }) => Err(Refused(contested(values, *observed_at, t, valid))),
            },
        }
    }
```

- [ ] **Step 3: Add the refusal message builder**

Add near the other free functions in `crates/rm-survivor/src/lib.rs`, beside `held`:

```rust
/// The refusal for a question that landed in a contested span.
///
/// Names the interval so the answer is actionable rather than a dead end: a
/// caller learns both that this instant is contested and where the history
/// resumes being answerable. That is the whole difference between a refusal
/// that fits the question and one that does not.
fn contested(values: &[Held], observed_at: Timestamp, t: Timestamp, valid: &Interval) -> String {
    let named: Vec<String> = values
        .iter()
        .map(|v| match v {
            Held::Value(s) => format!("{s:?}"),
            Held::Absent => "an asserted absence".to_string(),
        })
        .collect();
    let resumes = match valid.to {
        Some(to) => format!("outside [{}, {to})", valid.from),
        None => format!("before {}", valid.from),
    };
    format!(
        "{} opened at {} and were all observed at {observed_at}, so none supersedes \
         the others and none can be said to have held at {t}. Distinguish their \
         observation times, or ask about an instant {resumes}.",
        named.join(" and "),
        valid.from
    )
}
```

- [ ] **Step 4: Fix `survivor()` and `timeline()` for the new field name**

In `Outcome::survivor`, replace the `Timeline` arm:

```rust
            Outcome::Timeline(facts) => match facts.as_slice() {
                [Fact {
                    span: Span::Held(v),
                    ..
                }] => v.value(),
                _ => None,
            },
```

In `timeline()`, the two places that build and compare facts change field name only — the refusal stays exactly where it is:

```rust
    let mut facts: Vec<Fact> = Vec::new();
    for c in &asserted {
        let value = held(c.value);
        if facts
            .last()
            .is_some_and(|f| f.span == Span::Held(value.clone()))
        {
            continue; // same value restated: extends the open span, no new fact
        }
        facts.push(Fact {
            span: Span::Held(value),
            valid: Interval::since(c.valid.from),
        });
    }
```

- [ ] **Step 5: Fix the four call sites outside `rm-survivor`**

`crates/rm-engine/src/read.rs` — the `held_at` call gains a `?`. This is the target state; it changes nothing yet because `merge` still refuses first:

```rust
        let outcome = merge(&candidates, policy.for_attribute(attribute))?;
        Ok(match outcome.held_at(valid_t)? {
            Some(Held::Value(v)) => Believed::Value(v.clone()),
            Some(Held::Absent) => Believed::Absent,
            None => Believed::Unknown,
        })
```

`crates/rm-store/src/lib.rs`, the `Outcome::Timeline` arm — field rename only for now; the guard arrives in Task 2:

```rust
            Outcome::Timeline(facts) => {
                for fact in facts {
                    let Span::Held(value) = fact.span else {
                        unreachable!("merge still refuses a collision whole")
                    };
                    self.assert(
                        id,
                        attribute.clone(),
                        held_to_value(value),
                        fact.valid,
                        latest.clone(),
                        Supersession::Corrects,
                    )?;
                }
                Ok(())
            }
```

Add `Span` to that file's `use rm_survivor::{...}` list.

`crates/rm-conform/src/reference.rs` — `held_at` becomes fallible, and the timeline builder uses the new field. Replace `held_at`:

```rust
pub fn held_at(outcome: &Outcome, t: rm_core::Timestamp) -> Result<Option<&Held>, Refused> {
    match outcome {
        Outcome::Survivor(v) => Ok(v.as_ref()),
        // Half-open `[from, to)`, per `Interval`'s own docs.
        Outcome::Timeline(facts) => match facts
            .iter()
            .find(|f| f.valid.from <= t && f.valid.to.is_none_or(|to| t < to))
        {
            None => Ok(None),
            Some(f) => match &f.span {
                Span::Held(v) => Ok(Some(v)),
                Span::Contested { .. } => Err(Refused(
                    "nothing orders the values that opened here".to_string(),
                )),
            },
        },
    }
}
```

and in `valid_interval`, the fact-building loop:

```rust
    let mut facts: Vec<Fact> = Vec::new();
    for c in ordered {
        let value = held(c);
        if facts.last().map(|f| &f.span) == Some(&Span::Held(value.clone())) {
            continue;
        }
        if let Some(prev) = facts.last_mut() {
            prev.valid = Interval::between(prev.valid.from, c.valid.from);
        }
        facts.push(Fact {
            span: Span::Held(value),
            valid: Interval::since(c.valid.from),
        });
    }
```

Add `Span` to that file's `use rm_survivor::{...}` list.

`crates/rm-conform/src/engine_harness.rs:93` — absorb the `Result`. The reference refusing here would be a disagreement the outer match already ruled out, so it is an `expect` rather than a silent branch:

```rust
    let expected = match crate::reference::held_at(&outcome, valid_t)
        .expect("MostRecent yields a Survivor, which never refuses at an instant")
    {
        Some(rm_survivor::Held::Value(v)) => Believed::Value(v.clone()),
        Some(rm_survivor::Held::Absent) => Believed::Absent,
        None => Believed::Unknown,
    };
```

- [ ] **Step 6: Fix `rm-survivor`'s and `rm-conform`'s own tests for the field rename**

Both crates' test modules construct and assert on `Fact { value, valid }`. Each becomes `Fact { span: Span::Held(value), valid }`. Find them:

```bash
rg -n 'Fact \{' crates/rm-survivor/src/lib.rs crates/rm-conform/src/reference.rs
```

Change assertions mechanically. **Do not change what any test asserts** — this task changes no behaviour, so a test that needs its expectation altered means something has gone wrong.

- [ ] **Step 7: Verify the whole tree is green and unchanged**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace -q 2>&1 | rg 'test result' | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p" failed="f}'
```

Expected: `passed=771 failed=0`. **The count must be exactly 771** — this task adds no tests and removes none. A different number means a test was lost in the mechanical edit.

- [ ] **Step 8: Commit**

```bash
git add -A crates/ && git commit -m "$(cat <<'EOF'
A timeline that can name its holes

Fact carries a Span rather than a bare Held, and held_at/as_of return a
Result. Nothing constructs Span::Contested yet and timeline() still refuses
a collision whole, so this changes no behaviour: 771 tests, the same 771.

The Result is what lets the next change put the refusal at the instant
instead of at the read. rm-engine's held_at call takes the `?` here, where
it is inert, so the diff that changes behaviour is the rule and not the
plumbing.

as_of is fallible along with held_at rather than collapsing a contested
span into None. None there already means no coverage, and flattening "two
values and nothing orders them" into it is the collapse the Absent/Unknown
distinction exists to prevent.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
EOF
)"
```

---

### Task 2: The rule

The behaviour change, and it is atomic: the doc comment, the reference model, the engine, the store guard and the `rm-contrast` assertion move together. A reviewer cannot accept the engine without the store guard — `merge` no longer refusing means the write path would silently materialize a contested span if it were not stopped in the same commit.

**Files:**
- Modify: `crates/rm-survivor/src/lib.rs` — the `ValidInterval` doc comment and `timeline()`
- Modify: `crates/rm-conform/src/reference.rs:126-170` — `valid_interval`
- Modify: `crates/rm-store/src/lib.rs` — the write-path guard
- Modify: `crates/rm-contrast/src/surface.rs:198-203` — the inverted assertion

**Interfaces:**
- Consumes from Task 1: `Span`, `Fact { span, valid }`, `Outcome::held_at -> Result<Option<&Held>, Refused>`, `reference::held_at -> Result<Option<&Held>, Refused>`.
- Produces for Task 3: `merge(.., &Strategy::ValidInterval)` returns `Ok` for a colliding history; the refusal is reachable only through `held_at`.

- [ ] **Step 1: State the rule in prose, first**

In `crates/rm-survivor/src/lib.rs`, replace the `ValidInterval` variant's doc comment — everything from `/// Do not pick a winner.` down to the line before `ValidInterval,`. The section headed *"# The refusal is history-wide, not instant-local"* goes away with it.

```rust
    /// Do not pick a winner. Emit each distinct value with the validity range
    /// over which it stood.
    ///
    /// # What opens a span
    ///
    /// Sort the asserting candidates by `(valid.from, observed_at)`. Each
    /// distinct `valid.from` opens a span, closing where the next one opens;
    /// the last is open-ended. What opens there is decided by the **greatest
    /// `observed_at` heard for that moment** — anything said earlier about the
    /// same moment was superseded before any question could be asked.
    ///
    /// A restatement of the value already standing extends it rather than
    /// opening a second span: re-hearing a fact is not a change. A value that
    /// returns after being superseded yields three spans, because those spans
    /// are not adjacent.
    ///
    /// # The refusal is instant-local
    ///
    /// A span is *contested* when the greatest-`observed_at` group at its
    /// `valid.from` holds two or more distinct values: they share both clocks,
    /// so nothing orders them and none of them can be said to have held. The
    /// timeline still gets built, with that span named as [`Span::Contested`],
    /// and [`Outcome::held_at`] refuses only for an instant that lands inside
    /// one. Every other instant answers.
    ///
    /// A tombstone competes as a value, so [`Held::Absent`] colliding with a
    /// value contests the span. Silence never competes and never contests.
    ///
    /// Note what this excludes. Given `A@(F,1)`, `B@(F,1)`, `C@(F,2)`, the
    /// greatest `observed_at` at `F` is 2 and `C` stands alone: **no instant is
    /// ambiguous**, and the whole history answers. An earlier rule refused it,
    /// which was not merely a refusal that was too wide but one that fired on a
    /// history containing nothing ambiguous at all.
    ///
    /// # The write path still refuses whole
    ///
    /// `rm_store` materializes a merge result into stored versions, and
    /// `Version.value` is an `Option<String>` with no representation for a
    /// contested span. So a resolution containing one is refused entirely. One
    /// rule, two policies about what to do with a hole: a question can be asked
    /// about a single instant, and a materialized resolution cannot.
    ///
    /// # This has been wrong twice
    ///
    /// It once said "refuses when two different values share an observation
    /// timestamp", which was true when a `Candidate` carried no interval and
    /// the timeline could only be cut at observation. `rm-conform`'s
    /// differential sweep found the gap by disagreeing on 53 generated
    /// histories. It then said the refusal was history-wide, which was true of
    /// the code and disagreed with the oracle `rm-contrast` grades against —
    /// 4,067 of 6,353 answerable questions refused at a 25% tie rate. The
    /// second correction is this one.
    ValidInterval,
```

- [ ] **Step 2: Rewrite the reference model from that prose — and only from it**

In `crates/rm-conform/src/reference.rs`, replace `valid_interval`'s body and doc comment. **Read the doc comment written in Step 1 and implement what it says.** Do not open `rm-survivor`'s `timeline()`; the two must be written apart or the sweep in Step 4 proves nothing.

```rust
/// Do not pick a winner: emit each distinct value over the span it stood.
///
/// Each distinct `valid.from` opens a span. What opens there is the value
/// asserted at the greatest `observed_at` for that moment; where two or more
/// distinct values share that greatest `observed_at`, nothing orders them and
/// the span is contested. A restatement of the standing value extends it.
///
/// Written from `Strategy::ValidInterval`'s documentation rather than from
/// `rm_survivor::timeline`, which is the only reason a green sweep is evidence
/// of anything.
fn valid_interval(candidates: &[Candidate<'_>]) -> Result<Outcome, Refused> {
    let claims = claims(candidates);
    if claims.is_empty() {
        return Ok(Outcome::Timeline(vec![]));
    }

    let mut ordered: Vec<&&Candidate<'_>> = claims.iter().collect();
    ordered.sort_by(|a, b| {
        (a.valid.from, a.provenance.observed_at).cmp(&(b.valid.from, b.provenance.observed_at))
    });

    let mut facts: Vec<Fact> = Vec::new();
    let mut moments: Vec<Timestamp> = ordered.iter().map(|c| c.valid.from).collect();
    moments.dedup();

    for from in moments {
        let at_moment: Vec<&&&Candidate<'_>> =
            ordered.iter().filter(|c| c.valid.from == from).collect();
        let latest = at_moment
            .iter()
            .map(|c| c.provenance.observed_at)
            .max()
            .expect("a moment exists because a candidate opened it");
        let mut values: Vec<Held> = Vec::new();
        for c in at_moment
            .iter()
            .filter(|c| c.provenance.observed_at == latest)
        {
            let v = held(c);
            if !values.contains(&v) {
                values.push(v);
            }
        }

        let span = if values.len() == 1 {
            Span::Held(values.remove(0))
        } else {
            Span::Contested {
                values,
                observed_at: latest,
            }
        };

        // A restatement of the value already standing extends it.
        if facts.last().map(|f| &f.span) == Some(&span) && matches!(span, Span::Held(_)) {
            continue;
        }
        if let Some(prev) = facts.last_mut() {
            prev.valid = Interval::between(prev.valid.from, from);
        }
        facts.push(Fact {
            span,
            valid: Interval::since(from),
        });
    }

    Ok(Outcome::Timeline(facts))
}
```

Add `Timestamp` to that file's `rm_core` import if it is not already there.

- [ ] **Step 3: Commit the reference model and the prose, and confirm the tree is still green**

`rm-survivor`'s engine has not changed yet, so `differential::agrees` should now find disagreements — but the *suite* must still be green to commit. Check which:

```bash
cargo test -p rm-conform -q 2>&1 | rg 'test result|FAILED'
```

If `rm-conform`'s sweep test fails here, **do not commit**. Fold Steps 2–7 into one commit instead and record the disagreement from Step 4 in its message. If it passes (the sweep's default params may not generate a collision that separates the two rules), commit the prose and the reference model on their own.

Chain on real exit codes — `cargo test` returns non-zero on failure, so `&&` is the guard. Never pipe into `rg` and test *that*: `rg` exits 0 when it **finds** the word FAILED, which is how a red crate got committed in `73cb4b3`.

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace -q && git add -A crates/ && git commit -m "$(cat <<'EOF'
The rule, stated and modelled before it is built

ValidInterval's doc comment now states the instant-local rule: each
distinct valid.from opens a span decided by the greatest observed_at heard
for that moment, and a span is contested when that group holds two or more
distinct values. rm-conform's reference model is rewritten from that prose.

The engine has not moved yet, deliberately. The reference model is evidence
only because it is written from the documentation independently, so the
order is prose, then model, then engine -- written the other way round the
model is a transcription and the sweep is a tautology.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
EOF
)"
```

- [ ] **Step 4: Demonstrate the disagreement before fixing it**

This is the failing-test step, and the failing test is the differential itself. Write a temporary probe at the end of `crates/rm-conform/src/differential.rs`'s test module:

```rust
    /// Scaffolding. Deleted in the same commit that makes it pass.
    #[test]
    fn the_engine_and_the_reference_disagree_about_instant_local() {
        let params = crate::generate::Params {
            len: 10,
            alphabet: 3,
            tie_pct: 60,
            backdate_pct: 10,
            ..Default::default()
        };
        let mut disagreed = 0;
        for seed in 0..300 {
            let history = crate::generate::generate(seed, &params);
            if !agrees(&history, &Strategy::ValidInterval) {
                disagreed += 1;
            }
        }
        panic!("{disagreed} of 300 histories disagree");
    }
```

Run it and **record the number** — it goes in the commit message:

```bash
cargo test -p rm-conform the_engine_and_the_reference_disagree_about_instant_local -- --nocapture 2>&1 | rg 'histories disagree'
```

Expected: a non-zero count. **If it is zero, stop.** Either the generator produces no collisions at these parameters (raise `tie_pct`, lower `backdate_pct`, and retry) or the reference model was written from the engine rather than the prose. A zero here means Step 6's green result would prove nothing.

- [ ] **Step 4b: Write the unit tests the differential cannot replace**

The sweep proves the two implementations agree. It does not prove they agree about the *right* thing — two implementations of the same misreading agree perfectly. These pin the three cases the spec names by hand, and every one of them fails against the current engine.

Add to `crates/rm-survivor/src/lib.rs`'s test module:

```rust
    /// The property this change exists for: one timeline, one instant
    /// refused, another answered.
    #[test]
    fn a_collision_refuses_its_own_span_and_nothing_else() {
        // "Acme" and "Globex" both open at 10, both heard at 100.
        // "Initech" opens at 20 and settles it from there on.
        let provs = [prov(Source::UserAssertion, 100), prov(Source::UserAssertion, 100), prov(Source::UserAssertion, 200)];
        let cs = vec![
            Candidate::new(Some("Acme"), &provs[0]).over(Interval::since(10)),
            Candidate::new(Some("Globex"), &provs[1]).over(Interval::since(10)),
            Candidate::new(Some("Initech"), &provs[2]).over(Interval::since(20)),
        ];
        let out = merge(&cs, &Strategy::ValidInterval).expect("the merge answers now");

        assert!(out.held_at(5).unwrap().is_none(), "nothing had opened yet");
        assert!(out.held_at(10).is_err(), "the contested span must refuse");
        assert!(out.held_at(19).is_err(), "still inside it");
        assert_eq!(
            out.held_at(20).unwrap(),
            Some(&Held::Value("Initech".into())),
            "past the collision, and answerable -- this is the whole change"
        );

        let message = out.held_at(10).unwrap_err().to_string();
        assert!(message.contains("[10, 20)"), "the refusal must name where the history resumes: {message}");
        assert!(message.contains("100"), "and the observation time to separate: {message}");
    }

    /// A later hearing about the same moment settles what an earlier
    /// disagreement about it could not. No instant here is ambiguous, and the
    /// history-wide rule refused the whole read anyway.
    #[test]
    fn a_later_observation_about_the_same_moment_settles_it() {
        let provs = [prov(Source::UserAssertion, 100), prov(Source::UserAssertion, 100), prov(Source::UserAssertion, 200)];
        let cs = vec![
            Candidate::new(Some("Acme"), &provs[0]).over(Interval::since(10)),
            Candidate::new(Some("Globex"), &provs[1]).over(Interval::since(10)),
            Candidate::new(Some("Initech"), &provs[2]).over(Interval::since(10)),
        ];
        let out = merge(&cs, &Strategy::ValidInterval).expect("nothing is ambiguous here");
        assert_eq!(
            out.held_at(10).unwrap(),
            Some(&Held::Value("Initech".into())),
            "the greatest observed_at at a moment decides it"
        );
        assert_eq!(
            out.held_at(9_999).unwrap(),
            Some(&Held::Value("Initech".into()))
        );
    }

    /// A tombstone is a claim and competes as one, so it can contest a span.
    #[test]
    fn an_absence_can_contest_a_span() {
        let provs = [prov(Source::UserAssertion, 100), prov(Source::UserAssertion, 100)];
        let cs = vec![
            Candidate::new(Some("Acme"), &provs[0]).over(Interval::since(10)),
            Candidate::absent(&provs[1]).over(Interval::since(10)),
        ];
        let out = merge(&cs, &Strategy::ValidInterval).expect("the merge answers");
        let message = out.held_at(10).unwrap_err().to_string();
        assert!(
            message.contains("an asserted absence"),
            "a tombstone must be named as a claim, not omitted: {message}"
        );
    }
```

Add to `crates/rm-store/src/lib.rs`'s test module — the write path's asymmetry needs its own pin, because Task 2 Step 6 is the only thing keeping it:

```rust
    /// A resolution containing a contested span refuses whole, and leaves
    /// nothing behind. Storage has no way to record two values that nothing
    /// orders, so unlike a read this cannot be asked about one instant.
    #[test]
    fn a_resolution_with_a_contested_span_writes_nothing_at_all() {
        let mut s = Store::new();
        let id = s.mint_entity();
        let provs = [prov(100), prov(100)];
        let cs = vec![
            Candidate::new(Some("Acme"), &provs[0]).over(Interval::since(10)),
            Candidate::new(Some("Globex"), &provs[1]).over(Interval::since(10)),
        ];
        let refused = s.resolve_into(id, "employer", &cs, &Strategy::ValidInterval);
        assert!(refused.is_err(), "a contested resolution must not be written");
        assert!(
            s.history(id, "employer").is_empty(),
            "a refused resolution left a half-written timeline behind"
        );
    }
```

**Check the helper names before writing this one.** `rm-store`'s test module has its own `prov` and its own way of minting an entity, and the resolution method's real name is whatever wraps `merge` at `crates/rm-store/src/lib.rs:440`. Read them:

```bash
rg -n 'fn prov|fn mint_entity|fn resolve' crates/rm-store/src/lib.rs
```

Adjust the test to the names actually there rather than the ones written above.

- [ ] **Step 4c: Run them and watch them fail**

```bash
cargo test -p rm-survivor a_collision_refuses_its_own_span 2>&1 | rg -i 'panicked|test result'
```

Expected: FAIL. Against the current engine `merge` returns `Err`, so `.expect("the merge answers now")` panics. That panic is the defect, stated as a test.

- [ ] **Step 5: Rewrite the engine's `timeline()`**

In `crates/rm-survivor/src/lib.rs`, replace `timeline` entirely — note it no longer returns a `Result`:

```rust
/// Build a timeline of values, naming the spans nothing orders.
///
/// One span per distinct `valid.from`, decided by the greatest `observed_at`
/// heard for that moment; contested when that group holds more than one
/// distinct value. See [`Strategy::ValidInterval`] for the rule and why it is
/// instant-local.
fn timeline(candidates: &[Candidate<'_>]) -> Vec<Fact> {
    let mut asserted: Vec<&Candidate<'_>> = candidates
        .iter()
        .filter(|c| c.value.is_assertion())
        .collect();
    if asserted.is_empty() {
        return Vec::new();
    }

    // By when each held, with the observation breaking ties. Valid time is the
    // axis this strategy is named for; observation is what orders two things
    // said to have begun at the same moment, and is a total order because the
    // store stamps every write.
    asserted.sort_by_key(|c| (c.valid.from, c.provenance.observed_at));

    let mut facts: Vec<Fact> = Vec::new();
    let mut i = 0;
    while i < asserted.len() {
        let from = asserted[i].valid.from;
        let mut j = i;
        while j < asserted.len() && asserted[j].valid.from == from {
            j += 1;
        }
        // Sorted by `observed_at` within the moment, so the last one's is the
        // greatest. Everything heard earlier about this moment was superseded
        // before any question could be asked about it.
        let group = &asserted[i..j];
        let observed_at = group[group.len() - 1].provenance.observed_at;
        let mut values: Vec<Held> = Vec::new();
        for c in group
            .iter()
            .filter(|c| c.provenance.observed_at == observed_at)
        {
            let v = held(c.value);
            if !values.contains(&v) {
                values.push(v);
            }
        }
        let span = if values.len() == 1 {
            Span::Held(values.remove(0))
        } else {
            Span::Contested {
                values,
                observed_at,
            }
        };
        i = j;

        // A restatement of the value already standing extends it rather than
        // opening a second span. Contested spans never coalesce: each records
        // the `observed_at` its own collision happened at, which is what the
        // refusal hands back, and two collisions are two things to fix.
        let restatement = match (&span, facts.last()) {
            (
                Span::Held(opening),
                Some(Fact {
                    span: Span::Held(standing),
                    ..
                }),
            ) => opening == standing,
            _ => false,
        };
        if restatement {
            continue;
        }
        facts.push(Fact {
            span,
            valid: Interval::since(from),
        });
    }

    // Close each span where the next one opens, leaving the last open-ended.
    for i in 0..facts.len().saturating_sub(1) {
        facts[i].valid.to = Some(facts[i + 1].valid.from);
    }
    facts
}
```

Update `merge`'s early-out, which no longer has an error to propagate:

```rust
    if matches!(strategy, Strategy::ValidInterval) {
        return Ok(Outcome::Timeline(timeline(candidates)));
    }
```

- [ ] **Step 6: Add the write-path guard**

In `crates/rm-store/src/lib.rs`, replace the `Outcome::Timeline` arm written in Task 1. The scan runs **before** any write, so a refused resolution leaves nothing behind:

```rust
            Outcome::Timeline(facts) => {
                // A contested span has no representation here: `Version.value`
                // is an `Option<String>`, and there is nothing to write for
                // "two values and nothing orders them". A read can be asked
                // about one instant; a materialised resolution cannot, so this
                // refuses whole -- and scans before writing, so a refusal
                // leaves no half-written timeline behind.
                if let Some(fact) = facts
                    .iter()
                    .find(|f| matches!(f.span, Span::Contested { .. }))
                {
                    return Err(Refused(format!(
                        "the span opening at {} is contested, and a resolution written into \
                         storage has no way to record two values that nothing orders. Read it \
                         with `about` at an instant outside that span, or distinguish the \
                         observation times and resolve again.",
                        fact.valid.from
                    ))
                    .into());
                }
                for fact in facts {
                    let Span::Held(value) = fact.span else {
                        unreachable!("contested spans were refused above")
                    };
                    self.assert(
                        id,
                        attribute.clone(),
                        held_to_value(value),
                        fact.valid,
                        latest.clone(),
                        Supersession::Corrects,
                    )?;
                }
                Ok(())
            }
```

- [ ] **Step 7: Invert the assertion that pins the defect**

In `crates/rm-contrast/src/surface.rs`, in `questions_with_no_right_answer_occur_and_only_one_store_declines`, replace the final `assert!(store.declined > 0, ...)`:

```rust
        assert_eq!(
            store.declined, 0,
            "the store refused a question that had an answer. Its refusals are \
             meant to be exactly the instants the oracle in workload.rs calls \
             ambiguous, so a residue here is the two rules disagreeing -- a \
             finding to chase, not a threshold to relax"
        );
```

Also update `unanswerable`'s doc comment in the same file, which currently explains the history-wide refusal as the reason `declined` is interesting:

```rust
/// What each store does with a question that has no right answer.
///
/// Measured apart from the surface because it is a different phenomenon and
/// mixing it in would confound the temporal axes with the refusal behaviour.
///
/// Returns `(store, flat)`. The figures that matter are `ungradeable` -- the
/// questions that genuinely had no answer, which both stores meet equally --
/// and the store's `declined`, which is questions it refused that *did* have
/// an answer. That number was 4,067 when `ValidInterval` refused a whole read
/// over one collision. It is 0 now, and the test below pins it there.
```

- [ ] **Step 8: Delete the scaffolding test and run everything**

Remove `the_engine_and_the_reference_disagree_about_instant_local` from `crates/rm-conform/src/differential.rs`.

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace -q 2>&1 | rg 'test result' | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p" failed="f}'
```

Expected: `passed=775 failed=0` — the four tests from Step 4b, and the scaffolding probe from Step 4 deleted.

**If `rm-conform`'s `the_refusal_paths_are_actually_reached` now fails**, that is expected rather than a bug: `ValidInterval` no longer refuses at `merge`, so the measured refusal proportion across 8 strategies drops. Re-measure and update the floor and the comment naming the old figure (`crates/rm-conform/src/differential.rs:203`, "The measured figure is 450 refused / 1,950 answered") with the new one. Do not lower the floor below what the other strategies actually produce — read the number, then write it.

**If `rm-store` tests fail**, read them before changing them: a store test asserting that a colliding resolution refuses is still correct and should still pass.

- [ ] **Step 9: Record the measurement**

```bash
cargo run --release -p rm-contrast -- --report --full 2>&1 | rg -i 'declin|ungradeable|refus'
```

Record the `declined` and `ungradeable` figures. They go in the commit message and, in Task 4, in the README.

- [ ] **Step 10: Commit**

```bash
git add -A crates/ && git commit -m "$(cat <<'EOF'
A refusal that fits the question

ValidInterval refused the whole read when one pair of writes collided
anywhere in the visible history, including for instants nowhere near it.
It now builds the timeline either way, names the contested spans, and
refuses only an instant that lands inside one.

The rule was already in this repo as ground truth: rm-contrast's oracle
finds the winner for the instant asked about and asks whether anything
shares both of its clocks and disagrees. The engine was disagreeing with
the oracle it is graded against, and rm-contrast counted the disagreement
at 4,067 of 6,353 answerable questions declined. That figure is now 0, and
the assertion in surface.rs that used to pin the defect -- store.declined
> 0, with a comment saying that if it stopped happening the measurement had
gone quiet -- is inverted to pin the fix instead.

Narrower than a refusal over a smaller region. Given A@(F,1), B@(F,1),
C@(F,2) the greatest observed_at at F is 2 and C stands alone, so no
instant is ambiguous and the old rule refused it anyway.

The write path still refuses whole: Version.value is an Option<String> and
a contested span has no representation in storage. It scans before writing,
so a refusal leaves no half-written timeline.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
EOF
)"
```

---

### Task 3: The row that measures it

`rm-conform`'s existing `refusal_agreement` compares refusals at `merge`. `ValidInterval`'s refusal no longer lives there, so the property has to be measured where it now happens: at an instant. This adds the row and the anti-vacuity guard the spec requires.

**Files:**
- Modify: `crates/rm-conform/src/differential.rs`
- Modify: `crates/rm-conform/src/report.rs:60-145`

**Interfaces:**
- Consumes from Task 2: `merge(.., &Strategy::ValidInterval)` returning `Ok` over colliding histories; `Outcome::held_at` and `reference::held_at` both `Result<Option<&Held>, Refused>`.
- Produces: `pub struct InstantScore { agreed, disagreed, both_refused, both_answered, mixed_histories }`, `pub fn instant_agreement(seeds: impl Iterator<Item = u64>) -> InstantScore`, `InstantScore::exact(&self) -> bool`.

- [ ] **Step 1: Write the failing test**

Add to `crates/rm-conform/src/differential.rs`'s test module:

```rust
    /// The engine and the reference agree about which *instants* refuse, not
    /// merely about which histories do.
    #[test]
    fn instant_refusals_line_up_exactly() {
        let score = instant_agreement(0..300);
        assert_eq!(score.disagreed, 0, "{score:?}");
        assert!(score.agreed > 0, "nothing was compared: {score:?}");
    }

    /// The companion, and the one that stops this from being vacuous. A suite
    /// where every probe refused, or none did, would report perfect agreement
    /// having measured nothing. The demanding form is per *history*: contested
    /// and answerable instants have to occur in the same timeline, which is
    /// the whole property -- a refusal that fits the question rather than the
    /// history.
    #[test]
    fn contested_and_answerable_instants_occur_in_the_same_history() {
        let score = instant_agreement(0..300);
        assert!(
            score.both_refused > 0,
            "no probe ever landed in a contested span: {score:?}"
        );
        assert!(
            score.both_answered > 0,
            "every probe refused: {score:?}"
        );
        assert!(
            score.mixed_histories > 0,
            "no single history both refused an instant and answered another, \
             so instant-local was never actually exercised: {score:?}"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p rm-conform instant_refusals_line_up_exactly 2>&1 | rg -i 'error|cannot find'
```

Expected: FAIL to compile, `cannot find function 'instant_agreement' in this scope`.

- [ ] **Step 3: Implement `InstantScore` and `instant_agreement`**

Add to `crates/rm-conform/src/differential.rs`, beside `refusal_agreement`:

```rust
/// How the two implementations line up on *instants* rather than histories.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct InstantScore {
    pub agreed: usize,
    pub disagreed: usize,
    /// Probes both refused: the instant was genuinely contested.
    pub both_refused: usize,
    /// Probes both answered.
    pub both_answered: usize,
    /// Histories where some probe refused and another answered. The
    /// anti-vacuity figure: instant-local means nothing unless one timeline
    /// does both.
    pub mixed_histories: usize,
}

impl InstantScore {
    pub fn exact(&self) -> bool {
        self.disagreed == 0 && self.agreed > 0
    }
}

/// Whether the engine and the reference refuse the same *instants*.
///
/// `ValidInterval`'s refusal moved from the merge to the read, so
/// [`refusal_agreement`] no longer reaches it: `merge` answers over a
/// colliding history now, and only `held_at` refuses. This probes each
/// history at every moment a span could open, plus one on either side of it,
/// which guarantees landing both inside contested spans and outside them.
///
/// Ties are turned up and backdating down for the same reason
/// [`refusal_agreement`] does it: a collision needs `valid.from` and
/// `observed_at` to coincide, and a backdated assertion rarely collides on the
/// first.
pub fn instant_agreement(seeds: impl Iterator<Item = u64>) -> InstantScore {
    let params = Params {
        len: 10,
        alphabet: 3,
        tie_pct: 60,
        backdate_pct: 10,
        ..Params::default()
    };
    let mut score = InstantScore::default();
    for seed in seeds {
        let history = generate(seed, &params);
        let candidates: Vec<_> = history.iter().map(|a| a.candidate()).collect();

        let (Ok(engine_out), Ok(reference_out)) = (
            engine_merge(&candidates, &Strategy::ValidInterval),
            reference::merge(&candidates, &Strategy::ValidInterval),
        ) else {
            // Neither should refuse at the merge any more. If one does, that
            // is itself a disagreement with the rule and is counted as one.
            score.disagreed += 1;
            continue;
        };

        let mut probes: Vec<rm_core::Timestamp> = Vec::new();
        for c in &candidates {
            probes.push(c.valid.from.saturating_sub(1));
            probes.push(c.valid.from);
            probes.push(c.valid.from.saturating_add(1));
        }
        probes.sort_unstable();
        probes.dedup();

        let (mut refused_here, mut answered_here) = (false, false);
        for t in probes {
            let e = engine_out.held_at(t);
            let r = reference::held_at(&reference_out, t);
            match (e, r) {
                // Refusals compare as refusals and never by message.
                (Err(_), Err(_)) => {
                    score.agreed += 1;
                    score.both_refused += 1;
                    refused_here = true;
                }
                (Ok(a), Ok(b)) if a == b => {
                    score.agreed += 1;
                    score.both_answered += 1;
                    answered_here = true;
                }
                _ => score.disagreed += 1,
            }
        }
        if refused_here && answered_here {
            score.mixed_histories += 1;
        }
    }
    score
}
```

Add whatever imports the file needs — it already has `generate`, `Params`, `Strategy`, `engine_merge` and `reference` for `refusal_agreement`.

- [ ] **Step 4: Run to verify both tests pass**

```bash
cargo test -p rm-conform instant_refusals_line_up_exactly contested_and_answerable 2>&1 | rg 'test result'
```

Expected: PASS.

**If `disagreed` is non-zero**, do not adjust the test. Print the first disagreeing seed and instant, shrink the history with the existing `shrink` helper, and read which of the two rules is wrong. This is the sweep doing its job — it is how the stale `ValidInterval` sentence was found.

**If `mixed_histories` is 0**, the generator is not producing a collision alongside an answerable instant. Raise `tie_pct` toward 80 and re-run; do not weaken the assertion.

- [ ] **Step 5: Add the row to the report**

In `crates/rm-conform/src/report.rs`, after the `recall applicability` row:

```rust
    out.push_str(&format!(
        "| instant-local refusal agreement | {} |\n",
        verdict(crate::differential::instant_agreement(0..300).exact())
    ));
```

And add `"instant-local refusal agreement"` to the row list in `the_table_reports_every_row_and_no_failures`.

- [ ] **Step 6: Run the whole suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace -q 2>&1 | rg 'test result' | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p" failed="f}'
```

Expected: `passed=777 failed=0` — the two tests from Step 1.

- [ ] **Step 7: Record the figures for Task 4**

The assertion messages carry `{score:?}` but only print on failure, so read the figures with a throwaway probe rather than guessing. Add this test, run it, copy the numbers out of the panic, then delete it in the same edit:

```rust
    #[test]
    fn scaffolding_print_the_instant_score() {
        panic!("{:?}", instant_agreement(0..300));
    }
```

```bash
cargo test -p rm-conform scaffolding_print_the_instant_score -- --nocapture 2>&1 | rg 'InstantScore'
```

Record `both_refused`, `both_answered` and `mixed_histories` — Task 4 Step 2 quotes all three. Delete the test before moving on; `cargo test -p rm-conform` must be green at the end of this step.

- [ ] **Step 8: Commit**

```bash
git add -A crates/ && git commit -m "$(cat <<'EOF'
A row for the refusals that moved

ValidInterval's refusal is at the read now, so refusal_agreement no longer
reaches it -- merge answers over a colliding history and only held_at
refuses. instant_agreement probes each generated history at every moment a
span could open, plus one either side, and compares the engine's held_at
against the reference model's.

The anti-vacuity guard is per history rather than per probe: contested and
answerable instants have to occur in the same timeline. A suite where every
probe refused, or none did, would report perfect agreement having measured
nothing, and instant-local means nothing unless one timeline does both.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
EOF
)"
```

---

### Task 4: The record

Three documents now make claims that are no longer true, and one decision is recorded as rejected that this branch reverses.

**Files:**
- Modify: `crates/rm-conform/README.md:43-53` (the table) and `:150-160` (the stale sentence)
- Modify: `crates/rm-contrast/README.md:96-110` ("What cuts against this store")
- Modify: `docs/seed-decision-log.sh`

**Interfaces:**
- Consumes: the figures recorded in Task 2 Step 9 and Task 3 Step 7.

- [ ] **Step 1: Fix `rm-conform`'s stale claim, which predates this branch**

`crates/rm-conform/README.md` ends its "What this deliberately does not do" section with:

> **`rmem about --valid-at` is still inert under `most_recent`.**

That has been false since #45 (`0a54c24`), which made `about` refuse and name the strategy. Replace the bolded sentence:

```markdown
  **`rmem about --valid-at` refuses under `most_recent`** rather than
  answering about the wrong moment — it names the strategy and the
  `rmem.toml` line that would keep a timeline. This sentence said "is still
  inert" until #50; it stopped being true at #45 and nobody re-read it.
```

- [ ] **Step 2: Add the new row to `rm-conform`'s table**

In the table at `crates/rm-conform/README.md:43`, after `| recall applicability | 1.000 |`:

```markdown
| instant-local refusal agreement | 1.000 |
```

And beneath the existing sentence about refusal proportions, add the anti-vacuity figures recorded in Task 3 Step 7, with the real numbers substituted:

```markdown
Of the instant probes, N landed in a contested span and M answered, across
K histories that did both. That last figure is the one that matters: a
refusal is instant-local only if a single timeline can refuse one moment and
answer another, and a suite where no history did both would report 1.000
having measured nothing.
```

- [ ] **Step 3: Rewrite `rm-contrast`'s "What cuts against this store"**

That section currently reports the defect as a live finding. It becomes the record of a fixed one — the section keeps its place rather than being deleted, because a benchmark that quietly drops its own adverse finding is worth less than one that says what happened to it:

```markdown
## What cut against this store, and what happened to it

At a 25% tie rate, of 8,000 questions asked, 1,647 had no right answer.
**Of the remainder the store refused 4,067 it could have answered.** The
control refused none, because it has no way to.

`Strategy::ValidInterval` could not build a timeline when two segments
collided, so it refused **the whole read** — including for an instant where
nothing was ambiguous. The refusal was history-wide rather than
instant-local, and on a history with one in four writes colliding that was
most of the store's usefulness gone.

It was found by the calibration cell failing on its first run, recorded as a
decision rather than fixed, and then fixed in #50 after the argument for
leaving it turned out to be the wrong shape: it rested on the collision
never having fired on real data, which is a frequency claim about a
universally quantified property. **That count is now 0.** The assertion that
used to pin the defect — `store.declined > 0`, with a comment saying that if
it stopped happening the measurement had gone quiet — is now
`assert_eq!(store.declined, 0)`, and pins the fix.

Zero rather than a threshold, deliberately: the instants the store contests
and the instants `workload.rs` calls ambiguous are meant to be the same set,
so a residue would be the two rules disagreeing rather than a number to
relax.

An unanswerable question is still excluded from both stores' accuracy rather
than counted for or against either. Marking it either way is a thumb on the
scale: against, and refusal is punished; for, and the result is rigged. The
store scores no points for detecting its own ambiguity.
```

Substitute the real `declined` figure from Task 2 Step 9 if it is not 0 — and if it is not 0, stop and investigate rather than writing the number down.

- [ ] **Step 4: Record the re-decision**

In `docs/seed-decision-log.sh`, the rejected decision from #48 stays exactly where it is — the log's value is that it shows what was decided and then re-decided. Find it:

```bash
rg -n 'instant-local' docs/seed-decision-log.sh
```

Add the re-decision in the `re-decided` block, beside the `Retire recall@10` example. Note the `d` helper appends `--scope "$SEED_SCOPE"` after the positional arguments, so the title and choice come first:

```bash
d "Make ValidInterval's refusal instant-local" "build the timeline either way, name the contested spans, and refuse only an instant that lands in one"   --because "the reversal condition named when this was rejected -- a bulk import at day resolution on both axes -- has still not fired; what changed is the argument, which rested on the collision never having occurred in a live store, and rm-conform's README says correctness properties are universally quantified and an unrealistic input is still a valid one. Measured: 4,067 wrongly-refused questions to 0"
```

- [ ] **Step 5: Check the log still runs**

The script needs a store and an embedder, so run it against a throwaway one rather than the live store. `rmem init` writes an `rmem.toml`; the seeded decisions cost a few embeddings each.

```bash
bash -n docs/seed-decision-log.sh && echo "SYNTAX OK"
```

Syntax check only. A full run needs a key this session does not have; note in the pull request that the script was syntax-checked rather than executed, rather than implying it ran.

- [ ] **Step 6: Spellcheck and run the suite**

```bash
typos crates/rm-conform/README.md crates/rm-contrast/README.md docs/seed-decision-log.sh docs/superpowers/specs/2026-08-25-instant-local-refusal-design.md && cargo test --workspace -q 2>&1 | rg 'test result' | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p" failed="f}'
```

Expected: `passed=777 failed=0`. Task 4 touches documentation only, so the count must be unchanged from Task 3.

- [ ] **Step 7: Commit**

```bash
git add -A crates/ docs/ && git commit -m "$(cat <<'EOF'
What the documents claimed, and what is true now

rm-conform's README said "rmem about --valid-at is still inert under
most_recent". That stopped being true at #45, which made it refuse and name
the strategy, and nobody re-read the sentence. Corrected, and dated, because
the interesting part is that it drifted rather than that it was wrong.

rm-contrast's adverse finding keeps its section rather than being deleted. A
benchmark that quietly drops the thing that cut against it is worth less
than one that says what happened: found by the calibration cell, recorded as
a decision, then fixed once the argument for leaving it turned out to rest
on a frequency claim about a universally quantified property. 4,067 to 0.

The rejected decision from #48 stays in the log where it is, with the
re-decision recorded beside it. The reversal condition it named has still
not fired and the entry says so.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
EOF
)"
```

---

## Finishing

After Task 4, use `superpowers:finishing-a-development-branch`. Base branch is `main`.

The pull request reports three numbers, all measured rather than estimated: `rm-contrast`'s `declined` going from 4,067 to 0, `rm-conform`'s `instant-local refusal agreement` row, and the workspace test count going 771 → 777.
