//! The server, driven the way a client drives it: lines in, lines out.
//!
//! Every test here goes through [`Server::serve`] over a `&[u8]` and a
//! `Vec<u8>`, so what is being checked is the bytes a client would receive and
//! not an intermediate the protocol never sees. No process is spawned and no
//! socket is opened — the provider is a stub, which is the whole reason
//! `Completer` and `Embedder` are ports.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::{json, Value};

use rm_engine::{Completer, CompleterError, Embedder, EmbedderError};
use rm_host::config::{Config, TEMPLATE};
use rm_host::testing::TempDir;
use rm_host::HostError;
use rm_mcp::version::{LEGACY, MODERN};
use rm_mcp::{Server, CLIENT_CAPABILITIES_KEY, PROTOCOL_VERSION_KEY, SERVER_INFO_KEY};

/// A provider whose scripted answers are shared across every clone.
///
/// `rm_host::testing::StubProvider` would do for one call, but the server
/// builds a provider per tool invocation — deliberately, so that the tools
/// which never embed anything never demand a credential — and a fresh stub each
/// time would hand every `remember` in a test the same first answer. Sharing
/// one queue behind an `Rc` is what makes a two-turn test mean two turns.
#[derive(Clone)]
struct Script(Rc<RefCell<Vec<String>>>);

impl Script {
    fn new(answers: &[&str]) -> Self {
        Script(Rc::new(RefCell::new(
            answers.iter().map(|s| s.to_string()).collect(),
        )))
    }
}

impl Completer for Script {
    fn complete(&self, _prompt: &str) -> Result<String, CompleterError> {
        let mut left = self.0.borrow_mut();
        if left.is_empty() {
            return Err(CompleterError(
                "the script was asked for more answers than it was given".to_string(),
            ));
        }
        Ok(left.remove(0))
    }
}

impl Embedder for Script {
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

/// A worked extraction: a person, an organisation, and the fact joining them.
const AT_GLOBEX: &str = r#"{"mentions":[
    {"kind":"person","name":"Ben Severn","text":"Ben"},
    {"kind":"organisation","name":"Globex","text":"Globex"}],
  "facts":[{"subject":0,"attribute":"employer","value":"Globex",
            "text":"Ben works at Globex","days_ago":null}],
  "relations":[{"subject":0,"predicate":"employed_by","object":1,"days_ago":null}],
  "closures":[]}"#;

const AT_ACME: &str = r#"{"mentions":[
    {"kind":"person","name":"Ben Severn","text":"Ben"},
    {"kind":"organisation","name":"Acme","text":"Acme"}],
  "facts":[{"subject":0,"attribute":"employer","value":"Acme",
            "text":"Ben works at Acme","days_ago":null}],
  "relations":[],
  "closures":[]}"#;

/// A real `rmem.toml` in `dir`, with its store beside it.
///
/// `dimension = 3` because that is what the stub embeds, and the store refuses
/// a store whose vectors are a different width — which is the check working,
/// not a workaround.
fn config_in(dir: &TempDir) -> (PathBuf, PathBuf) {
    let path = dir.path().join("rmem.toml");
    let store = dir.path().join("memory.json");
    let text = TEMPLATE
        .replace(
            "path = \"memory.json\"",
            &format!("path = {:?}", store.display().to_string()),
        )
        .replace("dimension = 1536", "dimension = 3");
    std::fs::write(&path, text).unwrap();
    (path, store)
}

/// Feed `lines` to a server over `config`, and collect what comes back.
///
/// `now` is fixed rather than read from a clock, for the same reason nothing
/// else in this workspace reads one: a test that cannot control the time cannot
/// assert on anything that depends on it, and both of `about`'s axes do.
fn talk(config: &Path, script: Script, now: i64, lines: &[Value]) -> Vec<Value> {
    let input: String = lines
        .iter()
        .map(|l| format!("{l}\n"))
        .collect::<Vec<_>>()
        .join("");
    let mut output = Vec::new();
    let mut server =
        Server::open(config, move |_: &Config| Ok::<_, HostError>(script.clone())).unwrap();
    server.serve(input.as_bytes(), &mut output, || now).unwrap();

    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).expect("every line has to be JSON"))
        .collect()
}

