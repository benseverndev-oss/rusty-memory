//! JSON-RPC 2.0, only as much of it as MCP uses.
//!
//! No batching, no server-initiated requests, no `id` correlation table: over
//! stdio this server reads a line, answers it, and reads the next one. What is
//! left is small enough to write, and writing it is what keeps this crate's
//! dependency list at four crates that were already in the workspace.

use serde_json::{json, Map, Value};

/// The line could not be parsed as JSON at all.
pub const PARSE_ERROR: i64 = -32700;
/// It parsed, but it is not a JSON-RPC request.
pub const INVALID_REQUEST: i64 = -32600;
/// No such method.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// The method exists and the parameters do not satisfy it. MCP also uses this
/// for a request missing the `_meta` fields it requires, and for a `tools/call`
/// naming a tool that does not exist.
pub const INVALID_PARAMS: i64 = -32602;
/// MCP's `UnsupportedProtocolVersionError`.
///
/// From the range `-32020..=-32099`, which the specification reserves for
/// itself: an implementation may only emit codes from it that the spec defines,
/// and only with the spec's meaning. This is one of three so far.
pub const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// One incoming message.
///
/// A notification is a request with no `id`, and the difference is the whole
/// of the handling rule: a notification **MUST NOT** be answered. Modelling it
/// as `Option<Value>` rather than as a second variant keeps that check in one
/// place — the dispatcher asks for the id when it is about to build a reply,
/// and there is nothing to build a reply *to* when it is `None`.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub id: Option<Value>,
    pub method: String,
    /// `params`, or `Value::Null` when the message carried none. Null rather
    /// than `Option`, because every reader below wants "look this up and tell
    /// me it is missing" and `Value::get` on a null already says that.
    pub params: Value,
}

impl Request {
    /// `params._meta`, or null.
    pub fn meta(&self) -> &Value {
        self.params.get("_meta").unwrap_or(&Value::Null)
    }
}

/// Read one line.
///
/// The error is a finished response rather than a description of what went
/// wrong, because at this point there is nothing else to do with it and the
/// caller would only be re-deriving the code and the id. A message so broken
/// that its id cannot be read is answered with `"id": null`, which is what
/// JSON-RPC says to do and the one place a null id is legal.
pub fn parse(line: &str) -> Result<Request, Value> {
    let value: Value = serde_json::from_str(line).map_err(|e| {
        // The parser's own words, and no fragment of the line. A line this
        // server cannot parse may still be a line someone pasted a key into.
        error(None, PARSE_ERROR, &format!("Parse error: {e}"), None)
    })?;

    // The id is read before anything else is validated, so that a request with
    // a good id and a bad body is still answered on its own id rather than on
    // null -- a client correlating by id has no other way to retire it.
    let id = match value.get("id") {
        None => None,
        Some(Value::String(_)) | Some(Value::Number(_)) => value.get("id").cloned(),
        // MCP is stricter than JSON-RPC here: "Unlike base JSON-RPC, the ID
        // MUST NOT be null". A null id is therefore not a notification, it is
        // a malformed request, and treating it as a notification would drop
        // the message silently.
        Some(_) => {
            return Err(error(
                None,
                INVALID_REQUEST,
                "Invalid Request: id must be a string or a number, and must not be null",
                None,
            ))
        }
    };

    if value.get("jsonrpc") != Some(&json!("2.0")) {
        return Err(error(
            id.as_ref(),
            INVALID_REQUEST,
            "Invalid Request: jsonrpc must be \"2.0\"",
            None,
        ));
    }

    let Some(Value::String(method)) = value.get("method") else {
        return Err(error(
            id.as_ref(),
            INVALID_REQUEST,
            "Invalid Request: method must be a string",
            None,
        ));
    };

    Ok(Request {
        id,
        method: method.clone(),
        params: value.get("params").cloned().unwrap_or(Value::Null),
    })
}

