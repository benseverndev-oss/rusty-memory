# Documentation Ingest Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Read a tree of markdown into a scratch store as facts, skipping any chunk whose text has not changed since it was last read.

**Architecture:** A pure chunker turns markdown into chunks carrying a heading path and a content hash. Each chunk becomes a `Turn` with no speaker and a `source_ref` of `path#heading@hash`, which the existing `remember` path already handles. Because the hash is *in* the `source_ref`, "have I read this text before" is answerable from the store itself — no sidecar ledger, and nothing that can desync from it.

**Tech Stack:** Rust. Hashing via a small hand-written FNV-1a — no new dependency for sixteen lines of arithmetic.

**Spec:** `docs/superpowers/specs/2026-08-27-document-ingest-design.md`

## Global Constraints

- **No writes to a live store.** This is step 1 of the spec's three, and its whole purpose is to produce evidence without risking anything permanent. The command refuses a store it did not create — see Task 4.
- **Nothing is summarised.** Chunks are the document's own text, verbatim.
- **A document never becomes an entity.** It is where a fact came from, which
  is what provenance is for. The temptation is to create one to hold the
  content hash — Task 2 exists so that is unnecessary, and doing it anyway
  would make "what does this file say" answerable, which is a document index
  and a different product.
- **Removal writes nothing.** A chunk that has disappeared from a document is not an assertion of absence. There is no deletion path in this plan, and adding one is a spec change.
- **Valid time stays unstated.** A file's mtime is when somebody edited a file, not when a fact became true.
- **Markdown only.** Non-markdown needs a second segmentation rule with no author-supplied structure, which is its own piece of work.

## The ledger, which the spec did not specify

The spec requires a re-run over an unchanged tree to cost **zero completions**,
and says nothing about where the record of "already read" lives. Two options,
and the difference matters:

A **sidecar file** beside the store can desync from it. Delete the store, keep
the ledger, and ingest skips everything into an empty store — a silent no-op
that looks like success. This repository has been bitten twice this week by
two copies of one fact drifting apart.

So: **the hash goes in the `source_ref`**, and the ledger *is* the store.
`docs/positioning.md#the-uncomfortable-part@a1b2c3d4` says where a fact came
from and what the text was when it did. An empty store knows nothing and reads
everything, which is correct.

---

### Task 1: The chunker

**Files:**
- Create: `crates/rm-host/src/ingest.rs`
- Modify: `crates/rm-host/src/lib.rs` (add `pub mod ingest;`)

**Interfaces:**
- Produces, for Tasks 2–4:
  ```rust
  pub struct Chunk {
      /// Heading path, joined by " > ". Empty for text before any heading.
      pub heading: String,
      /// The document's own text, verbatim.
      pub text: String,
      /// FNV-1a of `text`, lowercase hex.
      pub hash: String,
  }
  pub fn chunks(markdown: &str) -> Vec<Chunk>;
  pub fn source_ref(path: &str, chunk: &Chunk) -> String;
  ```

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Headings are the author's own segmentation, so they are the split.
    #[test]
    fn a_document_splits_on_its_headings() {
        let out = chunks("# Title\n\nintro text\n\n## First\n\nalpha\n\n## Second\n\nbeta\n");
        let headings: Vec<&str> = out.iter().map(|c| c.heading.as_str()).collect();
        assert_eq!(headings, ["Title", "Title > First", "Title > Second"]);
        assert!(out[1].text.contains("alpha"));
        assert!(!out[1].text.contains("beta"), "a chunk took its neighbour's text");
    }

    /// Text before any heading is still text, and still gets a chunk.
    #[test]
    fn a_preamble_is_not_dropped() {
        let out = chunks("loose opening line\n\n# Title\n\nbody\n");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].heading, "");
        assert!(out[0].text.contains("loose opening line"));
    }

    /// An empty section produces no chunk.
    ///
    /// A heading with nothing under it asserts nothing, and sending it to a
    /// model costs a completion to be told so.
    #[test]
    fn a_heading_with_no_body_is_not_a_chunk() {
        let out = chunks("# Title\n\n## Empty\n\n## Full\n\nbody\n");
        let headings: Vec<&str> = out.iter().map(|c| c.heading.as_str()).collect();
        assert_eq!(headings, ["Title > Full"]);
    }

    /// The same text hashes the same way, and different text does not.
    ///
    /// The whole idempotency story rests on this, so it is asserted rather
    /// than assumed.
    #[test]
    fn the_hash_follows_the_text_and_nothing_else() {
        let a = chunks("# T\n\nbody\n");
        let b = chunks("# T\n\nbody\n");
        let c = chunks("# T\n\nbody edited\n");
        assert_eq!(a[0].hash, b[0].hash);
        assert_ne!(a[0].hash, c[0].hash);

        // ...and the heading is not part of it: renaming a heading moves a
        // fact's provenance without making its text look new.
        let renamed = chunks("# Renamed\n\nbody\n");
        assert_eq!(a[0].hash, renamed[0].hash);
    }

    /// A source_ref says where a fact came from and what the text was.
    #[test]
    fn a_source_ref_carries_the_path_the_heading_and_the_hash() {
        let c = &chunks("# Title\n\nbody\n")[0];
        let r = source_ref("docs/positioning.md", c);
        assert!(r.starts_with("docs/positioning.md#Title@"), "{r}");
        assert!(r.ends_with(&c.hash), "{r}");
    }
}
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p rusty-memory-host --lib ingest`
Expected: FAIL — `cannot find function chunks`.

- [ ] **Step 3: Implement**

```rust
//! Markdown into chunks a document can be read from.
//!
//! Headings are the split because they are the author's own segmentation, and
//! a heading path makes a provenance string a reader can act on:
//! `docs/positioning.md#Title > The uncomfortable part@a1b2c3d4` says which of
//! nine hundred lines to go and read.