/// A modern request: the `_meta` envelope every `2026-07-28` request carries.
fn modern(id: i64, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({
        PROTOCOL_VERSION_KEY: MODERN,
        CLIENT_CAPABILITIES_KEY: {},
        "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "0"},
    });
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// A legacy request: no envelope, because there was nowhere to put one.
fn legacy(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn call(id: i64, name: &str, arguments: Value) -> Value {
    modern(
        id,
        "tools/call",
        json!({"name": name, "arguments": arguments}),
    )
}

/// The text of a tool result, which is what most clients show a model.
fn text_of(response: &Value) -> &str {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content in {response}"))
}

// ---------------------------------------------------------------------------
// Both eras
// ---------------------------------------------------------------------------

#[test]
fn a_legacy_client_handshakes_and_is_served() {
    // The reason this server is dual-era at all. The specification's own
    // compatibility matrix says a legacy client meeting a modern-only server
    // *fails*, with no fall-forward mechanism to recover with, and the
    // revision that removed the handshake is weeks old.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[]),
        1_000,
        &[
            legacy(
                1,
                "initialize",
                json!({"protocolVersion": "2025-11-25", "capabilities": {},
                       "clientInfo": {"name": "old", "version": "1"}}),
            ),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            legacy(2, "tools/list", json!({})),
        ],
    );

    // Two responses, not three: the notification gets silence.
    assert_eq!(out.len(), 2, "{out:#?}");
    assert_eq!(out[0]["result"]["protocolVersion"], json!("2025-11-25"));
    assert_eq!(
        out[0]["result"]["serverInfo"]["name"],
        json!("rusty-memory")
    );
    assert!(out[0]["result"]["capabilities"]["tools"].is_object());
    // The instructions are where a model is told that an answer has three
    // states. Leaving them out would make this a memory server like any other.
    let instructions = out[0]["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("unknown"), "{instructions}");
    assert!(instructions.contains("absent"), "{instructions}");

    assert_eq!(out[1]["result"]["tools"].as_array().unwrap().len(), 9);
}

#[test]
fn a_legacy_handshake_at_a_version_we_do_not_serve_is_answered_with_one_we_do() {
    // "Otherwise, the server MUST respond with another protocol version it
    // supports. This SHOULD be the latest." Not an error: the client is the
    // one that decides whether it can speak what comes back.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[]),
        1_000,
        &[legacy(
            1,
            "initialize",
            json!({"protocolVersion": "1999-01-01"}),
        )],
    );
    assert_eq!(out[0]["result"]["protocolVersion"], json!(LEGACY[0]));
    assert!(out[0].get("error").is_none(), "{:#?}", out[0]);
}

#[test]
fn a_modern_client_needs_no_handshake_at_all() {
    // The whole shape of the change in 2026-07-28: no initialize, no session,
    // every request carrying its own version. A `tools/call` as the very first
    // message has to work.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[]),
        1_000,
        &[call(1, "reviews", json!({}))],
    );
    assert_eq!(out[0]["result"]["isError"], json!(false));
    assert_eq!(out[0]["result"]["resultType"], json!("complete"));
}

#[test]
fn server_discover_names_the_versions_a_meta_envelope_may_carry() {
    // Mandatory in the modern era, and a dual-era client's stdio probe: a
    // `DiscoverResult` says "modern server", anything else says "legacy".
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[]),
        1_000,
        &[modern(1, "server/discover", json!({}))],
    );
    let result = &out[0]["result"];
    assert_eq!(result["resultType"], json!("complete"));
    assert_eq!(result["supportedVersions"], json!([MODERN]));
    assert!(result["capabilities"]["tools"].is_object());
    assert_eq!(
        result["_meta"][SERVER_INFO_KEY]["name"],
        json!("rusty-memory")
    );
}

#[test]
fn result_type_and_server_info_belong_to_the_modern_era_alone() {
    // Modern results MUST carry `resultType` and SHOULD carry the server's
    // identity, because a stateless client has no handshake to have learned it
    // from. Legacy results had neither field, and emitting one is claiming to
    // speak a revision that did not have it.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[]),
        1_000,
        &[
            modern(1, "tools/list", json!({})),
            legacy(2, "initialize", json!({"protocolVersion": "2025-11-25"})),
            legacy(3, "tools/list", json!({})),
        ],
    );
    assert_eq!(out[0]["result"]["resultType"], json!("complete"));
    assert!(out[0]["result"]["_meta"][SERVER_INFO_KEY].is_object());

    assert!(
        out[2]["result"].get("resultType").is_none(),
        "{:#?}",
        out[2]
    );
    assert!(out[2]["result"].get("_meta").is_none(), "{:#?}", out[2]);
    // And it is the same five tools either way.
    assert_eq!(out[0]["result"]["tools"], out[2]["result"]["tools"]);
}

