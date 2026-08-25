//! Whether a memory reaches where it was asked from, measured.
//!
//! One rule governs every decision read:
//!
//! > A memory applies where its scope is an **ancestor-or-self** of the asker's
//! > position.
//!
//! It decides what a session is shown, and the live store has hundreds of
//! records under it. The headline table had five rows and none of them was
//! this.
//!
//! # This module never imports `rm_host::scope`
//!
//! Not `applies_at`, not `validate`, not `UNIVERSAL`. An oracle derived from
//! the code it judges is not an oracle. Scopes reach the store through
//! `command::decide` and `command::plan_rescope` like any other caller, so the
//! store is exercised normally; only the *expectation* is computed here.
//!
//! # The backfill case is not reachable here
//!
//! `decide` refuses without a scope, so every decision this module generates
//! carries one and every rescope over it is a correction. A decision with no
//! scope at all can only be a record written before scopes existed, and no
//! public write path produces one any more. That case stays covered by
//! `rm-host`'s own unit tests and by the live-store check in the scope PR --
//! 219 records, all unscoped, all still visible -- not by this sweep.

/// Whether a memory scoped `scope` reaches an asker standing at `position`.
///
/// The oracle. Derived from the rule as written rather than from
/// `rm_host`'s `applies_at`, and derived *differently*: this is
/// separator-anchored string work where the implementation zips segment
/// iterators. Two ways to the same claim is the whole value of a differential.
///
/// `"*"` is spelled out rather than imported. Importing the constant would make
/// this track a change to it silently; spelling it means a change surfaces as a
/// disagreement, which is the point.
pub fn reaches(scope: &str, position: &str) -> bool {
    scope == "*"
        || position == scope
        // The trailing separator is what stops `prod` reaching `production`.
        || position.starts_with(&format!("{scope}/"))
}

use crate::rng::Rng;

/// Segment names, chosen so some are string prefixes of others.
///
/// `prod`/`production` and `work`/`workshop` are the point of this list, not
/// decoration. Without a pair like them the sweep never builds a tree where a
/// bare `starts_with` would differ from a segment comparison, and
/// `applicability agreement` would report 1.000 having tested the one mistake
/// the rule exists to prevent exactly zero times.
const NAMES: [&str; 8] = [
    "prod",
    "production",
    "work",
    "workshop",
    "personal",
    "fs",
    "er",
    "arrow",
];

/// How much world to build.
#[derive(Clone, Debug)]
pub struct Params {
    /// How deep the scope tree goes. 1 is a flat list of top-level names.
    pub depth: usize,
    /// How many children each node gets.
    pub branching: usize,
    /// How many decisions to place in it.
    pub decisions: usize,
    /// Percent of decisions scoped `*`.
    pub universal_pct: u64,
}

impl Default for Params {
    fn default() -> Self {
        // Three levels and two-way branching gives a tree small enough to
        // compare exhaustively and deep enough that ancestor-or-self has more
        // than one ancestor to get wrong.
        Params {
            depth: 3,
            branching: 2,
            decisions: 12,
            universal_pct: 20,
        }
    }
}

/// A generated scope tree, the positions to ask from, and the decisions in it.
pub struct World {
    /// Every path in the tree, shallowest first. Never contains `*`.
    pub scopes: Vec<String>,
    /// Where to stand. Drawn from three places -- see [`world`].
    pub positions: Vec<String>,
    /// `(title, scope)`. Scope is a member of `scopes`, or `*`.
    pub decisions: Vec<(String, String)>,
}

