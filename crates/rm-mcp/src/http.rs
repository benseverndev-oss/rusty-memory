//! The same server, reachable from another machine.
//!
//! stdio serves one client on one box: the store is a file, the lock is a
//! `flock` on a sidecar beside it, and "many agents" means many processes on
//! one filesystem. A shared memory -- several agents reading each other's
//! decisions -- needs a socket, and this is it.
//!
//! # Why there is no SSE here
//!
//! MCP's Streamable HTTP transport lets a server answer a POST with either one
//! JSON object or an event stream. The stream exists for servers that send
//! things of their own: progress, notifications, `sampling/createMessage`. This
//! one never does. Every tool call is a question with exactly one answer, and
//! the sampling this store might have wanted was for embeddings, which the
//! specification's sampling does not cover. So every response is a single JSON
//! object, and the streaming half is absent rather than stubbed.
//!
//! # Why the HTTP is written out
//!
//! One method, one path, and a body with a length. What it must not do is
//! misparse a request arriving from a network, so it is strict rather than
//! accommodating: exact framing, hard limits, and a refusal for anything it
//! does not recognise.
//!
//! # What it does not do
//!
//! No TLS and no OAuth. The specification's authorization chapter describes an
//! OAuth 2.1 resource server, which is a great deal more than a bearer token,
//! and pretending otherwise in a doc comment would be worse than saying so.
//! Anything reachable from a hostile network wants a reverse proxy in front
//! that terminates TLS and does the real thing.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use rm_engine::{Completer, Embedder, Timestamp};
use rm_host::config::Config;
use rm_host::HostError;

/// The most a request head may be, in bytes.
///
/// Generous for the handful of headers MCP defines, and small enough that a
/// client which never sends the blank line cannot make this allocate.
const MAX_HEAD: usize = 8 * 1024;

/// The most a body may be, in bytes.
///
/// A `remember` of a very long turn is the biggest thing anyone sends, and it
/// is prose. A megabyte is far more than that and far less than a denial of
/// service.
const MAX_BODY: usize = 1024 * 1024;

/// How this listener is protected.
#[derive(Debug)]
pub struct Guard {
    /// Required in `Authorization: Bearer` when set.
    token: Option<String>,
}

impl Guard {
    /// The guard for an address, refusing a configuration that would expose the
    /// store unauthenticated.
    ///
    /// Loopback without a token is allowed: it is the local case the
    /// specification calls out, and anything that can reach it can already read
    /// the store off the disk. Anything else without a token is refused rather
    /// than served, because a memory on an open port with no credential is not
    /// a deployment anybody chooses on purpose.
    pub fn new(addr: SocketAddr, token: Option<String>) -> Result<Guard, String> {
        let loopback = match addr.ip() {
            IpAddr::V4(v4) => v4.is_loopback(),
            IpAddr::V6(v6) => v6.is_loopback(),
        };
        if !loopback && token.is_none() {
            return Err(format!(
                "refusing to serve {addr} without a token: this address is reachable from other machines, and the store would be readable and writable by anything that can open a socket to it. Set RMEM_TOKEN to a secret, which clients send as `Authorization: Bearer`, or bind a loopback address instead."
            ));
        }
        Ok(Guard { token })
    }
}

/// Serve until the listener stops, one thread per connection.
///
/// A [`Server`](crate::Server) per connection rather than one shared behind a
/// lock: the protocol version is negotiated per client, and shared state would
/// let one client's handshake decide another's era. Opening one costs reading
/// `rmem.toml`; the store itself is opened per operation either way.
pub fn serve<F, P>(
    listener: TcpListener,
    config: PathBuf,
    provider: F,
    guard: Guard,
    clock: fn() -> Timestamp,
) where
    F: Fn(&Config) -> Result<P, HostError> + Copy + Send + 'static,
    P: Completer + Embedder,
{
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let config = config.clone();
        let token = guard.token.clone();
        // Detached. A panic in one connection takes that connection and not the
        // listener, which is the reason each gets a thread rather than a loop.
        std::thread::spawn(move || {
            let _ = answer(stream, &config, provider, token.as_deref(), clock);
        });
    }
}

