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

/// What one document produced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Read {
    pub chunks_seen: usize,
    pub chunks_read: usize,
    pub chunks_skipped: usize,
    pub facts: usize,
}

/// Read one document into the store.
///
/// Each chunk becomes a turn with **no speaker**. A document has no first
/// person, and `rm_extract`'s prompt says so explicitly rather than leaving a
/// blank for the model to fill.
///
/// Nothing is ever deleted. A chunk that has vanished since the last read is
/// simply not seen, and what it once asserted goes on standing until something
/// contradicts it. A removed sentence is not somebody saying "there is none" --
/// writing a tombstone for it would manufacture absences at the rate documents
/// get edited, in a store whose whole claim is that it does not.
pub fn read_document(
    engine: &mut Engine,
    path: &str,
    markdown: &str,
    observed_at: Timestamp,
    completer: &impl Completer,
    embedder: &impl Embedder,
) -> Result<Read, HostError> {
    // Asked once per document rather than once per chunk: this is a walk over
    // the store, and the answer cannot change while we are reading.
    let seen = engine.source_refs();
    let all = chunks(markdown);
    let mut out = Read {
        chunks_seen: all.len(),
        ..Read::default()
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
            None,
            completer,
            embedder,
        )?;
        out.chunks_read += 1;
        if let Outcome::Remembered { ingested, .. } = outcome {
            out.facts += ingested.assertions.len();
        }
    }
    Ok(out)
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
    // ---- reading a document ---------------------------------------------

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
            // One mention and one fact, named after whatever the chunk said, so
            // two chunks do not collapse onto one entity.
            let who = prompt
                .lines()
                .find(|l| l.contains("alpha") || l.contains("beta") || l.contains("Okta"))
                .unwrap_or("someone")
                .trim()
                .chars()
                .take(20)
                .collect::<String>();
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

    /// A second read of an unchanged document calls no model.
    ///
    /// The spec makes this the measurement that decides whether ingest ships:
    /// if it does not hold, ingest is a one-shot import rather than something
    /// runnable on a schedule.
    #[test]
    fn re_reading_an_unchanged_document_calls_no_model() {
        let doc = "# Roles\n\nRosalind owns the Okta setup.\n";
        let mut e = doc_engine();
        let c = CountingCompleter::default();
        let emb = crate::testing::StubProvider::new(vec![]);

        let first = read_document(&mut e, "docs/team.md", doc, 100, &c, &emb).unwrap();
        assert_eq!(first.chunks_read, 1);
        let after_first = c.calls();
        assert!(after_first > 0, "the first read called no model at all");

        let second = read_document(&mut e, "docs/team.md", doc, 200, &c, &emb).unwrap();
        assert_eq!(second.chunks_read, 0);
        assert_eq!(second.chunks_skipped, 1);
        assert_eq!(
            c.calls(),
            after_first,
            "an unchanged chunk was sent to the model again"
        );
    }

    /// An edited section is read again; its unedited neighbours are not.
    #[test]
    fn editing_one_section_re_reads_only_that_section() {
        let before = "# A\n\nalpha\n\n# B\n\nbeta\n";
        let after = "# A\n\nalpha\n\n# B\n\nbeta edited\n";
        let mut e = doc_engine();
        let c = CountingCompleter::default();
        let emb = crate::testing::StubProvider::new(vec![]);

        read_document(&mut e, "docs/x.md", before, 100, &c, &emb).unwrap();
        let baseline = c.calls();

        let out = read_document(&mut e, "docs/x.md", after, 200, &c, &emb).unwrap();
        assert_eq!(out.chunks_read, 1, "both sections were re-read");
        assert_eq!(out.chunks_skipped, 1);
        assert_eq!(c.calls(), baseline + 1);
    }

    /// A section deleted from a document writes nothing.
    ///
    /// A removed sentence is not an assertion of absence: nobody said there is
    /// none, the document simply stopped saying it. Writing a tombstone here
    /// would manufacture absences at the rate documents get edited.
    #[test]
    fn deleting_a_section_asserts_nothing() {
        let before = "# A\n\nalpha\n\n# B\n\nbeta\n";
        let after = "# A\n\nalpha\n";
        let mut e = doc_engine();
        let c = CountingCompleter::default();
        let emb = crate::testing::StubProvider::new(vec![]);

        read_document(&mut e, "docs/x.md", before, 100, &c, &emb).unwrap();
        let sources_before = e.source_refs().len();
        let calls_before = c.calls();

        let out = read_document(&mut e, "docs/x.md", after, 200, &c, &emb).unwrap();
        assert_eq!(out.chunks_seen, 1, "the deleted section was still seen");
        assert_eq!(out.chunks_read, 0);
        assert_eq!(
            e.source_refs().len(),
            sources_before,
            "removing a section wrote something -- it must write nothing"
        );
        assert_eq!(c.calls(), calls_before, "a deletion called the model");
    }
}
