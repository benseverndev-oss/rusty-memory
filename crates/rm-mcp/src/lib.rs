//! `rmem-mcp`, an MCP server over rusty-memory.
//!
//! # What is here
//!
//! A protocol, and nothing else. The five tools are [`rm_host::command`]'s five
//! operations, unchanged and shared with `rmem`, so a bug in this crate is a
//! bug in the wire and cannot be a bug in what remembering means.
//!
//! # Both eras
//!
//! MCP's current revision, `2026-07-28`, removed the `initialize` handshake:
//! there is no session, every request declares its own protocol version in
//! `_meta`, and `server/discover` replaces the handshake. Revisions up to
//! `2025-11-25` still handshake. This server speaks both, because the
//! specification's own compatibility matrix says a legacy client meeting a
//! modern-only server *fails* with no way to recover, and the revision that
//! removed the handshake is weeks old. See [`mod@version`] for the routing rule.
//!
//! # A refusal is an answer
//!
//! MCP splits protocol errors from tool execution errors, and the split is the
//! same one `rmem`'s exit code already makes. A request that is wrong as a
//! request is a JSON-RPC error. Everything the library refuses — survivorship
//! declining, a review id that does not exist, a model that answered with
//! something that is not an extraction — comes back as an ordinary result with
//! `isError: true`, carrying the library's own words, because those are what a
//! model can act on. `Believed::Unknown` is not an error at all: the store was
//! asked and has no opinion, which is an answer.
//!
//! # One writer
//!
//! The engine is held in memory for the life of the process and written back
//! after any tool that changed it. Two servers, or a server and a `rmem`
//! invocation, against one `memory.json` will lose writes: the second to save
//! overwrites what the first learned and neither notices. `rm-cli` already
//! records that there is no lock file; a long-lived process makes the same
//! limit easier to hit, so it is said here too rather than discovered. Fixing
//! it means a lock file, not a reload-per-call — reloading narrows the window
//! without closing it, and pays a snapshot parse and an index rebuild on every
//! tool call for the privilege.

pub mod jsonrpc;
pub mod render;
pub mod serve;
pub mod tools;
pub mod version;

pub use serve::Server;

/// The `_meta` key carrying a request's protocol version.
///
/// Written out rather than assembled, because its presence is what decides
/// which era a request belongs to and a typo would silently route every modern
/// client to the legacy path.
pub const PROTOCOL_VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";

/// The `_meta` key carrying client capabilities. Required on every modern
/// request.
pub const CLIENT_CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";

/// The `_meta` key a server identifies itself under, on every modern result.
pub const SERVER_INFO_KEY: &str = "io.modelcontextprotocol/serverInfo";

/// What a client tells a model about this server.
///
/// `instructions` is optional and most servers leave it out. It is worth
/// writing here because the one thing an agent has to know about this store is
/// the thing no other memory server would have taught it: an answer has three
/// states rather than two, and a contradiction is not resolved by overwriting.
pub const INSTRUCTIONS: &str = "\
Memory that keeps contradictions instead of resolving them.

remember() appends and never overwrites. When something contradicts what was \
stored before, both are kept with their own validity, and which one is believed \
is decided when you ask -- so ask about() again with a different valid_at rather \
than assuming the newest fact replaced the old one.

about() answers one of three ways and the difference is load-bearing. \"absent\" \
means someone asserted there is no value. \"unknown\" means it has never been \
discussed. Treating the second as the first will make you state as fact that \
someone has no employer when nobody has ever mentioned their job.

Entities that score too close to call are never merged. They are filed as open \
questions for a person to answer; reviews() lists them and resolve_review() \
settles one. Do not guess on the caller's behalf: a wrong merge is silent and \
permanent.";
