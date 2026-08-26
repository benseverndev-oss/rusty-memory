//! What a read costs, in shape rather than in seconds.
//!
//! `rm-contrast`'s README tells a reader choosing between this store and the
//! flat control to weigh a cost it declines to quantify: *"The difference is
//! asymptotic rather than a constant factor, and it is not measured here."*
//! This file is the asymptotic half. `benches/read-cost` is the other half,
//! and is what checks that these predictions still track reality.
//!
//! # The units are arbitrary, and that is load-bearing
//!
//! A "unit" here is roughly one candidate touched. Ratios between strategies
//! and across depths are meaningful because both sides are counted the same
//! way. **Ratios against the control are not**, and this module deliberately
//! does not offer one: a store unit builds a `Candidate` and clones a `String`
//! where a control unit is a hash lookup, so dividing them would produce a
//! confident number that means nothing. The store-versus-control crossover is
//! measured in nanoseconds, in the bench, where the units really are the same.

use std::collections::BTreeMap;

use rm_engine::{Engine, Strategy};

/// Versions per attribute slot in the live store, measured rather than
/// assumed.
///
/// `D:\memory\decisions.json`, 2026-08-25: 219 entities, 1,086 attribute
/// slots, and every one of them holding exactly one version. Nothing has been
/// revised.
///
/// It is an anchor for where *that* store sits, not a claim about workloads in
/// general -- it is two days old and was seeded once.
/// # Overtaken, 2026-08-25
///
/// This was measured on a two-day-old store and was true of it. It stopped
/// being true the same day: an afternoon of five sessions correcting each
/// other's records took the store to `{1: 1238, 2: 17, 3: 1}`. Depth
/// arrives when things are *corrected*, and nothing had been corrected yet.
///
/// Kept rather than bumped, because the number is a dated observation and
/// not a setting. Run `benches/read-cost <store.json>` for the live figure;
/// it reports drift against this constant rather than trusting either.
pub const LIVE_STORE_DEPTH: usize = 1;

/// Predicted **variable** work for one `about()` against a slot holding `v`
/// versions.
///
/// # Fixed cost is measured, not modelled
///
/// A read also pays a cost that does not depend on `v` at all: the entity
/// lookup, the `Vec` allocation, the `Believed` it returns. `benches/read-cost`
/// measures that at roughly 350ns against a marginal ~7.6ns per version, so it
/// **dominates every read below about depth 50** -- which is every read this
/// project's live store performs.
///
/// It is deliberately absent here. A constant is not a function of `v`, so
/// modelling it would mean fitting a number from the engine's own timings, and
/// a model fitted to what it judges is not a reference model. The bench fits
/// it instead, with [`fit`], and reports it as a measurement.
///
/// # This models the path where the value changes
///
/// `merge` returns early when every assertion agrees
/// (`rm-survivor/src/lib.rs:424`), so a slot holding one value `v` times pays
/// the unanimity scan and nothing else -- no sort, no strategy. That path is
/// deliberately not modelled here, because it is deliberately not generated in
/// the bench: measuring it would measure the early-out rather than
/// survivorship. A model that ignored the early-out while the bench exercised
/// it would have two errors that cancel invisibly.
///
/// # Terms
///
/// | term | what it models |
/// |---|---|
/// | `v` | one `Candidate` per tx-visible version, `rm-engine/src/read.rs:284` |
/// | `v` | the unanimity scan before any strategy runs, `rm-survivor/src/lib.rs:424` |
/// | `2v` | `MostRecent`'s max-then-filter, `rm-survivor/src/lib.rs:531` |
/// | `v*log2(v) + 2v` | `ValidInterval`'s sort, grouping and span-closing passes, `rm-survivor/src/lib.rs:619` |
///
/// Strategies other than those two collapse a history in a single pass and are
/// modelled as one, which is enough for a shape.
pub fn predicted_work(v: usize, strategy: &Strategy) -> f64 {
    let v = v as f64;
    // Paid on every read whatever the strategy resolves to.
    let shared = 2.0 * v;
    let by_strategy = match strategy {
        Strategy::MostRecent => 2.0 * v,
        // `log2(1)` is 0, so a single-version slot pays no sort at all. That
        // is not a rounding convenience -- it is why the two strategies cost
        // the same at the depth the live store is at.
        Strategy::ValidInterval => v * v.log2() + 2.0 * v,
        _ => v,
    };
    shared + by_strategy
}

/// A read's cost, split into the part that depends on history and the part
/// that does not.
///
/// `ns = fixed_ns + marginal_ns * predicted_work(v, strategy)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// Cost paid by every read regardless of depth.
    pub fixed_ns: f64,
    /// Cost per unit of [`predicted_work`] -- roughly, per candidate touched.
    pub marginal_ns: f64,
}

