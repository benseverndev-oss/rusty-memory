//! `rmem-mcp`.
//!
//! Open the store, serve, exit. Everything with a decision in it lives in the
//! library, where it is tested.
//!
//! Two transports. With no arguments it serves stdin, which is one client on
//! one machine. With `--http <addr>` it serves a socket, which is what a store
//! several agents share has to be -- a `flock` on a sidecar file serialises
//! processes on one filesystem and shares nothing beyond it.
//!
//! # This file owns the one rule the compiler cannot check
//!
//! stdout is the transport. Nothing but MCP messages may go there, so this is
//! the only place in the crate that names `stdout` at all, and the only thing
//! it does with it is hand it to the serve loop. Everything else — including
//! the one refusal below that a person actually needs to read — goes to
//! stderr, which the specification leaves free for exactly this.

use std::io::Write;
use std::process::ExitCode;

use rm_host::config::Config;
use rm_mcp::Server;

const CONFIG: &str = "rmem.toml";

/// An environment variable naming the config to use instead of `./rmem.toml`.
///
/// Several agents sharing one store is the point, and each of them runs in its
/// own directory. Without this every project would need its own `rmem.toml`
/// pointing at the same store, and one of them would eventually point somewhere
/// else -- a divergence nothing reports, because two stores are not an error.
const CONFIG_ENV: &str = "RMEM_CONFIG";

/// The config this process should read.
fn config_path() -> std::path::PathBuf {
    std::env::var_os(CONFIG_ENV)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(CONFIG))
}

/// The environment variable holding the bearer token clients must present.
///
/// The NAME is here and the value is not, for the reason `rmem.toml` names an
/// `api_key_env` rather than a key: a secret written where a name belongs ends
/// up in a file somebody commits.
const TOKEN_ENV: &str = "RMEM_TOKEN";

/// Read per request, not once. `rmem` takes one reading because it does one
/// thing and exits; a server that took one would answer `about` with the time
/// it started, and a long-lived one would drift arbitrarily far.
fn now() -> rm_engine::Timestamp {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `--http <addr>`, if it was asked for.
fn http_addr() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--http" {
            return args.next();
        }
    }
    None
}

/// Serve a socket instead of stdin.
///
/// Nothing here writes to stdout: over HTTP the transport is the socket, and
/// the one thing a person needs -- which address it came up on -- goes to
/// stderr where it cannot be mistaken for a message.
fn serve_http(addr: &str) -> ExitCode {
    let listener = match std::net::TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "could not bind {addr}: {e}");
            return ExitCode::from(1);
        }
    };
    let bound = match listener.local_addr() {
        Ok(a) => a,
        Err(e) => {
            let _ = writeln!(
                std::io::stderr(),
                "bound {addr} but could not read it back: {e}"
            );
            return ExitCode::from(1);
        }
    };
    let guard = match rm_mcp::http::Guard::new(bound, std::env::var(TOKEN_ENV).ok()) {
        Ok(g) => g,
        Err(why) => {
            let _ = writeln!(std::io::stderr(), "{why}");
            return ExitCode::from(1);
        }
    };
    let _ = writeln!(std::io::stderr(), "serving MCP over HTTP on {bound}");
    rm_mcp::http::serve(
        listener,
        config_path().as_path().to_path_buf(),
        Config::provider,
        guard,
        now,
    );
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    // Built here rather than inside the server, and passed in as a factory, so
    // that the tools which never embed anything never demand an API key. The
    // closure is `Config::provider` itself: the server holds the config.
    let mut server = match Server::open(config_path().as_path(), Config::provider) {
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

    if let Some(addr) = http_addr() {
        return serve_http(&addr);
    }

    let stdin = std::io::stdin();
    let result = server.serve(stdin.lock(), std::io::stdout().lock(), now);

    match result {
        // The input ended, which is how a client shuts a stdio server down.
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "{e}");
            ExitCode::from(1)
        }
    }
}
