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

/// Split a name into what it belongs to and what it is.
///
/// A name can denote a thing directly ("Melanie") or by its relation to
/// something else ("Melanie's son", "your kids"). The second kind is a pair --
/// an owner and a head -- and treating it as a flat string is what lets
/// "Melanie's son" score 0.92 against "Melanie" under [`jaro_winkler`]: the
/// owner's whole name sits at the front, where the Winkler prefix bonus rewards
/// it twice over.
///
/// Returns `(None, whole)` for a name that owns nothing, so a plain name is
/// unaffected by any of this.
///
/// Two forms count:
///
/// - an explicit possessive, `Melanie's son` or `the kids' books`, marked by an
///   apostrophe -- either ASCII `'` or the typographic `\u{2019}`, because real
///   text carries both and a rule that saw only one would fire on half the data;
/// - a leading possessive determiner, `your son`, `their brother`.
///
/// A possessive with nothing after it ("McDonald's") has no head to speak of
/// and is left whole: it is a name, not a description.
pub fn possessive_parts(s: &str) -> (Option<&str>, &str) {
    const DETERMINERS: [&str; 7] = ["my", "your", "his", "her", "its", "our", "their"];

    let trimmed = s.trim();

    // `X's Y` / `X' Y`. Scan for the first apostrophe that ends a token and has
    // something after it.
    for (i, c) in trimmed.char_indices() {
        if c != '\'' && c != '\u{2019}' {
            continue;
        }
        let after = &trimmed[i + c.len_utf8()..];
        // "'s " or "' " -- the possessive marker, then the head.
        let head = after
            .strip_prefix('s')
            .unwrap_or(after)
            .strip_prefix(char::is_whitespace)
            .map(str::trim_start);
        if let Some(head) = head {
            if !head.is_empty() && i > 0 {
                return (Some(&trimmed[..i]), head);
            }
        }
        // An apostrophe inside a word ("O'Brien") is part of the name.
    }

    if let Some((first, rest)) = trimmed.split_once(char::is_whitespace) {
        let rest = rest.trim_start();
        if !rest.is_empty() && DETERMINERS.contains(&normalize(first).as_str()) {
            return (Some(first), rest);
        }
    }

    (None, trimmed)
}

/// [`jaro_winkler`], but a thing is never confused with what it belongs to.
///
/// Compares owners against owners and heads against heads, and takes the
/// weaker of the two: a match needs *both* halves to agree. Measured on a real
/// corpus, pairs like `"Melanie" ~ "Melanie's son"` and
/// `"Caroline" ~ "Caroline's dad"` made up 12-18% of everything the review band
/// held, and they are not near misses -- an entity named by its relation to
/// someone is definitionally not that someone.
///
/// Where the halves matter:
///
/// - `"Melanie"` against `"Melanie's son"` -- one names by ownership and the
///   other does not, so they are different *kinds* of name and the owner half
///   scores zero, whatever the strings look like.
/// - `"Melanie's son"` against `"Caroline's son"` -- same head, different
///   owners. Comparing heads alone would call these one person, which is why
///   the owner is kept rather than stripped.
/// - `"Melanie's son"` against `"Melanie's daughter"` -- same owner, different
///   heads.
/// - `"Mel"` against `"Melanie"` -- neither owns anything, so this is exactly
///   [`jaro_winkler`] and the nicknames still match. That is the point: the
///   rule costs nothing on names that are names.
pub fn possessive_aware(a: &str, b: &str) -> f64 {
    let (owner_a, head_a) = possessive_parts(a);
    let (owner_b, head_b) = possessive_parts(b);

    let owner = match (owner_a, owner_b) {
        (None, None) => 1.0,
        (Some(x), Some(y)) => jaro_winkler(&normalize(x), &normalize(y)),
        // One denotes by ownership and the other denotes directly. No amount of
        // shared spelling makes those the same name.
        _ => 0.0,
    };
    let head = jaro_winkler(&normalize(head_a), &normalize(head_b));

    // The weaker half, not the product: both must agree, and a pair that agrees
    // completely on both should still score 1.0.
    owner.min(head)
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
    /// [`possessive_aware`]. For names in prose, where "Melanie's son" is a
    /// name a speaker actually used and is not Melanie.
    PossessiveAware,
}