#[test]
fn structured_content_is_withheld_from_a_client_that_handshaked_before_it_existed() {
    // `structuredContent` arrived in 2025-06-18. A client that negotiated
    // 2024-11-05 gets the text block alone -- which is why every distinction
    // this server makes is made in words as well as in structure.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);

    for (version, expected) in [("2024-11-05", false), ("2025-06-18", true)] {
        let out = talk(
            &config,
            Script::new(&[]),
            1_000,
            &[
                legacy(1, "initialize", json!({"protocolVersion": version})),
                legacy(2, "tools/call", json!({"name": "reviews", "arguments": {}})),
            ],
        );
        assert_eq!(out[0]["result"]["protocolVersion"], json!(version));
        assert_eq!(
            out[1]["result"].get("structuredContent").is_some(),
            expected,
            "at {version}: {:#?}",
            out[1]
        );
        // The text block is there in both, always.
        assert!(!text_of(&out[1]).is_empty());
    }
}

#[test]
fn a_modern_envelope_at_an_unknown_version_is_minus_32022_and_says_where_the_other_door_is() {
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[]),
        1_000,
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {
            "_meta": {PROTOCOL_VERSION_KEY: "1999-01-01", CLIENT_CAPABILITIES_KEY: {}}}}),
        ],
    );
    assert_eq!(out[0]["error"]["code"], json!(-32022));
    assert_eq!(out[0]["error"]["data"]["supported"], json!([MODERN]));
    assert_eq!(out[0]["error"]["data"]["requested"], json!("1999-01-01"));
}

#[test]
fn a_modern_request_missing_client_capabilities_is_refused_by_name() {
    // Required on every modern request. The refusal names the key, because
    // that is the only thing a client can act on.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[]),
        1_000,
        &[
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/list", "params": {
            "_meta": {PROTOCOL_VERSION_KEY: MODERN}}}),
        ],
    );
    assert_eq!(out[0]["error"]["code"], json!(-32602));
    assert_eq!(out[0]["id"], json!(4));
    assert!(out[0]["error"]["message"]
        .as_str()
        .unwrap()
        .contains(CLIENT_CAPABILITIES_KEY));
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

#[test]
fn no_notification_is_ever_answered() {
    // A response to a notification is a line the client has no request to
    // match, and clients differ in how badly they take it. `cancelled` in
    // particular arrives for a request this loop has already finished, since
    // it handles one at a time.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[]),
        1_000,
        &[
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 1}}),
            json!({"jsonrpc": "2.0", "method": "notifications/something/we/never/heard/of"}),
        ],
    );
    assert!(out.is_empty(), "{out:#?}");
}

#[test]
fn every_response_is_exactly_one_line_with_no_newline_inside_it() {
    // The stdio binding's framing rule, and the one this server would break
    // first: a refusal carries the library's own words, and those run to
    // several sentences. They survive as `\n` escapes inside a JSON string
    // because nothing writes a response any way but through `serde_json`.
    let dir = TempDir::new();
    let (config, store) = config_in(&dir);
    let script = Script::new(&[AT_GLOBEX]);
    let input: String = [
        modern(1, "tools/list", json!({})),
        call(2, "remember", json!({"text": "I work at Globex"})),
        call(3, "recall", json!({"query": "where do I work"})),
        call(4, "resolve_review", json!({"id": 99, "same": true})),
    ]
    .iter()
    .map(|l| format!("{l}\n"))
    .collect();

    let mut output = Vec::new();
    let mut server = Server::open(&config, move |_: &Config| {
        Ok::<_, HostError>(script.clone())
    })
    .unwrap();
    server
        .serve(input.as_bytes(), &mut output, || 1_000)
        .unwrap();
    let output = String::from_utf8(output).unwrap();

    assert_eq!(output.lines().count(), 4, "one line per request");
    assert!(output.ends_with('\n'), "the last line is terminated too");
    for line in output.lines() {
        assert!(!line.is_empty());
        serde_json::from_str::<Value>(line).expect("each line stands alone as JSON");
    }
    // And the refusal really did carry a multi-sentence message, so the test
    // above was not vacuous.
    let refusal: Value = serde_json::from_str(output.lines().nth(3).unwrap()).unwrap();
    assert_eq!(refusal["result"]["isError"], json!(true));
    assert!(store.exists(), "the remember was saved");
}

