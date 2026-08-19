//! Deterministic k-hop traversal over `rm_store`'s edges.
//!
//! # No state
//!
//! This crate holds nothing. Every function takes a `&MemoryStore` and reads
//! it. That is deliberate: `rm_engine` already keeps several structures
//! consistent with one another, and the one serious defect found reviewing it
//! came from that seam — an operation in one crate silently invalidating state
//! in another. A traversal index here would be a fourth structure to keep in
//! step, bought with speed nothing has yet asked for.
//!
//! The cost is real and worth stating: every walk filters edges by time as it
//! crosses them rather than reading a pre-filtered structure, so a dense entity
//! does work per query. That is the same bet `rm_index` made shipping exact
//! search over an approximate index, and it was right there. If it stops being
//! right, an adjacency index belongs here, and nothing in this API forecloses
//! one.
//!
//! # Both axes, always
//!
//! A walk answers "who was connected to Alice in May, as far as we knew in
//! August". Neither timestamp is defaulted, because a graph query with one axis
//! silently answers a different question than the caller asked.

use std::collections::BTreeSet;

use rm_core::Timestamp;
use rm_store::{MemoryStore, StableId};

/// Which way to follow an edge.
///
/// Not a default: "where does Alice work" and "who works at Acme" are different
/// questions over the same edge, and guessing which one a caller meant is the
/// kind of plausible wrong answer this workspace refuses elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Follow edges out of the entity: subject to object.
    Out,
    /// Follow edges into the entity: object to subject.
    In,
    /// Both, treating the graph as undirected for reachability.
    Both,
}

/// A traversal request.
#[derive(Clone, Debug, PartialEq)]
pub struct Walk {
    pub seeds: Vec<StableId>,
    /// Maximum hop distance from a seed. `0` returns just the seeds.
    pub hops: u8,
    /// Maximum entities in the result, seeds included. Bounds the answer's
    /// size, which is what a caller is protecting: memory and downstream work
    /// scale with entities returned, not with edges crossed.
    pub budget: usize,
    pub direction: Direction,
    /// `None` traverses every predicate.
    pub predicates: Option<Vec<String>>,
    pub valid_t: Timestamp,
    pub tx_t: Timestamp,
}

impl Walk {
    /// A walk over every predicate, outward.
    pub fn new(
        seeds: Vec<StableId>,
        hops: u8,
        budget: usize,
        valid_t: Timestamp,
        tx_t: Timestamp,
    ) -> Self {
        Walk {
            seeds,
            hops,
            budget,
            direction: Direction::Out,
            predicates: None,
            valid_t,
            tx_t,
        }
    }

    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = direction;
        self
    }

    /// Restrict traversal to these predicates.
    pub fn via(mut self, predicates: Vec<String>) -> Self {
        self.predicates = Some(predicates);
        self
    }

    fn wants(&self, predicate: &str) -> bool {
        self.predicates
            .as_ref()
            .is_none_or(|ps| ps.iter().any(|p| p == predicate))
    }
}

/// One entity a walk reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reached {
    pub entity: StableId,
    /// Hops from the nearest seed. Shortest, not first-found — the search is
    /// breadth-first.
    pub distance: u8,
}

/// What a walk found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Neighborhood {
    /// Reached entities, ordered by `(distance, entity)`.
    pub reached: Vec<Reached>,
    /// The budget stopped the walk while entities remained to visit.
    ///
    /// Reported rather than left implicit: a silently truncated neighbourhood
    /// is indistinguishable from a genuinely small one, which is the same
    /// failure as post-filtering a top-`k`.
    pub truncated: bool,
}

/// Every entity reachable from `walk.seeds` within `walk.hops`.
///
/// Breadth-first, expanding each level in ascending entity order, so two runs
/// over the same store return the same list. Determinism is the same
/// requirement that makes `rm_index` break score ties by id: a retrieval that
/// reorders between runs makes every downstream result irreproducible.
///
/// A seed the store does not hold is skipped rather than reported. Asking about
/// something the store has never met is the same shape of question as asking
/// for an attribute it has never heard, which answers "nothing known" rather
/// than failing.
pub fn neighborhood(store: &MemoryStore, walk: &Walk) -> Neighborhood {
    let mut reached: Vec<Reached> = Vec::new();
    let mut seen: BTreeSet<StableId> = BTreeSet::new();
    let mut truncated = false;

    let mut frontier: BTreeSet<StableId> = walk
        .seeds
        .iter()
        .copied()
        .filter(|id| store.entity(*id).is_some())
        .collect();

    let mut distance: u8 = 0;
    while !frontier.is_empty() {
        let mut next: BTreeSet<StableId> = BTreeSet::new();

        for id in &frontier {
            if !seen.insert(*id) {
                continue;
            }
            if reached.len() >= walk.budget {
                truncated = true;
                break;
            }
            reached.push(Reached {
                entity: *id,
                distance,
            });

            if distance < walk.hops {
                for neighbour in neighbours(store, *id, walk) {
                    if !seen.contains(&neighbour) {
                        next.insert(neighbour);
                    }
                }
            }
        }

        if truncated {
            break;
        }
        if distance == walk.hops {
            break;
        }
        distance += 1;
        frontier = next;
    }

    Neighborhood { reached, truncated }
}

