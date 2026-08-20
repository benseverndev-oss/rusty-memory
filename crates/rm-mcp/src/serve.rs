//! The server: a line in, at most a line out.
//!
//! # Nothing here prints
//!
//! Over stdio, stdout *is* the transport: the server **MUST NOT** write
//! anything to it that is not an MCP message, and messages are newline
//! delimited and **MUST NOT** contain embedded newlines. That is a rule no
//! compiler enforces, so the shape of this module enforces it instead — the
//! loop is written over [`BufRead`] and [`Write`], `main` is the only place
//! that names the real streams, and every test here drives the whole server
//! through a `&[u8]` and a `Vec<u8>`.
//!
//! The no-embedded-newline half holds by construction rather than by
//! sanitising: `serde_json` escapes a newline inside a string to `\n`, and
//! nothing here writes a response any other way. A test says so, because it is
//! the sort of guarantee that survives until someone adds a `write!` in a
//! hurry.

use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{json, Value};

use rm_engine::{Completer, Embedder, Engine, Timestamp};
use rm_host::command::{self, Outcome};
use rm_host::config::Config;
use rm_host::{store, HostError};

use crate::jsonrpc::{self, Request};
use crate::render;
use crate::tools::{self, Call};
use crate::version::{self, Era};
use crate::{INSTRUCTIONS, SERVER_INFO_KEY};

/// A memory server over one store.
///
/// `P` is whatever implements the two ports. The binary passes a real HTTP
/// provider; a test passes a stub, and drives `remember` and `recall` end to
/// end without a socket. That is the entire reason those are ports.
///
/// `provider` is a factory rather than a provider because building one reads
/// an API key out of the environment and refuses when it is not set. Built
/// once up front, `about` and both review tools would demand a credential none
/// of them ever touches — and the review band exists so that a *human* answers
/// what the resolver would not guess at, which is local work over a file on
/// disk. `rm-cli` learned this the same way, one level down.
pub struct Server<P, F>
where
    P: Completer + Embedder,
    F: Fn(&Config) -> Result<P, HostError>,
{
    config: Config,
    engine: Engine,
    provider: F,
    /// The version a legacy `initialize` settled on.
    ///
    /// The modern era is stateless and this stays `None` throughout it. The
    /// legacy era is not — the specification scopes a handshake "to the stdio
    /// process" — and the alternative to remembering it is to guess, for every
    /// subsequent `tools/call`, whether the client on the other end can read
    /// `structuredContent`.
    negotiated: Option<String>,
}

