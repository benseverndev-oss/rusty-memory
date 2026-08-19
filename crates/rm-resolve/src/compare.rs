//! Field comparators: how similar are two strings, on a 0.0–1.0 scale.
//!
//! Every comparator returns 1.0 for identical input and 0.0 for input with
//! nothing in common, so scoring can interpolate between the agreement and
//! disagreement weights without knowing which comparator produced the number.

/// Lowercase, drop anything that is not alphanumeric or whitespace, and
/// collapse runs of whitespace.
///
/// Deliberately *not* Unicode-normalising or transliterating: folding "José" to
/// "Jose" makes two people the same person, and a memory store that quietly
/// merges them has done more damage than one that keeps them apart. Accents
/// survive; punctuation and casing do not.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            pending_space = !out.is_empty();
        } else if c.is_alphanumeric() {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.extend(c.to_lowercase());
        }
        // punctuation: dropped without joining the words around it
    }
    out
}

/// Jaro similarity over Unicode scalar values.
///
/// Chars rather than bytes: a byte-wise implementation gives different answers
/// for the same text depending on how many multi-byte characters it contains,
/// and would slice mid-codepoint on the matching window.
pub fn jaro(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    // Standard window: characters further apart than this are not "the same
    // character transposed", they are different characters.
    let window = (a.len().max(b.len()) / 2).saturating_sub(1);

    let mut a_matched = vec![false; a.len()];
    let mut b_matched = vec![false; b.len()];
    let mut matches = 0usize;

    for (i, ca) in a.iter().enumerate() {
        let lo = i.saturating_sub(window);
        let hi = (i + window + 1).min(b.len());
        for j in lo..hi {
            if !b_matched[j] && b[j] == *ca {
                a_matched[i] = true;
                b_matched[j] = true;
                matches += 1;
                break;
            }
        }
    }

    if matches == 0 {
        return 0.0;
    }

    // Transpositions: matched characters that appear in a different order.
    let mut transpositions = 0usize;
    let mut k = 0usize;
    for (i, ca) in a.iter().enumerate() {
        if !a_matched[i] {
            continue;
        }
        while !b_matched[k] {
            k += 1;
        }
        if *ca != b[k] {
            transpositions += 1;
        }
        k += 1;
    }
    let transpositions = transpositions as f64 / 2.0;

    let m = matches as f64;
    (m / a.len() as f64 + m / b.len() as f64 + (m - transpositions) / m) / 3.0
}

/// Jaro–Winkler: Jaro, boosted for a shared prefix of up to four characters.
///
/// The boost encodes the observation that people mistype the ends of names far
/// more often than the beginnings.
pub fn jaro_winkler(a: &str, b: &str) -> f64 {
    let base = jaro(a, b);
    // Only boost strings that are already plausibly the same; boosting a poor
    // match on a shared prefix promotes "Jonathan"/"Jose" on the strength of
    // "Jo".
    if base < 0.7 {
        return base;
    }
    let prefix = a
        .chars()
        .zip(b.chars())
        .take(4)
        .take_while(|(x, y)| x == y)
        .count() as f64;
    base + prefix * 0.1 * (1.0 - base)
}

/// Jaccard similarity over whitespace-separated tokens of the normalised input.
///
/// Order-insensitive, so "Severn, Ben" and "Ben Severn" agree completely. It
/// penalises length differences by design: "Acme" against "Acme Corporation
/// Holdings" scores 1/3, which is the honest answer — they share a token and
/// differ in two, and whether that means "same company" is a question for the
/// weights, not the comparator.
pub fn token_jaccard(a: &str, b: &str) -> f64 {
    let (na, nb) = (normalize(a), normalize(b));
    let ta: Vec<&str> = na.split_whitespace().collect();
    let tb: Vec<&str> = nb.split_whitespace().collect();
    if ta.is_empty() && tb.is_empty() {
        return 1.0;
    }
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let intersection = ta.iter().filter(|t| tb.contains(t)).count();
    let union = ta.len() + tb.len() - intersection;
    intersection as f64 / union as f64
}

/// How to compare one field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparator {
    /// Byte-for-byte equality. For identifiers, where "close" means "wrong".
    Exact,
    /// Equality after [`normalize`]. For values whose casing and punctuation
    /// carry no meaning.
    Normalized,
    /// [`jaro_winkler`] over the normalised values. For names and typos.
    JaroWinkler,
    /// [`token_jaccard`]. For multi-word values whose word order varies.
    TokenJaccard,
}

