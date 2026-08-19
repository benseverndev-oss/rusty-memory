//! Probabilistic entity resolution for agent memory.
//!
//! Deciding whether two memories are about the same thing. Embedding similarity
//! is the usual answer and it is a weak one: it collapses "Acme Corp" and "Acme
//! Corporation" together with "Acme's competitor", gives no account of *why*,
//! and offers no principled place to put the cases it cannot call.
//!
//! This is the Fellegi–Sunter model instead — the standard frame for record
//! linkage since 1969, and what Golden Suite's `score-core` trains with EM.
//! Each field contributes evidence in log-odds, the evidence sums, and the sum
//! is compared against thresholds.
//!
//! # The review band is the point
//!
//! Fellegi–Sunter produces *three* regions, not two: match, non-match, and a
//! middle band the original paper sends to human clerical review. Most
//! reimplementations quietly drop the middle by picking one threshold.
//!
//! Keeping it is the whole reason this crate reads the way it does. An agent
//! that merges two people because they scored 0.6 has corrupted its memory
//! permanently and silently. An agent that says "I know a Ben Severn and a B.
//! Severn — same person?" has done its job. [`Decision::Review`] pairs are
//! never merged; they are handed back for someone to answer.
//!
//! That is the same discipline as `rm_survivor`'s refusals and `rm_store`'s
//! distinction between absent and unknown: where the data cannot answer, say
//! so instead of guessing plausibly.
//!
//! # Not yet here
//!
//! Phonetic comparison (Golden Suite's `goldenphonetic-core` is 7,688 lines of
//! it) and EM-trained weights. Weights are caller-supplied for now, and the
//! module documents what they mean so they can be set deliberately rather than
//! by feel.

pub mod compare;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub use compare::{jaro, jaro_winkler, normalize, token_jaccard, Comparator};

/// A record to be resolved: a bag of named fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub fields: BTreeMap<String, String>,
}

impl Record {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style field setter.
    pub fn with(mut self, field: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(field.into(), value.into());
        self
    }

    pub fn get(&self, field: &str) -> Option<&str> {
        self.fields.get(field).map(String::as_str)
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for Record {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Record {
            fields: iter
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }
}

/// A ruleset that could not be built, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid resolution ruleset: {}", self.0)
    }
}

impl std::error::Error for ConfigError {}

/// How much one field's agreement is worth as evidence.
///
/// `m` and `u` are the Fellegi–Sunter parameters:
///
/// - `m` — probability the field agrees **given the records are the same
///   entity**. High when the field is recorded consistently; lower for one
///   people mistype or abbreviate.
/// - `u` — probability the field agrees **by chance given they are different
///   entities**. Essentially the field's commonness: a national insurance
///   number has a minuscule `u`, a first name a large one, "country: UK" an
///   enormous one.
///
/// The ratio is what matters. Agreement on a rare field is strong evidence
/// because `m/u` is large; agreement on a common one barely moves the score.
/// This is why the model beats a hand-tuned weighted average — it makes you
/// state *why* a field is informative rather than how much you like it.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldRule {
    pub field: String,
    pub comparator: Comparator,
    pub m: f64,
    pub u: f64,
}

impl FieldRule {
    pub fn new(field: impl Into<String>, comparator: Comparator, m: f64, u: f64) -> Self {
        FieldRule {
            field: field.into(),
            comparator,
            m,
            u,
        }
    }

    /// Evidence, in bits, for a match when the field agrees fully.
    pub fn agreement_weight(&self) -> f64 {
        (self.m / self.u).log2()
    }

    /// Evidence, in bits, when the field disagrees fully. Negative for any
    /// sane rule: disagreement argues against a match.
    pub fn disagreement_weight(&self) -> f64 {
        ((1.0 - self.m) / (1.0 - self.u)).log2()
    }

    fn validate(&self) -> Result<(), ConfigError> {
        for (name, p) in [("m", self.m), ("u", self.u)] {
            if !(p > 0.0 && p < 1.0) {
                return Err(ConfigError(format!(
                    "field {:?}: {name} = {p}, but it is a probability and must be strictly \
                     between 0 and 1 (0 or 1 would make one field's agreement decide every \
                     comparison on its own, whatever the other evidence says)",
                    self.field
                )));
            }
        }
        if self.m <= self.u {
            return Err(ConfigError(format!(
                "field {:?}: m = {} is not greater than u = {}, so agreement on this field \
                 would be evidence *against* a match. Either the values are swapped, or the \
                 field does not discriminate and should be left out.",
                self.field, self.m, self.u
            )));
        }
        Ok(())
    }
}

