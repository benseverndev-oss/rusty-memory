//! What a read costs as history deepens.
//!
//! Three configurations, because two would answer the wrong question: the flat
//! control, the store under `most_recent` (**what actually ships**, and what
//! `rm-contrast` has never measured), and the store under `valid_interval`
//! (what `rm-contrast`'s accuracy column is measured under).
//!
//! Not in the workspace and not run by CI, as `benches/ann-bakeoff` and
//! `benches/locomo` are not.

use std::collections::HashSet;
use std::hint::black_box;
use std::time::Instant;

use rm_contrast::cost::{depth_histogram, fit, predicted_work, Fit, LIVE_STORE_DEPTH};
use rm_contrast::flat::Flat;
use rm_engine::{
    BlockingKey, Comparator, Engine, FieldRule, Interval, Metric, Observation, Policy, Provenance,
    Record, Ruleset, Source, StableId, Strategy, Supersession, Timestamp, VectorIndex,
};

const ATTRIBUTE: &str = "employer";
const DEPTHS: [usize; 8] = [1, 2, 5, 10, 50, 100, 500, 1000];

fn ruleset() -> Ruleset {
    Ruleset::new(
        vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
        vec![BlockingKey::Prefix("name".to_string(), 3)],
        4.0,
        8.0,
    )
    .expect("a one-field ruleset is valid")
}

/// One entity whose single attribute has been written `depth` times, each with
/// a **different** value.
///
/// Different values are the whole point. `merge` returns early when every
/// assertion agrees (`rm-survivor/src/lib.rs:424`), so a slot holding one
/// value a thousand times exits after a single pass -- fast, flat across the
/// sweep, and entirely plausible. It would be measuring the early-out rather
/// than survivorship.
fn build(depth: usize, strategy: Strategy) -> (Engine, StableId) {
    let mut engine = Engine::new(
        VectorIndex::new(3, Metric::Cosine),
        ruleset(),
        Policy::new(strategy),
    );
    let mut id: Option<StableId> = None;
    for i in 0..depth {
        let t = (i + 1) as Timestamp;
        let obs = Observation {
            kind: "thing".to_string(),
            mention: Record::new().with("name", "subject one"),
            attribute: ATTRIBUTE.to_string(),
            // Distinct per version: never unanimous, never coalesced.
            value: Some(format!("employer {i}")),
            valid: Interval::since(t),
            provenance: Provenance::new(Source::UserAssertion, t, format!("read-cost-{i}")),
            supersession: Supersession::Corrects,
            embedding: vec![1.0, 0.0, 0.0],
        };
        let (got, _) = engine
            .remember_as(id, obs)
            .expect("a pinned write cannot fail");
        id = Some(got);
    }
    (engine, id.expect("depth is at least 1"))
}

/// The same writes, into the control.
fn build_flat(depth: usize) -> Flat {
    let mut flat = Flat::new();
    for i in 0..depth {
        flat.remember(1, ATTRIBUTE, Some(&format!("employer {i}")));
    }
    flat
}

/// Nanoseconds per call, averaged over enough calls that the clock is not the
/// thing being measured. Iterations scale down with depth so every rung takes
/// roughly the same wall time.
fn time<F: FnMut()>(depth: usize, mut f: F) -> f64 {
    let iters = (10_000_000 / depth.max(1)).clamp(1_000, 200_000);
    for _ in 0..iters / 10 {
        f(); // warm up
    }
    let t = Instant::now();
    for _ in 0..iters {
        f();
    }
    t.elapsed().as_nanos() as f64 / iters as f64
}