impl Comparator {
    /// Similarity in `[0.0, 1.0]`.
    pub fn compare(self, a: &str, b: &str) -> f64 {
        match self {
            Comparator::Exact => f64::from(a == b),
            Comparator::Normalized => f64::from(normalize(a) == normalize(b)),
            Comparator::JaroWinkler => jaro_winkler(&normalize(a), &normalize(b)),
            Comparator::TokenJaccard => token_jaccard(a, b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn jaro_matches_the_textbook_values() {
        assert!(
            close(jaro("martha", "marhta"), 0.944_44),
            "{}",
            jaro("martha", "marhta")
        );
        assert!(
            close(jaro("dwayne", "duane"), 0.822_22),
            "{}",
            jaro("dwayne", "duane")
        );
        assert!(
            close(jaro("dixon", "dicksonx"), 0.766_67),
            "{}",
            jaro("dixon", "dicksonx")
        );
    }

    #[test]
    fn jaro_winkler_matches_the_textbook_values() {
        assert!(close(jaro_winkler("martha", "marhta"), 0.961_11));
        assert!(close(jaro_winkler("dwayne", "duane"), 0.84));
        assert!(close(jaro_winkler("dixon", "dicksonx"), 0.813_33));
    }

    #[test]
    fn identical_strings_score_one_and_disjoint_ones_score_zero() {
        assert_eq!(jaro("abc", "abc"), 1.0);
        assert_eq!(jaro_winkler("abc", "abc"), 1.0);
        assert_eq!(jaro("abc", "xyz"), 0.0);
        assert_eq!(token_jaccard("abc", "xyz"), 0.0);
    }

    #[test]
    fn empty_input_does_not_panic_or_divide_by_zero() {
        assert_eq!(jaro("", ""), 1.0);
        assert_eq!(jaro("", "abc"), 0.0);
        assert_eq!(jaro_winkler("", ""), 1.0);
        assert_eq!(token_jaccard("", ""), 1.0);
        assert_eq!(token_jaccard("", "abc"), 0.0);
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn a_weak_match_is_not_boosted_by_a_shared_prefix() {
        // Both begin "jo", but they are not the same name and the prefix boost
        // must not push them toward one.
        let base = jaro("jonathan", "jose");
        assert!(base < 0.7, "precondition: {base}");
        assert_eq!(jaro_winkler("jonathan", "jose"), base);
    }

    #[test]
    fn normalize_folds_case_and_punctuation_but_keeps_accents() {
        assert_eq!(normalize("  Acme,  Inc.  "), "acme inc");
        assert_eq!(normalize("O'Brien"), "obrien");
        // Folding this to "jose" would merge two different people.
        assert_eq!(normalize("José"), "josé");
        assert_ne!(normalize("José"), normalize("Jose"));
    }

    #[test]
    fn normalize_does_not_join_words_across_removed_punctuation() {
        assert_eq!(normalize("Smith,Jones"), "smithjones");
        assert_eq!(normalize("Smith, Jones"), "smith jones");
    }

    #[test]
    fn token_jaccard_ignores_word_order() {
        assert_eq!(token_jaccard("Ben Severn", "Severn, Ben"), 1.0);
    }

    #[test]
    fn token_jaccard_reports_length_difference_honestly() {
        // 1 shared token, 3 in the union.
        assert!(close(
            token_jaccard("Acme", "Acme Corporation Holdings"),
            1.0 / 3.0
        ));
    }

    #[test]
    fn comparators_are_ordered_from_strict_to_forgiving() {
        let (a, b) = ("Acme Inc.", "acme inc");
        assert_eq!(Comparator::Exact.compare(a, b), 0.0);
        assert_eq!(Comparator::Normalized.compare(a, b), 1.0);
        assert_eq!(Comparator::JaroWinkler.compare(a, b), 1.0);
        assert_eq!(Comparator::TokenJaccard.compare(a, b), 1.0);
    }

    #[test]
    fn every_comparator_stays_within_zero_and_one() {
        let samples = [
            "",
            "a",
            "Acme",
            "José García",
            "the quick brown fox",
            "x y z",
        ];
        for a in samples {
            for b in samples {
                for c in [
                    Comparator::Exact,
                    Comparator::Normalized,
                    Comparator::JaroWinkler,
                    Comparator::TokenJaccard,
                ] {
                    let s = c.compare(a, b);
                    assert!((0.0..=1.0).contains(&s), "{c:?}({a:?},{b:?}) = {s}");
                }
            }
        }
    }

    #[test]
    fn comparison_is_symmetric() {
        let samples = ["Acme", "acme inc", "Jonathan", "jon", ""];
        for a in samples {
            for b in samples {
                for c in [
                    Comparator::Exact,
                    Comparator::Normalized,
                    Comparator::JaroWinkler,
                    Comparator::TokenJaccard,
                ] {
                    assert!(
                        (c.compare(a, b) - c.compare(b, a)).abs() < 1e-12,
                        "{c:?} asymmetric on {a:?}/{b:?}"
                    );
                }
            }
        }
    }
}
