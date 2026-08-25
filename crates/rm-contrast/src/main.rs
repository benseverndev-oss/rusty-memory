//! `rm-contrast --report [--full]`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--report") {
        println!(
            "{}",
            rm_contrast::report::table(args.iter().any(|a| a == "--full"))
        );
    } else {
        eprintln!(
            "rm-contrast --report [--full]    sweep the grid and print the surface\n\
             \n\
             The coarse grid runs under `cargo test -p rm-contrast`. `--full` is\n\
             the finer sweep behind the README figure."
        );
        std::process::exit(2);
    }
}