/// How to generate candidate pairs without comparing everything to everything.
///
/// Resolution is quadratic in the number of records, which is fine for a
/// session and ruinous for a memory store that has been running for a year.
/// Blocking trades recall for tractability: only records sharing a key are ever
/// compared, so a badly chosen key silently loses true matches. Prefer several
/// cheap keys over one clever one — a pair is compared if it shares *any*.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockingKey {
    /// Share the whole normalised value of a field.
    Exact(String),
    /// Share the first `n` characters of the normalised value. Catches typos
    /// past the prefix; misses typos inside it.
    Prefix(String, usize),
    /// Share any single token of the normalised value. The most forgiving, and
    /// the most expensive on fields with many tokens.
    Token(String),
}

impl BlockingKey {
    /// The blocking keys this rule derives from one record.
    ///
    /// Public so a caller maintaining its own incremental index can key a record
    /// as it arrives. [`Ruleset::candidate_pairs`] rebuilds every block from
    /// scratch, which is right for a batch and quadratic for a store that writes
    /// one record at a time. The alternative — callers reimplementing the key
    /// format — guarantees the two drift apart silently, and a blocking key that
    /// disagrees with the one used at query time loses true matches without
    /// erroring.
    pub fn keys_for(&self, record: &Record) -> Vec<String> {
        match self {
            BlockingKey::Exact(f) => record
                .get(f)
                .map(|v| vec![format!("{f}={}", normalize(v))])
                .unwrap_or_default(),
            BlockingKey::Prefix(f, n) => record
                .get(f)
                .map(|v| {
                    let norm = normalize(v);
                    let prefix: String = norm.chars().take(*n).collect();
                    vec![format!("{f}~{prefix}")]
                })
                .unwrap_or_default(),
            BlockingKey::Token(f) => record
                .get(f)
                .map(|v| {
                    normalize(v)
                        .split_whitespace()
                        .map(|t| format!("{f}#{t}"))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// What the model concluded about one pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Enough evidence to merge.
    Match,
    /// In the middle band. **Never merged.** Someone has to answer this.
    Review,
    /// Enough evidence to keep apart.
    NonMatch,
}

/// One scored comparison.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScoredPair {
    /// Index into the input slice. Always less than `b`.
    pub a: usize,
    pub b: usize,
    /// Total evidence in bits. Positive favours a match.
    pub score: f64,
    pub decision: Decision,
}

/// A complete configuration: what to compare, how to block, where the
/// thresholds sit.
#[derive(Clone, Debug, PartialEq)]
pub struct Ruleset {
    rules: Vec<FieldRule>,
    blocking: Vec<BlockingKey>,
    match_at: f64,
    review_at: f64,
}

impl Ruleset {
    /// Build and validate.
    ///
    /// `match_at` and `review_at` are in bits of evidence. A score of 0 means
    /// the evidence is balanced, so `review_at` below 0 asks to review pairs the
    /// model considers more likely different than same.
    ///
    /// An empty `blocking` list compares every pair — correct, and quadratic.
    /// Fine for a handful of records, not for a store.
    pub fn new(
        rules: Vec<FieldRule>,
        blocking: Vec<BlockingKey>,
        review_at: f64,
        match_at: f64,
    ) -> Result<Self, ConfigError> {
        if rules.is_empty() {
            return Err(ConfigError(
                "no field rules, so every pair scores 0 and the thresholds decide everything \
                 uniformly. Add at least one field to compare."
                    .to_string(),
            ));
        }
        for rule in &rules {
            rule.validate()?;
        }
        if !(review_at.is_finite() && match_at.is_finite()) {
            return Err(ConfigError(
                "thresholds must be finite numbers of bits".to_string(),
            ));
        }
        if review_at > match_at {
            return Err(ConfigError(format!(
                "review_at ({review_at}) is above match_at ({match_at}), which would leave \
                 no review band and make some matches unreachable. review_at must be at or \
                 below match_at."
            )));
        }
        Ok(Ruleset {
            rules,
            blocking,
            match_at,
            review_at,
        })
    }

    /// Total evidence, in bits, that two records are the same entity.
    ///
    /// Each rule contributes its agreement weight scaled by the comparator's
    /// similarity, interpolated toward the disagreement weight as similarity
    /// falls. A field missing from **either** record contributes exactly
    /// nothing: silence is not disagreement, and penalising it would rank a
    /// sparsely-recorded true match below a densely-recorded false one.
    pub fn score(&self, a: &Record, b: &Record) -> f64 {
        self.rules
            .iter()
            .filter_map(|rule| {
                let (va, vb) = (a.get(&rule.field)?, b.get(&rule.field)?);
                let s = rule.comparator.compare(va, vb);
                Some(s * rule.agreement_weight() + (1.0 - s) * rule.disagreement_weight())
            })
            .sum()
    }

    /// The blocking rules, for callers maintaining their own index.
    pub fn blocking(&self) -> &[BlockingKey] {
        &self.blocking
    }

    /// Which band a score falls in.
    pub fn decide(&self, score: f64) -> Decision {
        if score >= self.match_at {
            Decision::Match
        } else if score >= self.review_at {
            Decision::Review
        } else {
            Decision::NonMatch
        }
    }

    /// Candidate pairs `(a, b)` with `a < b`, deduplicated across blocking keys.
    ///
    /// With no blocking keys this is every pair.
    pub fn candidate_pairs(&self, records: &[Record]) -> Vec<(usize, usize)> {
        if self.blocking.is_empty() {
            return (0..records.len())
                .flat_map(|i| ((i + 1)..records.len()).map(move |j| (i, j)))
                .collect();
        }

        let mut blocks: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, record) in records.iter().enumerate() {
            for key in &self.blocking {
                for k in key.keys_for(record) {
                    blocks.entry(k).or_default().push(i);
                }
            }
        }

        let mut pairs: BTreeSet<(usize, usize)> = BTreeSet::new();
        for members in blocks.values() {
            for (x, &i) in members.iter().enumerate() {
                for &j in &members[x + 1..] {
                    pairs.insert((i.min(j), i.max(j)));
                }
            }
        }
        pairs.into_iter().collect()
    }
}

/// The result of resolving a set of records.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolution {
    /// Entities: each is a sorted list of record indices, and the clusters are
    /// ordered by their lowest member. Every record appears in exactly one,
    /// including records that matched nothing.
    pub clusters: Vec<Vec<usize>>,
    /// Pairs the model could not call, that were **not** already united by other
    /// evidence. These are questions for a human, not decisions.
    pub review: Vec<ScoredPair>,
}

impl Resolution {
    /// The cluster containing `record`, if any.
    pub fn cluster_of(&self, record: usize) -> Option<&[usize]> {
        self.clusters
            .iter()
            .find(|c| c.contains(&record))
            .map(|c| c.as_slice())
    }

