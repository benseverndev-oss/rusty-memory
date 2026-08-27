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
use rm_host::time::At;
use rm_host::{store, HostError};

use crate::jsonrpc::{self, Request};
use crate::render;
use crate::tools::{self, Call};

/// Where this server gets vectors: the injected provider, or a local embedder.
///
/// The provider reaches `Server` through a factory so a test can hand it a
/// script instead of a socket. Reading `[provider] embedder` and building the
/// embedder straight from the config would walk around that seam -- which is
/// exactly what happened, and two protocol tests caught it by failing.
///
/// So the choice is made here and the injected provider is still used for the
/// remote case. Owned rather than borrowed because the factory returns an
/// owned provider.
enum Vectors<P> {
    Injected(P),
    Local(rm_embed::Hashed),
}

impl<P: Embedder> Embedder for Vectors<P> {
    fn embed(&self, text: &str) -> Result<Vec<f32>, rm_engine::EmbedderError> {
        match self {
            Vectors::Injected(p) => p.embed(text),
            Vectors::Local(h) => h.embed(text),
        }
    }
}

/// A call's model calls, made before the lock and carried into it.
///
/// An enum rather than a bag of `Option`s so a plan cannot be paired with a
/// call it was not built from: each arm below matches exactly one `Call`
/// variant, and `Nothing` covers the tools that reach no model at all.
enum Planned {
    Remember(command::RememberPlan),
    Decide(command::DecidePlan),
    Note(command::NotePlan),
    Recall(Vec<f32>),
    Nothing,
}
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
    provider: F,
    /// The version a legacy `initialize` settled on.
    ///
    /// The modern era is stateless and this stays `None` throughout it. The
    /// legacy era is not — the specification scopes a handshake "to the stdio
    /// process" — and the alternative to remembering it is to guess, for every
    /// subsequent `tools/call`, whether the client on the other end can read
    /// `structuredContent`.
    negotiated: Option<String>,
    /// What the client called itself in the handshake.
    ///
    /// Recorded against every write it makes. A store one agent uses does not
    /// need this; a store several share does, and asking each to remember a
    /// `session` argument is asking for the writes where it was forgotten.
    client: Option<String>,
}

/// What `initialize` settled, for a transport that must carry it itself.
///
/// Over stdio the connection is the session: one [`Server`] handshakes and
/// then answers every call, so this never leaves it. Streamable HTTP gives
/// each request its own connection and its own `Server`, so the handshake and
/// the calls after it land in different ones -- and both fields below are set
/// by the first and read by the rest.
///
/// Losing them is two separate bugs, which is why they travel together:
/// unattributed writes, and a legacy client sent a field it cannot read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Handshake {
    /// What the client called itself. Recorded against everything it writes.
    pub client: Option<String>,
    /// The revision a legacy `initialize` agreed on.
    ///
    /// Load-bearing rather than bookkeeping: `structuredContent` exists from
    /// `2025-06-18`, and a client that settled on something older and is
    /// answered as though it had not gets a field its parser has never seen.
    pub negotiated: Option<String>,
}

