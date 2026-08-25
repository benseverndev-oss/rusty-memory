//! Two agents, one server, one store — and a log that says which is which.
//!
//! Over a real socket, because the thing under test *is* the transport. Every
//! other test in this crate drives [`Server`](rm_mcp::Server) over a byte slice,
//! which is the right shape for protocol questions and cannot reach this one:
//! the bug these tests exist for is that `initialize` and the calls after it
//! arrive on different connections, and a single in-memory server never has
//! that problem.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::Path;

use serde_json::{json, Value};

use rm_engine::{Completer, CompleterError, Embedder, EmbedderError};
use rm_host::config::{Config, TEMPLATE};
use rm_host::testing::TempDir;
use rm_mcp::http::{serve, Guard};
use rm_mcp::version::LEGACY;

/// A provider with no state, so the closure that builds it can be `Copy`.
///
/// `decide` never reaches a completion model — the shape of a decision is
/// known, which is the reason it does not — so `complete` refusing outright is
/// an assertion rather than a gap: a test that started needing it would say so
/// by failing here.
#[derive(Clone, Copy)]
struct Stub;

impl Completer for Stub {
    fn complete(&self, _prompt: &str) -> Result<String, CompleterError> {
        Err(CompleterError("decide must not need a completer".into()))
    }
}

impl Embedder for Stub {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedderError> {
        let mut v = [0.0f32; 3];
        for (i, b) in text.bytes().enumerate() {
            v[i % 3] += f32::from(b);
        }
        if v.iter().all(|x| *x == 0.0) {
            v[0] = 1.0;
        }
        Ok(v.to_vec())
    }
}

/// A clock that does not move, so nothing here can expire mid-test.
fn now() -> i64 {
    1_787_532_411_419
}

/// What a reply said.
struct Reply {
    status: u16,
    session: Option<String>,
    body: String,
}

impl Reply {
    /// The JSON-RPC `result`, or a panic naming what came back instead.
    fn result(&self) -> Value {
        let v: Value = serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.body));
        v.get("result")
            .cloned()
            .unwrap_or_else(|| panic!("no result in {}", self.body))
    }
}

/// One request, one connection — which is the whole point.
fn send(addr: SocketAddr, method: &str, session: Option<&str>, body: &str) -> Reply {
    let mut stream = TcpStream::connect(addr).expect("connect");
    let mut head = format!("{method} /mcp HTTP/1.1\r\nHost: localhost\r\n");
    if method == "POST" {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    if let Some(id) = session {
        head.push_str(&format!("Mcp-Session-Id: {id}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(body.as_bytes()).expect("write body");
    stream.flush().expect("flush");
    stream.shutdown(Shutdown::Write).ok();

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).expect("status line");
    let status: u16 = line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or_else(|| panic!("no status in {line:?}"));

    let mut session = None;
    loop {
        line.clear();
        reader.read_line(&mut line).expect("header");
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("mcp-session-id") {
                session = Some(value.trim().to_string());
            }
        }
    }
    // Read to EOF: every reply here closes the connection.
    let mut body = String::new();
    reader.read_to_string(&mut body).expect("body");
    Reply {
        status,
        session,
        body,
    }
}

fn initialize(client: &str, version: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {"name": client, "version": "1.0"}
        }
    })
    .to_string()
}

fn decide(id: u32, title: &str, choice: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": "decide", "arguments": {"title": title, "choice": choice, "scope": "*"}}
    })
    .to_string()
}

/// A server on a loopback port, and the directory its store lives in.
fn server() -> (SocketAddr, TempDir) {
    let dir = TempDir::new();
    let config_path = dir.path().join("rmem.toml");
    let toml = TEMPLATE.replace(
        "path = \"memory.json\"",
        &format!(
            "path = {:?}",
            dir.path().join("memory.json").display().to_string()
        ),
    );
    std::fs::write(&config_path, toml).expect("write config");
    // The stub embeds in three dimensions; the template does not.
    let text = std::fs::read_to_string(&config_path).expect("read config");
    let text = text
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("dimension") {
                "dimension = 3".to_string()
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&config_path, text).expect("rewrite config");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let guard = Guard::new(addr, None).expect("loopback needs no token");
    let path = config_path.clone();
    std::thread::spawn(move || {
        serve(listener, path, |_: &Config| Ok(Stub), guard, now);
    });
    (addr, dir)
}

/// Who wrote what, from the store the server has been writing to.
///
/// `source_ref` is provenance's name for it: opaque to `rm_core`, and the host
/// is what decides it holds a client name.
fn authors(dir: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join("memory.json")).expect("read store");
    let snapshot: Value = serde_json::from_str(&text).expect("parse snapshot");
    // The snapshot holds the store as a *string* of JSON rather than as nested
    // objects, so a walk over the outer value alone never reaches an
    // assertion. Parse the inner document before looking for anything in it.
    let inner = snapshot
        .get("store")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no store in the snapshot: {text}"));
    let store: Value = serde_json::from_str(inner).expect("parse store");
    let mut found: Vec<String> = Vec::new();
    collect_sessions(&store, &mut found);
    found.sort();
    found.dedup();
    found
}