    /// Whether two records were resolved to the same entity.
    pub fn same_entity(&self, a: usize, b: usize) -> bool {
        self.cluster_of(a).is_some_and(|c| c.contains(&b))
    }
}

/// Resolve `records` into entities.
///
/// Only [`Decision::Match`] pairs merge. Merging is transitive — if A matches B
/// and B matches C then all three are one entity, even where A and C were never
/// compared, which is what makes blocking safe to be aggressive with.
///
/// Transitivity is also this model's sharpest edge: one wrong `Match` merges two
/// whole clusters, and nothing downstream can tell. That is the argument for
/// setting `match_at` high and letting the review band be wide, rather than the
/// reverse.
pub fn resolve(records: &[Record], ruleset: &Ruleset) -> Resolution {
    let mut uf = UnionFind::new(records.len());
    let mut review: Vec<ScoredPair> = Vec::new();

    for (a, b) in ruleset.candidate_pairs(records) {
        let score = ruleset.score(&records[a], &records[b]);
        let decision = ruleset.decide(score);
        match decision {
            Decision::Match => uf.union(a, b),
            Decision::Review => review.push(ScoredPair {
                a,
                b,
                score,
                decision,
            }),
            Decision::NonMatch => {}
        }
    }

    let mut by_root: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..records.len() {
        by_root.entry(uf.find(i)).or_default().push(i);
    }
    let mut clusters: Vec<Vec<usize>> = by_root.into_values().collect();
    for c in &mut clusters {
        c.sort_unstable();
    }
    clusters.sort_by_key(|c| c[0]);

    // A pair already united by other evidence is no longer an open question:
    // asking about it would be asking someone to confirm what transitivity has
    // already settled.
    review.retain(|p| uf.find(p.a) != uf.find(p.b));
    review.sort_by_key(|p| (p.a, p.b));

    Resolution { clusters, review }
}

/// Union-find with path halving and union by size.
struct UnionFind {
    parent: Vec<usize>,
    size: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind {
            parent: (0..n).collect(),
            size: vec![1; n],
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (mut ra, mut rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        if self.size[ra] < self.size[rb] {
            std::mem::swap(&mut ra, &mut rb);
        }
        self.parent[rb] = ra;
        self.size[ra] += self.size[rb];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(name: &str, city: &str) -> Record {
        Record::new().with("name", name).with("city", city)
    }

    /// A deliberately ordinary ruleset: names are typo-prone but discriminating,
    /// cities agree often by chance.
    fn ruleset() -> Ruleset {
        Ruleset::new(
            vec![
                FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01),
                FieldRule::new("city", Comparator::Normalized, 0.9, 0.1),
            ],
            vec![BlockingKey::Prefix("name".into(), 2)],
            2.0,
            8.0,
        )
        .unwrap()
    }

    // ---- the model --------------------------------------------------------

    #[test]
    fn agreement_on_a_rare_field_outweighs_agreement_on_a_common_one() {
        let rare = FieldRule::new("nino", Comparator::Exact, 0.9, 0.000_01);
        let common = FieldRule::new("country", Comparator::Exact, 0.9, 0.9);
        assert!(rare.agreement_weight() > common.agreement_weight() * 10.0);
    }

    #[test]
    fn disagreement_argues_against_a_match() {
        let rule = FieldRule::new("name", Comparator::Exact, 0.9, 0.01);
        assert!(rule.agreement_weight() > 0.0);
        assert!(rule.disagreement_weight() < 0.0);
    }

    #[test]
    fn a_missing_field_is_not_a_disagreement() {
        let rs = ruleset();
        let full = person("Ben Severn", "London");
        let sparse = Record::new().with("name", "Ben Severn");
        // Same name, city unknown on one side: the score must be the name's
        // agreement alone, not name-agreement plus a city penalty.
        let with_city = rs.score(&full, &person("Ben Severn", "London"));
        let without = rs.score(&full, &sparse);
        assert!(without < with_city);
        assert!(
            without > rs.score(&full, &person("Ben Severn", "Tokyo")),
            "silence must beat contradiction"
        );
    }

    #[test]
    fn scoring_is_symmetric() {
        let rs = ruleset();
        let (a, b) = (
            person("Ben Severn", "London"),
            person("B. Severn", "london"),
        );
        assert!((rs.score(&a, &b) - rs.score(&b, &a)).abs() < 1e-12);
    }

    // ---- the review band --------------------------------------------------

    #[test]
    fn a_middling_pair_is_reviewed_not_merged() {
        // The case the whole crate exists for: enough signal to notice, not
        // enough to act. Two Severns in London with different given names share
        // one token of three, which is genuinely *negative* evidence -- and yet
        // nothing like the confident non-match of two unrelated names. The band
        // sits below zero on purpose: "probably not, but worth asking" is a real
        // state, and it is the one an agent must not silently resolve.
        let records = vec![
            person("Jon Severn", "London"),
            person("Ann Severn", "London"),
        ];
        let rs = Ruleset::new(
            vec![FieldRule::new("name", Comparator::TokenJaccard, 0.9, 0.05)],
            vec![],
            -2.0,
            4.0,
        )
        .unwrap();
        let score = rs.score(&records[0], &records[1]);
        assert!(
            (-2.0..0.0).contains(&score),
            "expected weak negative evidence, got {score}"
        );

        let out = resolve(&records, &rs);
        assert_eq!(out.clusters, vec![vec![0], vec![1]], "must not merge");
        assert_eq!(out.review.len(), 1);
        assert_eq!(out.review[0].decision, Decision::Review);

        // An unrelated name is not even worth asking about: the band has a
        // floor, or every pair becomes a question.
        let far = vec![records[0].clone(), person("Zoe Quinn", "Tokyo")];
        assert!(resolve(&far, &rs).review.is_empty());
    }

    #[test]
    fn a_reviewed_pair_settled_by_transitivity_is_dropped_from_review() {
        // 0 and 2 are only middling to each other, but both match 1 outright,
        // so the question is already answered.
        let records = vec![
            person("Benjamin Severn", "London"),
            person("Benjamin Severn", "London"),
            person("Benjamn Severn", "London"),
        ];
        let rs = Ruleset::new(
            vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
            vec![],
            0.0,
            6.0,
        )
        .unwrap();
        let out = resolve(&records, &rs);
        assert_eq!(out.clusters, vec![vec![0, 1, 2]]);
        assert!(out.review.is_empty(), "{:?}", out.review);
    }

    #[test]
    fn review_pairs_are_reported_deterministically() {
        let records = vec![
            person("Jon Severn", "London"),
            person("Ann Severn", "London"),
            person("Eve Severn", "London"),
        ];
        let rs = Ruleset::new(
            vec![FieldRule::new("name", Comparator::TokenJaccard, 0.9, 0.05)],
            vec![],
            -2.0,
            4.0,
        )
        .unwrap();
        let out = resolve(&records, &rs);
        let ids: Vec<(usize, usize)> = out.review.iter().map(|p| (p.a, p.b)).collect();
        assert_eq!(ids, vec![(0, 1), (0, 2), (1, 2)]);
    }

    // ---- clustering -------------------------------------------------------

    #[test]
    fn matching_is_transitive_across_pairs_never_compared() {
        let records = vec![
            person("Benjamin Severn", "London"),
            person("Benjamin Severn", "London"),
            person("Benjamin Severn", "London"),
        ];
        let out = resolve(&records, &ruleset());
        assert_eq!(out.clusters, vec![vec![0, 1, 2]]);
        assert!(out.same_entity(0, 2));
    }

    #[test]
    fn every_record_lands_in_exactly_one_cluster() {
        let records = vec![
            person("Ben Severn", "London"),
            person("Ben Severn", "London"),
            person("Zoe Quinn", "Tokyo"),
        ];
        let out = resolve(&records, &ruleset());
        let mut seen: Vec<usize> = out.clusters.concat();
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2]);
        assert!(!out.same_entity(0, 2));
        assert_eq!(out.cluster_of(2), Some(&[2usize][..]));
    }

