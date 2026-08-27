//! Markdown into chunks a document can be read from.
//!
//! Headings are the split because they are the author's own segmentation, and
//! a heading path makes a provenance string a reader can act on:
//! `docs/positioning.md#Title > The uncomfortable part@a1b2c3d4` says which of
//! nine hundred lines to go and read. A path alone does not.
//!
//! The content hash is part of that string on purpose. It makes the store its
//! own record of what has been read, so there is no sidecar file that can
//! desync from it -- delete the store, keep a ledger, and a re-run skips
//! everything into an empty store, which is a silent no-op that looks like
//! success.

use rm_engine::{Embedder, Engine, Timestamp};
use rm_extract::Completer;

use crate::command::Outcome;
use crate::HostError;

/// FNV-1a, 64-bit.
///
/// Hand-written rather than a dependency: this is a dozen lines of arithmetic
/// and the alternative is another crate in the tree of a published library.
/// Not a cryptographic hash and not used as one -- it answers "is this the
/// same text", where an adversary would have to be the person editing their
/// own documentation.
fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// One section of a document, as its author divided it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// Heading path, joined by " > ". Empty for text before any heading.
    pub heading: String,
    /// The document's own text, verbatim. Nothing is summarised.
    pub text: String,
    /// FNV-1a of `text`, lowercase hex.
    pub hash: String,
}

