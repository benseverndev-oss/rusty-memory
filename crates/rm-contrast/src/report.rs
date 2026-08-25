//! The surface, formatted. Every figure is computed, never typed.

use crate::surface::{
    calibration, crossover, sweep, unanswerable, Cell, COARSE_BACKDATES, COARSE_MIXES,
    COARSE_SEEDS, TIE_PCT,
};

/// The finer grid, for the README figure.
const FULL_BACKDATES: [u64; 9] = [0, 10, 20, 30, 40, 50, 60, 70, 80];
const FULL_MIXES: [u64; 5] = [0, 25, 50, 75, 100];
const FULL_SEEDS: u64 = 200;

/// The floor the crossover is read against.
const FLOOR: f64 = 0.95;

fn row(cells: &[Cell], backdate_pct: u64, mixes: &[u64]) -> String {
    let mut s = format!("| {backdate_pct}% ");
    for &m in mixes {
        let c = cells
            .iter()
            .find(|c| c.backdate_pct == backdate_pct && c.retrospective_pct == m)
            .expect("every grid point was swept");
        s.push_str(&format!(
            "| {:.3} / {:.3} ",
            c.flat.accuracy(),
            c.store.accuracy()
        ));
    }
    s.push_str("|\n");
    s
}

/// The whole surface, as markdown.
pub fn table(full: bool) -> String {
    let (backdates, mixes, seeds): (&[u64], &[u64], u64) = if full {
        (&FULL_BACKDATES, &FULL_MIXES, FULL_SEEDS)
    } else {
        (&COARSE_BACKDATES, &COARSE_MIXES, COARSE_SEEDS)
    };
    let cells = sweep(backdates, mixes, seeds);
    let mut out = String::new();

    out.push_str(&format!(
        "Backdate rate down, retrospective query share across. Each cell is \
         **flat / store** accuracy, summed over {seeds} seeds.\n\n"
    ));

    out.push_str("| backdate ");
    for m in mixes {
        out.push_str(&format!("| {m}% retrospective "));
    }
    out.push_str("|\n|---");
    for _ in mixes {
        out.push_str("|---");
    }
    out.push_str("|\n");
    for &b in backdates {
        out.push_str(&row(&cells, b, mixes));
    }

    // The guard, printed rather than assumed.
    let c = calibration(&cells).expect("the grid contains (0, 0)");
    let rigged = c.flat.accuracy() < 1.0;
    out.push_str(&format!(
        "\n**calibration** — at 0% backdating and 0% retrospective queries the \
         control scores {:.3} and the store {:.3}. {}\n\n",
        c.flat.accuracy(),
        c.store.accuracy(),
        if rigged {
            "**RIGGED**: the control failed a workload it is built for, so \
             every number above it is measuring an unfair generator."
        } else {
            "Both perfect, which is what a latest-wins store is for and what \
             makes the rest of the surface worth reading."
        }
    ));

    for &m in mixes {
        let at = crossover(&cells, m, FLOOR);
        out.push_str(&format!(
            "- at {m}% retrospective, the control first drops below {FLOOR:.2} at {}\n",
            match at {
                Some(b) => format!("**{b}% backdating**"),
                None => format!(
                    "**no backdating rate up to {}%**",
                    backdates[backdates.len() - 1]
                ),
            }
        ));
    }

    // Ties, measured off the grid because they confound the temporal axes.
    let (store, flat) = unanswerable(seeds);
    out.push_str(&format!(
        "\n**Questions with no right answer**, measured separately at a {TIE_PCT}% tie \
         rate because `ValidInterval` refuses a read whose history contains a \
         collision even at an instant where nothing is ambiguous.\n\n\
         Of {} asked, {} had no right answer. Of the rest the store refused {} \
         it could have answered; the control refused {}, because it has no way \
         to.\n",
        store.asked(),
        store.ungradeable,
        store.declined,
        flat.declined,
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_names_both_stores_and_the_calibration() {
        let t = table(false);
        for want in [
            "backdate",
            "retrospective",
            "calibration",
            "no right answer",
        ] {
            assert!(t.contains(want), "table is missing {want:?}\n{t}");
        }
        assert!(
            !t.contains("RIGGED"),
            "the calibration cell failed, so the surface means nothing:\n{t}"
        );
    }
}