fn main() {
    // `read-cost <path/to/store.json>` reports that store's depth instead of
    // sweeping. The sweep's whole conclusion turns on where a real store sits
    // on the curve, and `rm_contrast::cost::LIVE_STORE_DEPTH` records that as
    // a constant measured once by hand. A constant standing in for a moving
    // thing, with nothing able to re-check it, is the shape of drift this
    // project keeps finding -- so here is the way to re-check it.
    if let Some(path) = std::env::args().nth(1) {
        return depth_report(&path);
    }
    println!("# Read cost against history depth\n");
    println!("Iterations scale with depth. Values are nanoseconds per `about()`.\n");
    println!("| depth | distinct | flat | most_recent | valid_interval | mr ns/work | vi ns/work |");
    println!("|---|---|---|---|---|---|---|");

    // (predicted variable work, measured ns) per rung, per configuration.
    let mut mr_samples: Vec<(f64, f64)> = Vec::new();
    let mut vi_samples: Vec<(f64, f64)> = Vec::new();
    let mut flat_ns_all: Vec<f64> = Vec::new();

    for depth in DEPTHS {
        let (mr_engine, mr_id) = build(depth, Strategy::MostRecent);
        let (vi_engine, vi_id) = build(depth, Strategy::ValidInterval);
        let flat = build_flat(depth);

        // Guard, in the spirit of ann-bakeoff's README recording exactly this
        // class of silent error: a sweep that quietly built depth-1 stores at
        // every rung produces a flat table that looks like a finding.
        let achieved = mr_engine.store_history(mr_id, ATTRIBUTE).len();
        assert_eq!(
            achieved, depth,
            "asked for depth {depth}, built {achieved} -- the table would have been fiction"
        );

        // The second guard, and the one that actually catches the early-out.
        // Counting versions again would just repeat the check above. What
        // matters is that the values *differ*: identical values make `merge`
        // exit at the unanimity check, and make `ValidInterval` coalesce the
        // whole history into one span.
        let distinct: HashSet<String> = vi_engine
            .store_history(vi_id, ATTRIBUTE)
            .iter()
            .filter_map(|v| v.value.clone())
            .collect();
        assert_eq!(
            distinct.len(),
            depth,
            "only {} distinct values across {depth} versions -- the early-out is in play and this row would be measuring it",
            distinct.len()
        );

        let t = Timestamp::MAX;
        let flat_ns = time(depth, || {
            black_box(flat.about(1, ATTRIBUTE));
        });
        let mr_ns = time(depth, || {
            black_box(mr_engine.about(mr_id, ATTRIBUTE, t, t)).ok();
        });
        let vi_ns = time(depth, || {
            black_box(vi_engine.about(vi_id, ATTRIBUTE, t, t)).ok();
        });

        let mr_work = predicted_work(depth, &Strategy::MostRecent);
        let vi_work = predicted_work(depth, &Strategy::ValidInterval);
        let (mr_per, vi_per) = (mr_ns / mr_work, vi_ns / vi_work);
        mr_samples.push((mr_work, mr_ns));
        vi_samples.push((vi_work, vi_ns));
        flat_ns_all.push(flat_ns);

        println!(
            "| {depth} | {} | {flat_ns:.0} | {mr_ns:.0} | {vi_ns:.0} | {mr_per:.2} | {vi_per:.2} |",
            distinct.len()
        );
    }

    // Fixed and marginal, rather than one number that is neither.
    //
    // ns/work above falls by a factor of forty-five across this sweep, and
    // that is not the model failing to track the engine -- it is a fixed cost
    // the model has no term for, because a constant is not a function of
    // depth. Splitting it out is what makes both halves reportable.
    let mr_fit = fit(&mr_samples).expect("eight rungs at distinct depths");
    let vi_fit = fit(&vi_samples).expect("eight rungs at distinct depths");
    report("most_recent", &mr_fit, mr_samples[0].1);
    report("valid_interval", &vi_fit, vi_samples[0].1);

    let flat_lo = flat_ns_all.iter().cloned().fold(f64::INFINITY, f64::min);
    let flat_hi = flat_ns_all.iter().cloned().fold(0.0, f64::max);
    println!(
        "
The control: {flat_lo:.0}-{flat_hi:.0} ns across a 1000x depth range, which is the O(1) it claims to be."
    );

    println!(
        "
The live store is at depth {LIVE_STORE_DEPTH}, where the variable term is one unit of the marginal cost above and everything else is fixed."
    );

    // The guard. The asymptotic claim is about the *marginal* coefficient, so
    // that is what gets checked, and only where it dominates: below depth 50
    // the fixed cost is most of the read and a fit there measures the
    // constant. Fitting the deep rungs alone and the whole sweep should agree
    // -- a read that went quadratic would make the deep-only slope run away
    // from the overall one.
    for (name, samples) in [("most_recent", &mr_samples), ("valid_interval", &vi_samples)] {
        let deep: Vec<(f64, f64)> = samples.iter().copied().skip(4).collect();
        let all = fit(samples).expect("eight rungs");
        let deep_fit = fit(&deep).expect("four deep rungs");
        let drift = (deep_fit.marginal_ns / all.marginal_ns).max(all.marginal_ns / deep_fit.marginal_ns);
        println!(
            "{name}: marginal {:.2} ns/unit overall, {:.2} deep-only, drift {drift:.2}x",
            all.marginal_ns, deep_fit.marginal_ns
        );
        assert!(
            drift < 2.0,
            "{name}: the deep rungs stopped agreeing with the whole sweep about marginal cost              ({drift:.2}x). Either the read path changed shape or the model did."
        );
    }
}

