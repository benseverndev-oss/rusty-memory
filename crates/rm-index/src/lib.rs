//! Vector search over memories: exact, deletable, filterable, persistent.
//!
//! # Why this is exact rather than approximate
//!
//! The architecture sketch flagged `goldenhnsw` as the component most likely to
//! need replacing, and said to benchmark it before committing. That benchmark
//! is `benches/ann-bakeoff`, and it produced two results that changed the
//! plan.
//!
//! **The first: `goldenhnsw` won.** Against `hnsw_rs` 0.3 on 20k 128-dimensional
//! vectors, serial insert both sides, it built 3.3× faster, queried 3.4× faster,
//! and had *better* recall (0.976 vs 0.951 at `ef=200`). It does not need
//! replacing. (Caveat kept honest: `hnsw_rs` offers a parallel insert this
//! comparison did not use, so its build column is not its best achievable.)
//!
//! **The second, and the reason this crate has no graph in it:** at the scale a
//! personal memory store actually runs, approximate search is not yet worth its
//! costs. On clustered data at N=20,000 exact brute force answered in 2.2 ms.
//! HNSW answered in 292 µs — genuinely faster, and bought with ~5 s of build
//! time, a graph to keep consistent across deletions, a recall figure below 1.0,
//! and a filtered-search story that is materially harder to get right. A memory
//! store holding a few tens of thousands of memories does not need that trade,
//! and an agent turn that spends 400 ms waiting on an embedding API will not
//! notice the 2 ms.
//!
//! So: exact now, with the API shaped so an ANN tier slots underneath without
//! callers changing. When a store outgrows it, `goldenhnsw`'s design is the one
//! the numbers point to. Shipping the approximate thing first would have been
//! optimising a stage that is not the bottleneck, and paying in recall for it.
//!
//! Both distributions from that run, for the record:
//!
//! ```text
//! uniform on the sphere      build    query    recall@10
//!   brute force                  -    2.4ms        1.000
//!   goldenhnsw (ef=200)     21.70s    565µs        0.874
//!   hnsw_rs    (ef=200)     37.93s    2.7ms        0.839
//!
//! clustered (200 topics)     build    query    recall@10
//!   brute force                  -    2.2ms        1.000
//!   goldenhnsw (ef=200)      5.26s    292µs        0.976
//!   hnsw_rs    (ef=200)     17.55s    1.0ms        0.951
//! ```
//!
//! Uniform vectors are worth reporting because they are the case ANN benchmarks
//! usually hide: in 128 dimensions their pairwise distances concentrate so hard
//! that a navigable-graph index has almost nothing to navigate, and recall
//! collapses for both implementations. Real embeddings cluster by topic, which
//! is why the second table is the one that should inform the decision — and why
//! quoting only the first would have been misleading in the flattering
//! direction.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The caller's identifier for an indexed vector. Opaque here.
pub type EntryId = u64;

/// How closeness is measured.
///
/// Not a default: picking the wrong one is a silent quality bug — results stay
/// plausible and get subtly worse — so the caller states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Metric {
    /// Angle between vectors, ignoring magnitude. Vectors are normalised on
    /// insert, so scores are in `[-1, 1]` and 1.0 is identical direction. What
    /// most sentence-embedding models are trained for.
    Cosine,
    /// Euclidean distance, reported negated so that — as with every metric
    /// here — larger is nearer. Magnitude-sensitive.
    L2,
}

