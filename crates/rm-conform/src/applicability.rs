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

/// The last `/`-separated part of a path.
fn last_segment(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
