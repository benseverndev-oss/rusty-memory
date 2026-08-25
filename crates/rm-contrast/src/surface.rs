//! The grid, the crossover, and the cell that keeps the whole thing honest.

use crate::score::{score_flat, score_store, Score};
use crate::workload::{workload, Params};

/// The coarse grid CI runs. Both axes include 0, so the calibration cell
/// exists and both crossovers read straight off it.
pub const COARSE_BACKDATES: [u64; 4] = [0, 20, 40, 60];
pub const COARSE_MIXES: [u64; 3] = [0, 50, 100];
pub const COARSE_SEEDS: u64 = 30;

/// One point on the surface: both stores, summed over every seed.
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub backdate_pct: u64,
    pub retrospective_pct: u64,
    pub flat: Score,
    pub store: Score,
}

/// Sum both stores over the grid.
pub fn sweep(backdates: &[u64], mixes: &[u64], seeds: u64) -> Vec<Cell> {
    let mut out = Vec::with_capacity(backdates.len() * mixes.len());
    for &backdate_pct in backdates {
        for &retrospective_pct in mixes {
            let params = Params {
                backdate_pct,
                retrospective_pct,
                ..Params::default()
            };
            let (mut flat, mut store) = (Score::default(), Score::default());
            for seed in 0..seeds {
                let w = workload(seed, &params);
                let f = score_flat(&w);
                let s = score_store(&w);
                flat.right += f.right;
                flat.wrong += f.wrong;
                flat.declined += f.declined;
                flat.ungradeable += f.ungradeable;
                store.right += s.right;
                store.wrong += s.wrong;
                store.declined += s.declined;
                store.ungradeable += s.ungradeable;
            }
            out.push(Cell {
                backdate_pct,
                retrospective_pct,
                flat,
                store,
            });
        }
    }
    out
}

/// The cell at no backdating and no retrospective queries.
///
/// Both stores must be perfect here. A latest-wins store is exactly the right
/// tool for an in-order present-tense workload, so if the control misses, the
/// generator or the scorer is unfair and the surface above it means nothing.
pub fn calibration(cells: &[Cell]) -> Option<&Cell> {
    cells
        .iter()
        .find(|c| c.backdate_pct == 0 && c.retrospective_pct == 0)
}

/// The lowest backdate rate at which the control drops below `floor`, in the
/// column asking `retrospective_pct` of its questions about the past.
///
/// `None` means it never does -- which, if it happens at every mix, is the
/// finding that the machinery does not pay on this workload.
pub fn crossover(cells: &[Cell], retrospective_pct: u64, floor: f64) -> Option<u64> {
    let mut column: Vec<&Cell> = cells
        .iter()
        .filter(|c| c.retrospective_pct == retrospective_pct)
        .collect();
    column.sort_by_key(|c| c.backdate_pct);
    column
        .into_iter()
        .find(|c| c.flat.accuracy() < floor)
        .map(|c| c.backdate_pct)
}

/// The tie rate used by [`unanswerable`]. Off the grid, deliberately.
pub const TIE_PCT: u64 = 25;

