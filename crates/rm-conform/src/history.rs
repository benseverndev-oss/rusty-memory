//! The generated corpus: assertions that own their data.
//!
//! `rm_survivor::Candidate` borrows its value and its provenance, which is
//! right for a merge that runs inside one function and wrong for a corpus that
//! has to outlive the call. This owns both and lends a `Candidate` on demand.

use rm_core::{Interval, Provenance, Source, Supersession, Timestamp};
use rm_survivor::Candidate;

/// One thing said about one attribute, at one time, about one span of time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assertion {
    /// `None` is a tombstone: the source said the attribute has no value.
    /// Distinct from having said nothing, which is not an assertion at all and
    /// is not representable here.
    pub value: Option<String>,
    pub valid: Interval,
    pub provenance: Provenance,
    pub supersession: Supersession,
}

impl Assertion {
    /// A value asserted at `observed_at`, valid from `valid_from` onward.
    pub fn new(value: &str, valid_from: Timestamp, observed_at: Timestamp) -> Self {
        Assertion {
            value: Some(value.to_string()),
            valid: Interval::since(valid_from),
            provenance: Provenance::new(Source::UserAssertion, observed_at, "conform"),
            supersession: Supersession::Unstated,
        }
    }

    /// The borrowed form `rm_survivor::merge` takes.
    ///
    /// `Candidate::new(None, ..)` means *silent*, which is not what a tombstone
    /// is, so the two cases go through different constructors.
    pub fn candidate(&self) -> Candidate<'_> {
        match &self.value {
            Some(v) => Candidate::new(Some(v.as_str()), &self.provenance).over(self.valid),
            None => Candidate::absent(&self.provenance).over(self.valid),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_survivor::Asserted;

    #[test]
    fn an_assertion_lends_a_candidate_that_keeps_its_valid_span() {
        let a = Assertion::new("fly.io", 100, 500);
        let c = a.candidate();
        assert_eq!(c.value, Asserted::Value("fly.io"));
        assert_eq!(c.valid, Interval::since(100));
        assert_eq!(c.provenance.observed_at, 500);
    }

    #[test]
    fn a_tombstone_lends_an_absent_candidate_not_a_silent_one() {
        let a = Assertion {
            value: None,
            valid: Interval::since(100),
            provenance: Provenance::new(Source::UserAssertion, 500, "conform"),
            supersession: Supersession::Unstated,
        };
        assert_eq!(a.candidate().value, Asserted::Absent);
    }
}