#[test]
fn a_blank_line_is_skipped_rather_than_answered() {
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let mut output = Vec::new();
    let mut server =
        Server::open(&config, |_: &Config| Ok::<_, HostError>(Script::new(&[]))).unwrap();
    let input = format!("\n   \n{}\n", modern(1, "ping", json!({})));
    server
        .serve(input.as_bytes(), &mut output, || 1_000)
        .unwrap();
    assert_eq!(String::from_utf8(output).unwrap().lines().count(), 1);
}

#[test]
fn an_unparseable_line_does_not_stop_the_server() {
    // A client that sends one bad line still has a conversation to finish, and
    // a server that exits on it takes every unrelated request with it.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let mut output = Vec::new();
    let mut server =
        Server::open(&config, |_: &Config| Ok::<_, HostError>(Script::new(&[]))).unwrap();
    let input = format!("{{not json\n{}\n", modern(2, "ping", json!({})));
    server
        .serve(input.as_bytes(), &mut output, || 1_000)
        .unwrap();

    let lines: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["error"]["code"], json!(-32700));
    assert_eq!(lines[1]["result"]["resultType"], json!("complete"));
}

// ---------------------------------------------------------------------------
// A refusal is a result; a bad request is an error
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_tool_is_a_protocol_error_and_a_bad_argument_is_not() {
    // The split the specification draws and this server's whole error policy
    // rests on. A model cannot invent a tool the server does not have, so that
    // is a JSON-RPC error. It *can* fix an argument, so that comes back as an
    // ordinary result the client is told to hand back to it.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[]),
        1_000,
        &[
            call(1, "forget", json!({"entity": 1})),
            call(2, "about", json!({"attribute": "employer"})),
        ],
    );

    assert_eq!(out[0]["error"]["code"], json!(-32602));
    assert!(out[0]["error"]["message"]
        .as_str()
        .unwrap()
        .contains("forget"));
    assert!(out[0].get("result").is_none());

    assert!(out[1].get("error").is_none(), "{:#?}", out[1]);
    assert_eq!(out[1]["result"]["isError"], json!(true));
    assert!(text_of(&out[1]).contains("entity"), "{}", text_of(&out[1]));
}

#[test]
fn an_unknown_method_is_minus_32601() {
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[]),
        1_000,
        &[modern(1, "resources/list", json!({}))],
    );
    assert_eq!(out[0]["error"]["code"], json!(-32601));
}

#[test]
fn the_library_s_own_words_reach_the_model_verbatim() {
    // Every refusal in this workspace names what was missing, and that
    // sentence is the part that took effort to write. Wrapping it in "tool
    // failed" would discard exactly the half a model can act on.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[]),
        1_000,
        &[call(1, "resolve_review", json!({"id": 99, "same": false}))],
    );
    assert_eq!(out[0]["result"]["isError"], json!(true));
    let text = text_of(&out[0]);
    assert!(text.contains("99"), "it has to name the id: {text}");
    assert!(text.len() > 20, "a refusal names what was missing: {text}");
    // No structure to report, and inventing one would give a client
    // validating against a schema something to validate that never happened.
    assert!(out[0]["result"].get("structuredContent").is_none());
}

#[test]
fn unknown_is_an_answer_and_not_an_error() {
    // The same distinction `rmem about` makes with its exit code: the store
    // was asked and has no opinion, which is a result. Reporting it as a
    // failure would teach an agent to retry a question that has been answered.
    let dir = TempDir::new();
    let (config, store) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[]),
        1_000,
        &[call(
            1,
            "about",
            json!({"entity": 0, "attribute": "employer"}),
        )],
    );
    assert_eq!(out[0]["result"]["isError"], json!(false));
    assert_eq!(
        out[0]["result"]["structuredContent"],
        json!({"believed": "unknown"})
    );
    assert!(!store.exists(), "a read must not write the store");
}