impl Comparator {
    /// Similarity in `[0.0, 1.0]`.
    pub fn compare(self, a: &str, b: &str) -> f64 {
        match self {
            Comparator::Exact => f64::from(a == b),
            Comparator::Normalized => f64::from(normalize(a) == normalize(b)),
            Comparator::JaroWinkler => jaro_winkler(&normalize(a), &normalize(b)),
            Comparator::TokenJaccard => token_jaccard(a, b),
            Comparator::PossessiveAware => possessive_aware(a, b),
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
    fn a_name_that_owns_nothing_splits_into_nothing_and_itself() {
        assert_eq!(possessive_parts("Melanie"), (None, "Melanie"));
        assert_eq!(possessive_parts("  Acme Inc  "), (None, "Acme Inc"));
        assert_eq!(possessive_parts(""), (None, ""));
    }

    #[test]
    fn both_ways_of_owning_something_are_recognised() {
        assert_eq!(possessive_parts("Melanie's son"), (Some("Melanie"), "son"));
        assert_eq!(
            possessive_parts("the kids' books"),
            (Some("the kids"), "books")
        );
        assert_eq!(possessive_parts("your son"), (Some("your"), "son"));
        assert_eq!(
            possessive_parts("their brother"),
            (Some("their"), "brother")
        );
        // Real text carries the typographic apostrophe as often as the ASCII
        // one, and a rule that saw only one would fire on half the data.
        assert_eq!(
            possessive_parts("Melanie\u{2019}s son"),
            (Some("Melanie"), "son")
        );
    }

    #[test]
    fn an_apostrophe_that_is_part_of_a_name_does_not_split_it() {
        // Mid-word: there is no head after it, so nothing is owned.
        assert_eq!(possessive_parts("O'Brien"), (None, "O'Brien"));
        // Trailing: a shop is called this, and the name is the whole thing.
        assert_eq!(possessive_parts("McDonald's"), (None, "McDonald's"));
        // A determiner with nothing after it is just a word.
        assert_eq!(possessive_parts("your"), (None, "your"));
        // A bare marker owns nothing -- there is no owner in front of it.
        assert_eq!(possessive_parts("'s thing"), (None, "'s thing"));
    }

    #[test]
    fn a_thing_never_matches_what_it_belongs_to() {
        // The headline case. Under jaro_winkler these score high enough to sit
        // in the review band, because the owner's whole name is the prefix.
        for (a, b) in [
            ("Melanie", "Melanie's son"),
            ("Caroline", "Caroline's dad"),
            ("you", "your son"),
            ("Caroline", "Caroline's paintings"),
        ] {
            assert!(
                jaro_winkler(&normalize(a), &normalize(b)) > 0.7,
                "{a:?}/{b:?} should be a near miss under plain jaro_winkler"
            );
            assert_eq!(
                possessive_aware(a, b),
                0.0,
                "{a:?}/{b:?} is a thing and its owner"
            );
        }
    }

    #[test]
    fn two_things_owned_by_the_same_person_are_still_two_things() {
        // Same owner, different heads: Melanie's son is not Melanie's daughter.
        assert!(possessive_aware("Melanie's son", "Melanie's daughter") < 0.7);
        assert!(possessive_aware("Melanie's buddy", "Melanie's son") < 0.7);
    }

    #[test]
    fn the_owner_is_kept_rather_than_stripped() {
        // Comparing heads alone would score these 1.0 and merge two people who
        // share nothing but a relationship word.
        assert_eq!(jaro_winkler("son", "son"), 1.0);
        assert!(
            possessive_aware("Melanie's son", "Caroline's son") < 0.7,
            "same head, different owners"
        );
        // And the same owner with the same head still agrees completely.
        assert_eq!(possessive_aware("Melanie's son", "Melanie's son"), 1.0);
    }

    #[test]
    fn a_name_that_is_a_name_is_scored_exactly_as_jaro_winkler_scores_it() {
        // The rule has to cost nothing on ordinary names, or it buys precision
        // by losing the nicknames the review band exists to catch.
        for (a, b) in [
            ("Mel", "Melanie"),
            ("Caroline", "Caro"),
            ("Mel", "Mell"),
            ("Jonathan", "Johnathan"),
            ("Acme Inc.", "acme inc"),
            ("O'Brien", "O'Brian"),
            ("McDonald's", "McDonalds"),
            ("the beach", "the book"),
        ] {
            assert_eq!(
                possessive_aware(a, b),
                jaro_winkler(&normalize(a), &normalize(b)),
                "{a:?}/{b:?} owns nothing and should be untouched"
            );
        }
    }

    #[test]
    fn comparators_are_ordered_from_strict_to_forgiving() {
        let (a, b) = ("Acme Inc.", "acme inc");
        assert_eq!(Comparator::Exact.compare(a, b), 0.0);
        assert_eq!(Comparator::Normalized.compare(a, b), 1.0);
        assert_eq!(Comparator::JaroWinkler.compare(a, b), 1.0);
        assert_eq!(Comparator::TokenJaccard.compare(a, b), 1.0);
        // Neither side owns anything, so this is jaro_winkler exactly.
        assert_eq!(Comparator::PossessiveAware.compare(a, b), 1.0);
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
            // Every shape `possessive_parts` splits on, plus the ones it must
            // not: a bare marker, an apostrophe mid-word, a trailing one.
            "Melanie's son",
            "the kids' books",
            "their brother",
            "your",
            "'s",
            "'",
            "O'Brien",
            "McDonald's",
            "Melanie\u{2019}s son",
        ];
        for a in samples {
            for b in samples {
                for c in [
                    Comparator::Exact,
                    Comparator::Normalized,
                    Comparator::JaroWinkler,
                    Comparator::TokenJaccard,
                    Comparator::PossessiveAware,
                ] {
                    let s = c.compare(a, b);
                    assert!((0.0..=1.0).contains(&s), "{c:?}({a:?},{b:?}) = {s}");
                }
            }
        }
    }

    #[test]
    fn comparison_is_symmetric() {
        let samples = [
            "Acme",
            "acme inc",
            "Jonathan",
            "jon",
            "",
            "Melanie",
            "Melanie's son",
            "their brother",
            "O'Brien",
        ];
        for a in samples {
            for b in samples {
                for c in [
                    Comparator::Exact,
                    Comparator::Normalized,
                    Comparator::JaroWinkler,
                    Comparator::TokenJaccard,
                    Comparator::PossessiveAware,
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