/// Build one world from a seed.
///
/// Positions come from three places because they fail differently: a node in
/// the tree (the ordinary case), a node *below* the deepest scope (where only
/// ancestors reach), and a path sharing a segment prefix with a real scope
/// without being under it (`production` beside `prod`) -- which is the case a
/// string-prefix rule gets wrong and a segment rule does not.
pub fn world(seed: u64, params: &Params) -> World {
    let mut rng = Rng::new(seed);

    let mut scopes: Vec<String> = Vec::new();
    let mut frontier: Vec<String> = Vec::new();
    for _ in 0..params.branching {
        let name = NAMES[rng.below(NAMES.len() as u64) as usize];
        if !frontier.iter().any(|f| f == name) {
            frontier.push(name.to_string());
        }
    }
    scopes.extend(frontier.iter().cloned());
    for _ in 1..params.depth {
        let mut next = Vec::new();
        for parent in &frontier {
            for _ in 0..params.branching {
                let name = NAMES[rng.below(NAMES.len() as u64) as usize];
                let child = format!("{parent}/{name}");
                if !scopes.contains(&child) {
                    scopes.push(child.clone());
                    next.push(child);
                }
            }
        }
        frontier = next;
    }

    let mut decisions = Vec::new();
    for i in 0..params.decisions {
        let scope = if rng.below(100) < params.universal_pct {
            "*".to_string()
        } else {
            scopes[rng.below(scopes.len() as u64) as usize].clone()
        };
        decisions.push((format!("decision {i}"), scope));
    }

    let mut positions: Vec<String> = scopes.clone();
    // Below the deepest scope: only ancestors reach here.
    if let Some(deepest) = scopes.last() {
        positions.push(format!("{deepest}/deeper"));
    }
    // A sibling that shares a string prefix without being under anything.
    positions.push("production".to_string());
    positions.push("prod".to_string());
    // The root, where only `*` reaches.
    positions.push("*".to_string());

    World {
        scopes,
        positions,
        decisions,
    }
}

use rm_embed::Hashed;
use rm_engine::{Engine, Policy, Strategy};
use rm_host::command::{self, Outcome};
use rm_host::time::At;
use rm_index::{Metric, VectorIndex};
use rm_resolve::{BlockingKey, Comparator, FieldRule, Ruleset};

/// Its own fixture rather than sharing `decisions.rs`'s.
///
/// A fixture two measurements share is one either can silently reconfigure,
/// which is the coupling this crate exists to avoid rather than introduce.
fn engine() -> Engine {
    let ruleset = Ruleset::new(
        vec![FieldRule::new("name", Comparator::JaroWinkler, 0.9, 0.01)],
        vec![BlockingKey::Prefix("name".to_string(), 3)],
        4.0,
        8.0,
    )
    .expect("a one-field ruleset is valid");
    Engine::new(
        VectorIndex::new(3, Metric::Cosine),
        ruleset,
        Policy::new(Strategy::MostRecent),
    )
}

/// Record every decision in `world`, each with its stated reach.
pub fn build(world: &World) -> Engine {
    let mut e = engine();
    let embedder = Hashed::new(3);
    let mut observed_at = 1_000;
    for (title, scope) in &world.decisions {
        command::decide(
            &mut e,
            title,
            "the chosen option",
            scope,
            None, // status: accepted
            Some("a stated reason"),
            None, // context
            None, // supersedes
            None, // decided_at: defaults to observed_at
            observed_at,
            "conform",
            &embedder,
        )
        .expect("a decision with a fresh title and a valid scope is recorded");
        observed_at += 10;
    }
    e
}

/// The titles the read path returns from `position`, sorted.
pub fn visible(engine: &Engine, position: &str) -> Vec<String> {
    let Outcome::Decisions(ds) = command::decisions(engine, None, At::latest(), Some(position))
        .expect("listing decisions cannot fail on a store this builds")
    else {
        panic!("decisions did not return decisions")
    };
    let mut out: Vec<String> = ds.into_iter().map(|d| d.title).collect();
    out.sort();
    out
}

/// The titles that *should* be visible from `position`, sorted.
///
/// Computed from what the generator wrote, through [`reaches`] -- never by
/// asking the store.
pub fn expected(world: &World, position: &str) -> Vec<String> {
    let mut out: Vec<String> = world
        .decisions
        .iter()
        .filter(|(_, scope)| reaches(scope, position))
        .map(|(title, _)| title.clone())
        .collect();
    out.sort();
    out
}

/// Whether the read path and the oracle agree on every generated world.
pub fn agreement(seeds: std::ops::Range<u64>, params: &Params) -> bool {
    seeds.into_iter().all(|seed| {
        let w = world(seed, params);
        let e = build(&w);
        w.positions
            .iter()
            .all(|p| visible(&e, p) == expected(&w, p))
    })
}