/// Least squares over `(predicted_work, measured_ns)` samples.
///
/// A pure function, so CI can check it recovers a line it was given rather
/// than trusting it on measured data where the right answer is unknown.
///
/// Returns `None` for fewer than two samples, or when every sample sits at the
/// same `x` -- there is no slope to find and inventing one would be worse than
/// declining.
pub fn fit(samples: &[(f64, f64)]) -> Option<Fit> {
    let n = samples.len() as f64;
    if samples.len() < 2 {
        return None;
    }
    let sx: f64 = samples.iter().map(|(x, _)| x).sum();
    let sy: f64 = samples.iter().map(|(_, y)| y).sum();
    let sxx: f64 = samples.iter().map(|(x, _)| x * x).sum();
    let sxy: f64 = samples.iter().map(|(x, y)| x * y).sum();
    let denom = n * sxx - sx * sx;
    if denom.abs() < f64::EPSILON {
        return None;
    }
    let marginal_ns = (n * sxy - sx * sy) / denom;
    Some(Fit {
        fixed_ns: (sy - marginal_ns * sx) / n,
        marginal_ns,
    })
}

/// Versions per attribute slot, counted across a whole store.
///
/// The measurement [`LIVE_STORE_DEPTH`] records, as something that can be
/// run again. That constant was measured once by hand against one store, and
/// a constant standing in for a moving thing with nothing able to re-check it
/// is the drift this project keeps finding -- in its own README sentences, in
/// a strategy's doc comment, in a recorded refusal figure.
///
/// Lives here rather than in `benches/read-cost`, which is excluded from the
/// workspace and never built by CI: the bench does the file reading and the
/// printing, and this does the counting, so the part with a right answer is
/// the part under test.
pub fn depth_histogram(engine: &Engine) -> BTreeMap<usize, usize> {
    let mut histogram = BTreeMap::new();
    for id in engine.entity_ids() {
        for name in engine.attributes_of(id) {
            *histogram
                .entry(engine.store_history(id, name).len())
                .or_default() += 1;
        }
    }
    histogram
}