fn answer<F, P>(
    mut stream: TcpStream,
    config: &Path,
    provider: F,
    token: Option<&str>,
    clock: fn() -> Timestamp,
) -> std::io::Result<()>
where
    F: Fn(&Config) -> Result<P, HostError> + Copy,
    P: Completer + Embedder,
{
    let mut reader = BufReader::new(stream.try_clone()?);
    let head = match read_head(&mut reader) {
        Ok(head) => head,
        Err(s) => return reply(&mut stream, s, "text/plain", s.reason()),
    };
    if let Err(s) = head.check(token) {
        return reply(&mut stream, s, "text/plain", s.reason());
    }

    let mut body = vec![0u8; head.length];
    if reader.read_exact(&mut body).is_err() {
        let s = Status::BadRequest;
        return reply(
            &mut stream,
            s,
            "text/plain",
            "body shorter than Content-Length",
        );
    }
    let Ok(body) = String::from_utf8(body) else {
        let s = Status::BadRequest;
        return reply(&mut stream, s, "text/plain", "body is not UTF-8");
    };

    let mut server = match crate::Server::open(config, provider) {
        Ok(server) => server,
        // The config is wrong, which is the operator's problem and not the
        // client's. 500 rather than 400: the request was fine.
        Err(e) => {
            let s = Status::ServerError;
            return reply(&mut stream, s, "text/plain", &e.to_string());
        }
    };

    match server.handle(body.trim(), clock()) {
        // A notification, which MUST NOT be answered. 202 with no body is what
        // the transport says to send instead.
        None => reply(&mut stream, Status::Accepted, "text/plain", ""),
        Some(response) => reply(&mut stream, Status::Ok, "application/json", &response),
    }
}

/// What a request said, once it is known to be one this server will answer.
#[derive(Debug, PartialEq, Eq)]
struct Head {
    length: usize,
    origin: Option<String>,
    authorization: Option<String>,
}

impl Head {
    /// The checks that decide whether this is answered at all.
    fn check(&self, token: Option<&str>) -> Result<(), Status> {
        // DNS rebinding. A browser attaches `Origin`, and a script on any page
        // can point at a loopback port; this server has no browser clients, so
        // a request carrying one is not from a client of ours. The
        // specification requires the check and names 403.
        if self.origin.is_some() {
            return Err(Status::Forbidden);
        }
        if let Some(want) = token {
            let ok = self
                .authorization
                .as_deref()
                .and_then(|a| a.strip_prefix("Bearer "))
                .is_some_and(|got| constant_time_eq(got.trim(), want));
            if !ok {
                return Err(Status::Unauthorized);
            }
        }
        if self.length > MAX_BODY {
            return Err(Status::TooLarge);
        }
        Ok(())
    }
}

/// Equal, without telling the caller how far it got.
///
/// `==` on a token leaks its prefix through timing to anyone who can measure
/// enough requests. This is three lines and removes the question.
fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

