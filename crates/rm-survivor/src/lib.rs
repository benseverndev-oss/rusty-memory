//! Survivorship for agent memory: choosing what is true when the store holds
//! two answers to the same question.
//!
//! # Lineage, and the one thing that changes
//!
//! The value-only strategies here are ported from Golden Suite's
//! `survivorship-core`, tie-break for tie-break. That crate *refuses* two of its
//! strategies:
//!
//! - `source_priority` — "needs a sources list, which the Spark path does not
//!   supply -- Python raises rather than guessing, and so does this. Guessing an
//!   order would silently prefer the wrong system of record."
//! - `most_recent` — "needs a dates list, which the Spark path does not supply
//!   ... picking the first row would be an arbitrary answer wearing a
//!   deterministic hat."
//!
//! It refuses them because its call site is a Spark batch job that passes bare
//! values. **Agent memory passes both.** Every assertion arrives with a
//! [`Provenance`] naming its source and when it was observed, so the two
//! strategies that are unimplementable in a batch job are the two that matter
//! most here — a memory store's whole difficulty is that facts change and
//! sources disagree.
//!
//! And having timestamps makes a third strategy expressible that neither system
//! could state before: [`Strategy::ValidInterval`] declines to pick a winner at
//! all, and instead writes every value with a disjoint validity range. "Acme
//! until July, Globex after" is not a conflict to resolve; it is two facts with
//! different valid times. Resolving it to one winner destroys information the
//! store is capable of keeping.
//!
//! # The discipline that is *not* changing
//!
//! Upstream refuses rather than approximates, on the grounds that a survivor
//! chosen by the wrong rule is "a wrong golden record that looks right": no
//! exception, no null, just a plausible value that nothing downstream can flag.
//!
//! That reasoning is *stronger* for memory, not weaker. A wrong golden record
//! sits in a table where a human might eventually notice it. A wrong memory gets
//! silently laundered into an LLM's context as established fact and shapes every
//! later turn. So every strategy here refuses when its inputs cannot answer the
//! question, and the refusal names what was missing.

use rm_core::{Interval, Provenance, Source, Timestamp};
use serde::{Deserialize, Serialize};

/// A refusal to guess, and why.
///
/// Distinct from "no survivor" ([`Outcome::Survivor(None)`]), which is a real
/// answer: the candidates were considered and none won. A `Refused` means the
/// question could not be asked with the data supplied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "survivorship refused: {}", self.0)
    }
}

impl std::error::Error for Refused {}

/// One assertion of a field's value, with the provenance that decides how it
/// competes.
///
/// `value: None` is a missing observation, not an assertion of emptiness — the
/// source had nothing to say. Absence never contradicts presence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate<'a> {
    pub value: Option<&'a str>,
    pub provenance: &'a Provenance,
}

impl<'a> Candidate<'a> {
    pub fn new(value: Option<&'a str>, provenance: &'a Provenance) -> Self {
        Candidate { value, provenance }
    }
}

/// A value paired with the span of valid time over which it held.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub value: String,
    pub valid: Interval,
}

/// What survivorship produced.
///
/// Two shapes because the strategies genuinely answer different questions.
/// Every strategy but [`Strategy::ValidInterval`] answers "which one value
/// survives"; that one answers "what was true when".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// One winner, or `None` when the candidates yield no survivor.
    Survivor(Option<String>),
    /// A timeline of values with non-overlapping validity, oldest first. Empty
    /// when nothing was ever asserted.
    Timeline(Vec<Fact>),
}

impl Outcome {
    /// The surviving value, if this outcome names exactly one.
    ///
    /// A `Timeline` has no single survivor by construction, so this is `None`
    /// for one unless the timeline holds exactly one fact.
    pub fn survivor(&self) -> Option<&str> {
        match self {
            Outcome::Survivor(v) => v.as_deref(),
            Outcome::Timeline(facts) => match facts.as_slice() {
                [only] => Some(&only.value),
                _ => None,
            },
        }
    }

