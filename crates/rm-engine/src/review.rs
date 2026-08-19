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
