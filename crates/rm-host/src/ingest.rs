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
}