    #[test]
    fn resolving_nothing_yields_nothing() {
        let out = resolve(&[], &ruleset());
        assert!(out.clusters.is_empty());
        assert!(out.review.is_empty());
    }

    #[test]
    fn a_lone_record_is_its_own_entity() {
        let out = resolve(&[person("Ben Severn", "London")], &ruleset());
        assert_eq!(out.clusters, vec![vec![0]]);
    }

    // ---- blocking ---------------------------------------------------------

    #[test]
    fn blocking_compares_only_records_sharing_a_key() {
        let records = vec![
            person("Ben Severn", "London"),
            person("Ben Severn", "London"),
            person("Zoe Quinn", "Tokyo"),
        ];
        let pairs = ruleset().candidate_pairs(&records);
        assert_eq!(pairs, vec![(0, 1)], "Zoe shares no name prefix");
    }

    #[test]
    fn no_blocking_keys_compares_every_pair() {
        let rs = Ruleset::new(
            vec![FieldRule::new("name", Comparator::Exact, 0.9, 0.01)],
            vec![],
            0.0,
            1.0,
        )
        .unwrap();
        let records: Vec<Record> = (0..4).map(|i| person(&format!("p{i}"), "x")).collect();
        assert_eq!(rs.candidate_pairs(&records).len(), 6);
    }

