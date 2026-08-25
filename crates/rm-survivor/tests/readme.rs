//! The README's example, kept honest by running it.
//!
//! If this stops compiling or passing, the front page is lying.

use rm_core::{Provenance, Source};
use rm_survivor::{merge, Candidate, Strategy};

#[test]
fn readme_example_holds() {
    let march = Provenance::new(Source::UserAssertion, 1_710_000_000_000, "session-1");
    let july = Provenance::new(Source::UserAssertion, 1_720_000_000_000, "session-9");

    let outcome = merge(
        &[
            Candidate::new(Some("Acme"), &march),
            Candidate::new(Some("Globex"), &july),
        ],
        &Strategy::ValidInterval,
    )
    .unwrap();

    assert_eq!(outcome.as_of(1_715_000_000_000).unwrap(), Some("Acme"));
    assert_eq!(outcome.as_of(1_725_000_000_000).unwrap(), Some("Globex"));
}
