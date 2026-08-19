//! Reading the engine: survivorship at query time.

use rm_core::{Interval, Provenance, Source, Timestamp};
use rm_store::StableId;
use rm_survivor::{merge, Candidate, Held};

use crate::{AssertionId, Engine, EngineError, Policy};

/// What the engine concluded an attribute held.
///
/// Owned rather than borrowed: survivorship on read builds its answer from an
/// outcome computed inside the call, so when the strategy produces a value no
/// single stored version carried, there is nothing in the store to borrow from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Believed {
    Value(String),
    Absent,
    Unknown,
}

/// A recall request.
#[derive(Clone, Debug)]
pub struct Query {
    pub embedding: Vec<f32>,
    pub k: usize,
    /// `(valid_t, tx_t)`. Both axes, because "what did I believe last Tuesday
    /// about what was true in May" is a different question from either half.
    pub as_of: Option<(Timestamp, Timestamp)>,
    pub entity: Option<StableId>,
    pub source: Option<Source>,
    pub session: Option<String>,
}

/// One recalled assertion.
#[derive(Clone, Debug, PartialEq)]
pub struct Recalled {
    pub entity: StableId,
    pub assertion: AssertionId,
    pub attribute: String,
    /// `None` is a tombstone — this assertion claimed the attribute had no
    /// value. It is never "we have nothing": an assertion that says nothing is
    /// not stored and cannot be recalled.
    pub value: Option<String>,
    pub valid: Interval,
    pub provenance: Provenance,
    pub score: f32,
    /// A later assertion superseded this one as of the query's `tx_t`.
    pub superseded: bool,
}

impl Engine {
    /// What we believed at `tx_t` about what was true at `valid_t`.
    ///
    /// This is where survivorship runs. `remember` appends without resolving,
    /// so the strategy is applied to the whole history on every read — which is
    /// what makes it swappable, and what makes `Strategy::ValidInterval` need
    /// no special handling: its outcome is a timeline and the question is
    /// answered by asking the timeline.
    pub fn about(
        &self,
        entity: StableId,
        attribute: &str,
        valid_t: Timestamp,
        tx_t: Timestamp,
    ) -> Result<Believed, EngineError> {
        // Only what we had by tx_t. Later knowledge does not leak backwards.
        let versions: Vec<_> = self
            .store
            .history(entity, attribute)
            .iter()
            .filter(|v| v.ingested_at() <= tx_t)
            .collect();
        if versions.is_empty() {
            // Covers three cases that are all the same answer: an unknown
            // entity, an attribute never discussed, and every version having
            // arrived after tx_t. None of them is an error — the store simply
            // has no opinion yet.
            return Ok(Believed::Unknown);
        }

        let candidates: Vec<Candidate<'_>> = versions
            .iter()
            .map(|v| match &v.value {
                Some(s) => Candidate::new(Some(s.as_str()), &v.provenance),
                // A stored `None` is a tombstone — a positive claim of
                // absence — and has to compete as one. `Candidate::new(None,
                // ..)` would instead read as the source saying nothing, which
                // drops the tombstone out of the comparison entirely and lets
                // an earlier value win by default.
                None => Candidate::absent(&v.provenance),
            })
            .collect();

        // A refusal propagates rather than falling back to a looser strategy:
        // a memory chosen by a rule the caller did not ask for is exactly the
        // plausible-looking wrong answer the refusals exist to prevent.
        let outcome = merge(&candidates, self.policy.for_attribute(attribute))?;
        Ok(match outcome.held_at(valid_t) {
            // `held_at`, not `as_of`: `as_of` collapses an asserted absence
            // into `None`, the same shape as no coverage at all. `Believed`
            // exists to keep exactly that distinction, so the precise
            // accessor is the only one that can feed it.
            Some(Held::Value(v)) => Believed::Value(v.clone()),
            Some(Held::Absent) => Believed::Absent,
            None => Believed::Unknown,
        })
    }

    /// The same engine reading under a different policy.
    ///
    /// Cheap because nothing was resolved on write: changing the rule changes
    /// the answer without touching a single stored version.
    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }
}