/// A vector the index will not accept, and why.
///
/// Rejecting at the door rather than storing something unusable: a NaN
/// propagates into every comparison it touches and turns the ranking into
/// nonsense without ever erroring.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexError {
    /// The vector's length does not match the index's dimension.
    DimensionMismatch { expected: usize, got: usize },
    /// The vector contains a NaN or an infinity.
    NotFinite { position: usize },
    /// A zero vector under [`Metric::Cosine`], which has no direction and so
    /// no angle to anything.
    ZeroVectorUnderCosine,
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexError::DimensionMismatch { expected, got } => write!(
                f,
                "vector has {got} dimensions but the index holds {expected}; \
                 embeddings from two different models cannot share one index"
            ),
            IndexError::NotFinite { position } => write!(
                f,
                "vector component {position} is not finite; a NaN silently \
                 poisons every comparison it takes part in rather than erroring"
            ),
            IndexError::ZeroVectorUnderCosine => write!(
                f,
                "the zero vector has no direction, so it has no cosine \
                 similarity to anything; use Metric::L2 if magnitude is meaningful"
            ),
        }
    }
}

impl std::error::Error for IndexError {}

/// One search result. `score` is always "larger is nearer".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub id: EntryId,
    pub score: f32,
}

/// An exact vector index.
///
/// Every query scans every live vector, so results are the true top-`k` with no
/// recall parameter to tune and nothing to rebuild after a deletion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorIndex {
    dim: usize,
    metric: Metric,
    /// Parallel to `vectors`; `ids[i]` owns `vectors[i]`.
    ids: Vec<EntryId>,
    /// Flattened `len * dim`, so one allocation is scanned per query rather
    /// than chasing a pointer per entry.
    vectors: Vec<f32>,
    /// `EntryId` to row. Rebuilt on removal, which is why removal is O(1)
    /// amortised rather than O(n) — see [`VectorIndex::remove`].
    positions: HashMap<EntryId, usize>,
}