    #[test]
    fn a_pair_sharing_several_keys_is_compared_once() {
        let rs = Ruleset::new(
            vec![FieldRule::new("name", Comparator::Exact, 0.9, 0.01)],
            vec![
                BlockingKey::Exact("city".into()),
                BlockingKey::Token("city".into()),
                BlockingKey::Prefix("city".into(), 2),
            ],
            0.0,
            1.0,
        )
        .unwrap();
        let records = vec![person("a", "London"), person("b", "London")];
        assert_eq!(rs.candidate_pairs(&records), vec![(0, 1)]);
    }

    #[test]
    fn a_record_missing_the_blocked_field_is_compared_to_nothing() {
        let rs = ruleset();
        let records = vec![
            person("Ben Severn", "London"),
            Record::new().with("city", "London"),
        ];
        assert!(rs.candidate_pairs(&records).is_empty());
    }

    #[test]
    fn token_blocking_catches_reordered_names_that_prefix_blocking_misses() {
        let records = vec![
            person("Ben Severn", "London"),
            person("Severn Ben", "London"),
        ];
        let prefix = Ruleset::new(
            vec![FieldRule::new("name", Comparator::TokenJaccard, 0.9, 0.01)],
            vec![BlockingKey::Prefix("name".into(), 3)],
            0.0,
            1.0,
        )
        .unwrap();
        let token = Ruleset::new(
            vec![FieldRule::new("name", Comparator::TokenJaccard, 0.9, 0.01)],
            vec![BlockingKey::Token("name".into())],
            0.0,
            1.0,
        )
        .unwrap();
        assert!(prefix.candidate_pairs(&records).is_empty());
        assert_eq!(token.candidate_pairs(&records), vec![(0, 1)]);
    }

