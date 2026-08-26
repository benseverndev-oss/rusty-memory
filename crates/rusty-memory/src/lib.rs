//! A memory that knows what it doesn't know.
//!
//! Depend on this crate, not on the `rusty-memory-*` crates beneath it. Those
//! are how this one is built, they are published only because Cargo requires a
//! published crate's dependencies to be published too, and they change without
//! notice. This is the surface with a semver promise attached.
//!
//! # The distinction this exists for
//!
//! [`Believed`] has three states, and the difference between the last two is
//! the whole point:
//!
//! ```text
//! spouse    Alex                                    Believed::Value
//! employer  no value — asserted to have none        Believed::Absent
//! pets      nothing known — this was never discussed Believed::Unknown
//! ```
//!
//! "They have no employer" and "nobody has ever said" are different answers.
//! Treating the second as the first is how a memory comes to state as fact
//! that someone is unemployed because their job never came up. It is measured
//! in `docs/absence-benchmark.md`.
//!
//! The same refusal runs underneath. Contradicting facts are both kept and
//! resolved when asked rather than settled at write time, so the same history
//! reads one way under [`Strategy::MostRecent`] and another under
//! [`Strategy::ValidInterval`] with nothing rewritten between. Two entities
//! that score too close to call are filed as a question rather than merged,
//! because a wrong merge is silent and permanent while an open question is
//! neither.

#![forbid(unsafe_code)]

// The store, and the three-state answer it gives.
pub use rm_engine::{Believed, Engine, EngineError, Observation, Query, Remembered, Standing};

// Time. Both axes: when something was true, and when the store heard it.
pub use rm_engine::{Interval, Provenance, Source, Supersession, Version};

// How competing values are resolved, at read time rather than at write time.
pub use rm_engine::{Policy, Strategy};

// Vectors, for recall.
pub use rm_engine::{Metric, VectorIndex};

// Identity: how mentions are compared, and what counts as evidence.
pub use rm_engine::{BlockingKey, Comparator, FieldRule, Record, Ruleset};

#[cfg(test)]
mod tests {
    /// The surface an adopter gets from the one crate they are told to
    /// depend on.
    ///
    /// This is the semver promise, as a test. A re-export that silently stops
    /// compiling is the one breakage a facade exists to make impossible, and
    /// the README's example has to be reachable from here without naming an
    /// internal crate.
    #[test]
    fn the_facade_carries_everything_the_readme_example_needs() {
        use crate::{Believed, Metric, Policy, Strategy, VectorIndex};

        let _ = Policy::new(Strategy::ValidInterval);
        let _ = Policy::new(Strategy::MostRecent);
        let _ = VectorIndex::new(3, Metric::Cosine);

        // The three states, named from the facade alone.
        let answers = [
            Believed::Value("Alex".into()),
            Believed::Absent,
            Believed::Unknown,
        ];
        assert_eq!(answers.len(), 3);
        assert_ne!(answers[1], answers[2], "absent is not unknown");
    }

    /// A caller who cannot name the error a function returns cannot handle it.
    ///
    /// Sending them to an internal crate for the type would defeat the point
    /// of having a facade at all.
    #[test]
    fn the_error_type_is_nameable_without_reaching_past_the_facade() {
        fn handles(e: crate::EngineError) -> String {
            e.to_string()
        }
        let _: fn(crate::EngineError) -> String = handles;
    }
}