    /// The value in force at `t`.
    ///
    /// For a `Survivor` this ignores `t` — a single survivor is the answer at
    /// every instant, because nothing in the outcome says otherwise.
    pub fn as_of(&self, t: Timestamp) -> Option<&str> {
        match self {
            Outcome::Survivor(v) => v.as_deref(),
            Outcome::Timeline(facts) => facts
                .iter()
                .find(|f| f.valid.contains(t))
                .map(|f| f.value.as_str()),
        }
    }
}

/// How to resolve competing assertions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// Longest value wins; ties go to the first seen.
    MostComplete,
    /// Alias of [`Strategy::MostComplete`], kept because the two names mean
    /// different things to callers even where the rule coincides.
    LongestValue,
    /// Most frequently asserted value wins; count ties go to the first seen.
    MajorityVote,
    /// Count-majority. Upstream documents this as the fallback shape of a
    /// confidence-weighted vote when no pair scores are supplied; no weighted
    /// form is implemented here, and the name is kept so the behaviour is
    /// requested explicitly rather than inherited by accident.
    ConfidenceMajority,
    /// The first non-null assertion in input order.
    FirstNonNull,
    /// The value if every non-null assertion agrees, otherwise nothing.
    /// For fields where a heuristic answer is worse than a gap.
    UnanimousOrNull,
    /// The most recently observed value.
    ///
    /// Refuses when the latest observation is a tie between different values:
    /// simultaneous contradictory assertions have no "most recent".
    MostRecent,
    /// The value from the highest-priority source that asserted one.
    ///
    /// Refuses when any asserting source is absent from the priority list —
    /// ranking an unlisted source would silently prefer the wrong system of
    /// record. Within the winning source, ties resolve by [`Strategy::MostRecent`].
    SourcePriority(Vec<Source>),
    /// Do not pick a winner. Emit each distinct value with the validity range
    /// over which it stood, inferred from observation order.
    ///
    /// Refuses when two different values share an observation timestamp: with
    /// no order between them there is no way to say which superseded which.
    ValidInterval,
}

impl Strategy {
    /// Whether this strategy needs provenance beyond the values themselves.
    ///
    /// Exposed so a host can check its inputs once, up front, rather than
    /// discovering the gap per field.
    pub fn needs_provenance(&self) -> bool {
        matches!(
            self,
            Strategy::MostRecent | Strategy::SourcePriority(_) | Strategy::ValidInterval
        )
    }
}

