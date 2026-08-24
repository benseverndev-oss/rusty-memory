//! Scores this store on what it claims to do.
//!
//! See `docs/superpowers/specs/2026-08-24-rm-conform-design.md`. The short
//! version: recall@10 is the wrong *kind* of metric for a correctness claim, so
//! the headline here is a claim to hold rather than a score to raise.
//!
//! Everything measured elsewhere in this workspace measures retrieval, and
//! `benches/locomo` records the finding that ended that line of work: a
//! twenty-line control beats the pipeline on it, and none of the distinctive
//! machinery serves it. `rm_survivor`, the bi-temporal read, `Supersession` and
//! `Standing` are the parts that are not in every other memory store, and they
//! contributed nothing to the number the project reported.
//!
//! Ground truth here is computed by [`reference`], a second implementation of
//! survivorship written for auditability rather than performance. That is the
//! whole claim to be measuring anything: "known by construction" means nothing
//! unless the expected answer is worked out without asking the code under test.

pub mod differential;
pub mod engine_harness;
pub mod generate;
pub mod history;
pub mod invariants;
pub mod reference;
pub mod rng;
