//! The oracle: survivorship implemented for auditability, not performance.
//!
//! Identical signature to `rm_survivor::merge`, so a differential test is one
//! comparison. Written from the documented semantics of each `Strategy`, never
//! from that crate's implementation — an oracle derived from the code it judges
//! is not an oracle.
//!
//! It reuses `rm_survivor`'s *data* types (`Outcome`, `Held`, `Interval`) while
//! implementing the *logic* independently. A bug in what `Interval` means would
//! therefore be shared; the metamorphic invariants exist partly to cover that,
//! because they are derived from what bi-temporality means rather than from
//! either implementation.

use rm_core::Interval;
use rm_survivor::{Asserted, Candidate, Fact, Held, Outcome, Refused, Span, Strategy};

/// What `rm_survivor::merge` should have returned.
pub fn merge(candidates: &[Candidate<'_>], strategy: &Strategy) -> Result<Outcome, Refused> {
    match strategy {
        Strategy::MostRecent => most_recent(candidates),
        Strategy::ValidInterval => valid_interval(candidates),
        Strategy::MostComplete | Strategy::LongestValue => most_complete(candidates),
        Strategy::MajorityVote | Strategy::ConfidenceMajority => majority(candidates),
        Strategy::FirstNonNull => first_non_null(candidates),
        Strategy::UnanimousOrNull => unanimous(candidates),
        Strategy::SourcePriority(order) => source_priority(candidates, order),
    }
}

/// What held at `t`, distinguishing an asserted absence from no coverage.
///
/// Implemented here rather than calling `Outcome::held_at`, because the read
/// path applies this *after* merging and it is therefore part of what is being
/// scored, not a neutral accessor.
///
/// Note what this means for a `Survivor`: it has no time dimension, so it holds
/// at every `t` and valid time does not bite at all. Only a `Timeline` -- that
/// is, only `Strategy::ValidInterval` -- answers "what was true when".
pub fn held_at(outcome: &Outcome, t: rm_core::Timestamp) -> Result<Option<&Held>, Refused> {
    match outcome {
        Outcome::Survivor(v) => Ok(v.as_ref()),
        // Half-open `[from, to)`, per `Interval`'s own docs.
        Outcome::Timeline(facts) => match facts
            .iter()
            .find(|f| f.valid.from <= t && f.valid.to.is_none_or(|to| t < to))
        {
            None => Ok(None),
            Some(f) => match &f.span {
                Span::Held(v) => Ok(Some(v)),
                Span::Contested { .. } => Err(Refused(
                    "nothing orders the values that opened here".to_string(),
                )),
            },
        },
    }
}

/// Assertions only. Silence is not a claim and never competes.
fn claims<'a, 'b>(candidates: &'b [Candidate<'a>]) -> Vec<&'b Candidate<'a>> {
    candidates
        .iter()
        .filter(|c| c.value.is_assertion())
        .collect()
}

/// The owned form of what a candidate asserted. `Silent` never reaches here.
fn held(c: &Candidate<'_>) -> Held {
    match c.value {
        Asserted::Value(v) => Held::Value(v.to_string()),
        Asserted::Absent => Held::Absent,
        Asserted::Silent => unreachable!("filtered by claims()"),
    }
}

