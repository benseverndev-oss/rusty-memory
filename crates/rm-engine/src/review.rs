//! Pairs the resolver could not call.

use rm_store::StableId;
use serde::{Deserialize, Serialize};

/// Identifies one open question.
pub type ReviewId = u64;

/// Two entities that may be the same, and the evidence that could not decide.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingReview {
    pub id: ReviewId,
    pub a: StableId,
    pub b: StableId,
    /// Total evidence in bits. Positive favours a match.
    pub score: f64,
}

/// A pair someone has already answered "not the same".
///
/// Kept so the same near-miss does not raise the same question on every
/// subsequent observation. Being asked twice teaches a caller to stop reading
/// the queue, which costs more than the occasional missed merge.
///
/// Ordered `(lower, higher)` on construction, so a pair means the same thing
/// whichever of the two was mentioned first.
pub type Settled = (StableId, StableId);