/// Resolve `candidates` under `strategy`.
///
/// Candidates are in the caller's insertion order, which is load-bearing:
/// count and length ties resolve to the first seen, so the caller controls
/// tie-breaks by controlling order.
pub fn merge(candidates: &[Candidate<'_>], strategy: &Strategy) -> Result<Outcome, Refused> {
    // ValidInterval answers a different question and has its own early-outs
    // (one distinct value still yields a timeline, not a bare survivor).
    if matches!(strategy, Strategy::ValidInterval) {
        return timeline(candidates).map(Outcome::Timeline);
    }

    let non_null: Vec<&Candidate<'_>> = candidates.iter().filter(|c| c.value.is_some()).collect();
    if non_null.is_empty() {
        return Ok(Outcome::Survivor(None));
    }

    // Every non-null assertion agrees -> that value, whatever the strategy.
    // Runs before any strategy so unanimity never depends on the rule chosen.
    let first = non_null[0].value.expect("filtered to non-null");
    if non_null.iter().all(|c| c.value == Some(first)) {
        return Ok(Outcome::Survivor(Some(first.to_string())));
    }

    let winner: Option<String> = match strategy {
        Strategy::MostComplete | Strategy::LongestValue => longest(&non_null),
        Strategy::MajorityVote | Strategy::ConfidenceMajority => plurality(&non_null),
        Strategy::FirstNonNull => Some(first.to_string()),
        // Unanimity was handled by the early-out above, so reaching here means
        // the sources disagree. A gap is the point of this strategy.
        Strategy::UnanimousOrNull => None,
        Strategy::MostRecent => Some(most_recent(&non_null)?),
        Strategy::SourcePriority(order) => Some(by_source_priority(&non_null, order)?),
        Strategy::ValidInterval => unreachable!("handled above"),
    };

    Ok(Outcome::Survivor(winner))
}

/// Longest value, measured in characters.
///
/// Characters, not bytes: a byte length would rank "café" (4 chars, 5 bytes)
/// above "abcd" and tie it with "abcde", making completeness depend on the
/// accents in the text. Ties take the first seen.
fn longest(non_null: &[&Candidate<'_>]) -> Option<String> {
    let max = non_null
        .iter()
        .map(|c| c.value.unwrap_or_default().chars().count())
        .max()?;
    non_null
        .iter()
        .find(|c| c.value.unwrap_or_default().chars().count() == max)
        .map(|c| c.value.unwrap_or_default().to_string())
}

/// Most frequent value, breaking count ties by first appearance.
///
/// The tally is an insertion-ordered `Vec`, not a `HashMap`: iteration order
/// *is* the tie-break rule, and a hash map would make the winner depend on hash
/// seeding — a different answer per process for the same input.
fn plurality(non_null: &[&Candidate<'_>]) -> Option<String> {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for c in non_null {
        let v = c.value.unwrap_or_default();
        match counts.iter_mut().find(|(k, _)| *k == v) {
            Some((_, n)) => *n += 1,
            None => counts.push((v, 1)),
        }
    }
    // Strict `>` keeps the first maximum. `max_by_key` would keep the last.
    counts
        .iter()
        .fold(None::<(&str, usize)>, |best, &(v, n)| match best {
            Some((_, bn)) if n <= bn => best,
            _ => Some((v, n)),
        })
        .map(|(v, _)| v.to_string())
}

/// The value from the latest observation.
///
/// Refuses on a contradictory tie at the latest timestamp. Two different values
/// observed at the same instant have no order between them, and picking either
/// would be the arbitrary answer wearing a deterministic hat that upstream
/// refuses to give.
fn most_recent(non_null: &[&Candidate<'_>]) -> Result<String, Refused> {
    let latest = non_null
        .iter()
        .map(|c| c.provenance.observed_at)
        .max()
        .expect("non-empty");

    let mut at_latest: Vec<&str> = non_null
        .iter()
        .filter(|c| c.provenance.observed_at == latest)
        .map(|c| c.value.unwrap_or_default())
        .collect();
    at_latest.dedup();

    match at_latest.as_slice() {
        [only] => Ok((*only).to_string()),
        [first, ..] if at_latest.iter().all(|v| v == first) => Ok((*first).to_string()),
        _ => Err(Refused(format!(
            "{} different values share the latest observation time ({latest}); \
             simultaneous contradictory assertions have no \"most recent\". \
             Order them, or resolve with SourcePriority or ValidInterval.",
            at_latest.len()
        ))),
    }
}

/// The value from the first source in `order` that asserted one.
///
/// Refuses when an asserting source is unlisted rather than ranking it last:
/// an unranked source is an unanswered policy question, and defaulting it to
/// lowest priority silently prefers whatever the caller did remember to list.
fn by_source_priority(non_null: &[&Candidate<'_>], order: &[Source]) -> Result<String, Refused> {
    if order.is_empty() {
        return Err(Refused(
            "SourcePriority was given an empty priority list, so it ranks nothing. \
             Supply the source order, or choose a strategy that does not need one."
                .to_string(),
        ));
    }

    let unlisted: Vec<String> = {
        let mut seen: Vec<&Source> = Vec::new();
        for c in non_null {
            let s = &c.provenance.source;
            if !order.contains(s) && !seen.contains(&s) {
                seen.push(s);
            }
        }
        seen.iter().map(|s| format!("{s:?}")).collect()
    };
    if !unlisted.is_empty() {
        return Err(Refused(format!(
            "these sources asserted a value but are absent from the priority list: {}. \
             Ranking them last would silently prefer the sources you did list; \
             add them to the order, or choose a strategy that does not rank sources.",
            unlisted.join(", ")
        )));
    }

    for source in order {
        let tier: Vec<&Candidate<'_>> = non_null
            .iter()
            .copied()
            .filter(|c| &c.provenance.source == source)
            .collect();
        if tier.is_empty() {
            continue;
        }
        // Within one source, later observations supersede earlier ones — the
        // same source revising itself is not a conflict between sources.
        return most_recent(&tier);
    }

    // Unreachable in practice: non_null is non-empty and every source is
    // listed, so some tier matched. Refuse rather than unwrap if that changes.
    Err(Refused(
        "no candidate matched any listed source, despite every source being listed. \
         This is a bug in rm-survivor; please report it."
            .to_string(),
    ))
}

/// Build a timeline of values from observation order.
///
/// Each distinct value holds from when it was observed until the next *different*
/// value was observed; the last holds open-ended. Repeated assertions of the same
/// value extend its span rather than starting a new one — re-hearing a fact is
/// not a change. A value that returns after being superseded (Acme, Globex, Acme)
/// correctly yields three spans: it was true, then not, then true again.
fn timeline(candidates: &[Candidate<'_>]) -> Result<Vec<Fact>, Refused> {
    let mut non_null: Vec<&Candidate<'_>> =
        candidates.iter().filter(|c| c.value.is_some()).collect();
    if non_null.is_empty() {
        return Ok(Vec::new());
    }

    // Stable sort: candidates sharing a timestamp keep their input order, which
    // matters for the conflict check below reporting the caller's own ordering.
    non_null.sort_by_key(|c| c.provenance.observed_at);

    for pair in non_null.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a.provenance.observed_at == b.provenance.observed_at && a.value != b.value {
            return Err(Refused(format!(
                "{:?} and {:?} were both observed at {}, so neither supersedes the other \
                 and no validity range can be assigned. Distinguish their observation \
                 times, or resolve with SourcePriority.",
                a.value.unwrap_or_default(),
                b.value.unwrap_or_default(),
                a.provenance.observed_at
            )));
        }
    }

    let mut facts: Vec<Fact> = Vec::new();
    for c in &non_null {
        let value = c.value.expect("filtered to non-null");
        if facts.last().is_some_and(|f| f.value == value) {
            continue; // same value restated: extends the open span, no new fact
        }
        facts.push(Fact {
            value: value.to_string(),
            valid: Interval::since(c.provenance.observed_at),
        });
    }

    // Close each span where the next one opens, leaving the last open-ended.
    for i in 0..facts.len().saturating_sub(1) {
        let next_start = facts[i + 1].valid.from;
        facts[i].valid.to = Some(next_start);
    }

    Ok(facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prov(source: Source, observed_at: Timestamp) -> Provenance {
        Provenance::new(source, observed_at, "test")
    }

    /// Build candidates from `(value, source, observed_at)` triples.
    macro_rules! cands {
        ($($v:expr, $s:expr, $t:expr);* $(;)?) => {{
            let provs: Vec<Provenance> = vec![$(prov($s, $t)),*];
            let vals: Vec<Option<&str>> = vec![$($v),*];
            (provs, vals)
        }};
    }

    fn build<'a>(provs: &'a [Provenance], vals: &'a [Option<&'a str>]) -> Vec<Candidate<'a>> {
        vals.iter()
            .zip(provs.iter())
            .map(|(v, p)| Candidate::new(*v, p))
            .collect()
    }

    // ---- ported behaviour: the value-only strategies -----------------------

    #[test]
    fn all_null_yields_no_survivor_for_every_value_strategy() {
        let (p, v) = cands![None, Source::UserAssertion, 1; None, Source::ToolOutput, 2];
        let c = build(&p, &v);
        for s in [
            Strategy::MostComplete,
            Strategy::LongestValue,
            Strategy::MajorityVote,
            Strategy::ConfidenceMajority,
            Strategy::FirstNonNull,
            Strategy::UnanimousOrNull,
            Strategy::MostRecent,
        ] {
            assert_eq!(merge(&c, &s).unwrap(), Outcome::Survivor(None), "{s:?}");
        }
    }

    #[test]
    fn a_single_distinct_value_wins_regardless_of_strategy() {
        let (p, v) = cands![Some("a"), Source::UserAssertion, 1; None, Source::ToolOutput, 2; Some("a"), Source::AgentInference, 3];
        let c = build(&p, &v);
        for s in [
            Strategy::MostComplete,
            Strategy::MajorityVote,
            Strategy::FirstNonNull,
            Strategy::UnanimousOrNull,
            Strategy::MostRecent,
        ] {
            assert_eq!(merge(&c, &s).unwrap().survivor(), Some("a"), "{s:?}");
        }
    }

    #[test]
    fn most_complete_takes_the_longest_then_the_first() {
        let (p, v) =
            cands![Some("ab"), Source::UserAssertion, 1; Some("abcd"), Source::ToolOutput, 2];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::MostComplete)
                .unwrap()
                .survivor(),
            Some("abcd")
        );
        let (p, v) =
            cands![Some("ab"), Source::UserAssertion, 1; Some("cd"), Source::ToolOutput, 2];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::MostComplete)
                .unwrap()
                .survivor(),
            Some("ab")
        );
    }

    #[test]
    fn length_is_counted_in_characters_not_bytes() {
        // "café" is 4 chars / 5 bytes; "abcde" is 5 chars / 5 bytes. Comparing
        // bytes would call these tied and take the first.
        let (p, v) =
            cands![Some("café"), Source::UserAssertion, 1; Some("abcde"), Source::ToolOutput, 2];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::LongestValue)
                .unwrap()
                .survivor(),
            Some("abcde")
        );
    }

    #[test]
    fn majority_vote_breaks_count_ties_by_first_seen() {
        let (p, v) = cands![Some("b"), Source::UserAssertion, 1; Some("a"), Source::ToolOutput, 2; Some("a"), Source::ToolOutput, 3];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::MajorityVote)
                .unwrap()
                .survivor(),
            Some("a")
        );
        // 1-1: insertion order decides, so "b".
        let (p, v) = cands![Some("b"), Source::UserAssertion, 1; Some("a"), Source::ToolOutput, 2];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::MajorityVote)
                .unwrap()
                .survivor(),
            Some("b")
        );
    }

    #[test]
    fn unanimous_or_null_emits_nothing_on_disagreement() {
        let (p, v) = cands![Some("a"), Source::UserAssertion, 1; Some("b"), Source::ToolOutput, 2];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::UnanimousOrNull).unwrap(),
            Outcome::Survivor(None)
        );
        // Absence is not contradiction.
        let (p, v) = cands![Some("a"), Source::UserAssertion, 1; None, Source::ToolOutput, 2; Some("a"), Source::ToolOutput, 3];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::UnanimousOrNull)
                .unwrap()
                .survivor(),
            Some("a")
        );
    }

    #[test]
    fn first_non_null_skips_leading_nulls() {
        let (p, v) = cands![None, Source::UserAssertion, 1; Some("x"), Source::ToolOutput, 2; Some("y"), Source::ToolOutput, 3];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::FirstNonNull)
                .unwrap()
                .survivor(),
            Some("x")
        );
    }

    // ---- the strategies upstream had to refuse ----------------------------

    #[test]
    fn most_recent_uses_observation_time_not_input_order() {
        // "old" is listed last but observed first; recency must beat position.
        let (p, v) =
            cands![Some("new"), Source::ToolOutput, 500; Some("old"), Source::UserAssertion, 100];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::MostRecent)
                .unwrap()
                .survivor(),
            Some("new")
        );
    }

    #[test]
    fn most_recent_refuses_a_contradictory_simultaneous_tie() {
        let (p, v) = cands![Some("acme"), Source::UserAssertion, 100; Some("globex"), Source::ToolOutput, 100];
        let err = merge(&build(&p, &v), &Strategy::MostRecent).unwrap_err();
        assert!(err.0.contains("no \"most recent\""), "{}", err.0);
    }

    #[test]
    fn most_recent_tolerates_agreement_at_the_latest_instant() {
        let (p, v) = cands![Some("old"), Source::UserAssertion, 1; Some("new"), Source::ToolOutput, 9; Some("new"), Source::AgentInference, 9];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::MostRecent)
                .unwrap()
                .survivor(),
            Some("new")
        );
    }

    #[test]
    fn source_priority_prefers_the_higher_ranked_source() {
        let (p, v) = cands![Some("guessed"), Source::AgentInference, 900; Some("stated"), Source::UserAssertion, 100];
        let order = vec![Source::UserAssertion, Source::AgentInference];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::SourcePriority(order))
                .unwrap()
                .survivor(),
            // Rank beats recency: a fresh inference does not outrank what the
            // user actually said.
            Some("stated")
        );
    }

    #[test]
    fn source_priority_breaks_within_tier_ties_by_recency() {
        let (p, v) = cands![Some("first"), Source::UserAssertion, 100; Some("second"), Source::UserAssertion, 200];
        let order = vec![Source::UserAssertion];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::SourcePriority(order))
                .unwrap()
                .survivor(),
            Some("second")
        );
    }

    #[test]
    fn source_priority_refuses_an_unlisted_source() {
        let (p, v) = cands![Some("a"), Source::UserAssertion, 1; Some("b"), Source::External("crm".into()), 2];
        let order = vec![Source::UserAssertion];
        let err = merge(&build(&p, &v), &Strategy::SourcePriority(order)).unwrap_err();
        assert!(err.0.contains("absent from the priority list"), "{}", err.0);
        assert!(
            err.0.contains("crm"),
            "the refusal should name the source: {}",
            err.0
        );
    }

    #[test]
    fn source_priority_refuses_an_empty_order() {
        let (p, v) = cands![Some("a"), Source::UserAssertion, 1; Some("b"), Source::ToolOutput, 2];
        let err = merge(&build(&p, &v), &Strategy::SourcePriority(vec![])).unwrap_err();
        assert!(err.0.contains("empty priority list"), "{}", err.0);
    }

    // ---- the strategy neither system could state -------------------------

    #[test]
    fn valid_interval_keeps_both_values_instead_of_picking_one() {
        // The thesis: this is not a conflict, it is two facts.
        let (p, v) = cands![Some("acme"), Source::UserAssertion, 100; Some("globex"), Source::UserAssertion, 200];
        let out = merge(&build(&p, &v), &Strategy::ValidInterval).unwrap();
        assert_eq!(
            out,
            Outcome::Timeline(vec![
                Fact {
                    value: "acme".into(),
                    valid: Interval::between(100, 200)
                },
                Fact {
                    value: "globex".into(),
                    valid: Interval::since(200)
                },
            ])
        );
        // And the store can answer "as of when".
        assert_eq!(out.as_of(150), Some("acme"));
        assert_eq!(out.as_of(200), Some("globex"));
        assert_eq!(out.as_of(99), None); // nothing known before the first observation
    }

    #[test]
    fn valid_interval_orders_by_observation_not_input_order() {
        let (p, v) = cands![Some("globex"), Source::UserAssertion, 200; Some("acme"), Source::UserAssertion, 100];
        let out = merge(&build(&p, &v), &Strategy::ValidInterval).unwrap();
        assert_eq!(out.as_of(150), Some("acme"));
    }

    #[test]
    fn restating_a_value_extends_its_span_rather_than_splitting_it() {
        let (p, v) = cands![Some("acme"), Source::UserAssertion, 100; Some("acme"), Source::ToolOutput, 150; Some("globex"), Source::UserAssertion, 200];
        let Outcome::Timeline(facts) = merge(&build(&p, &v), &Strategy::ValidInterval).unwrap()
        else {
            panic!("expected a timeline")
        };
        assert_eq!(
            facts.len(),
            2,
            "re-hearing a fact is not a change: {facts:?}"
        );
        assert_eq!(facts[0].valid, Interval::between(100, 200));
    }

    #[test]
    fn a_value_that_returns_gets_a_second_span() {
        // Worked at Acme, left, came back. Three spans, not two.
        let (p, v) = cands![Some("acme"), Source::UserAssertion, 100; Some("globex"), Source::UserAssertion, 200; Some("acme"), Source::UserAssertion, 300];
        let Outcome::Timeline(facts) = merge(&build(&p, &v), &Strategy::ValidInterval).unwrap()
        else {
            panic!("expected a timeline")
        };
        assert_eq!(facts.len(), 3);
        assert_eq!(facts[2].valid, Interval::since(300));
        assert_eq!(
            merge(&build(&p, &v), &Strategy::ValidInterval)
                .unwrap()
                .as_of(250),
            Some("globex")
        );
    }

    #[test]
    fn valid_interval_spans_never_overlap() {
        let (p, v) = cands![Some("a"), Source::UserAssertion, 10; Some("b"), Source::UserAssertion, 20; Some("c"), Source::UserAssertion, 30];
        let Outcome::Timeline(facts) = merge(&build(&p, &v), &Strategy::ValidInterval).unwrap()
        else {
            panic!("expected a timeline")
        };
        for t in 0..40 {
            let hits = facts.iter().filter(|f| f.valid.contains(t)).count();
            assert!(hits <= 1, "t={t} matched {hits} spans: {facts:?}");
        }
    }

    #[test]
    fn valid_interval_refuses_simultaneous_contradictions() {
        let (p, v) = cands![Some("acme"), Source::UserAssertion, 100; Some("globex"), Source::ToolOutput, 100];
        let err = merge(&build(&p, &v), &Strategy::ValidInterval).unwrap_err();
        assert!(err.0.contains("neither supersedes the other"), "{}", err.0);
    }

    #[test]
    fn valid_interval_on_nothing_is_an_empty_timeline() {
        let (p, v) = cands![None, Source::UserAssertion, 1];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::ValidInterval).unwrap(),
            Outcome::Timeline(vec![])
        );
    }

    #[test]
    fn one_value_still_yields_a_timeline_open_ended_from_its_observation() {
        let (p, v) = cands![Some("acme"), Source::UserAssertion, 100];
        let out = merge(&build(&p, &v), &Strategy::ValidInterval).unwrap();
        assert_eq!(
            out,
            Outcome::Timeline(vec![Fact {
                value: "acme".into(),
                valid: Interval::since(100)
            }])
        );
        assert_eq!(out.survivor(), Some("acme"));
    }

    // ---- the discipline ---------------------------------------------------

    #[test]
    fn a_refusal_beats_a_plausible_wrong_survivor() {
        // The property every refusal exists for: these must NOT quietly return
        // a value. A wrong memory raises nothing and looks right.
        let (p, v) = cands![Some("acme"), Source::UserAssertion, 100; Some("globex"), Source::ToolOutput, 100];
        let c = build(&p, &v);
        assert!(merge(&c, &Strategy::MostRecent).is_err());
        assert!(merge(&c, &Strategy::ValidInterval).is_err());
        assert!(merge(&c, &Strategy::SourcePriority(vec![])).is_err());
    }

    #[test]
    fn every_refusal_explains_what_was_missing() {
        let (p, v) = cands![Some("a"), Source::UserAssertion, 5; Some("b"), Source::ToolOutput, 5];
        let c = build(&p, &v);
        for s in [
            Strategy::MostRecent,
            Strategy::ValidInterval,
            Strategy::SourcePriority(vec![]),
        ] {
            let err = merge(&c, &s).unwrap_err();
            assert!(
                err.0.len() > 40,
                "{s:?} refused without a reason: {}",
                err.0
            );
        }
    }

    #[test]
    fn needs_provenance_flags_exactly_the_strategies_that_read_it() {
        assert!(Strategy::MostRecent.needs_provenance());
        assert!(Strategy::ValidInterval.needs_provenance());
        assert!(Strategy::SourcePriority(vec![]).needs_provenance());
        assert!(!Strategy::MajorityVote.needs_provenance());
        assert!(!Strategy::MostComplete.needs_provenance());
        assert!(!Strategy::FirstNonNull.needs_provenance());
        assert!(!Strategy::UnanimousOrNull.needs_provenance());
        assert!(!Strategy::ConfidenceMajority.needs_provenance());
    }
}