impl VectorIndex {
    pub fn new(dim: usize, metric: Metric) -> Self {
        VectorIndex {
            dim,
            metric,
            ids: Vec::new(),
            vectors: Vec::new(),
            positions: HashMap::new(),
        }
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn metric(&self) -> Metric {
        self.metric
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn contains(&self, id: EntryId) -> bool {
        self.positions.contains_key(&id)
    }

    /// Add or replace the vector for `id`.
    ///
    /// Re-inserting an existing id overwrites in place. A memory whose text was
    /// corrected gets a new embedding, and leaving the old one alongside would
    /// let one memory answer a query twice.
    pub fn insert(&mut self, id: EntryId, vector: &[f32]) -> Result<(), IndexError> {
        let prepared = self.prepare(vector)?;
        match self.positions.get(&id) {
            Some(&row) => {
                let start = row * self.dim;
                self.vectors[start..start + self.dim].copy_from_slice(&prepared);
            }
            None => {
                self.positions.insert(id, self.ids.len());
                self.ids.push(id);
                self.vectors.extend_from_slice(&prepared);
            }
        }
        Ok(())
    }

    /// Remove `id`, returning whether it was present.
    ///
    /// Removal is real, not a tombstone: the row is swapped with the last and
    /// truncated. Nothing accumulates, so a store that churns does not slow
    /// down over time, and a forgotten memory cannot resurface in a result.
    ///
    /// (Forgetting *within* the store is a different question, with its own
    /// answer in `rm_store` — this only stops the vector being searchable.)
    pub fn remove(&mut self, id: EntryId) -> bool {
        let Some(row) = self.positions.remove(&id) else {
            return false;
        };
        let last = self.ids.len() - 1;
        if row != last {
            let moved = self.ids[last];
            self.ids.swap(row, last);
            let (dst, src) = (row * self.dim, last * self.dim);
            self.vectors.copy_within(src..src + self.dim, dst);
            self.positions.insert(moved, row);
        }
        self.ids.truncate(last);
        self.vectors.truncate(last * self.dim);
        true
    }

    /// The `k` nearest entries, best first.
    pub fn search(&self, query: &[f32], k: usize) -> Result<Vec<Hit>, IndexError> {
        self.search_filtered(query, k, |_| true)
    }

    /// The `k` nearest entries for which `keep` returns true.
    ///
    /// The filter is applied *during* the scan, not to a fixed-size result
    /// afterwards, so a selective filter still returns a full `k` where `k`
    /// matching entries exist. Post-filtering a top-`k` is the usual shortcut
    /// and it silently returns two results for "what do I know about Alice in
    /// this session" whenever eight better-scoring memories belong to other
    /// sessions.
    ///
    /// This is also the property an ANN tier would have to preserve, and the
    /// main reason filtered approximate search is harder than it looks.
    pub fn search_filtered(
        &self,
        query: &[f32],
        k: usize,
        keep: impl Fn(EntryId) -> bool,
    ) -> Result<Vec<Hit>, IndexError> {
        let q = self.prepare(query)?;
        if k == 0 {
            return Ok(Vec::new());
        }

        let mut hits: Vec<Hit> = self
            .ids
            .iter()
            .enumerate()
            .filter(|(_, id)| keep(**id))
            .map(|(row, &id)| {
                let start = row * self.dim;
                let v = &self.vectors[start..start + self.dim];
                Hit {
                    id,
                    score: self.score(&q, v),
                }
            })
            .collect();

        // Descending by score; equal scores fall back to id so a tie does not
        // reorder between runs.
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        hits.truncate(k);
        Ok(hits)
    }

    /// Serialise to JSON.
    pub fn snapshot(&self) -> String {
        serde_json::to_string(self).expect("VectorIndex is always serialisable")
    }

    /// Restore from a snapshot.
    pub fn open(snapshot: &str) -> Result<Self, String> {
        serde_json::from_str(snapshot).map_err(|e| e.to_string())
    }

    /// Validate, and normalise if the metric calls for it.
    fn prepare(&self, vector: &[f32]) -> Result<Vec<f32>, IndexError> {
        if vector.len() != self.dim {
            return Err(IndexError::DimensionMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }
        if let Some(position) = vector.iter().position(|x| !x.is_finite()) {
            return Err(IndexError::NotFinite { position });
        }
        match self.metric {
            Metric::Cosine => {
                let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
                if norm == 0.0 {
                    return Err(IndexError::ZeroVectorUnderCosine);
                }
                Ok(vector.iter().map(|x| x / norm).collect())
            }
            Metric::L2 => Ok(vector.to_vec()),
        }
    }

    /// Larger is nearer, for both metrics.
    fn score(&self, q: &[f32], v: &[f32]) -> f32 {
        match self.metric {
            // Both sides are unit vectors, so the inner product is the cosine.
            Metric::Cosine => q.iter().zip(v).map(|(a, b)| a * b).sum(),
            // Negated so callers never have to ask which direction is better.
            Metric::L2 => -q
                .iter()
                .zip(v)
                .map(|(a, b)| (a - b) * (a - b))
                .sum::<f32>()
                .sqrt(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> VectorIndex {
        let mut ix = VectorIndex::new(3, Metric::Cosine);
        ix.insert(1, &[1.0, 0.0, 0.0]).unwrap();
        ix.insert(2, &[0.0, 1.0, 0.0]).unwrap();
        ix.insert(3, &[0.9, 0.1, 0.0]).unwrap();
        ix
    }

    fn ids(hits: &[Hit]) -> Vec<EntryId> {
        hits.iter().map(|h| h.id).collect()
    }

    // ---- exactness ---------------------------------------------------------

    #[test]
    fn results_are_the_true_top_k_in_order() {
        let hits = index().search(&[1.0, 0.0, 0.0], 3).unwrap();
        assert_eq!(ids(&hits), vec![1, 3, 2]);
        assert!((hits[0].score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_ignores_magnitude() {
        let ix = index();
        let near = ix.search(&[1.0, 0.0, 0.0], 1).unwrap();
        let far = ix.search(&[500.0, 0.0, 0.0], 1).unwrap();
        assert_eq!(near[0].id, far[0].id);
        assert!((near[0].score - far[0].score).abs() < 1e-6);
    }

    #[test]
    fn l2_does_not_ignore_magnitude() {
        let mut ix = VectorIndex::new(2, Metric::L2);
        ix.insert(1, &[1.0, 0.0]).unwrap();
        ix.insert(2, &[10.0, 0.0]).unwrap();
        // Same direction, different length: under L2 these are far apart.
        assert_eq!(ids(&ix.search(&[1.0, 0.0], 1).unwrap()), vec![1]);
        assert_eq!(ids(&ix.search(&[9.0, 0.0], 1).unwrap()), vec![2]);
    }

    #[test]
    fn l2_scores_are_negated_so_larger_is_always_nearer() {
        let mut ix = VectorIndex::new(2, Metric::L2);
        ix.insert(1, &[0.0, 0.0]).unwrap();
        let hits = ix.search(&[3.0, 4.0], 1).unwrap();
        assert!((hits[0].score - -5.0).abs() < 1e-6);
    }

    #[test]
    fn asking_for_more_than_exists_returns_what_exists() {
        assert_eq!(index().search(&[1.0, 0.0, 0.0], 99).unwrap().len(), 3);
    }

    #[test]
    fn an_empty_index_and_a_zero_k_both_return_nothing() {
        let empty = VectorIndex::new(3, Metric::Cosine);
        assert!(empty.search(&[1.0, 0.0, 0.0], 5).unwrap().is_empty());
        assert!(index().search(&[1.0, 0.0, 0.0], 0).unwrap().is_empty());
    }

    #[test]
    fn ties_are_broken_deterministically() {
        let mut ix = VectorIndex::new(2, Metric::Cosine);
        for id in [7, 3, 5] {
            ix.insert(id, &[1.0, 0.0]).unwrap();
        }
        assert_eq!(ids(&ix.search(&[1.0, 0.0], 3).unwrap()), vec![3, 5, 7]);
    }

    // ---- filtering ---------------------------------------------------------

    #[test]
    fn filtering_happens_during_the_scan_not_after_it() {
        // 1 and 3 outscore 2, so a post-filtered top-1 would return nothing.
        let hits = index()
            .search_filtered(&[1.0, 0.0, 0.0], 1, |id| id == 2)
            .unwrap();
        assert_eq!(ids(&hits), vec![2]);
    }

    #[test]
    fn a_filter_still_yields_a_full_k_when_enough_entries_match() {
        let mut ix = VectorIndex::new(2, Metric::Cosine);
        // Ten better-scoring entries the filter rejects, then five it accepts.
        for id in 0..10u64 {
            ix.insert(id, &[1.0, 0.0]).unwrap();
        }
        for id in 100..105u64 {
            ix.insert(id, &[0.6, 0.8]).unwrap();
        }
        let hits = ix.search_filtered(&[1.0, 0.0], 5, |id| id >= 100).unwrap();
        assert_eq!(hits.len(), 5, "post-filtering would have returned 0");
    }

    #[test]
    fn a_filter_matching_nothing_returns_nothing() {
        assert!(index()
            .search_filtered(&[1.0, 0.0, 0.0], 3, |_| false)
            .unwrap()
            .is_empty());
    }

    // ---- mutation ----------------------------------------------------------

    #[test]
    fn removal_takes_an_entry_out_of_results_for_good() {
        let mut ix = index();
        assert!(ix.remove(1));
        assert!(!ix.contains(1));
        assert_eq!(ix.len(), 2);
        assert_eq!(ids(&ix.search(&[1.0, 0.0, 0.0], 3).unwrap()), vec![3, 2]);
    }

    #[test]
    fn removal_keeps_the_remaining_entries_intact() {
        // The swap-with-last has to carry the moved entry's id mapping with it.
        let mut ix = VectorIndex::new(2, Metric::Cosine);
        for (id, v) in [(1u64, [1.0, 0.0]), (2, [0.0, 1.0]), (3, [0.7, 0.7])] {
            ix.insert(id, &v).unwrap();
        }
        ix.remove(1);
        assert_eq!(ix.len(), 2);
        assert_eq!(ids(&ix.search(&[0.0, 1.0], 2).unwrap()), vec![2, 3]);
        // And the moved row still answers for itself, not for its old neighbour.
        assert_eq!(ids(&ix.search(&[0.7, 0.7], 1).unwrap()), vec![3]);
    }

    #[test]
    fn removing_something_absent_reports_it_rather_than_panicking() {
        let mut ix = index();
        assert!(!ix.remove(404));
        assert_eq!(ix.len(), 3);
    }

    #[test]
    fn removing_every_entry_leaves_a_usable_empty_index() {
        let mut ix = index();
        for id in [1, 2, 3] {
            assert!(ix.remove(id));
        }
        assert!(ix.is_empty());
        assert!(ix.search(&[1.0, 0.0, 0.0], 1).unwrap().is_empty());
        ix.insert(9, &[1.0, 0.0, 0.0]).unwrap();
        assert_eq!(ids(&ix.search(&[1.0, 0.0, 0.0], 1).unwrap()), vec![9]);
    }

    #[test]
    fn reinserting_an_id_replaces_it_rather_than_duplicating_it() {
        let mut ix = index();
        ix.insert(1, &[0.0, 0.0, 1.0]).unwrap();
        assert_eq!(ix.len(), 3, "a corrected memory must not answer twice");
        let hits = ix.search(&[0.0, 0.0, 1.0], 1).unwrap();
        assert_eq!(hits[0].id, 1);
        assert!((hits[0].score - 1.0).abs() < 1e-6);
    }

    // ---- rejection at the door ---------------------------------------------

    #[test]
    fn a_wrong_dimension_is_rejected_on_insert_and_on_search() {
        let mut ix = index();
        assert_eq!(
            ix.insert(4, &[1.0, 0.0]),
            Err(IndexError::DimensionMismatch {
                expected: 3,
                got: 2
            })
        );
        assert!(ix.search(&[1.0, 0.0], 1).is_err());
    }

    #[test]
    fn a_non_finite_component_is_rejected_and_located() {
        let mut ix = index();
        assert_eq!(
            ix.insert(4, &[1.0, f32::NAN, 0.0]),
            Err(IndexError::NotFinite { position: 1 })
        );
        assert_eq!(
            ix.insert(4, &[1.0, 0.0, f32::INFINITY]),
            Err(IndexError::NotFinite { position: 2 })
        );
        assert_eq!(ix.len(), 3, "a rejected vector must not be stored");
    }

    #[test]
    fn the_zero_vector_is_rejected_under_cosine_and_allowed_under_l2() {
        let mut cosine = VectorIndex::new(2, Metric::Cosine);
        assert_eq!(
            cosine.insert(1, &[0.0, 0.0]),
            Err(IndexError::ZeroVectorUnderCosine)
        );
        let mut l2 = VectorIndex::new(2, Metric::L2);
        assert!(l2.insert(1, &[0.0, 0.0]).is_ok());
    }

    #[test]
    fn every_rejection_explains_itself() {
        for err in [
            IndexError::DimensionMismatch {
                expected: 3,
                got: 2,
            },
            IndexError::NotFinite { position: 1 },
            IndexError::ZeroVectorUnderCosine,
        ] {
            assert!(err.to_string().len() > 40, "{err:?}");
        }
    }

    // ---- persistence -------------------------------------------------------

    #[test]
    fn a_snapshot_round_trips() {
        let ix = index();
        let restored = VectorIndex::open(&ix.snapshot()).unwrap();
        assert_eq!(restored, ix);
        assert_eq!(
            ids(&restored.search(&[1.0, 0.0, 0.0], 3).unwrap()),
            vec![1, 3, 2]
        );
    }

    #[test]
    fn a_snapshot_survives_a_removal() {
        let mut ix = index();
        ix.remove(1);
        let restored = VectorIndex::open(&ix.snapshot()).unwrap();
        assert!(!restored.contains(1));
        assert_eq!(restored.len(), 2);
    }

    #[test]
    fn a_malformed_snapshot_is_reported_not_panicked_on() {
        assert!(VectorIndex::open("{ not json").is_err());
    }
}
