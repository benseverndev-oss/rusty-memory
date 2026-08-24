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

use rm_survivor::{Asserted, Candidate, Held, Outcome, Refused, Strategy};

/// What `rm_survivor::merge` should have returned.
pub fn merge(candidates: &[Candidate<'_>], strategy: &Strategy) -> Result<Outcome, Refused> {
    match strategy {
        Strategy::MostRecent => most_recent(candidates),
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

#[cfg(test)]
mod tests {
    use super::*;
    use rm_core::{Provenance, Source};

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
}