/// Whether descending the tree only ever adds.
///
/// A metamorphic property, derived from what ancestor-or-self *means* rather
/// than from either implementation: if a scope reaches `p`, it reaches every
/// position below `p`, so `visible(p)` is a subset of `visible(q)` whenever `q`
/// sits under `p`.
///
/// That derivation is the point. The oracle and the engine were written by the
/// same author against the same mental model and can agree enthusiastically on
/// a shared misunderstanding; this is the cover for that, in the same role
/// `invariants::monotonic_in_transaction_time` plays on the temporal axis.
///
/// Nesting is decided with [`reaches`] rather than by asking the store, so the
/// property does not depend on the thing it is checking.
pub fn depth_monotonic(seeds: std::ops::Range<u64>, params: &Params) -> bool {
    seeds.into_iter().all(|seed| {
        let w = world(seed, params);
        let e = build(&w);
        w.positions.iter().all(|p| {
            let above = visible(&e, p);
            w.positions
                .iter()
                .filter(|q| *q != p && reaches(p, q))
                .all(|q| {
                    let below = visible(&e, q);
                    above.iter().all(|t| below.contains(t))
                })
        })
    })
}

use rm_core::Timestamp;

/// The `(valid, tx)` grid the correction is probed on.
///
/// `pub` so `report.rs` and the tests read one grid rather than two that could
/// drift apart. `build` records at 1000 and steps by 10; the correction below
/// lands at 9000, so 5000 is before it on both axes and 12000 is after.
pub fn rescope_probes() -> Vec<(Timestamp, Timestamp)> {
    let mut out = Vec::new();
    for valid_t in [5_000, 12_000] {
        for tx_t in [5_000, 12_000] {
            out.push((valid_t, tx_t));
        }
    }
    out
}

/// A position from which `scope` reaches, for asking "is it here".
///
/// `*` reaches everywhere, so any position does; for anything else the scope
/// itself is the nearest position that admits it.
fn position_for(scope: &str) -> &str {
    if scope == "*" {
        "anywhere/at/all"
    } else {
        scope
    }
}