impl<P, F> Server<P, F>
where
    P: Completer + Embedder,
    F: Fn(&Config) -> Result<P, HostError>,
{
    /// Read the config and open the store beside it.
    ///
    /// Both before the loop starts, deliberately: a server that accepted a
    /// connection and then failed its first tool call on a config error has
    /// reported the problem to a model instead of to the person who can fix
    /// it, and on stdio that message may never be seen at all.
    pub fn open(config_path: &Path, provider: F) -> Result<Self, HostError> {
        let config = Config::load(config_path)?;
        let engine = store::load(
            &config.store.path,
            config.ruleset()?,
            config.policy_for_engine()?,
            config.provider.dimension,
            config.metric()?,
        )?;
        Ok(Server {
            config,
            engine,
            provider,
            negotiated: None,
        })
    }

    /// Read messages until the input ends.
    ///
    /// Ending on EOF is the specification's primary graceful-shutdown signal
    /// and the only portable one: the client closes our stdin and waits, and a
    /// server that honours it never has to be killed.
    ///
    /// Flushed after every message. A buffered response is a client waiting
    /// for its full timeout on an answer that is already written.
    pub fn serve(
        &mut self,
        input: impl BufRead,
        mut output: impl Write,
        clock: impl Fn() -> Timestamp,
    ) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            // A blank line is not a message. Skipped rather than answered with
            // a parse error, which would put a response on the wire for
            // something no client is waiting on.
            if line.trim().is_empty() {
                continue;
            }
            if let Some(response) = self.handle(&line, clock()) {
                writeln!(output, "{response}")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    /// Answer one line, or decide it needs no answer.
    ///
    /// `None` means a notification, which **MUST NOT** be answered. Everything
    /// else produces exactly one line.
    pub fn handle(&mut self, line: &str, now: Timestamp) -> Option<String> {
        let request = match jsonrpc::parse(line) {
            Ok(request) => request,
            Err(response) => return Some(response.to_string()),
        };

        // Before anything else, including version checks. A notification is
        // one-way, so a notification this server dislikes is still a
        // notification and still gets silence -- and `notifications/cancelled`
        // in particular arrives for a request this loop has already finished,
        // since it handles one at a time.
        request.id.as_ref()?;

        let era = match version::era_of(&request, self.negotiated.as_deref()) {
            Ok(era) => era,
            Err(response) => return Some(response.to_string()),
        };

        let id = request.id.clone().expect("checked above");
        let result = match request.method.as_str() {
            "initialize" => self.initialize(&request),
            "server/discover" => discover(),
            "ping" => empty(&era),
            "tools/list" => list_tools(&era),
            "tools/call" => match self.call_tool(&request, &era, now) {
                Ok(result) => result,
                Err(response) => return Some(response.to_string()),
            },
            _ => {
                return Some(
                    jsonrpc::error(
                        Some(&id),
                        jsonrpc::METHOD_NOT_FOUND,
                        &format!("Method not found: {}", request.method),
                        None,
                    )
                    .to_string(),
                )
            }
        };
        Some(jsonrpc::result(&id, result).to_string())
    }

    /// The legacy handshake.
    fn initialize(&mut self, request: &Request) -> Value {
        let requested = request
            .params
            .get("protocolVersion")
            .and_then(Value::as_str);
        let agreed = version::negotiate(requested);
        self.negotiated = Some(agreed.clone());
        json!({
            "protocolVersion": agreed,
            // No `listChanged`: this table is a constant, so promising
            // notifications about it would be promising something that can
            // never happen.
            "capabilities": {"tools": {}},
            "serverInfo": server_info(),
            "instructions": INSTRUCTIONS,
        })
    }

    /// Run a `tools/call`.
    ///
    /// `Err` is a *protocol* error and `Ok` may still be a failure: the
    /// specification splits the two, and the split is the same one
    /// `rm-cli::run::exit_code` already makes. A request that is wrong as a
    /// request — no tool name, a tool that does not exist — is a JSON-RPC
    /// error, because a model cannot correct its way out of either. Everything
    /// the library refuses is an ordinary result with `isError: true`, whose
    /// text clients **SHOULD** hand back to the model precisely so that it
    /// can.
    fn call_tool(&mut self, request: &Request, era: &Era, now: Timestamp) -> Result<Value, Value> {
        let Some(Value::String(name)) = request.params.get("name") else {
            return Err(jsonrpc::error(
                request.id.as_ref(),
                jsonrpc::INVALID_PARAMS,
                "Invalid params: name is required and must be a string",
                None,
            ));
        };
        if !tools::is_known(name) {
            return Err(jsonrpc::error(
                request.id.as_ref(),
                jsonrpc::INVALID_PARAMS,
                &format!("Unknown tool: {name}"),
                None,
            ));
        }

        // Absent arguments are an empty object, not a failure: `reviews` takes
        // none, and a client that omits the member entirely is within the
        // schema.
        let arguments = request
            .params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let call = match Call::read(name, &arguments, now) {
            Ok(call) => call,
            Err(why) => return Ok(refused(era, &why)),
        };

        let mutates = call.mutates();
        match self.run(call, now) {
            Ok(outcome) => {
                if mutates {
                    // A failed save leaves this process ahead of the file:
                    // what was learned is in memory and not on disk. Reported
                    // rather than hidden, and the engine is deliberately not
                    // rolled back -- the next write that succeeds carries both
                    // turns, which is the better of the two available
                    // outcomes.
                    if let Err(e) = store::save(&self.config.store.path, &self.engine) {
                        return Ok(refused(era, &e.to_string()));
                    }
                }
                Ok(answered(era, &render::render(&outcome)))
            }
            // The library's own words, verbatim. Every refusal in this
            // workspace names what was missing, and that sentence is the part
            // a model can act on.
            Err(e) => Ok(refused(era, &e.to_string())),
        }
    }

    fn run(&mut self, call: Call, now: Timestamp) -> Result<Outcome, HostError> {
        match call {
            Call::Remember { text, session } => {
                let provider = (self.provider)(&self.config)?;
                command::remember(&mut self.engine, &text, now, &session, &provider, &provider)
            }
            Call::Recall { query, k } => {
                let provider = (self.provider)(&self.config)?;
                command::recall(&self.engine, &query, k, &provider)
            }
            Call::About {
                entity,
                attribute,
                valid_at,
                as_of,
            } => command::about(&self.engine, entity, &attribute, valid_at, as_of),
            Call::Reviews => command::review_list(&self.engine),
            Call::ResolveReview { id, same } => {
                if same {
                    command::review_confirm(&mut self.engine, id)
                } else {
                    command::review_reject(&mut self.engine, id)
                }
            }
        }
    }
}

/// Who this server says it is.
fn server_info() -> Value {
    json!({
        "name": "rusty-memory",
        "title": "rusty-memory",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// `server/discover`, which the specification says servers **MUST** implement.
///
/// `supportedVersions` names the modern revision alone, matching what
/// [`version::era_of`] will actually accept in a `_meta` envelope. The
/// handshake revisions are reachable through `initialize` and not through this,
/// and listing them here would send a client back through a door that is shut.
fn discover() -> Value {
    json!({
        "resultType": "complete",
        "supportedVersions": [version::MODERN],
        "capabilities": {"tools": {}},
        "instructions": INSTRUCTIONS,
        "_meta": {SERVER_INFO_KEY: server_info()},
    })
}

fn list_tools(era: &Era) -> Value {
    // No `nextCursor`: five tools fit in one page, and a cursor a client could
    // follow to an empty second page is a round trip bought for nothing.
    complete(era, json!({"tools": tools::definitions()}))
}

fn empty(era: &Era) -> Value {
    complete(era, json!({}))
}

/// A tool call that ran.
fn answered(era: &Era, rendered: &render::Rendered) -> Value {
    let mut result = json!({
        "content": [{"type": "text", "text": rendered.text}],
        "isError": false,
    });
    if era.structured_content() {
        result["structuredContent"] = rendered.structured.clone();
    }
    complete(era, result)
}

/// A tool call the library refused.
///
/// `isError: true` on an ordinary result, and no `structuredContent`: there is
/// no structure to report, and inventing one would give a client validating
/// against a schema something to validate that never happened.
fn refused(era: &Era, why: &str) -> Value {
    complete(
        era,
        json!({
            "content": [{"type": "text", "text": why}],
            "isError": true,
        }),
    )
}

/// Stamp a result with what the era requires.
///
/// Modern results **MUST** carry `resultType`, and **SHOULD** carry the
/// server's identity in `_meta` so a stateless client knows who answered.
/// Legacy results have neither field, and emitting one is claiming to speak a
/// revision that did not have it.
fn complete(era: &Era, mut result: Value) -> Value {
    if era.result_type() {
        result["resultType"] = json!("complete");
        result["_meta"] = json!({SERVER_INFO_KEY: server_info()});
    }
    result
}