/// Where a read is asked from.
///
/// `all` beats an explicit scope, which beats the environment; `None` is no
/// position, which suspends the applicability rule.
///
/// The environment is read here rather than threaded from startup, matching
/// `tools::definitions`, which reads `TOOLS_ENV` the same way. This binary has
/// no clock-style spine to hang it on, and a long-lived process's environment
/// does not change under it.
fn position(scope: Option<String>, all: bool) -> Option<String> {
    if all {
        return None;
    }
    // Normalised rather than taken raw: `RMEM_SCOPE=` in a shell, or an empty
    // string in a JSON `env` block, reads as "not configured" and would
    // otherwise be the root position, where only `*` reaches.
    rm_host::scope::position(scope.or_else(|| std::env::var(crate::tools::SCOPE_ENV).ok()))
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
        // Opened and dropped. Nothing is kept: the store is re-read under a
        // lock on every call (see `call_tool`), and a copy held here would be
        // the stale snapshot that whole arrangement exists to prevent. What
        // this buys is the failure *timing* -- a bad config or an
        // unopenable store is reported now, to the person who can fix it,
        // rather than to a model on the first tool call.
        drop(store::load(
            &config.store.path,
            config.ruleset()?,
            config.policy_for_engine()?,
            config.provider.dimension,
            config.metric()?,
        )?);
        Ok(Server {
            config,
            provider,
            negotiated: None,
            client: None,
        })
    }

    /// What a handshake settled, for a transport that must carry it itself.
    ///
    /// Both fields at once, because they are lost together and for the same
    /// reason: they are set by `initialize` and read by every call after it,
    /// which over a connection-per-request transport is a different [`Server`].
    /// See [`Handshake`].
    pub fn handshake(&self) -> Handshake {
        Handshake {
            client: self.client.clone(),
            negotiated: self.negotiated.clone(),
        }
    }

    /// Restore what a previous request's handshake settled.
    ///
    /// The counterpart to [`Server::handshake`], and only for a transport
    /// holding the session itself. On stdio the connection *is* the session
    /// and nothing calls this.
    pub fn resume(&mut self, handshake: Handshake) {
        self.client = handshake.client;
        self.negotiated = handshake.negotiated;
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
        // Taken once, here, because this is the only message that carries it.
        // The version too, since two builds of one agent are two writers and a
        // log that cannot tell them apart cannot answer why they disagreed.
        self.client = request
            .params
            .get("clientInfo")
            .and_then(|c| {
                let name = c.get("name")?.as_str()?;
                Some(match c.get("version").and_then(Value::as_str) {
                    Some(v) => format!("{name} {v}"),
                    None => name.to_string(),
                })
            })
            .filter(|c| !c.trim().is_empty());
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

        let call = match Call::read(name, &arguments, self.client.as_deref()) {
            Ok(call) => call,
            Err(why) => return Ok(refused(era, &why)),
        };

        // One lock per call, spanning the read and the write.
        //
        // This module used to say the fix for two writers was "a lock file,
        // not a reload-per-call". That was wrong, and in a way worth stating:
        // they are not alternatives. A server holding an engine for the life
        // of the process has a snapshot that goes stale the moment anything
        // else writes, and a lock around `save` would faithfully serialise
        // writing that stale snapshot over the top of the other process's
        // work. Reloading without a lock narrows the window; locking without
        // reloading does not close it either. Only both together do.
        //
        // The cost the old note named is real and is now paid deliberately: a
        // snapshot parse and an index rebuild per tool call. It buys
        // correctness against a second writer, and it lands on a turn that has
        // already spent hundreds of milliseconds on an embedding API.
        let path = self.config.store.path.clone();
        // The shape the store has to be opened with. A failure here is a
        // refusal rather than a JSON-RPC error: it is the config being wrong,
        // which `open` has already reported once to whoever can fix it, and a
        // tool result is what reaches a model that might say so out loud.
        let shape = || -> Result<_, HostError> {
            Ok((
                self.config.ruleset()?,
                self.config.policy_for_engine()?,
                self.config.provider.dimension,
                self.config.metric()?,
            ))
        };
        let (ruleset, policy, dimension, metric) = match shape() {
            Ok(parts) => parts,
            Err(e) => return Ok(refused(era, &e.to_string())),
        };

        // Every model call this tool needs, made before either lock is taken.
        // A refusal here has cost nothing and blocked nobody, which is the
        // point: the locks below span a load, a change and a save, and
        // `Lock::acquire` gives up after five seconds. An embedding inside
        // that window made one write a three-second outage for every other
        // writer, and put the ceiling at three concurrent ones.
        let planned = match Self::plan(&self.config, &self.provider, &call, now, dimension, metric)
        {
            Ok(planned) => planned,
            Err(e) => return Ok(refused(era, &e.to_string())),
        };

        let outcome = if call.mutates() {
            store::with_write(&path, ruleset, policy, dimension, metric, |engine| {
                Self::write(engine, call, planned)
            })
        } else {
            store::with_read(&path, ruleset, policy, dimension, metric, |engine| {
                Self::read(engine, call, planned, now, self.config.retrieval.weak_below)
            })
        };

        match outcome {
            Ok(outcome) => Ok(answered(era, &render::render(&outcome))),
            // The library's own words, verbatim. Every refusal in this
            // workspace names what was missing, and that sentence is the part
            // a model can act on. A lock that could not be taken arrives here
            // too, and says so in the same voice.
            Err(e) => Ok(refused(era, &e.to_string())),
        }
    }

    /// Run a call that changes the store, against the engine the exclusive
    /// lock is holding.
    ///
    /// Split from [`Server::read`] along exactly the line [`Call::mutates`]
    /// draws, so the two cannot disagree: a call routed here has an exclusive
    /// lock and will be saved, and one routed there has a shared lock and a
    /// `&Engine` that cannot be written through even by mistake. The
    /// alternative — one function over `&mut Engine` for both — would have
    /// needed a clone of the whole engine per read to satisfy the borrow, and
    /// a type that says "read-only" is worth more than five saved lines.
    ///
    /// Associated rather than a method, and taking the engine as an argument,
    /// because the engine now belongs to the lock rather than to the server.
    fn write(engine: &mut Engine, call: Call, planned: Planned) -> Result<Outcome, HostError> {
        match (call, planned) {
            (Call::Remember { .. }, Planned::Remember(plan)) => {
                command::commit_remember(engine, plan)
            }
            (Call::Decide { .. }, Planned::Decide(plan)) => command::commit_decide(engine, plan),
            (Call::Note { .. }, Planned::Note(plan)) => command::commit_note(engine, plan),
            (Call::ResolveReview { id, same }, _) => {
                if same {
                    command::review_confirm(engine, id)
                } else {
                    command::review_reject(engine, id)
                }
            }
            // Guarded by `Call::mutates` at the one call site, and paired with
            // a plan built from the same variant by `Server::plan`. Reaching
            // here means those two were edited apart.
            (other, _) => unreachable!("{other:?} does not write, or was not planned"),
        }
    }

    /// The model calls a tool needs, made before any lock is taken.
    ///
    /// Everything here is a function of the call: an extraction reads the turn
    /// and an embedding reads its text, and neither asks the store anything.
    /// What genuinely needs the store -- resolving a mention against what is
    /// already known -- stays inside the lock, in [`Server::write`].
    /// The embedder for this call, honouring `[provider] embedder` without
    /// walking around the factory the remote one arrives through.
    fn vectors(config: &Config, provider: &F) -> Result<Vectors<P>, HostError> {
        match config.provider.embedder.as_str() {
            "local" => Ok(Vectors::Local(rm_embed::Hashed::new(
                config.provider.dimension,
            ))),
            "http" => Ok(Vectors::Injected(provider(config)?)),
            other => Err(HostError::Config(format!(
                "rmem.toml's [provider] embedder is {other:?}, which this build does not know; use \"http\" or \"local\"."
            ))),
        }
    }

    fn plan(
        config: &Config,
        provider: &F,
        call: &Call,
        now: Timestamp,
        dimension: usize,
        metric: rm_engine::Metric,
    ) -> Result<Planned, HostError> {
        match call {
            Call::Remember {
                text,
                session,
                speaker,
            } => {
                let provider = provider(config)?;
                Ok(Planned::Remember(command::plan_remember(
                    text,
                    now,
                    session,
                    speaker.as_deref(),
                    &provider,
                    &provider,
                    dimension,
                    metric,
                    command::Witness::Speaker,
                )?))
            }
            Call::Note {
                who,
                kind,
                attribute,
                value,
                fields,
                valid_from,
                scope,
                session,
                according_to,
            } => {
                // An embedder alone, like `decide`. A fact someone already
                // knows has a known shape, so nothing has to be guessed out of
                // prose -- which is what `remember` pays a completion for.
                let embedder = Self::vectors(config, provider)?;
                Ok(Planned::Note(command::plan_note(
                    who,
                    kind,
                    attribute,
                    value.as_deref(),
                    fields,
                    *valid_from,
                    now,
                    session,
                    scope.as_deref(),
                    *according_to,
                    &embedder,
                )?))
            }
            Call::Decide {
                title,
                choice,
                scope,
                status,
                decided_at,
                because,
                context,
                supersedes,
                session,
            } => {
                // An embedder alone. `decide` makes no completion call: a
                // decision has a known shape, so nothing has to be guessed out
                // of prose.
                let embedder = Self::vectors(config, provider)?;
                Ok(Planned::Decide(command::plan_decide(
                    title,
                    choice,
                    scope,
                    status.as_deref(),
                    because.as_deref(),
                    context.as_deref(),
                    supersedes.as_deref(),
                    *decided_at,
                    now,
                    session,
                    &embedder,
                )?))
            }
            Call::Recall { query, .. } => {
                let embedder = Self::vectors(config, provider)?;
                Ok(Planned::Recall(command::plan_recall(query, &embedder)?))
            }
            // Everything else is local work over a file on disk -- notably the
            // review answers, which write. The question was already asked and
            // answering it needs no model, so building a provider for them
            // would demand a credential none of them ever uses.
            _ => Ok(Planned::Nothing),
        }
    }

    /// Run a call that only reads, under the shared lock.
    fn read(
        engine: &Engine,
        call: Call,
        planned: Planned,
        now: Timestamp,
        weak_below: f32,
    ) -> Result<Outcome, HostError> {
        let _ = now;
        match (call, planned) {
            (
                Call::Recall {
                    k,
                    scope,
                    all,
                    depth,
                    ..
                },
                Planned::Recall(vector),
            ) => {
                let here = position(scope, all);
                command::commit_recall(engine, vector, k, weak_below, here.as_deref(), depth)
            }
            (
                Call::About {
                    entity,
                    attribute,
                    valid_at,
                    as_of,
                },
                _,
            ) => command::about(engine, entity, &attribute, valid_at, as_of, now, None),
            (Call::Reviews, _) => command::review_list(engine),
            (
                Call::Decisions {
                    status,
                    valid_at,
                    as_of,
                    scope,
                    all,
                },
                _,
            ) => {
                let here = position(scope, all);
                command::decisions(
                    engine,
                    status.as_deref(),
                    At {
                        valid: valid_at.unwrap_or(Timestamp::MAX),
                        tx: as_of.unwrap_or(Timestamp::MAX),
                    },
                    here.as_deref(),
                )
            }
            (
                Call::Decision {
                    title,
                    valid_at,
                    as_of,
                    scope,
                    all,
                },
                _,
            ) => {
                let here = position(scope, all);
                command::decision(
                    engine,
                    &title,
                    At {
                        valid: valid_at.unwrap_or(Timestamp::MAX),
                        tx: as_of.unwrap_or(Timestamp::MAX),
                    },
                    here.as_deref(),
                )
            }
            (other, _) => unreachable!("{other:?} writes, or was not planned"),
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
