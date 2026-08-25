//! Where the machinery starts paying.
//!
//! `benches/locomo` measured retrieval, found that a twenty-line control beat
//! the pipeline, and this project answered that the machinery is for something
//! else: *"Raw turns cannot answer `about(entity, attribute, valid_t, tx_t)`.
//! They cannot say a fact was corrected [...] or what was believed last Tuesday
//! about last May."*
//!
//! That is an argument, not a measurement. This crate is the measurement.
//!
//! # It is not built to win
//!
//! A task showing bi-temporality beating latest-wins is trivial to build and
//! worth nothing: make every query retrospective and the control loses by
//! construction. What is unknown is *where the crossover sits* -- how much
//! backdating and how many retrospective questions a workload needs before the
//! machinery earns its cost.
//!
//! The control wins the entire low end of the surface by design, and a test
//! asserts it does. See [`surface::calibration`].

pub mod flat;
pub mod score;
pub mod surface;
pub mod workload;