/// The entities one hop from `id`, deduplicated and in ascending order.
fn neighbours(store: &MemoryStore, id: StableId, walk: &Walk) -> BTreeSet<StableId> {
    let mut out = BTreeSet::new();
    if matches!(walk.direction, Direction::Out | Direction::Both) {
        for e in store.edges_from(id, walk.valid_t, walk.tx_t) {
            if walk.wants(e.predicate) {
                out.insert(e.object);
            }
        }
    }
    if matches!(walk.direction, Direction::In | Direction::Both) {
        for e in store.edges_into(id, walk.valid_t, walk.tx_t) {
            if walk.wants(e.predicate) {
                out.insert(e.subject);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_core::{Interval, Provenance, Source};
    use rm_store::MemoryStore;

    fn said(at: Timestamp) -> Provenance {
        Provenance::new(Source::UserAssertion, at, "test")
    }

    /// A chain: 0 -> 1 -> 2 -> 3, all `knows`, all valid from t=1.
    fn chain(len: usize) -> (MemoryStore, Vec<StableId>) {
        let mut store = MemoryStore::new();
        let ids: Vec<StableId> = (0..len).map(|_| store.create_entity("person", 1)).collect();
        for pair in ids.windows(2) {
            store
                .relate(pair[0], "knows", pair[1], Interval::since(1), said(1))
                .unwrap();
        }
        (store, ids)
    }

    fn walk(seeds: Vec<StableId>, hops: u8, budget: usize) -> Walk {
        Walk::new(seeds, hops, budget, 5, 5).direction(Direction::Out)
    }

    #[test]
    fn a_walk_includes_its_seeds_at_distance_zero() {
        // Omitting them would make every caller re-add them, and a caller who
        // forgets has a bug nothing can see.
        let (store, ids) = chain(3);
        let n = neighborhood(&store, &walk(vec![ids[0]], 0, 10));
        assert_eq!(
            n.reached,
            vec![Reached {
                entity: ids[0],
                distance: 0
            }]
        );
        assert!(!n.truncated);
    }

    #[test]
    fn hop_distance_is_the_shortest_path_not_the_first_seen() {
        // 0 -> 1 -> 2 and also 0 -> 2 directly: 2 is one hop away, not two.
        let mut store = MemoryStore::new();
        let a = store.create_entity("person", 1);
        let b = store.create_entity("person", 1);
        let c = store.create_entity("person", 1);
        store
            .relate(a, "knows", b, Interval::since(1), said(1))
            .unwrap();
        store
            .relate(b, "knows", c, Interval::since(1), said(1))
            .unwrap();
        store
            .relate(a, "knows", c, Interval::since(1), said(1))
            .unwrap();

        let n = neighborhood(&store, &walk(vec![a], 3, 10));
        let c_hit = n.reached.iter().find(|r| r.entity == c).unwrap();
        assert_eq!(c_hit.distance, 1);
    }

    #[test]
    fn a_walk_stops_at_the_hop_limit() {
        let (store, ids) = chain(4);
        let n = neighborhood(&store, &walk(vec![ids[0]], 2, 10));
        assert_eq!(n.reached.len(), 3, "seed plus two hops");
        assert!(
            !n.truncated,
            "a hop limit is the question asked, not a truncation"
        );
    }

    #[test]
    fn a_walk_returns_the_same_order_every_run() {
        let (store, ids) = chain(4);
        let first = neighborhood(&store, &walk(vec![ids[0]], 3, 10));
        for _ in 0..5 {
            assert_eq!(
                neighborhood(&store, &walk(vec![ids[0]], 3, 10)).reached,
                first.reached
            );
        }
        let distances: Vec<u8> = first.reached.iter().map(|r| r.distance).collect();
        assert_eq!(distances, vec![0, 1, 2, 3], "ordered by distance");
    }

    #[test]
    fn a_truncated_walk_says_so() {
        // A budget that silently drops the tail returns a short neighbourhood
        // indistinguishable from a genuinely small one.
        let (store, ids) = chain(4);
        let n = neighborhood(&store, &walk(vec![ids[0]], 3, 2));
        assert_eq!(n.reached.len(), 2);
        assert!(n.truncated, "the caller must be able to tell it was cut");

        let full = neighborhood(&store, &walk(vec![ids[0]], 3, 4));
        assert_eq!(full.reached.len(), 4);
        assert!(
            !full.truncated,
            "a budget that was never reached is not a truncation"
        );
    }

    #[test]
    fn a_cycle_terminates() {
        let mut store = MemoryStore::new();
        let a = store.create_entity("person", 1);
        let b = store.create_entity("person", 1);
        store
            .relate(a, "knows", b, Interval::since(1), said(1))
            .unwrap();
        store
            .relate(b, "knows", a, Interval::since(1), said(1))
            .unwrap();

        let n = neighborhood(&store, &walk(vec![a], 8, 10));
        assert_eq!(n.reached.len(), 2);
    }

    #[test]
    fn a_seed_the_store_does_not_hold_is_skipped_rather_than_failing() {
        // The same shape of question as asking about an unknown entity, which
        // answers "nothing known" rather than erroring.
        let (store, ids) = chain(2);
        let n = neighborhood(&store, &walk(vec![ids[0], 9999], 1, 10));
        assert_eq!(n.reached.len(), 2);
        assert!(n.reached.iter().all(|r| r.entity != 9999));
    }

    #[test]
    fn seeds_count_against_the_budget() {
        let (store, ids) = chain(3);
        let n = neighborhood(&store, &walk(vec![ids[0]], 3, 1));
        assert_eq!(n.reached.len(), 1, "the seed alone filled it");
        assert!(n.truncated);
    }

    #[test]
    fn direction_decides_which_question_is_being_asked() {
        // Alice employed_by Acme. Outward from Alice finds Acme; outward from
        // Acme finds nobody; inward from Acme finds Alice.
        let mut store = MemoryStore::new();
        let alice = store.create_entity("person", 1);
        let acme = store.create_entity("org", 1);
        store
            .relate(alice, "employed_by", acme, Interval::since(1), said(1))
            .unwrap();

        let out = neighborhood(&store, &Walk::new(vec![alice], 1, 10, 5, 5));
        assert_eq!(out.reached.len(), 2);

        let from_acme = neighborhood(&store, &Walk::new(vec![acme], 1, 10, 5, 5));
        assert_eq!(from_acme.reached.len(), 1, "nothing leaves Acme");

        let into_acme = neighborhood(
            &store,
            &Walk::new(vec![acme], 1, 10, 5, 5).direction(Direction::In),
        );
        assert_eq!(into_acme.reached.len(), 2, "who works at Acme");

        let both = neighborhood(
            &store,
            &Walk::new(vec![acme], 1, 10, 5, 5).direction(Direction::Both),
        );
        assert_eq!(both.reached.len(), 2);
    }

    #[test]
    fn a_predicate_filter_keeps_a_walk_off_unrelated_edges() {
        let mut store = MemoryStore::new();
        let alice = store.create_entity("person", 1);
        let acme = store.create_entity("org", 1);
        let bob = store.create_entity("person", 1);
        store
            .relate(alice, "employed_by", acme, Interval::since(1), said(1))
            .unwrap();
        store
            .relate(alice, "knows", bob, Interval::since(1), said(1))
            .unwrap();

        let employment = neighborhood(
            &store,
            &Walk::new(vec![alice], 2, 10, 5, 5).via(vec!["employed_by".to_string()]),
        );
        assert_eq!(employment.reached.len(), 2);
        assert!(employment.reached.iter().all(|r| r.entity != bob));

        let all = neighborhood(&store, &Walk::new(vec![alice], 2, 10, 5, 5));
        assert_eq!(all.reached.len(), 3);
    }

    #[test]
    fn a_walk_as_of_a_past_tx_time_does_not_cross_edges_learned_later() {
        // Told in September that Alice joined Acme in July. A walk as we stood
        // in August must not reach Acme, even asking about August.
        let mut store = MemoryStore::new();
        let alice = store.create_entity("person", 1);
        let acme = store.create_entity("org", 1);
        store
            .relate(alice, "employed_by", acme, Interval::since(7), said(9))
            .unwrap();

        let in_august = neighborhood(&store, &Walk::new(vec![alice], 2, 10, 8, 8));
        assert_eq!(in_august.reached.len(), 1, "we did not know yet");

        let in_october = neighborhood(&store, &Walk::new(vec![alice], 2, 10, 8, 10));
        assert_eq!(in_october.reached.len(), 2, "learned since, true then");
    }

    #[test]
    fn a_walk_does_not_cross_an_edge_that_has_been_unrelated() {
        let mut store = MemoryStore::new();
        let alice = store.create_entity("person", 1);
        let acme = store.create_entity("org", 1);
        store
            .relate(alice, "employed_by", acme, Interval::since(1), said(1))
            .unwrap();
        store
            .unrelate(alice, "employed_by", acme, 5, said(5))
            .unwrap();

        assert_eq!(
            neighborhood(&store, &Walk::new(vec![alice], 2, 10, 3, 9))
                .reached
                .len(),
            2
        );
        assert_eq!(
            neighborhood(&store, &Walk::new(vec![alice], 2, 10, 7, 9))
                .reached
                .len(),
            1
        );
    }

    #[test]
    fn a_two_hop_walk_reaches_what_one_hop_cannot() {
        // The reason this crate exists: "what do I know about Alice's
        // employer's city" is not a question recall() can answer.
        let mut store = MemoryStore::new();
        let alice = store.create_entity("person", 1);
        let acme = store.create_entity("org", 1);
        let bristol = store.create_entity("place", 1);
        store
            .relate(alice, "employed_by", acme, Interval::since(1), said(1))
            .unwrap();
        store
            .relate(acme, "based_in", bristol, Interval::since(1), said(1))
            .unwrap();

        let one = neighborhood(&store, &Walk::new(vec![alice], 1, 10, 5, 5));
        assert!(one.reached.iter().all(|r| r.entity != bristol));

        let two = neighborhood(&store, &Walk::new(vec![alice], 2, 10, 5, 5));
        let hit = two.reached.iter().find(|r| r.entity == bristol).unwrap();
        assert_eq!(hit.distance, 2);
    }

    #[test]
    fn a_level_with_siblings_out_of_discovery_order_is_returned_by_ascending_id() {
        // Every other fixture in this file has one entity per level, so a walk
        // that visited a level in discovery order rather than sorted id order
        // would still pass every other test here.
        //
        // `relate` call order alone cannot produce that disagreement: the store
        // keys objects of one predicate in a `BTreeMap<StableId, _>`
        // (`rm_store`'s `edges: BTreeMap<StableId, BTreeMap<String,
        // BTreeMap<StableId, _>>>`), so `edges_from` already yields a single
        // predicate's objects in ascending id order no matter what order they
        // were related in -- the first version of this test related three
        // same-predicate edges out of id order and it made no difference. What
        // *can* disagree is predicate order: predicates are also a `BTreeMap`,
        // walked alphabetically, so three different predicates whose alphabetical
        // order is the reverse of their objects' ids gives a hub whose raw,
        // pre-sort discovery order is high, mid, low -- the opposite of what the
        // result must be.
        //
        // Verified as a real pin, not a tautology: swapping the `BTreeSet` that
        // orders a level (both `neighbours`'s `out` and `neighborhood`'s
        // `frontier`/`next`) for `Vec`s built by push, no sort, made this test
        // fail -- it returned `[high, mid, low]`. Reverting restored the pass.
        // See the task report for the exact diff used.
        let mut store = MemoryStore::new();
        let hub = store.create_entity("person", 1); // id 0
        let low = store.create_entity("person", 1); // id 1
        let mid = store.create_entity("person", 1); // id 2
        let high = store.create_entity("person", 1); // id 3

        // Predicate names sort "a" < "m" < "z", pointed at ids in the opposite
        // order, so raw discovery order (by predicate, then by object within a
        // predicate) is high, mid, low -- fully reversed from id order.
        store
            .relate(hub, "a_knows", high, Interval::since(1), said(1))
            .unwrap();
        store
            .relate(hub, "m_knows", mid, Interval::since(1), said(1))
            .unwrap();
        store
            .relate(hub, "z_knows", low, Interval::since(1), said(1))
            .unwrap();

        let n = neighborhood(&store, &walk(vec![hub], 1, 10));
        let level_one: Vec<StableId> = n
            .reached
            .iter()
            .filter(|r| r.distance == 1)
            .map(|r| r.entity)
            .collect();
        assert_eq!(
            level_one,
            vec![low, mid, high],
            "a level is ordered by ascending entity id, not discovery order"
        );
    }

    #[test]
    fn a_zero_budget_returns_nothing_and_reports_truncated() {
        // An entity existed at the seed and was not returned: that is exactly
        // what `truncated` exists to distinguish from a genuinely empty result.
        let (store, ids) = chain(1);
        let n = neighborhood(&store, &walk(vec![ids[0]], 1, 0));
        assert_eq!(n.reached.len(), 0);
        assert!(n.truncated);
    }
}