/// FNV-1a, 64-bit.
///
/// Hand-written rather than a dependency: this is sixteen lines of arithmetic
/// and the alternative is a crate in the tree of a published library. Not a
/// cryptographic hash and not used as one -- it answers "is this the same
/// text", where an adversary would have to be the person editing their own
/// documentation.
fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

pub struct Chunk {
    pub heading: String,
    pub text: String,
    pub hash: String,
}

pub fn chunks(markdown: &str) -> Vec<Chunk> {
    let mut out = Vec::new();
    // One entry per open heading level, so a path can be rebuilt at any depth.
    let mut path: Vec<String> = Vec::new();
    let mut heading = String::new();
    let mut body = String::new();

    let mut flush = |heading: &str, body: &mut String, out: &mut Vec<Chunk>| {
        let text = body.trim().to_string();
        body.clear();
        // A heading with nothing under it asserts nothing, and sending it to a
        // model costs a completion to be told so.
        if text.is_empty() {
            return;
        }
        out.push(Chunk {
            heading: heading.to_string(),
            hash: hash(&text),
            text,
        });
    };

    for line in markdown.lines() {
        if let Some(rest) = line.strip_prefix('#') {
            let depth = 1 + rest.chars().take_while(|c| *c == '#').count();
            let title = rest.trim_start_matches('#').trim().to_string();
            flush(&heading, &mut body, &mut out);
            path.truncate(depth - 1);
            path.push(title);
            heading = path.join(" > ");
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    flush(&heading, &mut body, &mut out);
    out
}

/// Where a fact came from, and what the text was when it did.
///
/// The hash is part of it on purpose: it makes the store its own ledger of
/// what has been read, so nothing can desync from it. See the plan.
pub fn source_ref(path: &str, chunk: &Chunk) -> String {
    format!("{path}#{}@{}", chunk.heading, chunk.hash)
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rusty-memory-host --lib ingest`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-host/src/ingest.rs crates/rm-host/src/lib.rs
git commit -m "Markdown into chunks that say where they came from"
```

---

### Task 2: The store is its own ledger

**Files:**
- Modify: `crates/rm-engine/src/read.rs`
- Modify: `crates/rm-engine/src/lib.rs` (re-export)

**Interfaces:**
- Produces, for Task 3:
  ```rust
  impl Engine {
      /// Every `provenance.source_ref` the store holds, deduplicated.
      pub fn source_refs(&self) -> std::collections::BTreeSet<String>;
  }
  ```

- [ ] **Step 1: Write the failing test**

```rust
/// The store can say what it has already read.
///
/// This is what makes a sidecar ledger unnecessary, and a sidecar is what
/// could desync: delete the store, keep the ledger, and a re-run skips
/// everything into an empty store -- a silent no-op that looks like success.
#[test]
fn a_store_knows_which_sources_it_has_seen() {
    let mut e = engine();
    told_from(&mut e, "docs/a.md#Title@aaaa");
    told_from(&mut e, "docs/a.md#Title@aaaa");
    told_from(&mut e, "docs/b.md#Other@bbbb");

    let seen = e.source_refs();
    assert_eq!(seen.len(), 2, "a repeated source counted twice: {seen:?}");
    assert!(seen.contains("docs/a.md#Title@aaaa"));

    // An empty store knows nothing, which is why it reads everything.
    assert!(engine().source_refs().is_empty());
}
```

Write `told_from` as a helper that remembers one observation whose
`Provenance::source_ref` is the given string, following the pattern in
`crates/rm-engine/tests/holders.rs`.

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rusty-memory-engine --test ingest_ledger`
Expected: FAIL — `no method named source_refs`.

- [ ] **Step 3: Implement**

```rust
    /// Every `provenance.source_ref` the store holds.
    ///
    /// A `BTreeSet` so the answer is stable across runs and diffable, and so a
    /// caller asking "have I read this" pays a lookup rather than a scan per
    /// question.
    ///
    /// Deliberately not an index: this is a scan over the store, and the
    /// caller that needs it -- ingest -- asks once per run rather than once
    /// per chunk. An index would be a second copy of something already
    /// stored, which is the drift this design exists to avoid.
    pub fn source_refs(&self) -> std::collections::BTreeSet<String> {
        self.store
            .entities()
            .flat_map(|e| e.attributes.values())
            .flatten()
            .map(|v| v.provenance.source_ref.clone())
            .collect()
    }
```

If `MemoryStore` exposes no entity iterator, add
`pub fn entities(&self) -> impl Iterator<Item = &Entity>` to `rm-store`
alongside it — the field is private and this is the one caller that needs it.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rusty-memory-engine`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-engine/src/ crates/rm-store/src/lib.rs crates/rm-engine/tests/ingest_ledger.rs
git commit -m "A store can say which sources it has already read"
```

---

### Task 3: Reading a document

**Files:**
- Modify: `crates/rm-host/src/ingest.rs`

**Interfaces:**
- Consumes: `chunks`, `source_ref` from Task 1; `Engine::source_refs` from Task 2; `command::remember`.
- Produces, for Task 4:
  ```rust
  pub struct Read {
      pub chunks_seen: usize,
      pub chunks_read: usize,
      pub chunks_skipped: usize,
      pub facts: usize,
  }
  pub fn read_document(
      engine: &mut Engine,
      path: &str,
      markdown: &str,
      observed_at: Timestamp,
      completer: &impl Completer,
      embedder: &impl Embedder,
  ) -> Result<Read, HostError>;
  ```

- [ ] **Step 1: Write the failing tests**

```rust
/// A second read of an unchanged document costs nothing.
///
/// The spec makes this the measurement that decides whether ingest ships: if
/// it does not hold, ingest is a one-shot import rather than something
/// runnable on a schedule.
#[test]
fn re_reading_an_unchanged_document_calls_no_model() {
    let doc = "# Roles\n\nRosalind owns the Okta setup.\n";
    let mut e = engine();
    let completer = CountingCompleter::default();

    let first = read_document(&mut e, "docs/team.md", doc, 100, &completer, &Hashed::new(3)).unwrap();
    assert_eq!(first.chunks_read, 1);
    let after_first = completer.calls();
    assert!(after_first > 0, "the first read called no model at all");

    let second = read_document(&mut e, "docs/team.md", doc, 200, &completer, &Hashed::new(3)).unwrap();
    assert_eq!(second.chunks_read, 0);
    assert_eq!(second.chunks_skipped, 1);
    assert_eq!(
        completer.calls(),
        after_first,
        "an unchanged chunk was sent to the model again"
    );
}

/// An edited chunk is read again; its unedited neighbours are not.
#[test]
fn editing_one_section_re_reads_only_that_section() {
    let before = "# A\n\nalpha\n\n# B\n\nbeta\n";
    let after = "# A\n\nalpha\n\n# B\n\nbeta edited\n";
    let mut e = engine();
    let completer = CountingCompleter::default();

    read_document(&mut e, "docs/x.md", before, 100, &completer, &Hashed::new(3)).unwrap();
    let baseline = completer.calls();

    let out = read_document(&mut e, "docs/x.md", after, 200, &completer, &Hashed::new(3)).unwrap();
    assert_eq!(out.chunks_read, 1, "both sections were re-read");
    assert_eq!(out.chunks_skipped, 1);
    assert!(completer.calls() > baseline);
}

/// A section deleted from a document writes nothing.
///
/// A removed sentence is not an assertion of absence: nobody said there is
/// none, the document simply stopped saying it. Writing a tombstone here would
/// manufacture absences at the rate documents get edited.
#[test]
fn deleting_a_section_asserts_nothing() {
    let before = "# A\n\nalpha\n\n# B\n\nbeta\n";
    let after = "# A\n\nalpha\n";
    let mut e = engine();
    let completer = CountingCompleter::default();

    read_document(&mut e, "docs/x.md", before, 100, &completer, &Hashed::new(3)).unwrap();
    let before_count = e.assertion_count();

    let out = read_document(&mut e, "docs/x.md", after, 200, &completer, &Hashed::new(3)).unwrap();
    assert_eq!(out.chunks_seen, 1);
    assert_eq!(
        e.assertion_count(),
        before_count,
        "removing a section wrote something -- it must write nothing"
    );
}
```

`CountingCompleter` wraps `rm_host::testing::StubProvider` and counts calls;
write it in the same test module. If `Engine` has no `assertion_count`, use
`engine.source_refs().len()` plus a per-entity attribute count — but say which
in the test rather than leaving it implicit.

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p rusty-memory-host --lib ingest`
Expected: FAIL — `cannot find function read_document`.

- [ ] **Step 3: Implement**

```rust
/// What one document produced.
pub struct Read {
    pub chunks_seen: usize,
    pub chunks_read: usize,
    pub chunks_skipped: usize,
    pub facts: usize,
}

/// Read one document into the store.
///
/// Each chunk becomes a turn with **no speaker** -- a document has no first
/// person, and `rm_extract`'s prompt says so explicitly rather than leaving a
/// blank for the model to fill.
///
/// Nothing is deleted. A chunk that has vanished since the last read is simply
/// not seen, and what it once asserted goes on standing until something
/// contradicts it.
pub fn read_document(
    engine: &mut Engine,
    path: &str,
    markdown: &str,
    observed_at: Timestamp,
    completer: &impl Completer,
    embedder: &impl Embedder,
) -> Result<Read, HostError> {
    let seen = engine.source_refs();
    let all = chunks(markdown);
    let mut out = Read {
        chunks_seen: all.len(),
        chunks_read: 0,
        chunks_skipped: 0,
        facts: 0,
    };

    for chunk in &all {
        let reference = source_ref(path, chunk);
        // The hash is in the reference, so an unchanged chunk is one the store
        // has already seen under exactly this name.
        if seen.contains(&reference) {
            out.chunks_skipped += 1;
            continue;
        }
        let outcome = crate::command::remember(
            engine,
            &chunk.text,
            observed_at,
            &reference,
            // No speaker: a document has no first person.
            None,
            completer,
            embedder,
        )?;
        out.chunks_read += 1;
        if let Outcome::Remembered { ingested, .. } = outcome {
            out.facts += ingested.len();
        }
    }
    Ok(out)
}
```

Check `Outcome::Remembered`'s actual field for the fact count before writing
that last block — `ingested` is the name in this crate today, and a plan that
guesses it is a plan that does not compile.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rusty-memory-host`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-host/src/ingest.rs
git commit -m "Read a document, and read an unchanged one for free"
```

---

### Task 4: The command, and the refusal that keeps it off a live store

**Files:**
- Modify: `crates/rm-cli/src/args.rs`, `crates/rm-cli/src/run.rs`, `crates/rm-cli/src/format.rs`
- Modify: `crates/rm-host/src/ingest.rs`

**Interfaces:**
- Consumes: `read_document` from Task 3.
- Produces: `rmem ingest <path> [--dry-run]`.

- [ ] **Step 1: Write the failing test**

The spec's first constraint is that this step writes to no live store, and a
constraint nothing enforces is a comment.

```rust
/// Ingest refuses a store that already holds anything but ingested facts.
///
/// Step 1 of the spec exists to produce evidence without risking anything
/// permanent, and "please point it somewhere scratch" is not a mechanism.
/// The check is deliberately crude: a store with assertions from any source
/// other than a document is somebody's real store.
#[test]
fn ingest_refuses_a_store_that_is_not_a_scratch_one() {
    let mut e = engine();
    // One hand-written note is enough to make this somebody's store.
    note_into(&mut e, "Jon Severn", "role", "leads circ");

    let err = read_tree(&mut e, "docs", 100, &stub(), &Hashed::new(3)).unwrap_err();
    assert!(
        format!("{err}").contains("scratch"),
        "ingest wrote into a store holding hand-written facts: {err}"
    );
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rusty-memory-host --lib refuses_a_store`
Expected: FAIL — `cannot find function read_tree`.

- [ ] **Step 3: Implement `read_tree` and the guard**

```rust
/// Read every markdown file under a directory.
///
/// Refuses a store holding assertions that did not come from a document. Step
/// 1 of the spec writes to a scratch store only, and this is what makes that a
/// mechanism rather than an instruction. A `source_ref` from ingest always
/// carries an `@`; nothing else in this workspace writes one.
pub fn read_tree(
    engine: &mut Engine,
    root: &str,
    observed_at: Timestamp,
    completer: &impl Completer,
    embedder: &impl Embedder,
) -> Result<Read, HostError> {
    if engine.source_refs().iter().any(|r| !r.contains('@')) {
        return Err(HostError::Refused(
            "this store holds facts that did not come from a document, so it is not a scratch store -- ingest writes to a scratch store only until an extractor can decline a reading it is unsure of".into(),
        ));
    }
    // ...walk `root` for `*.md`, call `read_document` per file, sum the counts.
}
```

Walk with `std::fs::read_dir` recursively; there is no `walkdir` in this
workspace and one file's worth of recursion does not justify adding it.

- [ ] **Step 4: Wire the CLI**

`rmem ingest <path>` with `--dry-run`, which chunks and reports without calling
the model. The output is the measurement:

```
docs/  47 chunks, 12 read, 35 unchanged, 88 facts
```

- [ ] **Step 5: Run the tests and the whole gate**

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/rm-cli/src/ crates/rm-host/src/ingest.rs
git commit -m "rmem ingest, and a refusal that keeps it off a real store"
```

---

### Task 5: The evidence this exists to produce

**Files:**
- Create: `docs/ingest-findings.md`

- [ ] **Step 1: Run it on this repository's own docs**

```bash
rmem ingest docs/ --dry-run     # chunk counts, no model
rmem ingest docs/               # the real run, into a scratch store
rmem ingest docs/               # and again -- this must read 0
```

- [ ] **Step 2: Write down the three numbers the spec asks for**

Facts per document; completions on an unchanged re-run (**must be zero**); and
the review-band rate — how often the resolver files a question when subjects
come from documents rather than conversation. The coworker register produced
four questions from thirty-four writes, and there is no reason to expect
documents to match it.

- [ ] **Step 3: Read a sample of what was extracted, and say what is wrong with it**

This is the deliverable, not the counts. The declines spec was written from
argument; this is where it gets evidence. For twenty extracted facts, record
which are right, which are wrong, and **what kind of wrong** — an ambiguous
subject, a hedge read as an assertion, a heading's context lost.

Those categories are what an extractor would need to decline, and they should
be compared against what
`docs/superpowers/specs/2026-08-26-extraction-declines-design.md` predicted.
Where they differ, that spec is wrong and should be revised before it is built.

- [ ] **Step 4: Record the decision**

From a script file, never inline:

```bash
rmem decide "Documentation ingest reads a tree into a scratch store first" \
  "chunk on headings, put the content hash in the source_ref so the store is its own ledger, and refuse a store holding hand-written facts" \
  --context "step 1 of three: ingest, then the declines work built against what it produces, then ingest to a live store" \
  --because "ingest is a volume feature and that changes the risk rather than the kind -- one document is a handful of facts and a tree is hundreds, and an extractor that cannot say a reading is ambiguous will assert one and be wrong in ways nothing can distinguish from being right. A sidecar ledger could desync from the store, so the hash lives in the source_ref and an empty store correctly reads everything" \
  --scope "*"
```

---

## Finishing

Use `superpowers:finishing-a-development-branch`. Base branch is `main`.

The pull request leads with the three measurements and with **what the
extracted facts got wrong**, because that is what step 2 needs. It states that
nothing was written to a live store and that the refusal in Task 4 is what
enforces it.

If the unchanged re-run costs anything other than zero completions, say so
plainly: that is the spec's ship-or-drop condition, and the feature is a
one-shot import rather than something runnable on a schedule.