#[test]
fn the_tools_that_never_embed_anything_never_ask_for_a_provider() {
    // `rm-cli` learned this one level down: building a provider reads an API
    // key and refuses when it is not set, so a server that built one up front
    // would make `about` and both review tools demand a credential none of
    // them touches. The review band exists so a *human* answers what the
    // resolver would not guess at, and answering it is local work over a file
    // on disk. A factory that panics is the only way to catch a regression --
    // every other test's provider happens to succeed.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let mut output = Vec::new();
    let mut server = Server::open(&config, |_: &Config| -> Result<Script, HostError> {
        panic!("no provider may be built for a tool that does not embed")
    })
    .unwrap();

    let input: String = [
        modern(1, "tools/list", json!({})),
        call(2, "about", json!({"entity": 0, "attribute": "employer"})),
        call(3, "reviews", json!({})),
        call(4, "resolve_review", json!({"id": 0, "same": true})),
    ]
    .iter()
    .map(|l| format!("{l}\n"))
    .collect();
    server
        .serve(input.as_bytes(), &mut output, || 1_000)
        .unwrap();
    assert_eq!(String::from_utf8(output).unwrap().lines().count(), 4);
}

// ---------------------------------------------------------------------------
// The library, through the protocol
// ---------------------------------------------------------------------------

#[test]
fn a_turn_is_remembered_and_can_then_be_asked_about() {
    // Extraction, resolution, ingest, survivorship and rendering, driven
    // entirely through the wire. Nothing below this test knows a protocol
    // exists, and nothing in this crate knows what an employer is.
    let dir = TempDir::new();
    let (config, store) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[AT_GLOBEX]),
        1_000,
        &[call(1, "remember", json!({"text": "I work at Globex"}))],
    );

    let structured = &out[0]["result"]["structuredContent"];
    assert_eq!(out[0]["result"]["isError"], json!(false));
    assert_eq!(structured["mentions"].as_array().unwrap().len(), 2);
    assert_eq!(structured["mentions"][0]["name"], json!("Ben Severn"));
    assert_eq!(structured["mentions"][0]["was_new"], json!(true));
    assert_eq!(structured["facts"], json!(1));
    assert_eq!(structured["relations"], json!(1));
    assert!(store.exists(), "a write has to reach the file");

    // A second server over the same file: the store is what was written, not
    // what happened to be in memory.
    let ben = structured["mentions"][0]["entity"].as_i64().unwrap();
    let out = talk(
        &config,
        Script::new(&[]),
        2_000,
        &[call(
            1,
            "about",
            json!({"entity": ben, "attribute": "employer"}),
        )],
    );
    assert_eq!(
        out[0]["result"]["structuredContent"],
        json!({"believed": "value", "value": "Globex"})
    );
    assert_eq!(text_of(&out[0]), "Globex");
}

#[test]
fn the_two_time_axes_are_answerable_separately_through_the_wire() {
    // The claim this project exists to make, reachable by an agent. The same
    // stored history answers differently depending on *when you ask about*
    // and *when you asked* -- and nothing was rewritten between the two calls
    // below, because they are the same call with a different number.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[AT_GLOBEX]),
        1_000,
        &[call(1, "remember", json!({"text": "I work at Globex"}))],
    );
    let ben = out[0]["result"]["structuredContent"]["mentions"][0]["entity"]
        .as_i64()
        .unwrap();

    let out = talk(
        &config,
        Script::new(&[]),
        5_000,
        &[
            // What was known at 500, which is before the turn arrived. Later
            // knowledge does not leak backwards.
            call(
                1,
                "about",
                json!({"entity": ben, "attribute": "employer", "as_of": 500}),
            ),
            // And what is known now.
            call(
                2,
                "about",
                json!({"entity": ben, "attribute": "employer", "as_of": 5_000}),
            ),
        ],
    );
    assert_eq!(
        out[0]["result"]["structuredContent"],
        json!({"believed": "unknown"}),
        "nothing had been said yet at 500"
    );
    assert_eq!(
        out[1]["result"]["structuredContent"]["believed"],
        json!("value")
    );
    // Neither is an error. Both are answers.
    assert_eq!(out[0]["result"]["isError"], json!(false));
    assert_eq!(out[1]["result"]["isError"], json!(false));
}