fn collect_sessions(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if k == "source_ref" {
                    if let Some(s) = val.as_str() {
                        out.push(s.to_string());
                    }
                }
                collect_sessions(val, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect_sessions(i, out)),
        _ => {}
    }
}

/// The bug this module exists for: two agents, one store, and a log that could
/// not tell them apart because every write was recorded as `mcp`.
#[test]
fn two_clients_on_one_server_are_told_apart_in_the_log() {
    let (addr, dir) = server();

    let a = send(addr, "POST", None, &initialize("agent-a", LEGACY[0]));
    let b = send(addr, "POST", None, &initialize("agent-b", LEGACY[0]));
    let a_id = a.session.expect("agent-a got no session id");
    let b_id = b.session.expect("agent-b got no session id");
    assert_ne!(a_id, b_id, "two clients were handed one session");

    // Each writes on its own connection, which is where this used to fall down.
    let wrote = send(
        addr,
        "POST",
        Some(&a_id),
        &decide(2, "Ship on Friday", "yes"),
    );
    assert_eq!(wrote.status, 200, "{}", wrote.body);
    let wrote = send(
        addr,
        "POST",
        Some(&b_id),
        &decide(3, "Ship on Monday", "no"),
    );
    assert_eq!(wrote.status, 200, "{}", wrote.body);

    let authors = authors(dir.path());
    assert!(
        authors.iter().any(|s| s.contains("agent-a")),
        "agent-a is not in the log: {authors:?}"
    );
    assert!(
        authors.iter().any(|s| s.contains("agent-b")),
        "agent-b is not in the log: {authors:?}"
    );
}

/// A client that never handshakes is still served, and its writes say `mcp`.
///
/// The specification would rather this were a 400. Refusing it would reverse a
/// decision this server already made deliberately for the same client, so the
/// behaviour is pinned here rather than left to be rediscovered.
#[test]
fn a_client_with_no_session_is_served_and_written_down_as_mcp() {
    let (addr, dir) = server();
    let wrote = send(addr, "POST", None, &decide(1, "Anonymous choice", "yes"));
    assert_eq!(wrote.status, 200, "{}", wrote.body);
    let authors = authors(dir.path());
    assert!(
        authors.iter().any(|s| s == "mcp"),
        "an unattributed write should say mcp: {authors:?}"
    );
}

/// An id this server did not mint is a 404, so a client that thinks it has a
/// session finds out that it does not.
#[test]
fn an_unknown_session_is_not_quietly_served() {
    let (addr, _dir) = server();
    let r = send(
        addr,
        "POST",
        Some("0123456789abcdef0123456789abcdef"),
        &decide(1, "X", "y"),
    );
    assert_eq!(r.status, 404, "{}", r.body);
}

/// `DELETE` ends a session, and the id stops working the moment it does.
#[test]
fn a_deleted_session_stops_being_one() {
    let (addr, _dir) = server();
    let id = send(addr, "POST", None, &initialize("agent-a", LEGACY[0]))
        .session
        .expect("no session id");

    // It works, then it is ended, then it does not.
    assert_eq!(
        send(addr, "POST", Some(&id), &decide(2, "A", "y")).status,
        200
    );
    assert_eq!(send(addr, "DELETE", Some(&id), "").status, 204);
    assert_eq!(
        send(addr, "POST", Some(&id), &decide(3, "B", "y")).status,
        404
    );
    // Ending it twice is a 404 too, not a crash.
    assert_eq!(send(addr, "DELETE", Some(&id), "").status, 404);
}

/// The second bug the session fixes: a legacy client that agreed on a revision
/// older than `structuredContent` used to be answered as though it had not.
///
/// `2025-03-26` predates the field. Before the session carried the negotiated
/// version, the tool call arrived on a fresh server with nothing negotiated,
/// fell back to the newest legacy revision, and sent a field this client's
/// parser has never seen.
#[test]
fn a_revision_agreed_at_the_handshake_still_holds_on_the_next_connection() {
    let (addr, _dir) = server();
    let old = "2025-03-26";
    let hello = send(addr, "POST", None, &initialize("agent-old", old));
    assert_eq!(
        hello
            .result()
            .get("protocolVersion")
            .and_then(Value::as_str),
        Some(old),
        "the server did not agree to the revision asked for"
    );
    let id = hello.session.expect("no session id");

    let called = send(
        addr,
        "POST",
        Some(&id),
        &decide(2, "Old client choice", "yes"),
    );
    assert_eq!(called.status, 200, "{}", called.body);
    assert!(
        called.result().get("structuredContent").is_none(),
        "a client that handshaked before structuredContent existed was sent it: {}",
        called.body
    );

    // And the same call without the session gets the field, which is what
    // makes the line above about the session rather than about the tool.
    let bare = send(addr, "POST", None, &decide(3, "Another choice", "yes"));
    assert!(
        bare.result().get("structuredContent").is_some(),
        "the fallback should be the newest revision: {}",
        bare.body
    );
}