fn read_head(reader: &mut impl BufRead) -> Result<Head, Status> {
    let mut line = String::new();
    let mut read = reader
        .read_line(&mut line)
        .map_err(|_| Status::BadRequest)?;

    let method = line.split_whitespace().next().unwrap_or_default();
    if method != "POST" {
        // GET included: the standalone stream a GET opens carries
        // server-initiated messages, and this server sends none.
        return Err(Status::MethodNotAllowed);
    }

    let (mut length, mut origin, mut authorization) = (None, None, None);
    loop {
        line.clear();
        read += reader
            .read_line(&mut line)
            .map_err(|_| Status::BadRequest)?;
        if read > MAX_HEAD {
            return Err(Status::TooLarge);
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        let Some((name, value)) = trimmed.split_once(':') else {
            return Err(Status::BadRequest);
        };
        let value = value.trim().to_string();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse::<usize>().ok(),
            "origin" => origin = Some(value),
            "authorization" => authorization = Some(value),
            _ => {}
        }
    }

    // No length, no body, no request. Chunked transfer is refused rather than
    // half-supported.
    let Some(length) = length else {
        return Err(Status::LengthRequired);
    };
    Ok(Head {
        length,
        origin,
        authorization,
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Ok,
    Accepted,
    BadRequest,
    Unauthorized,
    Forbidden,
    MethodNotAllowed,
    LengthRequired,
    TooLarge,
    ServerError,
}

impl Status {
    fn code(self) -> u16 {
        match self {
            Status::Ok => 200,
            Status::Accepted => 202,
            Status::BadRequest => 400,
            Status::Unauthorized => 401,
            Status::Forbidden => 403,
            Status::MethodNotAllowed => 405,
            Status::LengthRequired => 411,
            Status::TooLarge => 413,
            Status::ServerError => 500,
        }
    }

    /// Deliberately terse. A refusal here reaches whatever opened the socket,
    /// which may not be a client of ours, so it says what is wrong and nothing
    /// about the store.
    fn reason(self) -> &'static str {
        match self {
            Status::Ok => "OK",
            Status::Accepted => "Accepted",
            Status::BadRequest => "Bad Request",
            Status::Unauthorized => "Unauthorized",
            Status::Forbidden => "Forbidden",
            Status::MethodNotAllowed => "Method Not Allowed",
            Status::LengthRequired => "Length Required",
            Status::TooLarge => "Payload Too Large",
            Status::ServerError => "Internal Server Error",
        }
    }
}

fn reply(out: &mut impl Write, status: Status, kind: &str, body: &str) -> std::io::Result<()> {
    write!(
        out,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status.code(),
        status.reason(),
        kind,
        body.len(),
        body
    )?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(raw: &str) -> Result<Head, Status> {
        read_head(&mut BufReader::new(raw.as_bytes()))
    }

    #[test]
    fn a_post_with_a_length_is_read_and_anything_else_is_refused() {
        let h = head("POST /mcp HTTP/1.1\r\nContent-Length: 12\r\n\r\n").unwrap();
        assert_eq!(h.length, 12);

        // Header names are case-insensitive on the wire.
        let h =
            head("POST /mcp HTTP/1.1\r\ncontent-length: 7\r\nOrigin: http://x\r\n\r\n").unwrap();
        assert_eq!(h.length, 7);
        assert_eq!(h.origin.as_deref(), Some("http://x"));

        assert_eq!(
            head("GET /mcp HTTP/1.1\r\n\r\n"),
            Err(Status::MethodNotAllowed)
        );
        assert_eq!(
            head("POST /mcp HTTP/1.1\r\n\r\n"),
            Err(Status::LengthRequired),
            "chunked is refused rather than half-supported"
        );
        assert_eq!(
            head("POST /mcp HTTP/1.1\r\nnot a header\r\n\r\n"),
            Err(Status::BadRequest)
        );
    }

    /// A request carrying `Origin` is a browser, and this server has none.
    ///
    /// The specification requires the check by name: without it a page on any
    /// site can drive a loopback MCP server through the user's browser.
    #[test]
    fn a_browser_origin_is_forbidden_even_with_a_good_token() {
        let h = Head {
            length: 10,
            origin: Some("https://evil.example".into()),
            authorization: Some("Bearer secret".into()),
        };
        assert_eq!(h.check(Some("secret")), Err(Status::Forbidden));
    }

    #[test]
    fn a_token_is_required_when_one_is_configured() {
        let with = |auth: Option<&str>| Head {
            length: 10,
            origin: None,
            authorization: auth.map(str::to_string),
        };
        assert_eq!(with(None).check(Some("secret")), Err(Status::Unauthorized));
        assert_eq!(
            with(Some("Bearer wrong")).check(Some("secret")),
            Err(Status::Unauthorized)
        );
        assert_eq!(
            with(Some("secret")).check(Some("secret")),
            Err(Status::Unauthorized),
            "the scheme is part of the header, not optional"
        );
        assert_eq!(with(Some("Bearer secret")).check(Some("secret")), Ok(()));
        // No token configured: anything passes, which is the loopback case.
        assert_eq!(with(None).check(None), Ok(()));
    }

    #[test]
    fn a_body_over_the_limit_is_refused_before_it_is_read() {
        let h = Head {
            length: MAX_BODY + 1,
            origin: None,
            authorization: None,
        };
        assert_eq!(h.check(None), Err(Status::TooLarge));
    }

    /// An address other people can reach will not be served without a token.
    #[test]
    fn a_reachable_address_refuses_to_serve_unauthenticated() {
        let public: SocketAddr = "0.0.0.0:9000".parse().unwrap();
        let local: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let err = Guard::new(public, None).unwrap_err();
        assert!(err.contains("RMEM_TOKEN"), "{err}");
        assert!(Guard::new(public, Some("s".into())).is_ok());
        assert!(
            Guard::new(local, None).is_ok(),
            "loopback is the local case the specification calls out"
        );
    }

    #[test]
    fn constant_time_eq_is_still_equality() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
        assert!(constant_time_eq("", ""));
    }
}
