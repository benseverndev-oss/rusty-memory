//! An embedder that opens no socket and reads no model file.
//!
//! # Why this exists
//!
//! Every vector in this workspace came from a remote service. That is a per-call
//! cost, a network dependency, an API key, and — for a memory — every fact you
//! record leaving the machine. The [`Embedder`] port was always the seam for
//! doing otherwise; this is the first implementation to take it.
//!
//! # What it is, and what it is not
//!
//! Subword hashing. Text is lowercased, split into tokens, expanded into
//! character n-grams, and each n-gram is hashed to a coordinate whose sign
//! comes from the same hash. That is the hashing trick with subword features,
//! and it is perhaps a hundred lines of arithmetic with no table behind it.
//!
//! It has **no semantics**. "car" and "automobile" share no n-grams and land
//! orthogonal; a real embedding model puts them next to each other. What it does
//! capture is morphology and overlap: "rerank" and "reranking" share most of
//! their n-grams, and a query reusing a title's words scores against it.
//!
//! That is a bet about the corpus rather than about language. A decision log is
//! titles somebody chose to be findable, and questions asked in the same words
//! they were written in. Whether the bet pays is a measurement, not an argument,
//! and `benches/locomo/README.md` is where measurements live.
//!
//! Built first because it is the cheapest thing that could work. A distilled
//! static table — real vectors from a real model, looked up and averaged —
//! would have semantics and would cost tens of megabytes of weights in the
//! repository. There is no point paying that until this has been shown not to
//! be enough.

use rm_engine::{Embedder, EmbedderError};

/// The n-gram sizes taken from each token.
///
/// Three to five characters. Two is too common to discriminate — "re" appears
/// in half the vocabulary — and six is long enough that a word rarely matches
/// anything but itself, which is the lexical matching this exists to improve on.
const NGRAMS: std::ops::RangeInclusive<usize> = 3..=5;

/// An [`Embedder`] with no model behind it.
///
/// Deterministic: the same text gives the same vector on every machine and every
/// run, because the hash is written here rather than taken from a standard
/// library whose iteration order or seed may change.
#[derive(Clone, Debug)]
pub struct Hashed {
    dimension: usize,
}

impl Hashed {
    /// An embedder producing vectors of `dimension` components.
    ///
    /// The dimension is the caller's, not this crate's: it has to match the
    /// index the vectors are destined for, which `rmem.toml` pins and the store
    /// checks on open.
    pub fn new(dimension: usize) -> Self {
        Hashed { dimension }
    }
}

impl Embedder for Hashed {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedderError> {
        if self.dimension == 0 {
            return Err(EmbedderError(
                "a zero-dimensional embedder can represent nothing".to_string(),
            ));
        }
        let mut v = vec![0.0f32; self.dimension];
        let mut features = 0usize;

        for token in tokens(text) {
            // The whole token as well as its pieces. Two texts sharing a rare
            // word should score for the word, not only for the fragments it
            // happens to be made of.
            add(&mut v, &format!("\u{2}{token}\u{3}"));
            features += 1;
            // Boundary markers, so a prefix is distinguishable from the same
            // letters in the middle of a longer word: "^rer" is not "rer".
            let padded: Vec<char> = format!("\u{2}{token}\u{3}").chars().collect();
            for n in NGRAMS {
                if padded.len() < n {
                    break;
                }
                for w in padded.windows(n) {
                    add(&mut v, &w.iter().collect::<String>());
                    features += 1;
                }
            }
        }

        if features == 0 {
            // Text with nothing to hash -- punctuation, or empty. A zero vector
            // is refused under cosine and would be a silent hole in the index,
            // so this is an error rather than a vector nothing can match.
            return Err(EmbedderError(format!(
                "{:?} has no tokens to embed -- it is empty, or entirely punctuation",
                truncate(text)
            )));
        }

        normalise(&mut v);
        Ok(v)
    }
}

/// Lowercased runs of alphanumerics.
///
/// Deliberately not a real tokenizer. Splitting on non-alphanumerics keeps
/// "rust-toolchain.toml" as three tokens that a query saying "rust toolchain"
/// matches, where treating punctuation as part of the word would not.
fn tokens(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
}

/// Add one feature to the vector, at a coordinate and sign both drawn from it.
///
/// The sign matters. Hashing every feature positive makes each coordinate a
/// count, so two unrelated texts of similar length correlate simply for being
/// texts. A signed hash makes collisions cancel on average rather than
/// accumulate, which is the whole reason the trick works at this size.
fn add(v: &mut [f32], feature: &str) {
    let h = hash(feature);
    let i = (h % v.len() as u64) as usize;
    // A bit the coordinate does not use, so index and sign are independent.
    let sign = if (h >> 32) & 1 == 0 { 1.0 } else { -1.0 };
    v[i] += sign;
}

