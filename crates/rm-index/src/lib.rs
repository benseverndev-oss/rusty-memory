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
//! vectors, serial insert both sides, it returned perfect recall where `hnsw_rs`
//! returned 0.987–0.990, and answered about 5× faster at matched `ef`. It does
//! not need replacing. (Caveat kept honest: `hnsw_rs` offers a parallel insert
//! this comparison did not use, so its build column is not its best achievable.)
//!
//! **The second, and the reason this crate has no graph in it:** at the scale a
//! personal memory store actually runs, approximate search is not yet worth its
//! costs. On clustered data at N=20,000 exact brute force answered in 2.7 ms,
//! against 104 µs for HNSW at `ef=50`. Genuinely faster — and bought with a
//! graph that has to stay consistent across deletions, and a filtered-search
//! story that is materially harder to get right over a graph than over a scan.
//! Filtering is the common case for memory, not the exception. An agent turn
//! that spends 400 ms waiting on an embedding API will not notice the 2.7 ms.
//!
//! Note what is *not* in that list. An earlier draft of this argument also cited
//! HNSW's recall being below 1.0, on numbers from a benchmark whose clustered
//! queries were accidentally drawn from a different set of centroids than the
//! corpus. Corrected, recall is 1.000 at `ef=50`, and the case for exact search
//! does not get to lean on accuracy. It rests on complexity alone, which is a
//! narrower claim and the honest one.
//!
//! So: exact now, with the API shaped so an ANN tier slots underneath without
//! callers changing. When a store outgrows it, `goldenhnsw`'s design is the one
//! the numbers point to.
//!
//! Both distributions from that run, for the record:
//!
//! ```text
//! uniform on the sphere      build    query    recall@10   mean top-1 cos 0.347
//!   brute force                  -    4.2ms        1.000
//!   goldenhnsw (ef=200)     32.55s    1.1ms        0.874
//!   hnsw_rs    (ef=200)     57.09s    2.9ms        0.841
//!
//! clustered (200 topics)     build    query    recall@10   mean top-1 cos 0.919
//!   brute force                  -    2.7ms        1.000
//!   goldenhnsw (ef=50)       6.83s    104µs        1.000
//!   hnsw_rs    (ef=50)      27.84s    582µs        0.987
//! ```
//!
//! Uniform vectors are worth reporting because they are the case ANN benchmarks
//! usually hide: in 128 dimensions their pairwise distances concentrate so hard
//! that a navigable-graph index has almost nothing to navigate, and recall
//! collapses for both implementations. Real embeddings cluster by topic, which
//! is why the second table is the one that should inform the decision — and why
//! quoting only the first would have been misleading in the flattering
//! direction.
//!
//! Build timings vary enough run to run (6.83 s and 15.60 s for the same
//! clustered index) that only the recall column should be read as a measurement.

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
/// Where a snapshot's JSON failed to parse, and what kind of failure it was —
/// never what `serde_json` said about it.
///
/// This used to be carried as `serde_json::Error::to_string()` inside
/// [`IndexError::CorruptSnapshot`], and that string quotes whatever the parser
/// did not like: `` unknown variant `sk-...`, expected `Cosine` or `L2` `` for
/// a bad `metric`. That example renders inside *backticks*, and `serde_json`
/// does not escape one that appears in the text it quotes -- so a backtick
/// inside a backtick span is indistinguishable from the span's own close, and
/// a fuzz of 180,000 configs proved no downstream scanner can tell them apart.
/// `line()`, `column()` and `classify()` are structural facts about *where*
/// and *what kind* of failure it was, not text read out of the snapshot, so a
/// type built only from those three cannot carry a secret regardless of what
/// malformed snapshot produced it. The cost: the parser's own reason --
/// `expected \`Cosine\` or \`L2\`` -- is gone, and that is the only part of
/// the old message that could ever have been one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotParseError {
    pub line: usize,
    pub column: usize,
    pub category: ParseCategory,
}

impl std::fmt::Display for SnapshotParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} at line {}, column {}",
            self.category.reason(),
            self.line,
            self.column
        )
    }
}

impl From<serde_json::Error> for SnapshotParseError {
    fn from(e: serde_json::Error) -> Self {
        SnapshotParseError {
            line: e.line(),
            column: e.column(),
            category: e.classify().into(),
        }
    }
}