/// What each store does with a question that has no right answer.
///
/// Measured apart from the surface because it is a different phenomenon and
/// mixing it in would confound the temporal axes with the refusal behaviour:
/// `ValidInterval` cannot build a timeline when two segments collide, so it
/// refuses the *whole read* even for an instant where nothing is ambiguous.
///
/// Returns `(store, flat)`. The interesting figures are the store's `declined`
/// -- questions it refused that did have an answer, a cost of the history-wide
/// refusal -- and the fact that the control's is always zero, because it
/// answers everything it is asked whether or not an answer exists.
pub fn unanswerable(seeds: u64) -> (Score, Score) {
    let params = Params {
        tie_pct: TIE_PCT,
        ..Params::default()
    };
    let (mut store, mut flat) = (Score::default(), Score::default());
    for seed in 0..seeds {
        let w = workload(seed, &params);
        let s = score_store(&w);
        let f = score_flat(&w);
        store.right += s.right;
        store.wrong += s.wrong;
        store.declined += s.declined;
        store.ungradeable += s.ungradeable;
        flat.right += f.right;
        flat.wrong += f.wrong;
        flat.declined += f.declined;
        flat.ungradeable += f.ungradeable;
    }
    (store, flat)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coarse() -> Vec<Cell> {
        sweep(&COARSE_BACKDATES, &COARSE_MIXES, COARSE_SEEDS)
    }

    /// **The guard.** At no backdating and no retrospective queries, a
    /// latest-wins store is exactly the right tool. If it misses here the
    /// benchmark is rigged, and this fails rather than reporting a flattering
    /// surface.
    #[test]
    fn the_calibration_cell_is_perfect_for_both_stores() {
        let cells = coarse();
        let c = calibration(&cells).expect("the grid must contain (0, 0)");
        assert_eq!(
            c.flat.accuracy(),
            1.0,
            "the control failed a workload it is built for: the benchmark is \
             unfair, not the control weak"
        );
        assert_eq!(c.store.accuracy(), 1.0, "this store failed the easy case");
    }

    /// Companion one: the x-axis has to move the control.
    #[test]
    fn backdating_costs_the_control_something() {
        let cells = coarse();
        let worst = cells
            .iter()
            .filter(|c| c.retrospective_pct == 0)
            .map(|c| c.flat.accuracy())
            .fold(f64::INFINITY, f64::min);
        assert!(
            worst < 1.0,
            "backdating never cost the control a present-tense answer, so the \
             x-axis is measuring nothing"
        );
    }

    /// Companion two: the control is not rigged against. It must get some
    /// retrospective questions right -- the ones where nothing changed.
    #[test]
    fn the_control_still_answers_some_retrospective_questions() {
        let cells = coarse();
        let best = cells
            .iter()
            .filter(|c| c.retrospective_pct == 100)
            .map(|c| c.flat.accuracy())
            .fold(0.0, f64::max);
        assert!(
            best > 0.0,
            "the control got no retrospective question right anywhere, so the \
             query set is rigged against it"
        );
    }

    /// Companion three: the sharpest difference between the two stores is
    /// reachable at all, and it is measured off the grid.
    ///
    /// A question with no right answer is where this store declines and the
    /// control answers anyway, having no way not to. If the generator never
    /// produced one, the refusal machinery would never be exercised.
    #[test]
    fn questions_with_no_right_answer_occur_and_only_one_store_declines() {
        let (store, flat) = unanswerable(COARSE_SEEDS);
        assert!(
            store.ungradeable > 0,
            "nothing was ever ambiguous, so the store refusal had nothing to              refuse"
        );
        assert_eq!(
            flat.ungradeable, store.ungradeable,
            "both stores met the same unanswerable questions"
        );
        assert_eq!(
            flat.declined, 0,
            "the control declined something, which it has no way to do"
        );
        assert!(
            store.declined > 0,
            "ValidInterval refuses a read whose history contains a collision,              even at an instant where nothing is ambiguous -- if that stopped              happening, this measurement has gone quiet rather than clean"
        );
    }

    /// The grid itself is tie-free, so the surface measures the two temporal
    /// axes and nothing else.
    #[test]
    fn the_grid_carries_no_unanswerable_questions() {
        let cells = coarse();
        assert!(
            cells.iter().all(|c| c.store.ungradeable == 0),
            "a tie reached the grid, confounding the temporal axes with the              refusal behaviour"
        );
    }

    #[test]
    fn a_crossover_is_the_first_rate_that_drops_below_the_floor() {
        let cells = vec![
            Cell {
                backdate_pct: 0,
                retrospective_pct: 0,
                flat: Score {
                    right: 10,
                    wrong: 0,
                    declined: 0,
                    ungradeable: 0,
                },
                store: Score {
                    right: 10,
                    wrong: 0,
                    declined: 0,
                    ungradeable: 0,
                },
            },
            Cell {
                backdate_pct: 20,
                retrospective_pct: 0,
                flat: Score {
                    right: 8,
                    wrong: 2,
                    declined: 0,
                    ungradeable: 0,
                },
                store: Score {
                    right: 10,
                    wrong: 0,
                    declined: 0,
                    ungradeable: 0,
                },
            },
        ];
        assert_eq!(crossover(&cells, 0, 0.95), Some(20));
        assert_eq!(crossover(&cells, 0, 0.5), None, "never drops that far");
        assert_eq!(crossover(&cells, 100, 0.95), None, "no such column");
    }
}