/// One configuration's split.
///
/// The fixed cost is **measured at depth 1**, not read off the fit's
/// intercept. Least squares over a range this wide is dominated by the deep
/// rungs, and its intercept came out at roughly twice what a depth-1 read
/// actually costs. The number a reader wants here is what a shallow read
/// costs, and that is a thing we measured directly.
fn report(name: &str, f: &Fit, measured_at_depth_1: f64) {
    let crossover = (1..100_000).find(|&v| {
        f.marginal_ns * predicted_work(v, &Strategy::MostRecent) > measured_at_depth_1
    });
    println!(
        "
{name}: {measured_at_depth_1:.0} ns at depth 1, marginal {:.2} ns per predicted unit (fit intercept {:.0}). History overtakes the depth-1 cost at about depth {}.",
        f.marginal_ns,
        f.fixed_ns,
        crossover.map_or("never".to_string(), |v| v.to_string())
    );
}

/// Versions per attribute slot in a real store, and how that compares with
/// the figure `rm-contrast` was written against.
///
/// Reads through `Engine::open_split` rather than picking the snapshot apart,
/// so this cannot drift from the format the store actually writes. The
/// ruleset and policy are throwaway: nothing here resolves or survives
/// anything, it only counts version logs.
fn depth_report(path: &str) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("could not read {path}: {e}");
            std::process::exit(1);
        }
    };
    // The vector sidecar is not needed to count versions, and a store whose
    // vectors are missing still has a history worth reporting.
    let vectors = std::fs::read(format!("{}.vec", path.trim_end_matches(".json"))).ok();
    let engine = match Engine::open_split(
        &text,
        vectors.as_deref(),
        ruleset(),
        Policy::new(Strategy::MostRecent),
    ) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{path} is not a store this build can open: {e}");
            std::process::exit(1);
        }
    };

    // The counting lives in rm-contrast so CI can test it; this crate is
    // excluded from the workspace and never built there.
    let histogram = depth_histogram(&engine);
    let mut deepest: Option<(usize, StableId, String)> = None;
    for id in engine.entity_ids() {
        for name in engine.attributes_of(id) {
            let depth = engine.store_history(id, name).len();
            if deepest.as_ref().is_none_or(|(d, _, _)| depth > *d) {
                deepest = Some((depth, id, name.to_string()));
            }
        }
    }

    let slots: usize = histogram.values().sum();
    println!("# Store depth: {path}\n");
    println!("entities: {}", engine.entity_ids().len());
    println!("slots:    {slots}");
    println!("depth histogram (versions per slot -> slots): {histogram:?}");
    if let Some((depth, id, name)) = &deepest {
        println!("deepest:  {depth} versions, entity {id} {name:?}");
    }

    // The comparison is the reason this exists. Reported rather than
    // asserted: a store that has moved on is news, not a failure, and this
    // runs against whatever store it is pointed at.
    let recorded = LIVE_STORE_DEPTH;
    match histogram.keys().next_back() {
        None => println!("\nempty store: nothing to compare against the recorded depth of {recorded}"),
        Some(&max) if max == recorded => println!(
            "\ndeepest slot is {max}, matching rm_contrast::cost::LIVE_STORE_DEPTH. The cost curve's first row is still the row that describes this store."
        ),
        Some(&max) => println!(
            "\ndeepest slot is {max}, where LIVE_STORE_DEPTH records {recorded}. That constant and the two READMEs quoting it were measured against a different store than this one -- re-read them before trusting the depth-1 conclusion."
        ),
    }
}