/// The shallowest depth at which `ValidInterval` costs `factor` times
/// `MostRecent`.
///
/// `None` when they never diverge that far within a depth any store would
/// plausibly reach, which is itself an answer rather than a failure.
pub fn depth_where_ratio_exceeds(factor: f64) -> Option<usize> {
    (1..100_000).find(|&v| {
        predicted_work(v, &Strategy::ValidInterval) / predicted_work(v, &Strategy::MostRecent)
            >= factor
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the live store's depth means for the variable term.
    ///
    /// An earlier version of this test asserted that the two strategies cost
    /// *the same* at depth 1, since `log2(1)` is 0 and `ValidInterval` has no
    /// sort to pay. The bench falsified it: 358ns against 528ns. The variable
    /// terms really are equal; `ValidInterval` pays more in the fixed cost
    /// this model does not carry, allocating a `Vec<Fact>` where `MostRecent`
    /// returns one winner.
    ///
    /// So the true claim is narrower, and it is the one that matters: at the
    /// depth the live store is at, the variable term is a rounding error
    /// against what it becomes, and a read there is fixed-cost dominated.
    #[test]
    fn the_live_store_sits_where_history_costs_almost_nothing() {
        assert_eq!(LIVE_STORE_DEPTH, 1);
        for s in [Strategy::MostRecent, Strategy::ValidInterval] {
            let here = predicted_work(LIVE_STORE_DEPTH, &s);
            let deep = predicted_work(1000, &s);
            assert!(
                deep / here > 500.0,
                "{s:?}: variable work at depth 1 is not negligible against depth 1000 ({:.0}x)",
                deep / here
            );
        }
    }

    /// `fit` recovers a line it was given.
    ///
    /// Checked against a known answer rather than against measured data, where
    /// nobody knows the right one. A fitter that quietly returned nonsense
    /// would make every number in the bench nonsense too.
    #[test]
    fn the_fitter_recovers_a_line_it_was_handed() {
        // y = 350 + 7.5x, sampled exactly.
        let samples: Vec<(f64, f64)> = (1..=10)
            .map(|i| {
                let x = i as f64 * 4.0;
                (x, 350.0 + 7.5 * x)
            })
            .collect();
        let f = fit(&samples).expect("ten samples with distinct x");
        assert!((f.fixed_ns - 350.0).abs() < 1e-6, "{f:?}");
        assert!((f.marginal_ns - 7.5).abs() < 1e-9, "{f:?}");
    }

    /// And it declines rather than inventing a slope.
    #[test]
    fn the_fitter_declines_when_there_is_no_slope_to_find() {
        assert_eq!(fit(&[]), None);
        assert_eq!(fit(&[(4.0, 100.0)]), None, "one point is not a line");
        assert_eq!(
            fit(&[(4.0, 100.0), (4.0, 200.0)]),
            None,
            "two points at the same depth are not a line either"
        );
    }

    /// And the companion, because the test above would also pass for a model
    /// that returned a constant. The two must diverge somewhere.
    #[test]
    fn the_strategies_diverge_once_there_is_a_history_to_sort() {
        let ratio = |v| {
            predicted_work(v, &Strategy::ValidInterval) / predicted_work(v, &Strategy::MostRecent)
        };
        assert_eq!(ratio(1), 1.0);
        assert!(ratio(1000) > ratio(10), "the sort term must show up");
        assert!(
            ratio(1000) < 4.0,
            "a log factor, not a catastrophe: {}",
            ratio(1000)
        );
    }

    /// The depth at which the sort starts to matter, computed rather than
    /// typed. `None` would mean the strategies never diverge that far, which
    /// is itself an answer.
    #[test]
    fn the_divergence_depth_is_a_number_not_an_opinion() {
        let d = depth_where_ratio_exceeds(1.5).expect("they do diverge");
        assert!(
            d > LIVE_STORE_DEPTH,
            "must be past where the real store sits"
        );
        assert!(
            d < 10_000,
            "found at a plausible depth, not off the end: {d}"
        );
    }

    /// Work grows with depth for the store, on every strategy. A model that
    /// flattened would make the whole bench meaningless.
    #[test]
    fn more_history_is_never_less_work() {
        for s in [Strategy::MostRecent, Strategy::ValidInterval] {
            for v in 1..200 {
                assert!(
                    predicted_work(v + 1, &s) >= predicted_work(v, &s),
                    "{s:?} went down from {v} to {}",
                    v + 1
                );
            }
        }
    }
    /// The histogram counts slots, not entities and not assertions.
    ///
    /// One entity with two attributes at different depths has to show up as
    /// two slots at two depths -- the mistake worth guarding is summing per
    /// entity, which would report one number for a store whose slots differ.
    #[test]
    fn the_depth_histogram_counts_each_slot_at_its_own_depth() {
        let mut e = engine();
        // `employer` written three times, `spouse` once.
        let (id, _) = e.remember_as(None, obs("employer", "Acme", 1)).unwrap();
        e.remember_as(Some(id), obs("employer", "Globex", 2))
            .unwrap();
        e.remember_as(Some(id), obs("employer", "Initech", 3))
            .unwrap();
        e.remember_as(Some(id), obs("spouse", "Sam", 4)).unwrap();

        let h = depth_histogram(&e);
        assert_eq!(
            h.get(&3),
            Some(&1),
            "employer is one slot at depth 3: {h:?}"
        );
        assert_eq!(h.get(&1), Some(&1), "spouse is one slot at depth 1: {h:?}");
        assert_eq!(h.values().sum::<usize>(), 2, "two slots in total: {h:?}");
    }

    /// An empty store is an empty histogram, not a panic and not a zero.
    #[test]
    fn an_empty_store_has_no_slots_at_all() {
        assert!(depth_histogram(&engine()).is_empty());
    }

    fn engine() -> Engine {
        use rm_engine::{Metric, Policy, VectorIndex};
        Engine::new(
            VectorIndex::new(3, Metric::Cosine),
            rm_resolve::Ruleset::new(
                vec![rm_resolve::FieldRule::new(
                    "name",
                    rm_resolve::Comparator::JaroWinkler,
                    0.9,
                    0.01,
                )],
                vec![rm_resolve::BlockingKey::Prefix("name".to_string(), 3)],
                4.0,
                8.0,
            )
            .expect("a one-field ruleset is valid"),
            Policy::new(Strategy::MostRecent),
        )
    }

    fn obs(attribute: &str, value: &str, at: rm_engine::Timestamp) -> rm_engine::Observation {
        use rm_engine::{Interval, Observation, Provenance, Record, Source, Supersession};
        Observation {
            kind: "thing".to_string(),
            mention: Record::new().with("name", "subject one"),
            attribute: attribute.to_string(),
            value: Some(value.to_string()),
            valid: Interval::since(at),
            provenance: Provenance::new(Source::UserAssertion, at, format!("cost-{at}")),
            supersession: Supersession::Corrects,
            embedding: vec![1.0, 0.0, 0.0],
        }
    }
}