    // ---- configuration is validated, not trusted --------------------------

    #[test]
    fn a_field_whose_agreement_argues_against_matching_is_rejected() {
        let err = Ruleset::new(
            vec![FieldRule::new("name", Comparator::Exact, 0.1, 0.9)],
            vec![],
            0.0,
            1.0,
        )
        .unwrap_err();
        assert!(err.0.contains("evidence *against* a match"), "{}", err.0);
    }

    #[test]
    fn certainty_in_a_probability_is_rejected() {
        for (m, u) in [(1.0, 0.01), (0.9, 0.0), (0.0, 0.5), (0.9, 1.0)] {
            let err = Ruleset::new(
                vec![FieldRule::new("name", Comparator::Exact, m, u)],
                vec![],
                0.0,
                1.0,
            )
            .unwrap_err();
            assert!(
                err.0.contains("strictly between 0 and 1"),
                "m={m} u={u}: {}",
                err.0
            );
        }
    }

    #[test]
    fn inverted_thresholds_are_rejected() {
        let err = Ruleset::new(
            vec![FieldRule::new("name", Comparator::Exact, 0.9, 0.01)],
            vec![],
            9.0,
            1.0,
        )
        .unwrap_err();
        assert!(err.0.contains("no review band"), "{}", err.0);
    }

    #[test]
    fn a_ruleset_with_no_fields_is_rejected() {
        let err = Ruleset::new(vec![], vec![], 0.0, 1.0).unwrap_err();
        assert!(err.0.contains("no field rules"), "{}", err.0);
    }

    #[test]
    fn equal_thresholds_are_allowed_and_leave_no_review_band() {
        let rs = Ruleset::new(
            vec![FieldRule::new("name", Comparator::Exact, 0.9, 0.01)],
            vec![],
            3.0,
            3.0,
        )
        .unwrap();
        // Opting out of review is a decision the caller may make explicitly.
        assert_eq!(rs.decide(3.0), Decision::Match);
        assert_eq!(rs.decide(2.999), Decision::NonMatch);
    }

    #[test]
    fn thresholds_are_inclusive_at_their_lower_edge() {
        let rs = ruleset();
        assert_eq!(rs.decide(8.0), Decision::Match);
        assert_eq!(rs.decide(7.999), Decision::Review);
        assert_eq!(rs.decide(2.0), Decision::Review);
        assert_eq!(rs.decide(1.999), Decision::NonMatch);
    }

    #[test]
    fn blocking_keys_are_derivable_by_a_caller_maintaining_its_own_index() {
        // An incremental store cannot call candidate_pairs on every write without
        // rebuilding every block, so it needs the keys for one record at a time.
        let key = BlockingKey::Exact("city".to_string());
        let record = Record::new().with("city", "  Bristol ");
        assert_eq!(key.keys_for(&record), vec!["city=bristol".to_string()]);

        let ruleset = ruleset();
        assert!(!ruleset.blocking().is_empty());
    }

    #[test]
    fn a_record_round_trips_through_json() {
        // The engine persists its identity records, so these have to survive a
        // snapshot without the engine reaching inside them to do it by hand.
        let r = Record::new().with("name", "Ben Severn");
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), r);
    }
}