/// Split a document on its headings.
pub fn chunks(markdown: &str) -> Vec<Chunk> {
    let mut out: Vec<Chunk> = Vec::new();
    // One entry per open heading level, so a path can be rebuilt at any depth.
    let mut path: Vec<String> = Vec::new();
    let mut heading = String::new();
    let mut body = String::new();

    fn flush(heading: &str, body: &mut String, out: &mut Vec<Chunk>) {
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
    }

    for line in markdown.lines() {
        if line.starts_with('#') {
            let depth = line.chars().take_while(|c| *c == '#').count();
            let title = line.trim_start_matches('#').trim().to_string();
            flush(&heading, &mut body, &mut out);
            path.truncate(depth.saturating_sub(1));
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
/// The hash is part of it so the store can answer "have I read this" without a
/// second file to keep in step. See this module's own documentation.
pub fn source_ref(path: &str, chunk: &Chunk) -> String {
    format!("{path}#{}@{}", chunk.heading, chunk.hash)
}

/// What a run produced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Read {
    pub chunks_seen: usize,
    pub chunks_read: usize,
    pub chunks_skipped: usize,
    pub facts: usize,
}

/// A chunk that has to be read, with its extraction already done.
pub struct Planned {
    pub source_ref: String,
    plan: crate::command::RememberPlan,
}

/// Everything a run will write, and what it skipped getting there.
pub struct Plan {
    pub planned: Vec<Planned>,
    pub seen: usize,
    pub skipped: usize,
}

/// Extract every chunk the store has not already read.
///
/// # No store, on purpose
///
/// This takes `seen` rather than an `Engine`, because every model call in this
/// crate happens *above* the lock rather than inside it. `command`'s own note
/// records why: extraction and embedding are seconds each across a network,
/// `Lock::acquire` waits five seconds, and doing them under the lock was
/// measured to cap a live store at three concurrent writers.
///
/// A tree makes that worse rather than differently: one run is one extraction
/// per changed chunk, so holding the lock across them would hold it for
/// minutes.
pub fn plan_tree(
    seen: &std::collections::BTreeSet<String>,
    root: &std::path::Path,
    observed_at: Timestamp,
    completer: &impl Completer,
    embedder: &impl Embedder,
    dimension: usize,
    metric: rm_engine::Metric,
) -> Result<Plan, HostError> {
    let mut files = Vec::new();
    collect(root, &mut files)?;
    // Sorted, so a run's order does not depend on how a filesystem happens to
    // enumerate a directory.
    files.sort();

    let mut out = Plan {
        planned: Vec::new(),
        seen: 0,
        skipped: 0,
    };

    for file in files {
        let text = std::fs::read_to_string(&file)
            .map_err(|e| HostError::Refused(format!("could not read {}: {e}", file.display())))?;
        let shown = file.to_string_lossy().replace('\\', "/");

        for chunk in chunks(&text) {
            out.seen += 1;
            let reference = source_ref(&shown, &chunk);
            // The hash is in the reference, so an unchanged chunk is one the
            // store has already seen under exactly this name.
            if seen.contains(&reference) {
                out.skipped += 1;
                continue;
            }
            let plan = crate::command::plan_remember(
                &chunk.text,
                observed_at,
                &reference,
                // A document has no first person, and rm-extract's prompt says
                // so rather than leaving a blank for the model to fill.
                None,
                completer,
                embedder,
                dimension,
                metric,
            )?;
            out.planned.push(Planned {
                source_ref: reference,
                plan,
            });
        }
    }
    Ok(out)
}

/// Count what a run would read, without calling anything.
///
/// What `--dry-run` reports. The point is to know a run's cost before paying
/// it: a tree whose chunks are almost all unchanged costs almost nothing, and
/// one being read for the first time costs a completion per chunk.
pub fn survey(
    seen: &std::collections::BTreeSet<String>,
    root: &std::path::Path,
) -> Result<Read, HostError> {
    let mut files = Vec::new();
    collect(root, &mut files)?;
    files.sort();

    let mut out = Read::default();
    for file in files {
        let text = std::fs::read_to_string(&file)
            .map_err(|e| HostError::Refused(format!("could not read {}: {e}", file.display())))?;
        let shown = file.to_string_lossy().replace('\\', "/");
        for chunk in chunks(&text) {
            out.chunks_seen += 1;
            if seen.contains(&source_ref(&shown, &chunk)) {
                out.chunks_skipped += 1;
            } else {
                out.chunks_read += 1;
            }
        }
    }
    Ok(out)
}

/// Write what [`plan_tree`] extracted.
///
/// # Refusing a store that is not a scratch one
///
/// Step 1 of `docs/superpowers/specs/2026-08-27-document-ingest-design.md`
/// writes to a scratch store only, until an extractor can decline a reading it
/// is unsure of. "Please point it somewhere scratch" is not a mechanism, so
/// this refuses.
///
/// The check is deliberately crude: an ingested assertion's `source_ref`
/// always carries an `@`, and nothing else in this workspace writes one. A
/// single hand-written `note` is therefore enough to make ingest refuse, which
/// is the direction this error should lean.
///
/// Nothing is ever deleted. A chunk that has vanished since the last read is
/// simply not planned, and what it once asserted goes on standing until
/// something contradicts it -- a removed sentence is not somebody saying
/// "there is none".
pub fn commit_tree(engine: &mut Engine, plan: Plan) -> Result<Read, HostError> {
    if let Some(theirs) = engine.source_refs().iter().find(|r| !r.contains('@')) {
        return Err(HostError::Refused(format!(
            "this store holds facts that did not come from a document ({theirs:?}), so it is not a scratch store. Ingest writes to a scratch store only until an extractor can decline a reading it is unsure of -- point RMEM_CONFIG at a fresh store"
        )));
    }

    let mut out = Read {
        chunks_seen: plan.seen,
        chunks_skipped: plan.skipped,
        ..Read::default()
    };
    for Planned { plan, source_ref } in plan.planned {
        let outcome = crate::command::commit_remember(engine, plan)?;
        // Recorded whatever it yielded. A chunk of prose that asserts nothing
        // still cost a model call, and a ledger derived from what was written
        // would forget it and pay again on every run -- measured at 21 chunks
        // in 30 on this repository's own documentation.
        engine.mark_read(source_ref);
        out.chunks_read += 1;
        if let Outcome::Remembered { ingested, .. } = outcome {
            out.facts += ingested.assertions.len();
        }
    }
    Ok(out)
}

/// Every `.md` under a directory, recursively.
///
/// `read_dir` rather than a crate: one function's worth of recursion does not
/// justify a dependency in a published library's tree.
fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), HostError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| HostError::Refused(format!("could not read {}: {e}", dir.display())))?;
    for entry in entries {
        let entry =
            entry.map_err(|e| HostError::Refused(format!("could not read an entry: {e}")))?;
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
    Ok(())
}

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
        assert!(
            !out[1].text.contains("beta"),
            "a chunk took its neighbour's text"
        );
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

    /// A deeper heading nests under its parent, and a shallower one pops back.
    ///
    /// The path is what makes provenance legible, so it has to survive going
    /// down and coming back up.
    #[test]
    fn a_heading_path_pops_back_to_its_ancestor() {
        let out = chunks("# A\n\na\n\n## B\n\nb\n\n### C\n\nc\n\n## D\n\nd\n\n# E\n\ne\n");
        let headings: Vec<&str> = out.iter().map(|c| c.heading.as_str()).collect();
        assert_eq!(headings, ["A", "A > B", "A > B > C", "A > D", "E"]);
    }

    // ---- reading a tree --------------------------------------------------

    use rm_engine::{
        BlockingKey, Comparator, FieldRule, Metric, Policy, Ruleset, Strategy, VectorIndex,
    };
    use rm_extract::CompleterError;

    /// A stub that counts, so "no model was called" fails legibly rather than
    /// as a stub running out of canned answers.
    #[derive(Default)]
    struct CountingCompleter {
        calls: std::cell::Cell<usize>,
    }

    impl CountingCompleter {
        fn calls(&self) -> usize {
            self.calls.get()
        }
    }

    impl Completer for CountingCompleter {
        fn complete(&self, prompt: &str) -> Result<String, CompleterError> {
            self.calls.set(self.calls.get() + 1);
            // Named after whatever the chunk said, so two chunks do not
            // collapse onto one entity and hide a miscount.
            let who: String = prompt
                .lines()
                .find(|l| l.contains("alpha") || l.contains("beta") || l.contains("gamma"))
                .unwrap_or("someone")
                .trim()
                .chars()
                .take(12)
                .collect();
            Ok(format!(
                r#"{{"mentions":[{{"kind":"person","name":"{who}","text":"{who}"}}],"facts":[{{"subject":0,"attribute":"role","value":"noted","text":"{who} role"}}],"relations":[],"closures":[]}}"#
            ))
        }
    }

    fn doc_engine() -> Engine {
        Engine::new(
            VectorIndex::new(3, Metric::Cosine),
            Ruleset::new(
                vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
                vec![BlockingKey::Prefix("name".to_string(), 3)],
                4.0,
                6.0,
            )
            .unwrap(),
            Policy::new(Strategy::MostRecent),
        )
    }

    fn tree() -> crate::testing::TempDir {
        let dir = crate::testing::TempDir::new();
        std::fs::create_dir_all(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("a.md"), "# A\n\nalpha\n").unwrap();
        std::fs::write(dir.path().join("nested/b.md"), "# B\n\nbeta\n").unwrap();
        std::fs::write(dir.path().join("skip.txt"), "not markdown").unwrap();
        dir
    }

    fn run(
        e: &mut Engine,
        dir: &std::path::Path,
        at: Timestamp,
        c: &CountingCompleter,
    ) -> Result<Read, HostError> {
        let emb = crate::testing::StubProvider::new(vec![]);
        let plan = plan_tree(e.read_sources(), dir, at, c, &emb, 3, Metric::Cosine)?;
        commit_tree(e, plan)
    }

    /// A tree of markdown becomes facts, and nothing else does.
    #[test]
    fn a_tree_of_markdown_is_read_and_other_files_are_not() {
        let dir = tree();
        let mut e = doc_engine();
        let c = CountingCompleter::default();

        let out = run(&mut e, dir.path(), 100, &c).unwrap();
        assert_eq!(out.chunks_seen, 2, "a non-markdown file was read");
        assert_eq!(out.chunks_read, 2);
        assert!(out.facts > 0);
    }

    /// A second run over an unchanged tree calls no model.
    ///
    /// The spec makes this the measurement that decides whether ingest ships:
    /// if it does not hold, ingest is a one-shot import rather than something
    /// runnable on a schedule.
    #[test]
    fn re_running_over_an_unchanged_tree_calls_no_model() {
        let dir = tree();
        let mut e = doc_engine();
        let c = CountingCompleter::default();

        run(&mut e, dir.path(), 100, &c).unwrap();
        let after_first = c.calls();
        assert!(after_first > 0, "the first run called no model at all");

        let again = run(&mut e, dir.path(), 200, &c).unwrap();
        assert_eq!(again.chunks_read, 0);
        assert_eq!(again.chunks_skipped, 2);
        assert_eq!(
            c.calls(),
            after_first,
            "an unchanged chunk was sent to the model again"
        );
    }

    /// An edited file is read again; its unedited neighbours are not.
    #[test]
    fn editing_one_file_re_reads_only_that_file() {
        let dir = tree();
        let mut e = doc_engine();
        let c = CountingCompleter::default();

        run(&mut e, dir.path(), 100, &c).unwrap();
        let baseline = c.calls();

        std::fs::write(dir.path().join("a.md"), "# A\n\ngamma\n").unwrap();
        let out = run(&mut e, dir.path(), 200, &c).unwrap();
        assert_eq!(out.chunks_read, 1, "both files were re-read");
        assert_eq!(out.chunks_skipped, 1);
        assert_eq!(c.calls(), baseline + 1);
    }

    /// Deleting a file writes nothing and calls nothing.
    ///
    /// A removed section is not an assertion of absence: nobody said there is
    /// none, the document simply stopped saying it. Tombstoning here would
    /// manufacture absences at the rate documents get edited.
    #[test]
    fn deleting_a_file_asserts_nothing() {
        let dir = tree();
        let mut e = doc_engine();
        let c = CountingCompleter::default();

        run(&mut e, dir.path(), 100, &c).unwrap();
        let sources_before = e.source_refs().len();
        let calls_before = c.calls();

        std::fs::remove_file(dir.path().join("a.md")).unwrap();
        let out = run(&mut e, dir.path(), 200, &c).unwrap();
        assert_eq!(out.chunks_seen, 1, "the deleted file was still seen");
        assert_eq!(out.chunks_read, 0);
        assert_eq!(
            e.source_refs().len(),
            sources_before,
            "removing a file wrote something -- it must write nothing"
        );
        assert_eq!(c.calls(), calls_before, "a deletion called the model");
    }

    /// Ingest refuses a store that holds anything it did not write.
    ///
    /// Step 1 of the spec exists to produce evidence without risking anything
    /// permanent, and "please point it somewhere scratch" is not a mechanism.
    #[test]
    fn ingest_refuses_a_store_that_is_not_a_scratch_one() {
        let dir = tree();
        let mut e = doc_engine();
        let c = CountingCompleter::default();

        // A note: written by a person, so its source_ref carries no '@'.
        e.remember(rm_engine::Observation {
            kind: "person".into(),
            mention: rm_engine::Record::new().with("name", "Jon Severn"),
            attribute: "role".into(),
            value: Some("leads circ".into()),
            valid: rm_engine::Interval::since(100),
            provenance: rm_engine::Provenance::new(rm_engine::Source::UserAssertion, 100, "cli"),
            supersession: rm_engine::Supersession::Corrects,
            according_to: None,
            embedding: vec![1.0, 0.0, 0.0],
        })
        .unwrap();

        let err = run(&mut e, dir.path(), 100, &c).unwrap_err();
        assert!(format!("{err}").contains("scratch"), "{err}");
    }

    /// Planning needs no store at all.
    ///
    /// The type is the guarantee that every model call happens above the lock:
    /// `plan_tree` cannot touch an `Engine` because it is not given one, and
    /// `command`'s own note records why that matters -- extraction under the
    /// lock was measured to cap a live store at three concurrent writers.
    #[test]
    fn planning_takes_no_engine_so_it_cannot_run_under_the_lock() {
        let dir = tree();
        let c = CountingCompleter::default();
        let emb = crate::testing::StubProvider::new(vec![]);

        let plan = plan_tree(
            &std::collections::BTreeSet::new(),
            dir.path(),
            100,
            &c,
            &emb,
            3,
            Metric::Cosine,
        )
        .unwrap();

        assert_eq!(plan.planned.len(), 2);
        assert_eq!(plan.skipped, 0);
        assert!(c.calls() > 0, "planning called no model");
    }
    /// A chunk that yields no facts leaves no trace, and is read again forever.
    ///
    /// The defect a real run found, now fixed. It is kept because it is the
    /// only test that reaches the zero-yield path: 30 chunks of this repository's own
    /// documentation produced 9 source_refs, because 21 of them extracted to
    /// nothing. The store cannot be its own ledger of what was *read* when it
    /// only records what was *written*.
    ///
    /// Every other test here missed it because their stub always returns a
    /// fact, so the zero-yield path did not exist. Ignored rather than
    /// deleted: the fix is a design decision -- see
    /// `docs/ingest-findings.md` -- and a defect nobody can see is one nobody
    /// fixes -- and one nobody can retest is one that comes back.
    #[test]
    fn a_chunk_that_yields_nothing_is_still_remembered_as_read() {
        /// Extracts nothing, which is what most prose does.
        struct Silent;
        impl Completer for Silent {
            fn complete(&self, _: &str) -> Result<String, CompleterError> {
                Ok(r#"{"mentions":[],"facts":[],"relations":[],"closures":[]}"#.to_string())
            }
        }

        let dir = tree();
        let mut e = doc_engine();
        let emb = crate::testing::StubProvider::new(vec![]);

        let plan = plan_tree(
            e.read_sources(),
            dir.path(),
            100,
            &Silent,
            &emb,
            3,
            Metric::Cosine,
        )
        .unwrap();
        commit_tree(&mut e, plan).unwrap();

        let plan = plan_tree(
            e.read_sources(),
            dir.path(),
            200,
            &Silent,
            &emb,
            3,
            Metric::Cosine,
        )
        .unwrap();
        let again = commit_tree(&mut e, plan).unwrap();
        assert_eq!(
            again.chunks_read, 0,
            "a chunk that asserted nothing was sent to the model a second time"
        );
    }
}
