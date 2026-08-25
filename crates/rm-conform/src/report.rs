//! The headline table, computed.
//!
//! Every figure here comes from the same functions the tests call. A README
//! number that was typed by hand is a number that goes stale silently -- which
//! is, with some irony, the exact failure this harness found twice in the
//! codebase it measures.

use crate::applicability::{self, agreement, depth_monotonic, rescope_history};
use crate::differential::{default_strategies, refusal_agreement, sweep, Disagreement};
use crate::generate::{generate, Params};
use crate::invariants::{monotonic_in_transaction_time, order_independent, probe_grid};
use rm_engine::Strategy;

/// Seeds swept for the reported figures. Fixed and printed, so any number here
/// can be reproduced.
pub const SEEDS: u64 = 500;

/// Seeds swept for the applicability rows.
///
/// Fewer than [`SEEDS`] and printed alongside it, because each one builds an
/// engine and writes a dozen decisions through the real command path rather
/// than comparing two pure functions. A number that was quietly smaller than
/// the one above it would overstate what was measured.
pub const SCOPE_SEEDS: u64 = 60;

fn verdict(passed: bool) -> &'static str {
    if passed {
        "1.000"
    } else {
        "**FAILED**"
    }
}

/// The whole table, as markdown.
pub fn table() -> String {
    let params = Params::default();
    let probes = probe_grid();

    let disagreements: Vec<Disagreement> = sweep(0..SEEDS, &params, &default_strategies());
    let refusals = refusal_agreement(0..SEEDS, &default_strategies());

    let temporal = [Strategy::MostRecent, Strategy::ValidInterval];
    let monotonic = temporal.iter().all(|s| {
        (0..SEEDS).all(|seed| {
            let h = generate(seed, &params);
            [1, 3, 6, 9]
                .iter()
                .all(|c| monotonic_in_transaction_time(&h, *c, &probes, s.clone()))
        })
    });
    let ordered = temporal.iter().all(|s| {
        (0..SEEDS).all(|seed| order_independent(&generate(seed, &params), &probes, s.clone()))
    });

    let comparisons = refusals.both_refused + refusals.both_answered;
    let mut out = String::new();

    out.push_str(&format!(
        "Seeds `0..{SEEDS}` for the merge sweep and `0..{SCOPE_SEEDS}` for the \
         applicability rows, params `{params:?}`, {} probes per history.\n\n",
        probes.len()
    ));
    out.push_str("| property | result |\n|---|---|\n");
    out.push_str(&format!(
        "| merge agreement, 8 strategies | {} |\n",
        verdict(disagreements.is_empty())
    ));
    out.push_str(&format!(
        "| refusal correctness | {} |\n",
        verdict(refusals.exact())
    ));
    out.push_str(&format!(
        "| transaction-time monotonicity | {} |\n",
        verdict(monotonic)
    ));
    out.push_str(&format!(
        "| arrival-order independence | {} |\n",
        verdict(ordered)
    ));
    out.push_str(&format!(
        "| decision-layer time coverage | {:.3} |\n",
        crate::decisions::time_coverage()
    ));

    // A smaller seed range than the merge sweep: each world builds a real
    // engine and records a dozen decisions through it, where the merge sweep
    // compares two pure functions.
    let scope_params = applicability::Params::default();
    out.push_str(&format!(
        "| applicability agreement | {} |\n",
        verdict(agreement(0..SCOPE_SEEDS, &scope_params))
    ));
    out.push_str(&format!(
        "| depth monotonicity | {} |\n",
        verdict(depth_monotonic(0..SCOPE_SEEDS, &scope_params))
    ));
    out.push_str(&format!(
        "| rescope keeps its history | {} |\n",
        verdict(rescope_history(0..SCOPE_SEEDS, &scope_params))
    ));

    out.push_str(&format!(
        "\n{} of {comparisons} comparisons reached a refusal, {} answered. \
         A suite in which nothing refused would report perfect refusal \
         correctness having measured none of it.\n",
        refusals.both_refused, refusals.both_answered
    ));

    if !disagreements.is_empty() {
        out.push_str(&format!(
            "\n## {} disagreement(s)\n\nFirst, shrunk to its minimum:\n\n```\n{:#?}\n```\n",
            disagreements.len(),
            disagreements[0]
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_reports_every_row_and_no_failures() {
        let t = table();
        for row in [
            "merge agreement",
            "refusal correctness",
            "transaction-time monotonicity",
            "arrival-order independence",
            "decision-layer time coverage",
            "applicability agreement",
            "depth monotonicity",
            "rescope keeps its history",
        ] {
            assert!(t.contains(row), "row missing from the table: {row}\n{t}");
        }
        assert!(!t.contains("FAILED"), "{t}");
        assert!(!t.contains("disagreement(s)"), "{t}");
    }
}
