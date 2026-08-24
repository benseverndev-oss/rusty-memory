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
use rm_survivor::{Asserted, Candidate, Fact, Held, Outcome, Refused, Strategy};

/// What `rm_survivor::merge` should have returned.
pub fn merge(candidates: &[Candidate<'_>], strategy: &Strategy) -> Result<Outcome, Refused> {
    match strategy {
        Strategy::MostRecent => most_recent(candidates),
        Strategy::ValidInterval => valid_interval(candidates),
        _ => unimplemented!("later tasks"),
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
/// validity range over which it stood, inferred from observation order.
/// Refuses when two different values share an observation timestamp: with no
/// order between them there is no way to say which superseded which."
///
/// Ordered by when each value began to hold, ties broken by when it was heard.
/// Sorting by `valid.from` rather than `observed_at` is the whole difference
/// between a valid-time timeline and a transaction-time one wearing its name.
fn valid_interval(candidates: &[Candidate<'_>]) -> Result<Outcome, Refused> {
    let claims = claims(candidates);
    if claims.is_empty() {
        return Ok(Outcome::Timeline(vec![]));
    }

    // Refusal first: two different values heard at the same instant have no
    // order between them, so no timeline can say which replaced which.
    for a in &claims {
        for b in &claims {
            if a.provenance.observed_at == b.provenance.observed_at && held(a) != held(b) {
                return Err(Refused(
                    "two different values share an observation timestamp".to_string(),
                ));
            }
        }
    }

    let mut ordered: Vec<&&Candidate<'_>> = claims.iter().collect();
    ordered.sort_by(|a, b| {
        (a.valid.from, a.provenance.observed_at).cmp(&(b.valid.from, b.provenance.observed_at))
    });

    let mut facts: Vec<Fact> = Vec::new();
    for c in ordered {
        let value = held(c);
        // A repeat of the value already standing extends it rather than
        // opening a second span: the timeline holds *distinct* values.
        if facts.last().map(|f| &f.value) == Some(&value) {
            continue;
        }
        // Close the previous span where this one opens.
        if let Some(prev) = facts.last_mut() {
            prev.valid = Interval::between(prev.valid.from, c.valid.from);
        }
        facts.push(Fact {
            value,
            valid: Interval::since(c.valid.from),
        });
    }
    Ok(Outcome::Timeline(facts))
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
                    value: Held::Value("fly.io".into()),
                    valid: Interval::between(100, 300)
                },
                Fact {
                    value: Held::Value("render".into()),
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
                    value: Held::Value("fly.io".into()),
                    valid: Interval::between(100, 200)
                },
                Fact {
                    value: Held::Value("render".into()),
                    valid: Interval::since(200)
                },
            ])
        );
    }

    #[test]
    fn two_different_values_sharing_an_observation_instant_refuse() {
        let (p1, p2) = (prov(500), prov(500));
        let cs = [
            Candidate::new(Some("fly.io"), &p1).over(Interval::since(100)),
            Candidate::new(Some("render"), &p2).over(Interval::since(300)),
        ];
        assert!(merge(&cs, &Strategy::ValidInterval).is_err());
    }

    #[test]
    fn nothing_asserted_is_an_empty_timeline_not_a_refusal() {
        let out = merge(&[], &Strategy::ValidInterval).unwrap();
        assert_eq!(out, Outcome::Timeline(vec![]));
    }
}