/// Whether correcting a reach leaves the past alone.
///
/// Three claims, and the middle one is the reason this row exists:
///
/// - At a transaction time *before* the correction, the store had not heard of
///   the new reach, so the decision answers under its original one.
/// - At a valid time before the correction but a transaction time after it, the
///   decision answers under its **original** reach -- a correction is dated
///   from now, because the reach genuinely changed today. Dating it from the
///   decision's start would assert it always reached somewhere it did not.
/// - At both after, the new reach applies.
///
/// Checked by asking from a position the reach admits and seeing whether the
/// title comes back, rather than by reading the scope attribute -- the question
/// is what a reader is shown, not what is stored.
pub fn rescope_history(seeds: std::ops::Range<u64>, params: &Params) -> bool {
    const CORRECTED_AT: Timestamp = 9_000;
    let embedder = Hashed::new(3);

    seeds.into_iter().all(|seed| {
        let w = world(seed, params);
        let mut e = build(&w);
        let (title, original) = w.decisions[0].clone();
        // A reach the original does not cover, so the two are distinguishable.
        let new_scope = if original == "*" { "work" } else { "*" };

        let Ok(plan) = command::plan_rescope(&title, new_scope, CORRECTED_AT, "conform", &embedder)
        else {
            return false;
        };
        if command::commit_rescope(&mut e, plan).is_err() {
            return false;
        }

        rescope_probes().into_iter().all(|(valid_t, tx_t)| {
            let at = At {
                valid: valid_t,
                tx: tx_t,
            };
            let under = |scope: &str| {
                let Ok(Outcome::Decisions(ds)) =
                    command::decisions(&e, None, at, Some(position_for(scope)))
                else {
                    return false;
                };
                ds.into_iter().any(|d| d.title == title)
            };
            let corrected = tx_t >= CORRECTED_AT && valid_t >= CORRECTED_AT;
            // `original.as_str()`, not `&original`: both arms of an if-else
            // must be the same type, and `&String` does not coerce here.
            let want = if corrected {
                new_scope
            } else {
                original.as_str()
            };
            under(want)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The last `/`-separated part of a path. Only the vacuity check
    /// needs it, so it lives here rather than shipping unused.
    fn last_segment(path: &str) -> &str {
        path.rsplit('/').next().unwrap_or(path)
    }

    #[test]
    fn a_scope_reaches_its_own_position_and_everything_below() {
        assert!(reaches("work", "work"));
        assert!(reaches("work", "work/goldenmatch"));
        assert!(reaches("work/goldenmatch", "work/goldenmatch/fs"));
        assert!(reaches("*", "anything/at/all"));
        assert!(reaches("*", "*"));
    }

    #[test]
    fn a_scope_reaches_neither_sideways_nor_upwards() {
        assert!(!reaches("work/goldenmatch/fs", "work/goldenmatch/er"));
        assert!(!reaches("personal", "work"));
        assert!(
            !reaches("work/goldenmatch", "work"),
            "narrower than the asker"
        );
        assert!(!reaches("work", "*"), "the root, where only * reaches");
    }

    /// The mistake the whole rule exists to prevent, and therefore the one an
    /// oracle must not share by construction. A bare `starts_with` says true
    /// to every line here.
    #[test]
    fn a_segment_boundary_is_not_a_string_prefix() {
        assert!(!reaches("prod", "production"));
        assert!(!reaches("work", "workshop"));
        assert!(!reaches("work", "workshop/thing"));
        assert!(reaches("prod", "prod/deploy"));
    }

    /// The constraint the whole module rests on, asserted rather than trusted
    /// to review. `include_str!` reads this file at compile time, so an import
    /// added later fails the suite rather than quietly voiding the measurement.
    #[test]
    fn this_module_does_not_import_the_code_it_judges() {
        let me = include_str!("applicability.rs");
        for banned in ["rm_host::scope", "scope::applies_at", "scope::UNIVERSAL"] {
            let uses = me
                .lines()
                .filter(|l| l.trim_start().starts_with("use "))
                .filter(|l| l.contains(banned))
                .count();
            assert_eq!(
                uses, 0,
                "applicability imports {banned}, so it judges itself"
            );
        }
    }

    /// The alphabet has to contain names that are string prefixes of each
    /// other, or `a_segment_boundary_is_not_a_string_prefix` is the only place
    /// that property is ever exercised and the sweep tests it zero times.
    #[test]
    fn generated_trees_contain_names_that_nearly_collide() {
        let params = Params::default();
        let mut found = false;
        for seed in 0..40 {
            let w = world(seed, &params);
            for a in &w.scopes {
                for b in &w.scopes {
                    if a != b {
                        for (x, y) in [(a, b), (b, a)] {
                            let (x, y) = (last_segment(x), last_segment(y));
                            if x != y && y.starts_with(x) {
                                found = true;
                            }
                        }
                    }
                }
            }
        }
        assert!(
            found,
            "no generated tree had one segment name as a string prefix of              another, so the property the oracle exists to check is untested"
        );
    }

    #[test]
    fn a_world_is_deterministic_and_well_formed() {
        let params = Params::default();
        let a = world(7, &params);
        let b = world(7, &params);
        assert_eq!(a.scopes, b.scopes, "same seed, same world");
        assert_eq!(a.decisions, b.decisions);
        assert_ne!(
            world(8, &params).scopes,
            a.scopes,
            "different seed, different world"
        );

        assert_eq!(a.decisions.len(), params.decisions);
        let titles: std::collections::HashSet<&String> =
            a.decisions.iter().map(|(t, _)| t).collect();
        assert_eq!(titles.len(), a.decisions.len(), "titles must be unique");
        for (_, s) in &a.decisions {
            assert!(s == "*" || a.scopes.contains(s), "{s:?} is not in the tree");
        }
        assert!(!a.positions.is_empty());
    }

    /// Both halves matter. All-universal makes exclusion untestable; none
    /// makes the row that must appear everywhere untestable.
    #[test]
    fn some_decisions_are_universal_and_some_are_not() {
        let params = Params::default();
        let mut universal = 0usize;
        let mut narrow = 0usize;
        for seed in 0..40 {
            for (_, s) in world(seed, &params).decisions {
                if s == "*" {
                    universal += 1;
                } else {
                    narrow += 1;
                }
            }
        }
        assert!(universal > 0, "nothing was scoped everywhere");
        assert!(narrow > 0, "everything was scoped everywhere");
    }

    #[test]
    fn the_read_path_returns_exactly_what_the_oracle_expects() {
        assert!(
            agreement(0..60, &Params::default()),
            "the read path disagreed with the oracle on some (world, position)"
        );
    }

    /// A sweep where every position saw everything, or nothing, would report
    /// perfect agreement having measured neither filtering nor inclusion.
    #[test]
    fn the_sweep_both_includes_and_excludes() {
        let params = Params::default();
        let (mut saw_some, mut saw_none) = (0, 0);
        for seed in 0..40 {
            let w = world(seed, &params);
            for p in &w.positions {
                let n = expected(&w, p).len();
                if n == 0 {
                    saw_none += 1;
                } else if n < w.decisions.len() {
                    saw_some += 1;
                }
            }
        }
        assert!(saw_some > 0, "no position ever saw a strict subset");
        assert!(saw_none > 0, "no position ever excluded everything");
    }

    /// The guard that makes the agreement row mean something: the oracle must
    /// be capable of disagreeing. A string-prefix rule differs from a segment
    /// rule on exactly the colliding names the generator plants.
    #[test]
    fn a_string_prefix_oracle_would_disagree_with_this_one() {
        fn naive(scope: &str, position: &str) -> bool {
            scope == "*" || position.starts_with(scope)
        }
        let params = Params::default();
        let differ = (0..40).any(|seed| {
            let w = world(seed, &params);
            w.positions.iter().any(|p| {
                w.decisions
                    .iter()
                    .any(|(_, s)| naive(s, p) != reaches(s, p))
            })
        });
        assert!(
            differ,
            "a bare starts_with agreed with the oracle everywhere, so the              sweep cannot tell a segment rule from a string one"
        );
    }

    #[test]
    fn a_deeper_position_never_sees_less() {
        assert!(
            depth_monotonic(0..60, &Params::default()),
            "descending removed a decision, which ancestor-or-self forbids"
        );
    }

    /// The companion. If no generated pair were ever nested, the property
    /// above would hold across every seed having compared nothing.
    #[test]
    fn the_monotonicity_check_finds_nested_pairs_to_compare() {
        let params = Params::default();
        let mut pairs = 0usize;
        for seed in 0..40 {
            let w = world(seed, &params);
            for p in &w.positions {
                for q in &w.positions {
                    if p != q && reaches(p, q) {
                        pairs += 1;
                    }
                }
            }
        }
        assert!(
            pairs > 20,
            "only {pairs} nested position pairs across 40 worlds, so the              property is close to vacuous"
        );
    }

    #[test]
    fn a_correction_does_not_rewrite_what_was_true_before_it() {
        assert!(
            rescope_history(0..40, &Params::default()),
            "a corrected reach answered wrongly on some (valid, tx) probe"
        );
    }

    /// This is where the correction branch stops being unexercised. Every one
    /// of the 219 records in the live store was a backfill, so `rescope` with a
    /// previous scope has only ever run in unit tests.
    #[test]
    fn the_correction_branch_actually_fires() {
        let params = Params::default();
        let w = world(3, &params);
        let mut e = build(&w);
        let embedder = Hashed::new(3);
        let (title, original) = w.decisions[0].clone();

        let new_scope = if original == "*" { "work" } else { "*" };
        let plan = command::plan_rescope(&title, new_scope, 9_000, "conform", &embedder)
            .expect("a known title with a valid scope");
        let Outcome::Rescoped { previous, .. } =
            command::commit_rescope(&mut e, plan).expect("the title resolves")
        else {
            panic!("rescope did not report a rescope")
        };
        assert_eq!(
            previous.as_deref(),
            Some(original.as_str()),
            "the correction branch did not see a previous scope"
        );
    }

    /// Re-stating a reach writes nothing. Cheap to get wrong, and the failure
    /// is silent: a second identical scope version would inflate every
    /// backfilled record's history without changing a single answer.
    #[test]
    fn restating_the_same_reach_writes_nothing() {
        let params = Params::default();
        let w = world(5, &params);
        let mut e = build(&w);
        let embedder = Hashed::new(3);
        let (title, original) = w.decisions[0].clone();

        let plan = command::plan_rescope(&title, &original, 9_000, "conform", &embedder)
            .expect("a known title with a valid scope");
        // The entity comes from the outcome; `find_decision` is private to
        // `rm-host` and widening it for a test's convenience would be the
        // wrong trade.
        let Outcome::Rescoped {
            entity, previous, ..
        } = command::commit_rescope(&mut e, plan).expect("the title resolves")
        else {
            panic!("rescope did not report a rescope")
        };
        assert_eq!(previous.as_deref(), Some(original.as_str()));
        assert_eq!(
            e.store_history(entity, "scope").len(),
            1,
            "re-stating a reach wrote a second version"
        );
    }
}