#[test]
fn a_second_mention_of_someone_known_is_reported_as_recognised() {
    // The most useful thing an agent can be told here: whether memory learned
    // about someone or recognised them.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[AT_GLOBEX, AT_ACME]),
        1_000,
        &[
            call(1, "remember", json!({"text": "I work at Globex"})),
            call(2, "remember", json!({"text": "Actually, Acme"})),
        ],
    );
    assert_eq!(
        out[0]["result"]["structuredContent"]["mentions"][0]["was_new"],
        json!(true)
    );
    assert_eq!(
        out[1]["result"]["structuredContent"]["mentions"][0]["was_new"],
        json!(false),
        "Ben was already known: {:#?}",
        out[1]
    );
}

#[test]
fn recall_reports_the_entity_behind_every_hit() {
    // Without the entity id an agent that recalled something cannot then ask
    // about it, which makes recall a dead end.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[AT_GLOBEX]),
        1_000,
        &[
            call(1, "remember", json!({"text": "I work at Globex"})),
            call(2, "recall", json!({"query": "Ben works at Globex", "k": 3})),
        ],
    );
    let hits = out[1]["result"]["structuredContent"]["hits"]
        .as_array()
        .unwrap();
    assert!(!hits.is_empty(), "{:#?}", out[1]);
    assert!(hits[0]["entity"].is_number());
    assert!(hits[0]["attribute"].is_string());
    assert!(hits[0]["source"].is_string());
}

#[test]
fn recalling_an_empty_store_is_an_empty_answer_rather_than_a_failure() {
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&[]),
        1_000,
        &[call(1, "recall", json!({"query": "anything"}))],
    );
    assert_eq!(out[0]["result"]["isError"], json!(false));
    assert_eq!(out[0]["result"]["structuredContent"], json!({"hits": []}));
}

#[test]
fn a_near_miss_is_filed_as_a_question_and_answering_it_merges() {
    // The review band end to end. "Ben Severn" against "Ben Sanderson" scores
    // between the template's review_at and match_at, so it creates a separate
    // entity and files the pair rather than fusing two people on a score that
    // could not be called.
    let dir = TempDir::new();
    let (config, _) = config_in(&dir);
    let severn = r#"{"mentions":[{"kind":"person","name":"Ben Severn","text":"Ben"}],
        "facts":[],"relations":[],"closures":[]}"#;
    let sanderson = r#"{"mentions":[{"kind":"person","name":"Ben Sanderson","text":"Ben"}],
        "facts":[],"relations":[],"closures":[]}"#;

    let out = talk(
        &config,
        Script::new(&[severn, sanderson]),
        1_000,
        &[
            call(1, "remember", json!({"text": "Ben"})),
            call(2, "remember", json!({"text": "Ben again"})),
            call(3, "reviews", json!({})),
        ],
    );

    // Nothing merged on the way in, and the turn that raised the question says
    // so where a model will read it.
    let second = text_of(&out[1]);
    assert!(second.contains("nothing was merged"), "{second}");
    let reviews = out[2]["result"]["structuredContent"]["reviews"]
        .as_array()
        .unwrap();
    assert_eq!(reviews.len(), 1, "{:#?}", out[2]);
    assert!(reviews[0]["score"].as_f64().unwrap() > 0.0);
    let id = reviews[0]["id"].as_i64().unwrap();

    let out = talk(
        &config,
        Script::new(&[]),
        2_000,
        &[
            call(1, "resolve_review", json!({"id": id, "same": true})),
            call(2, "reviews", json!({})),
        ],
    );
    assert_eq!(out[0]["result"]["structuredContent"]["merged"], json!(true));
    assert!(out[0]["result"]["structuredContent"]["survivor"].is_number());
    assert_eq!(
        out[1]["result"]["structuredContent"],
        json!({"reviews": []}),
        "the question is answered and stays answered"
    );
}

#[test]
fn a_model_that_answered_with_nonsense_is_refused_with_the_reason() {
    let dir = TempDir::new();
    let (config, store) = config_in(&dir);
    let out = talk(
        &config,
        Script::new(&["I'm afraid I can't do that"]),
        1_000,
        &[call(1, "remember", json!({"text": "anything"}))],
    );
    assert_eq!(out[0]["result"]["isError"], json!(true));
    assert!(text_of(&out[0]).len() > 30, "{}", text_of(&out[0]));
    assert!(
        !store.exists(),
        "nothing was learned, so nothing is written"
    );
}
