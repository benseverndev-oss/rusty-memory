//! Core types for rusty-memory.
//!
//! Deliberately small and dependency-light: every other crate depends on this
//! one, so anything that could plausibly live a layer up does.
//!
//! The two ideas that shape everything downstream:
//!
//! - **Time has two axes.** When a fact was *true* (valid time) is not when we
//!   *learned* it (transaction time). An agent told in September that the user
//!   changed jobs in July holds a fact whose valid time starts in July and whose
//!   transaction time starts in September. Collapsing the two makes the
//!   September conversation retroactively rewrite what the agent knew in August.
//! - **Provenance is not metadata.** Which source asserted a fact decides who
//!   wins when two sources disagree, so it travels with the value rather than
//!   beside it.

use serde::{Deserialize, Serialize};

/// Milliseconds since the Unix epoch.
///
/// Signed, so pre-1970 valid times (a birth date, a founding date) are
/// representable — valid time is not restricted to the agent's lifetime.
pub type Timestamp = i64;

/// Where an assertion came from.
///
/// Ordering is *not* derived: priority between sources is a policy the caller
/// supplies (see `rm_survivor::Strategy::SourcePriority`), not a property of the
/// enum. Deriving `Ord` here would invite `max()` to silently pick a winner by
/// declaration order.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    /// The user stated it directly.
    UserAssertion,
    /// A tool or API returned it.
    ToolOutput,
    /// The agent concluded it. The weakest source: inferences are derived from
    /// the others and re-deriving one does not make it more true.
    AgentInference,
    /// Anything else, named by the host (a CRM, a calendar, another agent).
    External(String),
}

/// A half-open interval `[from, to)` on one time axis.
///
/// Half-open so adjacent intervals tile without overlap: a fact valid until
/// `t` and its successor valid from `t` do not both answer a query at `t`.
/// `to: None` is open-ended — true from `from` until something supersedes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interval {
    pub from: Timestamp,
    pub to: Option<Timestamp>,
}

impl Interval {
    /// An interval starting at `from` with no known end.
    pub fn since(from: Timestamp) -> Self {
        Interval { from, to: None }
    }

    /// An interval bounded at both ends.
    pub fn between(from: Timestamp, to: Timestamp) -> Self {
        Interval { from, to: Some(to) }
    }

    /// Whether `t` falls inside this interval. Half-open: `from` is included,
    /// `to` is not.
    pub fn contains(&self, t: Timestamp) -> bool {
        t >= self.from && self.to.is_none_or(|end| t < end)
    }

    /// Whether the interval carries no time at all (`from >= to`). An
    /// open-ended interval is never empty.
    pub fn is_empty(&self) -> bool {
        self.to.is_some_and(|end| end <= self.from)
    }
}

/// How an assertion came to be known: which source said so, and when we heard
/// it.
///
/// `observed_at` is transaction time — when this reached the agent — and is
/// distinct from the valid time carried by [`Interval`]. Survivorship reads
/// both: "most recent" means most recently *observed*, because that is what the
/// agent actually knows about the order in which it learned things.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub source: Source,
    pub observed_at: Timestamp,
    /// The session, turn, or document this came from. Opaque to this crate;
    /// the host decides its shape.
    pub source_ref: String,
}

impl Provenance {
    pub fn new(source: Source, observed_at: Timestamp, source_ref: impl Into<String>) -> Self {
        Provenance {
            source,
            observed_at,
            source_ref: source_ref.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_are_half_open_so_adjacent_ones_do_not_overlap() {
        let first = Interval::between(0, 100);
        let second = Interval::since(100);
        // The boundary belongs to exactly one of them.
        assert!(first.contains(99));
        assert!(!first.contains(100));
        assert!(second.contains(100));
    }

    #[test]
    fn an_open_ended_interval_contains_everything_after_its_start() {
        let i = Interval::since(50);
        assert!(!i.contains(49));
        assert!(i.contains(50));
        assert!(i.contains(Timestamp::MAX));
        assert!(!i.is_empty());
    }

    #[test]
    fn a_zero_width_or_inverted_interval_is_empty() {
        assert!(Interval::between(10, 10).is_empty());
        assert!(Interval::between(10, 5).is_empty());
        assert!(!Interval::between(10, 11).is_empty());
    }

    #[test]
    fn valid_time_may_precede_the_epoch() {
        // A birth date is a valid time long before any agent observed it.
        let i = Interval::since(-1_000_000_000);
        assert!(i.contains(0));
    }
}