/// What kind of thing went wrong while reading the JSON, per
/// [`serde_json::error::Category`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseCategory {
    /// The reader itself failed -- not applicable to a snapshot already held
    /// as a `&str`, but kept so this mirrors `serde_json`'s own categories
    /// rather than inventing a smaller set that a future `serde_json` could
    /// outgrow.
    Io,
    /// The bytes are not well-formed JSON at all.
    Syntax,
    /// The JSON is well-formed but does not match the shape an index
    /// snapshot takes -- a field of the wrong type, an unknown enum variant,
    /// a missing field.
    Data,
    /// The input ended before a complete value was read.
    Eof,
}

impl ParseCategory {
    fn reason(self) -> &'static str {
        match self {
            ParseCategory::Io => "the snapshot could not be read as bytes",
            ParseCategory::Syntax => "the snapshot is not well-formed JSON",
            ParseCategory::Data => {
                "the JSON there does not match the shape an index snapshot takes"
            }
            ParseCategory::Eof => "the snapshot ends before its JSON is complete",
        }
    }
}

impl From<serde_json::error::Category> for ParseCategory {
    fn from(c: serde_json::error::Category) -> Self {
        match c {
            serde_json::error::Category::Io => ParseCategory::Io,
            serde_json::error::Category::Syntax => ParseCategory::Syntax,
            serde_json::error::Category::Data => ParseCategory::Data,
            serde_json::error::Category::Eof => ParseCategory::Eof,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexError {
    /// The vector's length does not match the index's dimension.
    DimensionMismatch { expected: usize, got: usize },
    /// The vector contains a NaN or an infinity.
    NotFinite { position: usize },
    /// A zero vector under [`Metric::Cosine`], which has no direction and so
    /// no angle to anything.
    ZeroVectorUnderCosine,
    /// A snapshot's JSON never parsed at all.
    ///
    /// Was folded into [`IndexError::CorruptSnapshot`], which this crate's own
    /// doc comment describes as "the bytes were well-formed" -- true of every
    /// other case that variant covers and false of this one, so a caller
    /// reading `open`'s error had no way to tell "these bytes are not JSON, or
    /// not this shape of JSON" from "this is a well-formed index whose own
    /// numbers disagree with each other" without inspecting the string inside.
    /// Split out because the two ask different things of a caller: `Parse`
    /// means a truncated write or a wrong file, and a retry with better bytes
    /// can fix it. `CorruptSnapshot` means the bytes *are* a well-formed
    /// index and its invariants still contradict each other, which a retry of
    /// the same bytes cannot fix. `rm_store` and `rm_engine` already draw this
    /// line with a variant of this name; drawing it differently here would
    /// make the three doors read as three unrelated designs.
    Parse(SnapshotParseError),
    /// A snapshot parsed as JSON but described an index that cannot exist.
    ///
    /// Separate from [`IndexError::Parse`]: the bytes were well-formed, so
    /// nothing upstream would have flagged them, and the damage only shows up
    /// as a panic on the first query.
    CorruptSnapshot(String),
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
            IndexError::Parse(e) => write!(f, "could not read snapshot: {e}"),
            IndexError::CorruptSnapshot(why) => write!(
                f,
                "snapshot parsed but describes an impossible index: {why}"
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
    /// `EntryId` to row. Patched in place on removal — one entry, not a rebuild
    /// — which is what keeps removal O(1); see [`VectorIndex::remove`].
    ///
    /// Not serialised. It is wholly derived from `ids`, so persisting it would
    /// (a) let a snapshot describe an index whose map disagrees with its rows,
    /// and (b) make `snapshot()` non-deterministic, since `HashMap` iterates in
    /// a per-process-randomised order and `rm_store` promises snapshots that
    /// diff. [`VectorIndex::open`] rebuilds it and validates as it goes.
    #[serde(skip)]
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

    /// Whether this vector would be accepted, without storing it.
    ///
    /// For callers that write to more than one place and need the rejection to
    /// arrive before the first write, not between the first and the second.
    pub fn check(&self, vector: &[f32]) -> Result<(), IndexError> {
        self.prepare(vector).map(|_| ())
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
        // reorder between runs. Scores are finite by construction — `prepare`
        // rejects non-finite input at both doors — so the comparison is total.
        let order = |a: &Hit, b: &Hit| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        };

        // Partition around k before sorting, so the cost is O(n) in the scan
        // plus O(k log k) on the survivors rather than O(n log n) on all of
        // them. At N=20,000 and k=10 the full sort was a real share of the
        // 2.7 ms this crate's design argument rests on.
        if k < hits.len() {
            hits.select_nth_unstable_by(k, order);
            hits.truncate(k);
        }
        hits.sort_unstable_by(order);
        Ok(hits)
    }

    /// Serialise to JSON.
    pub fn snapshot(&self) -> String {
        serde_json::to_string(self).expect("VectorIndex is always serialisable")
    }

    /// Restore from a snapshot.
    ///
    /// Rebuilds the id-to-row map rather than trusting a stored one, and checks
    /// the invariants the rest of the crate indexes on faith. Without this a
    /// snapshot with the wrong number of floats parses cleanly and then panics
    /// on the first search, which is the failure mode the whole "reject at the
    /// door" premise exists to avoid — a restore path is a door.
    ///
    /// # Errors
    ///
    /// [`IndexError::Parse`] if the bytes never became a `VectorIndex` at all,
    /// and [`IndexError::CorruptSnapshot`] if they did and the numbers inside
    /// it contradict each other.
    pub fn open(snapshot: &str) -> Result<Self, IndexError> {
        let mut ix: VectorIndex =
            serde_json::from_str(snapshot).map_err(|e| IndexError::Parse(e.into()))?;

        if ix.dim == 0 {
            return Err(IndexError::CorruptSnapshot(
                "dimension is 0, so no vector could ever be inserted".to_string(),
            ));
        }
        let expected = ix.ids.len() * ix.dim;
        if ix.vectors.len() != expected {
            return Err(IndexError::CorruptSnapshot(format!(
                "{} ids at {} dimensions need {expected} floats, but the snapshot holds {}",
                ix.ids.len(),
                ix.dim,
                ix.vectors.len()
            )));
        }
        if let Some(position) = ix.vectors.iter().position(|x| !x.is_finite()) {
            return Err(IndexError::CorruptSnapshot(format!(
                "stored component {position} is not finite"
            )));
        }

        ix.positions = HashMap::with_capacity(ix.ids.len());
        for (row, &id) in ix.ids.iter().enumerate() {
            if ix.positions.insert(id, row).is_some() {
                return Err(IndexError::CorruptSnapshot(format!(
                    "id {id} appears more than once, so one memory would answer a query twice"
                )));
            }
        }
        Ok(ix)
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
                // Accumulated in f64. In f32 the sum of squares overflows to
                // infinity for components around 1e19, and `x / inf` is 0.0 for
                // every one of them — so a large finite vector passed the zero
                // check and was then stored as exactly the zero vector this
                // error exists to reject, scoring 0.0 against everything
                // including itself.
                let norm = vector
                    .iter()
                    .map(|x| f64::from(*x) * f64::from(*x))
                    .sum::<f64>()
                    .sqrt();
                if norm == 0.0 {
                    return Err(IndexError::ZeroVectorUnderCosine);
                }
                Ok(vector
                    .iter()
                    .map(|x| (f64::from(*x) / norm) as f32)
                    .collect())
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

    #[test]
    fn check_rejects_what_insert_would_reject_without_storing_it() {
        let ix = index();
        assert!(ix.check(&[1.0, 0.0]).is_err());
        assert!(ix.check(&[1.0, 0.0, 0.0]).is_ok());
        assert_eq!(ix.len(), 3, "check must not store");
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
        let err = VectorIndex::open("{ not json").unwrap_err();
        assert!(matches!(err, IndexError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn a_snapshot_parse_failure_names_a_position_never_the_text_that_broke_it() {
        // The awkward fixture: a backtick embedded in the very value
        // `serde_json` used to quote. Before this fix that failure travelled
        // as `IndexError::CorruptSnapshot(e.to_string())`, and the real
        // message this crate's own docs quote is `` unknown variant
        // `CANARY-...`, expected `Cosine` or `L2` `` -- rendered inside
        // backticks that `serde_json` does not escape, so a backtick inside
        // the value is indistinguishable from the span's own close. A
        // 180,000-config fuzz proved no downstream scanner can filter that
        // shape, which is why it is closed at the source instead.
        let canary = "CANARY-`-0123456789abcdef";
        let snapshot = format!(r#"{{"dim":2,"metric":"{canary}","ids":[],"vectors":[]}}"#);

        let err = VectorIndex::open(&snapshot).unwrap_err();
        let rendered = err.to_string();
        let IndexError::Parse(parse) = err else {
            panic!("expected a parse failure, got {rendered}");
        };
        assert_eq!(parse.category, ParseCategory::Data);
        assert_eq!(parse.line, 1);
        assert!(!rendered.contains(canary), "{rendered}");
        assert!(!rendered.contains('`'), "{rendered}");
        assert!(!rendered.contains("CANARY"), "{rendered}");
    }

    #[test]
    fn a_snapshot_with_the_wrong_number_of_floats_is_rejected_not_left_to_panic() {
        // Parses cleanly: two ids, but only one vector's worth of floats. Before
        // `open` validated, this restored fine, reported len() == 2, and panicked
        // on the first search with a slice range error.
        let broken = r#"{"dim":2,"metric":"Cosine","ids":[1,2],"vectors":[1.0,0.0]}"#;
        let err = VectorIndex::open(broken).unwrap_err();
        assert!(matches!(err, IndexError::CorruptSnapshot(_)), "got {err:?}");
        assert!(err.to_string().contains("need 4 floats"), "{err}");
    }

    #[test]
    fn a_snapshot_repeating_an_id_is_rejected() {
        // Two rows claiming one id: whichever won the map, the other row would
        // stay searchable and answer for an id it does not own.
        let broken = r#"{"dim":2,"metric":"Cosine","ids":[7,7],"vectors":[1.0,0.0,0.0,1.0]}"#;
        assert!(matches!(
            VectorIndex::open(broken).unwrap_err(),
            IndexError::CorruptSnapshot(_)
        ));
    }

    #[test]
    fn a_snapshot_holding_a_non_finite_component_is_rejected() {
        let broken = r#"{"dim":2,"metric":"L2","ids":[1],"vectors":[1e400,0.0]}"#;
        assert!(VectorIndex::open(broken).is_err());
    }

    #[test]
    fn snapshots_are_byte_stable() {
        // `rm_store` promises snapshots that diff, and this crate has to keep
        // that promise: a serialised HashMap iterates in a per-process order, so
        // an identical index used to emit different bytes every run.
        let mut ix = VectorIndex::new(2, Metric::Cosine);
        for id in 0..24u64 {
            ix.insert(id, &[1.0, id as f32 + 1.0]).unwrap();
        }
        let once = ix.snapshot();
        let twice = VectorIndex::open(&once).unwrap().snapshot();
        assert_eq!(once, twice);
        assert!(
            !once.contains("positions"),
            "the derived map should not be persisted at all"
        );
    }

    #[test]
    fn a_restored_index_still_removes_correctly() {
        // The rebuilt map has to be a working map, not just a populated one.
        let mut ix = VectorIndex::open(&index().snapshot()).unwrap();
        assert!(ix.remove(1));
        assert!(!ix.contains(1));
        assert_eq!(ids(&ix.search(&[1.0, 0.0, 0.0], 3).unwrap()), vec![3, 2]);
    }

    // ---- numeric edges -----------------------------------------------------

    #[test]
    fn a_huge_but_finite_vector_normalises_instead_of_collapsing_to_zero() {
        // Summing squares in f32 overflows to infinity around 1e19, and x/inf is
        // 0.0 — so this passed the zero-vector check and was then stored as the
        // very zero vector that check exists to reject, scoring 0.0 against
        // everything including itself.
        let mut ix = VectorIndex::new(4, Metric::Cosine);
        let huge = [2e19f32; 4];
        ix.insert(1, &huge).unwrap();
        let hits = ix.search(&huge, 1).unwrap();
        assert_eq!(hits[0].id, 1);
        assert!(
            (hits[0].score - 1.0).abs() < 1e-5,
            "a vector must be identical to itself, got {}",
            hits[0].score
        );
    }

    #[test]
    fn a_tiny_vector_keeps_its_direction() {
        // The same arithmetic at the other end: squares underflow to zero in f32
        // long before the vector itself is zero.
        let mut ix = VectorIndex::new(2, Metric::Cosine);
        ix.insert(1, &[3e-23, 4e-23]).unwrap();
        let hits = ix.search(&[3.0, 4.0], 1).unwrap();
        assert!((hits[0].score - 1.0).abs() < 1e-5, "got {}", hits[0].score);
    }
}
