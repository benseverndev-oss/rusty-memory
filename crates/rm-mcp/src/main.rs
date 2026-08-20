//! `rmem-mcp`.
//!
//! Open the store, serve stdin, exit. Everything with a decision in it lives in
//! the library, where it is tested.
//!
//! # This file owns the one rule the compiler cannot check
//!
//! stdout is the transport. Nothing but MCP messages may go there, so this is
//! the only place in the crate that names `stdout` at all, and the only thing
//! it does with it is hand it to the serve loop. Everything else — including
//! the one refusal below that a person actually needs to read — goes to
//! stderr, which the specification leaves free for exactly this.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use rm_host::config::Config;
use rm_mcp::Server;

const CONFIG: &str = "rmem.toml";

fn main() -> ExitCode {
    // Built here rather than inside the server, and passed in as a factory, so
    // that the tools which never embed anything never demand an API key. The
    // closure is `Config::provider` itself: the server holds the config.
    let mut server = match Server::open(Path::new(CONFIG), Config::provider) {
        Ok(server) => server,
        Err(e) => {
            // To stderr, and before a single byte reaches stdout. A config
            // error reported through the protocol would be a config error
            // reported to a model, and the person who can fix it may never see
            // the transcript.
            let _ = writeln!(std::io::stderr(), "{e}");
            return ExitCode::from(1);
        }
    };

    let stdin = std::io::stdin();
    let result = server.serve(stdin.lock(), std::io::stdout().lock(), || {
        // Read per request, not once. `rmem` takes one reading because it does
        // one thing and exits; a server that took one would answer `about`
        // with the time it started, and a long-lived one would drift
        // arbitrarily far.
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    });

    match result {
        // The input ended, which is how a client shuts a stdio server down.
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "{e}");
            ExitCode::from(1)
        }
    }
}
