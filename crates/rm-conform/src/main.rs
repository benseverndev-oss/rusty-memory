//! `rm-conform --report`.
//!
//! The suite runs in CI as ordinary tests; this is the same measurements
//! formatted for the README, so the two cannot drift apart.

fn main() {
    if std::env::args().skip(1).any(|a| a == "--report") {
        println!("{}", rm_conform::report::table());
    } else {
        eprintln!(
            "rm-conform --report    run the sweep and print the headline table\n\
             \n\
             The properties themselves run under `cargo test -p rm-conform`.\n\
             This exists so the table in the README is computed rather than typed."
        );
        std::process::exit(2);
    }
}
