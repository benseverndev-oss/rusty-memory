# Tool Table Cost Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cut per-property schema prose that restates the schema, keeping the sentences that stop a model using the store wrongly, and measure the result.

**Architecture:** A prose edit to `all_definitions()` in `crates/rm-mcp/src/tools.rs`, guarded by a per-tool byte assertion so descriptions cannot grow back unnoticed. No tool is added, removed, renamed or reordered, and no default changes.

**Tech Stack:** Rust, `serde_json`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-26-tool-table-cost-design.md`

## Global Constraints

- **No tool is removed and the default does not change.** Narrowing the default would silently strip `remember`, `note` and `about` from any session that has not set `RMEM_TOOLS`, with no error — the model simply stops having the capability.
- Four sentences are **load-bearing and stay**, in substance if not in wording: what separates `note` from `remember`; what `absent` means against `unknown`; that `scope` is the field most often wrong; that `fields` identify *who* rather than describing them.
- No byte target is set. A budget invites cutting the sentence hardest to justify per byte, which is the one about `absent`.
- The chars-per-token ratio lives in exactly one place. If `2026-08-26-executable-claims.md` has landed, consume its `CHARS_PER_TOKEN`; if not, define it here with the same provenance comment and that plan consumes this one.
- The README's cost table and `definitions()`' doc comment are updated in the **same commit** as any size change. Those two disagreed for a day because one moved and the other did not.

---

### Task 1: Pin the current sizes before touching anything

**Files:**
- Modify: `crates/rm-mcp/src/tools.rs`

**Interfaces:**
- Produces, for Task 2: `fn tool_bytes() -> Vec<(String, usize)>` in the test module, returning each tool's serialised length.

- [ ] **Step 1: Write the test that records today's sizes**

```rust
/// Per-tool size, pinned before the edit so the edit can be measured.
///
/// The band is generous: a reworded sentence must not fail this, a tool
/// appearing or vanishing must. The point is that a description growing back
/// over a year shows up in a diff rather than in a token bill.
#[test]
fn each_tool_is_about_the_size_it_was_measured_at() {
    let expected = [
        ("remember", 788),
        ("note", 2164),
        ("recall", 1174),
        ("about", 889),
        ("reviews", 380),
        ("decide", 2089),
        ("decisions", 1188),
        ("decision", 1194),
        ("resolve_review", 492),
    ];
    for (name, was) in expected {
        let now = tool_bytes()
            .into_iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{name} left the table"))
            .1;
        assert!(
            (now as i64 - was as i64).abs() < 250,
            "{name} is {now} bytes, was {was} -- update this and the README's row together"
        );
    }
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p rm-mcp --lib each_tool_is_about_the_size`
Expected: PASS. These numbers were measured on 2026-08-26 at commit `8086d25`.

- [ ] **Step 3: Commit**

```bash
git add crates/rm-mcp/src/tools.rs
git commit -m "Pin each tool's size before editing any of the prose"
```

---

### Task 2: Cut property prose that restates the schema

**Files:**
- Modify: `crates/rm-mcp/src/tools.rs`

- [ ] **Step 1: Work tool by tool, largest first**

`note` (2,164) and `decide` (2,089) are together 41% of the table. Their
top-level descriptions are only 677 and 723 bytes, so the bulk is inside
`inputSchema`.

For each property, ask: **does this sentence say anything `"type"`,
`"required"`, `"enum"` or `"default"` cannot?**

- It restates a type, a default, or requiredness → cut, and express it
  structurally if it is not already.
- It repeats what the tool description said → cut.
- It says what goes wrong if you get this wrong, or when to reach for this
  rather than its neighbour → **keep**.

Worked example — `note`'s `scope`, which is a keep:

```
"How far this fact reaches, if it is not true everywhere. Omit it for an
 ordinary fact about a person: with no scope it reaches every project, which
 is usually right."
```

The first clause restates the field name and could go. The rest is the
keep: it names the default behaviour and tells the model when *not* to think
about it, and `scope` is the field most often wrong.

A cut, from the same tool — `kind`:

```
"What sort of thing this is. Defaults to person."
```

`"default": "person"` in the schema says the second sentence, structurally and
more reliably. Add the `default` key and cut the prose to `"What sort of thing
this is."`

- [ ] **Step 2: Run the table tests continuously**

Run: `cargo test -p rm-mcp`
Expected: PASS throughout. The example-per-schema test, the fixed-order test
and the protocol handshake's count all still pass — property descriptions are
not read by `Call::read`, so this edit cannot break argument parsing.

**That is also this task's honest limit.** No test in this repository can see a
model understanding a tool less well. It is the reason for cutting only what
demonstrably restates the schema and leaving anything arguable.

- [ ] **Step 3: Confirm the four load-bearing sentences survive**

```rust
/// The sentences that are not fat.
///
/// Each stops a specific wrong answer: conflating an asserted absence with an
/// unasked question is how a model comes to state that someone has no
/// employer because nobody mentioned one.
#[test]
fn the_distinctions_a_model_gets_wrong_without_are_still_stated() {
    let table = serde_json::to_string(&all_definitions()).unwrap();
    for phrase in ["absent", "never been discussed", "remember", "reaches"] {
        assert!(table.contains(phrase), "the table stopped saying {phrase:?}");
    }
}
```

Adjust the phrases to the wording actually kept — but assert on wording that
carries the *distinction*, not on a word that could survive a rewrite that lost
the meaning.

- [ ] **Step 4: Re-measure and update all three places at once**

```bash
cargo test -p rm-mcp --lib sizing -- --ignored --nocapture
```

Update, in one commit: the pinned numbers in Task 1's test, the README's
`| everything | 9 | ~N |` row, and `definitions()`' doc comment.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-mcp/src/tools.rs README.md
git commit -m "Cut schema prose that the schema already says"
```

---

### Task 3: The record

- [ ] **Step 1: Record the decision**

From a script file, never inline:

```bash
rmem decide "The tool table's cost is per-property prose, not the tool count" \
  "cut property descriptions that restate the schema; keep the ones naming a consequence, and do not narrow the default" \
  --context "measured per tool: the top-level description is 31 to 35 percent of a tool's bytes and the rest is inputSchema, overwhelmingly per-property prose. note is 2164 bytes with 677 in its description" \
  --because "the obvious reading of a 2600-token table is that there are too many tools, and narrowing the default would have saved the most. It also silently removes capabilities from every session that never set RMEM_TOOLS, with no error to notice. Measuring first moved the work to the two thirds of the bytes nobody was looking at, and the honest limit is that no test here can see a model understanding a tool less well -- so only prose that demonstrably restates the schema is safe to cut" \
  --scope "*"
```

- [ ] **Step 2: Run the whole gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
```

---

## Finishing

Use `superpowers:finishing-a-development-branch`. Base branch is `main`.

The pull request reports the before and after byte counts per tool and the
table total, states that no tool was removed and no default changed, and names
the four distinctions that were deliberately kept.