/// A success response.
pub fn result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// An error response. `id` is `None` only when the request's own id could not
/// be read.
pub fn error(id: Option<&Value>, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut e = Map::new();
    e.insert("code".into(), json!(code));
    e.insert("message".into(), json!(message));
    if let Some(data) = data {
        e.insert("data".into(), data);
    }
    json!({"jsonrpc": "2.0", "id": id.cloned().unwrap_or(Value::Null), "error": Value::Object(e)})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(line: &str) -> Request {
        parse(line).unwrap()
    }

    fn err_code(line: &str) -> i64 {
        parse(line).unwrap_err()["error"]["code"].as_i64().unwrap()
    }

    #[test]
    fn a_request_carries_its_id_its_method_and_its_params() {
        let r = req(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"cursor":"c"}}"#);
        assert_eq!(r.id, Some(json!(1)));
        assert_eq!(r.method, "tools/list");
        assert_eq!(r.params["cursor"], json!("c"));
    }

    #[test]
    fn a_message_with_no_id_is_a_notification() {
        // The distinction the whole dispatcher hangs on: a notification MUST
        // NOT be answered, and answering one puts a response on the wire the
        // client has no request to match it to.
        let r = req(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert_eq!(r.id, None);
    }

    #[test]
    fn a_null_id_is_malformed_rather_than_a_notification() {
        // MCP is stricter than JSON-RPC: "Unlike base JSON-RPC, the ID MUST
        // NOT be null." Reading `null` as absent would turn a broken request
        // into a message this server drops without a word, which is the worst
        // of the available behaviours -- the client waits for a reply that was
        // never going to come.
        assert_eq!(
            err_code(r#"{"jsonrpc":"2.0","id":null,"method":"tools/list"}"#),
            INVALID_REQUEST
        );
    }

    #[test]
    fn a_string_id_survives_as_a_string() {
        // Ids are strings or numbers and clients use both. Coercing one to the
        // other would break correlation for every client that uses strings,
        // which includes the spec's own `server/discover` example.
        let r = req(r#"{"jsonrpc":"2.0","id":"discover-1","method":"server/discover"}"#);
        assert_eq!(r.id, Some(json!("discover-1")));
    }

    #[test]
    fn params_default_to_null_rather_than_being_absent() {
        let r = req(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
        assert_eq!(r.params, Value::Null);
        // And looking inside a null is a miss, not a panic, which is the
        // reason for the default.
        assert_eq!(r.meta(), &Value::Null);
    }

    #[test]
    fn an_unparseable_line_is_answered_on_a_null_id() {
        let e = parse("{not json").unwrap_err();
        assert_eq!(e["error"]["code"], json!(PARSE_ERROR));
        assert_eq!(e["id"], Value::Null);
    }

    #[test]
    fn a_parse_error_does_not_quote_the_line_back() {
        // Every refusal in this workspace names a field or a location and
        // never a value, because a value may be a key someone pasted. A line
        // this server could not parse is exactly where a mis-paste lands.
        let e = parse(r#"{"jsonrpc":"2.0" "id":1, "key":"sk-secret-value-here"#).unwrap_err();
        let message = e["error"]["message"].as_str().unwrap();
        assert!(!message.contains("sk-secret"), "{message}");
    }

    #[test]
    fn a_bad_body_is_still_answered_on_its_own_id() {
        // A client correlating by id has no other way to retire the request.
        // Answering on null leaves it waiting for the full timeout.
        let e = parse(r#"{"jsonrpc":"1.0","id":7,"method":"tools/list"}"#).unwrap_err();
        assert_eq!(e["error"]["code"], json!(INVALID_REQUEST));
        assert_eq!(e["id"], json!(7));
    }

    #[test]
    fn a_missing_or_non_string_method_is_an_invalid_request() {
        assert_eq!(err_code(r#"{"jsonrpc":"2.0","id":1}"#), INVALID_REQUEST);
        assert_eq!(
            err_code(r#"{"jsonrpc":"2.0","id":1,"method":7}"#),
            INVALID_REQUEST
        );
    }

    #[test]
    fn an_error_omits_data_entirely_when_there_is_none() {
        // Rather than sending `"data": null`, which a strict client may read as
        // a data member that is present and empty.
        let e = error(Some(&json!(1)), METHOD_NOT_FOUND, "nope", None);
        assert!(e["error"].get("data").is_none(), "{e}");
        let with = error(
            Some(&json!(1)),
            INVALID_PARAMS,
            "nope",
            Some(json!({"a": 1})),
        );
        assert_eq!(with["error"]["data"]["a"], json!(1));
    }
}