/// FNV-1a, 64-bit.
///
/// Written out rather than taken from `DefaultHasher`, whose output is
/// explicitly not guaranteed stable between releases. A vector that changed
/// meaning on a compiler upgrade would strand every store built with it, which
/// is the failure `rmem reindex` exists to make recoverable and this avoids
/// having in the first place.
fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// To unit length, so cosine similarity is a dot product.
fn normalise(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(40).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cos(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    fn e() -> Hashed {
        Hashed::new(256)
    }

    /// The same text gives the same vector, always.
    ///
    /// The property the whole thing rests on: a store is vectors written once
    /// and compared later, possibly by a different build. A hash whose output
    /// moved between releases would strand every store silently, which is why
    /// `hash` is written here rather than taken from `DefaultHasher`.
    #[test]
    fn embedding_is_deterministic_and_unit_length() {
        let a = e().embed("Pin the compiler").unwrap();
        let b = e().embed("Pin the compiler").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 256);
        let norm = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm was {norm}");
        // FNV-1a of a known string, checked against an independent
        // implementation rather than against whatever this one happens to
        // produce. Pinning its own output would have enshrined the first
        // version of this function, which mis-grouped the prime by one hex
        // digit and was not FNV at all.
        assert_eq!(hash("rerank"), 11_518_796_580_700_293_262);
    }

    /// Words that share their letters score together; words that share their
    /// meaning do not.
    ///
    /// Both halves are the point. The first is what makes this usable on a
    /// corpus of chosen titles; the second is the limit, and stating it in a
    /// test is what stops anyone mistaking this for a language model.
    #[test]
    fn morphology_scores_and_meaning_does_not() {
        let x = e();
        let rerank = x.embed("rerank").unwrap();
        let reranking = x.embed("reranking").unwrap();
        let reranker = x.embed("reranker").unwrap();
        assert!(
            cos(&rerank, &reranking) > 0.4,
            "rerank/reranking scored {}",
            cos(&rerank, &reranking)
        );
        assert!(
            cos(&rerank, &reranker) > 0.4,
            "rerank/reranker scored {}",
            cos(&rerank, &reranker)
        );

        // Synonyms are strangers here. Recorded, not lamented: it is what a
        // distilled table would buy and this deliberately does not.
        let car = x.embed("car").unwrap();
        let automobile = x.embed("automobile").unwrap();
        assert!(
            cos(&car, &automobile).abs() < 0.2,
            "no semantics is the deal: car/automobile scored {}",
            cos(&car, &automobile)
        );
    }

    /// A query reusing a title's words scores above one that does not.
    ///
    /// The bet this crate makes, in miniature.
    #[test]
    fn a_query_in_the_titles_own_words_wins() {
        let x = e();
        let title = x.embed("decision Rerank the recall results: choice is a cross-encoder over a deep candidate list").unwrap();
        let asked = x
            .embed("should we add a reranker to improve recall")
            .unwrap();
        let unrelated = x
            .embed("what is our policy on database migrations")
            .unwrap();
        assert!(
            cos(&title, &asked) > cos(&title, &unrelated),
            "on-topic {} should beat off-topic {}",
            cos(&title, &asked),
            cos(&title, &unrelated)
        );
    }

    /// Punctuation splits, so a query saying the words matches a title spelling
    /// them with dots and dashes.
    #[test]
    fn punctuation_is_a_boundary_not_a_character() {
        let x = e();
        let dotted = x.embed("rust-toolchain.toml").unwrap();
        let spaced = x.embed("rust toolchain toml").unwrap();
        assert!(
            cos(&dotted, &spaced) > 0.95,
            "they should be nearly the same text: {}",
            cos(&dotted, &spaced)
        );
    }

    /// Nothing to hash is refused, not returned as a zero vector.
    ///
    /// A zero vector is refused by the index under cosine, so returning one
    /// would put the failure somewhere further away from its cause. Worse, an
    /// index that accepted it would hold an assertion nothing can ever match.
    #[test]
    fn text_with_no_tokens_is_refused_where_it_happens() {
        let x = e();
        for empty in ["", "   ", "!!! ---", "..."] {
            let err = x.embed(empty).unwrap_err();
            assert!(err.0.contains("no tokens"), "for {empty:?}: {}", err.0);
        }
        // And a zero-dimensional embedder says so rather than producing one.
        assert!(Hashed::new(0).embed("anything").is_err());
    }
}