/// The most recently observed value.
///
/// Documented rule: "The most recently observed value. Refuses when the latest
/// observation is a tie between different values: simultaneous contradictory
/// assertions have no 'most recent'."
fn most_recent(candidates: &[Candidate<'_>]) -> Result<Outcome, Refused> {
    let claims = claims(candidates);
    if claims.is_empty() {
        return Ok(Outcome::Survivor(None));
    }
    let latest = claims
        .iter()
        .map(|c| c.provenance.observed_at)
        .max()
        .expect("non-empty");

    let mut distinct: Vec<Held> = Vec::new();
    for c in claims.iter().filter(|c| c.provenance.observed_at == latest) {
        let h = held(c);
        if !distinct.contains(&h) {
            distinct.push(h);
        }
    }
    if distinct.len() > 1 {
        return Err(Refused(
            "simultaneous contradictory assertions have no most recent".to_string(),
        ));
    }
    Ok(Outcome::Survivor(Some(distinct.remove(0))))
}

/// Each distinct value with the span of valid time over which it stood.
///
/// Documented rule: "Do not pick a winner. Emit each distinct value with the
/// validity range over which it stood. Refuses only when two different values
/// collide on *both* axes -- same `valid.from` and same `observed_at` --
/// because then nothing orders them and there is no way to say which
/// superseded which."
///
/// Ordered by when each value began to hold, ties broken by when it was heard.
/// Sorting by `valid.from` rather than `observed_at` is the whole difference
/// between a valid-time timeline and a transaction-time one wearing its name.
///
/// # That sentence is quotable because the sweep made it true
///
/// It used to read "refuses when two different values share an observation
/// timestamp", which taken literally refuses two values heard in the same
/// instant however their valid spans differ. `rm_survivor` never did that: it
/// refuses only on the double collision above.
///
/// The implementation was the correct one and the sentence was out of date,
/// written when a `Candidate` carried no `valid` and the timeline was cut at
/// `observed_at` -- at which point two values sharing an observation really did
/// have no order between them. Adding valid time gave them one.
///
/// The sweep found the gap by disagreeing on 53 generated histories. This
/// reference was written against the behaviour and the divergence recorded
/// here; `rm_survivor`'s comment has since been corrected to match, so the
/// quote above is now the rule rather than a claim about it.
fn valid_interval(candidates: &[Candidate<'_>]) -> Result<Outcome, Refused> {
    let claims = claims(candidates);
    if claims.is_empty() {
        return Ok(Outcome::Timeline(vec![]));
    }

    let mut ordered: Vec<&&Candidate<'_>> = claims.iter().collect();
    ordered.sort_by(|a, b| {
        (a.valid.from, a.provenance.observed_at).cmp(&(b.valid.from, b.provenance.observed_at))
    });

    // Refusal: only where nothing orders the two. Adjacent pairs suffice once
    // sorted, because the colliding keys are equal and therefore neighbours.
    for pair in ordered.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a.valid.from == b.valid.from
            && a.provenance.observed_at == b.provenance.observed_at
            && held(a) != held(b)
        {
            return Err(Refused(
                "neither supersedes the other and no validity range can be assigned".to_string(),
            ));
        }
    }

    let mut facts: Vec<Fact> = Vec::new();
    for c in ordered {
        let value = held(c);
        // A repeat of the value already standing extends it rather than
        // opening a second span: the timeline holds *distinct* values.
        if facts.last().map(|f| &f.span) == Some(&Span::Held(value.clone())) {
            continue;
        }
        // Close the previous span where this one opens.
        if let Some(prev) = facts.last_mut() {
            prev.valid = Interval::between(prev.valid.from, c.valid.from);
        }
        facts.push(Fact {
            span: Span::Held(value),
            valid: Interval::since(c.valid.from),
        });
    }
    Ok(Outcome::Timeline(facts))
}

/// Longest value wins; ties go to the first seen.
///
/// The tie direction is why this is a loop rather than `max_by_key`, which
/// returns the *last* maximum and would quietly invert the documented rule.
fn most_complete(candidates: &[Candidate<'_>]) -> Result<Outcome, Refused> {
    let mut best: Option<Held> = None;
    for c in claims(candidates) {
        let h = held(c);
        let len = h.value().map(str::len).unwrap_or(0);
        let better = match &best {
            None => true,
            Some(b) => len > b.value().map(str::len).unwrap_or(0),
        };
        if better {
            best = Some(h);
        }
    }
    Ok(Outcome::Survivor(best))
}

/// Most frequently asserted value wins; count ties go to the first seen.
///
/// `counts` keeps insertion order and the comparison is strict, so the first
/// value to reach a given count keeps it.
fn majority(candidates: &[Candidate<'_>]) -> Result<Outcome, Refused> {
    let mut counts: Vec<(Held, usize)> = Vec::new();
    for c in claims(candidates) {
        let h = held(c);
        match counts.iter_mut().find(|(k, _)| *k == h) {
            Some((_, n)) => *n += 1,
            None => counts.push((h, 1)),
        }
    }
    let mut best: Option<(Held, usize)> = None;
    for (h, n) in counts {
        if best.as_ref().is_none_or(|(_, bn)| n > *bn) {
            best = Some((h, n));
        }
    }
    Ok(Outcome::Survivor(best.map(|(h, _)| h)))
}

/// The first non-null assertion in input order.
///
/// "Null" here means [`Asserted::Silent`] -- a gap in what was heard -- not
/// [`Asserted::Absent`]. A tombstone is a positive claim that the field is
/// empty and it competes like any other: `rm_survivor`'s own test says so in
/// as many words, in `an_absence_competes_rather_than_being_treated_as_silence`
/// ("they left Acme and are between jobs" is an assertion, not a gap).
///
/// This reference read it the other way round at first and the differential
/// sweep caught it on a one-assertion history. Recorded because the classical
/// data-quality meaning of "non-null" is the other one, so the mistake is an
/// easy one to make twice.
fn first_non_null(candidates: &[Candidate<'_>]) -> Result<Outcome, Refused> {
    let first = claims(candidates).into_iter().next().map(held);
    Ok(Outcome::Survivor(first))
}

/// The value if every non-null assertion agrees, otherwise nothing.
///
/// Same reading of "non-null" as [`first_non_null`]: a tombstone participates,
/// so a tombstone disagreeing with a value is not unanimous.
fn unanimous(candidates: &[Candidate<'_>]) -> Result<Outcome, Refused> {
    let values: Vec<Held> = claims(candidates).into_iter().map(held).collect();
    match values.first() {
        None => Ok(Outcome::Survivor(None)),
        Some(first) if values.iter().all(|v| v == first) => {
            Ok(Outcome::Survivor(Some(first.clone())))
        }
        _ => Ok(Outcome::Survivor(None)),
    }
}

/// The value from the highest-priority source that asserted one.
///
/// Refuses when any asserting source is absent from the priority list: ranking
/// an unlisted source would silently prefer the wrong system of record. Within
/// the winning source, ties resolve by `MostRecent`.
fn source_priority(
    candidates: &[Candidate<'_>],
    order: &[rm_core::Source],
) -> Result<Outcome, Refused> {
    let claims = claims(candidates);
    for c in &claims {
        if !order.contains(&c.provenance.source) {
            return Err(Refused(
                "an asserting source is absent from the priority list".to_string(),
            ));
        }
    }
    for source in order {
        let at_source: Vec<Candidate<'_>> = claims
            .iter()
            .filter(|c| c.provenance.source == *source)
            .map(|c| (*c).clone())
            .collect();
        if !at_source.is_empty() {
            return most_recent(&at_source);
        }
    }
    Ok(Outcome::Survivor(None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_core::{Interval, Provenance, Source};
    use rm_survivor::Fact;

    fn prov(at: i64) -> Provenance {
        Provenance::new(Source::UserAssertion, at, "t")
    }

    #[test]
    fn nothing_asserted_survives_as_nothing() {
        let out = merge(&[], &Strategy::MostRecent).unwrap();
        assert_eq!(out, Outcome::Survivor(None));
    }

    #[test]
    fn the_latest_observation_wins() {
        let (p1, p2) = (prov(100), prov(200));
        let cs = [
            Candidate::new(Some("fly.io"), &p1),
            Candidate::new(Some("render"), &p2),
        ];
        let out = merge(&cs, &Strategy::MostRecent).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Value("render".into()))));
    }

    #[test]
    fn a_tie_between_different_values_at_the_same_instant_refuses() {
        let (p1, p2) = (prov(200), prov(200));
        let cs = [
            Candidate::new(Some("fly.io"), &p1),
            Candidate::new(Some("render"), &p2),
        ];
        assert!(merge(&cs, &Strategy::MostRecent).is_err());
    }

    #[test]
    fn a_tie_on_the_same_value_is_not_a_contradiction() {
        let (p1, p2) = (prov(200), prov(200));
        let cs = [
            Candidate::new(Some("render"), &p1),
            Candidate::new(Some("render"), &p2),
        ];
        let out = merge(&cs, &Strategy::MostRecent).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Value("render".into()))));
    }

    #[test]
    fn silence_never_wins_however_late_it_arrives() {
        let (p1, p2) = (prov(100), prov(900));
        let cs = [
            Candidate::new(Some("fly.io"), &p1),
            Candidate::new(None, &p2), // Silent
        ];
        let out = merge(&cs, &Strategy::MostRecent).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Value("fly.io".into()))));
    }

    #[test]
    fn a_tombstone_is_a_claim_and_can_win() {
        let (p1, p2) = (prov(100), prov(900));
        let cs = [Candidate::new(Some("fly.io"), &p1), Candidate::absent(&p2)];
        let out = merge(&cs, &Strategy::MostRecent).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Absent)));
    }

    #[test]
    fn a_timeline_tiles_valid_time_without_overlap() {
        let (p1, p2) = (prov(500), prov(600));
        let cs = [
            Candidate::new(Some("fly.io"), &p1).over(Interval::since(100)),
            Candidate::new(Some("render"), &p2).over(Interval::since(300)),
        ];
        let out = merge(&cs, &Strategy::ValidInterval).unwrap();
        assert_eq!(
            out,
            Outcome::Timeline(vec![
                Fact {
                    span: Span::Held(Held::Value("fly.io".into())),
                    valid: Interval::between(100, 300)
                },
                Fact {
                    span: Span::Held(Held::Value("render".into())),
                    valid: Interval::since(300)
                },
            ])
        );
    }

    #[test]
    fn a_backdated_correction_takes_effect_when_it_happened_not_when_it_was_said() {
        // The store's own motivating example, stated as a property rather than
        // as a fixture: told at t=900 that the value changed at t=200, the
        // timeline says so from 200.
        let (p1, p2) = (prov(100), prov(900));
        let cs = [
            Candidate::new(Some("fly.io"), &p1).over(Interval::since(100)),
            Candidate::new(Some("render"), &p2).over(Interval::since(200)),
        ];
        let out = merge(&cs, &Strategy::ValidInterval).unwrap();
        assert_eq!(
            out,
            Outcome::Timeline(vec![
                Fact {
                    span: Span::Held(Held::Value("fly.io".into())),
                    valid: Interval::between(100, 200)
                },
                Fact {
                    span: Span::Held(Held::Value("render".into())),
                    valid: Interval::since(200)
                },
            ])
        );
    }

    #[test]
    fn two_values_with_nothing_at_all_to_order_them_refuse() {
        // Same instant heard, same instant held: nothing orders these two, so
        // no validity range can be assigned to either.
        let (p1, p2) = (prov(500), prov(500));
        let cs = [
            Candidate::new(Some("fly.io"), &p1).over(Interval::since(100)),
            Candidate::new(Some("render"), &p2).over(Interval::since(100)),
        ];
        assert!(merge(&cs, &Strategy::ValidInterval).is_err());
    }

    #[test]
    fn sharing_an_observation_instant_is_not_enough_to_refuse() {
        // This pins the divergence the sweep found. `Strategy::ValidInterval`'s
        // doc comment says it "refuses when two different values share an
        // observation timestamp", which taken literally would refuse here.
        // It does not, and should not: the two were heard together but held
        // from different moments, and valid time orders them perfectly well.
        //
        // The prose predates `Candidate::valid` and did not follow the code.
        let (p1, p2) = (prov(500), prov(500));
        let cs = [
            Candidate::new(Some("fly.io"), &p1).over(Interval::since(100)),
            Candidate::new(Some("render"), &p2).over(Interval::since(300)),
        ];
        assert_eq!(
            merge(&cs, &Strategy::ValidInterval).unwrap(),
            Outcome::Timeline(vec![
                Fact {
                    span: Span::Held(Held::Value("fly.io".into())),
                    valid: Interval::between(100, 300)
                },
                Fact {
                    span: Span::Held(Held::Value("render".into())),
                    valid: Interval::since(300)
                },
            ])
        );
    }

    #[test]
    fn nothing_asserted_is_an_empty_timeline_not_a_refusal() {
        let out = merge(&[], &Strategy::ValidInterval).unwrap();
        assert_eq!(out, Outcome::Timeline(vec![]));
    }

    #[test]
    fn most_complete_takes_the_longest_value() {
        let (p1, p2, p3) = (prov(100), prov(200), prov(300));
        let cs = [
            Candidate::new(Some("aa"), &p1),
            Candidate::new(Some("bbbb"), &p2),
            Candidate::new(Some("ccc"), &p3),
        ];
        let out = merge(&cs, &Strategy::MostComplete).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Value("bbbb".into()))));
    }

    #[test]
    fn most_complete_gives_a_length_tie_to_the_first_seen() {
        let (p1, p2) = (prov(100), prov(200));
        let cs = [
            Candidate::new(Some("aaaa"), &p1),
            Candidate::new(Some("bbbb"), &p2),
        ];
        let out = merge(&cs, &Strategy::MostComplete).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Value("aaaa".into()))));
    }

    #[test]
    fn longest_value_is_the_same_rule_as_most_complete() {
        let (p1, p2) = (prov(100), prov(200));
        let cs = [
            Candidate::new(Some("aa"), &p1),
            Candidate::new(Some("bbb"), &p2),
        ];
        assert_eq!(
            merge(&cs, &Strategy::LongestValue).unwrap(),
            merge(&cs, &Strategy::MostComplete).unwrap()
        );
    }

    #[test]
    fn majority_vote_counts_assertions_not_recency() {
        let (p1, p2, p3) = (prov(100), prov(200), prov(300));
        let cs = [
            Candidate::new(Some("fly.io"), &p1),
            Candidate::new(Some("fly.io"), &p2),
            Candidate::new(Some("render"), &p3),
        ];
        let out = merge(&cs, &Strategy::MajorityVote).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Value("fly.io".into()))));
    }

    #[test]
    fn majority_vote_gives_a_count_tie_to_the_first_seen() {
        let (p1, p2) = (prov(100), prov(200));
        let cs = [
            Candidate::new(Some("first"), &p1),
            Candidate::new(Some("second"), &p2),
        ];
        let out = merge(&cs, &Strategy::MajorityVote).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Value("first".into()))));
    }

    #[test]
    fn first_non_null_takes_input_order_not_time_order() {
        let (p1, p2) = (prov(900), prov(100));
        let cs = [
            Candidate::new(Some("first"), &p1),
            Candidate::new(Some("second"), &p2),
        ];
        let out = merge(&cs, &Strategy::FirstNonNull).unwrap();
        assert_eq!(out, Outcome::Survivor(Some(Held::Value("first".into()))));
    }

    #[test]
    fn unanimous_or_null_yields_nothing_when_sources_disagree() {
        let (p1, p2) = (prov(100), prov(200));
        let cs = [
            Candidate::new(Some("fly.io"), &p1),
            Candidate::new(Some("render"), &p2),
        ];
        assert_eq!(
            merge(&cs, &Strategy::UnanimousOrNull).unwrap(),
            Outcome::Survivor(None)
        );
    }

    #[test]
    fn unanimous_or_null_yields_the_value_when_they_agree() {
        let (p1, p2) = (prov(100), prov(200));
        let cs = [
            Candidate::new(Some("render"), &p1),
            Candidate::new(Some("render"), &p2),
        ];
        assert_eq!(
            merge(&cs, &Strategy::UnanimousOrNull).unwrap(),
            Outcome::Survivor(Some(Held::Value("render".into())))
        );
    }

    #[test]
    fn source_priority_refuses_an_asserting_source_it_was_not_told_how_to_rank() {
        let p1 = Provenance::new(Source::AgentInference, 100, "t");
        let cs = [Candidate::new(Some("guess"), &p1)];
        let ranked = Strategy::SourcePriority(vec![Source::UserAssertion]);
        assert!(merge(&cs, &ranked).is_err());
    }

    #[test]
    fn source_priority_prefers_the_higher_ranked_source_however_old() {
        let p1 = Provenance::new(Source::UserAssertion, 100, "t");
        let p2 = Provenance::new(Source::ToolOutput, 900, "t");
        let cs = [
            Candidate::new(Some("stated"), &p1),
            Candidate::new(Some("fetched"), &p2),
        ];
        let ranked = Strategy::SourcePriority(vec![Source::UserAssertion, Source::ToolOutput]);
        assert_eq!(
            merge(&cs, &ranked).unwrap(),
            Outcome::Survivor(Some(Held::Value("stated".into())))
        );
    }
}
