//! What goes on the wire, and what comes back.
//!
//! Pure by design. Everything here is a `&str` in and a value out, so the
//! decisions worth testing — how a prompt full of quotes and newlines is
//! encoded, what an error response means, how an empty one differs from an
//! empty answer — are tested without a socket.
//!
//! Every item below is exercised by the tests at the bottom of this file, but
//! nothing in the crate's normal build calls them yet: the transport that
//! will is the next task. Until then rustc sees `pub(crate)` functions and
//! response structs with no non-test caller and calls them dead. The allow
//! below is scoped to this file and comes off the day `HttpProvider` starts
//! calling into it.
#![allow(dead_code)]

use serde::Deserialize;

use crate::ProviderError;

/// An error body, which either provider may return in place of a result.
#[derive(Deserialize)]
struct ApiError {
    message: String,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ApiError,
}

#[derive(Deserialize)]
struct CompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    content: String,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingDatum>,
}

#[derive(Deserialize)]
struct EmbeddingDatum {
    embedding: Vec<f32>,
}

/// The body of a chat-completion request.
///
/// Built with `serde_json` rather than by formatting a string. An extraction
/// prompt is full of JSON examples, quotes and newlines, and concatenating it
/// into a template would produce a request the provider rejects — with an error
/// naming the API rather than the cause.
pub(crate) fn completion_body(model: &str, prompt: &str) -> String {
    serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": prompt }],
    })
    .to_string()
}

/// The body of an embedding request.
pub(crate) fn embedding_body(model: &str, text: &str) -> String {
    serde_json::json!({ "model": model, "input": text }).to_string()
}

/// The message content from a completion response.
pub(crate) fn parse_completion(body: &str) -> Result<String, ProviderError> {
    if let Some(err) = api_error(body) {
        return Err(err);
    }
    let parsed: CompletionResponse =
        serde_json::from_str(body).map_err(|e| ProviderError::Unparsable(e.to_string()))?;
    match parsed.choices.into_iter().next() {
        Some(choice) => Ok(choice.message.content),
        // Distinct from a choice whose content is "": that is an answer, and
        // this is the absence of one.
        None => Err(ProviderError::Empty("it carried no choices")),
    }
}

/// The vector from an embedding response.
pub(crate) fn parse_embedding(body: &str) -> Result<Vec<f32>, ProviderError> {
    if let Some(err) = api_error(body) {
        return Err(err);
    }
    let parsed: EmbeddingResponse =
        serde_json::from_str(body).map_err(|e| ProviderError::Unparsable(e.to_string()))?;
    match parsed.data.into_iter().next() {
        Some(datum) => Ok(datum.embedding),
        None => Err(ProviderError::Empty("it carried no embeddings")),
    }
}

/// An API error, if the body is one.
///
/// Checked before the success shape because a provider returns an error body
/// with a 200 often enough that relying on the status code alone would report
/// a parse failure for something that explained itself perfectly well.
fn api_error(body: &str) -> Option<ProviderError> {
    serde_json::from_str::<ErrorEnvelope>(body)
        .ok()
        .map(|e| ProviderError::Api(e.error.message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_completion_request_names_the_model_and_carries_the_prompt() {
        let body = completion_body("gpt-4o-mini", "extract this");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["model"], "gpt-4o-mini");
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"], "extract this");
    }

    #[test]
    fn a_prompt_containing_quotes_and_newlines_survives_the_round_trip() {
        // Extraction prompts are full of JSON examples and line breaks. Building
        // the body by string concatenation would produce a request the provider rejects, and the error would name the API rather than the cause.
        let awkward = "say {\"a\": 1}\nand \"quote\" it\\";
        let body = completion_body("m", awkward);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["messages"][0]["content"], awkward);
    }

    #[test]
    fn a_completion_response_yields_the_message_content() {
        let out = parse_completion(r#"{"choices":[{"message":{"content":"hello"}}]}"#).unwrap();
        assert_eq!(out, "hello");
    }

    #[test]
    fn a_response_with_no_choices_is_refused_rather_than_read_as_an_empty_answer() {
        // An empty answer and no answer are different, and only one of them is something the caller should act on.
        let err = parse_completion(r#"{"choices":[]}"#).unwrap_err();
        assert!(matches!(err, ProviderError::Empty(_)), "{err:?}");
    }

    #[test]
    fn an_api_error_reaches_the_caller_with_the_provider_s_own_words() {
        let err = parse_completion(
            r#"{"error":{"message":"model not found","type":"invalid_request_error"}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ProviderError::Api(_)), "{err:?}");
        assert!(err.to_string().contains("model not found"), "{err}");
    }

    #[test]
    fn a_response_that_is_not_json_says_so_rather_than_blaming_the_model() {
        let err = parse_completion("<html>502 Bad Gateway</html>").unwrap_err();
        assert!(matches!(err, ProviderError::Unparsable(_)), "{err:?}");
    }

    #[test]
    fn an_embedding_request_names_the_model_and_carries_the_text() {
        let body = embedding_body("text-embedding-3-small", "Ben works at Globex");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["model"], "text-embedding-3-small");
        assert_eq!(v["input"], "Ben works at Globex");
    }

    #[test]
    fn an_embedding_response_yields_the_vector() {
        let v = parse_embedding(r#"{"data":[{"embedding":[0.5,-0.25,0.125]}]}"#).unwrap();
        assert_eq!(v, vec![0.5, -0.25, 0.125]);
    }

    #[test]
    fn an_embedding_response_with_no_data_is_refused() {
        // A zero-length vector is rejected by the index under cosine anyway, but refusing here names the cause instead of surfacing it three layers later as a vector complaint.
        let err = parse_embedding(r#"{"data":[]}"#).unwrap_err();
        assert!(matches!(err, ProviderError::Empty(_)), "{err:?}");
    }

    #[test]
    fn an_embedding_error_reaches_the_caller_with_the_provider_s_own_words() {
        let err = parse_embedding(r#"{"error":{"message":"quota exceeded"}}"#).unwrap_err();
        assert!(err.to_string().contains("quota exceeded"), "{err}");
    }

    #[test]
    fn every_refusal_says_more_than_that_something_went_wrong() {
        for err in [
            ProviderError::Transport("connection refused".into()),
            ProviderError::Api("model not found".into()),
            ProviderError::Unparsable("expected value at line 1".into()),
            ProviderError::Empty("the response carried no choices"),
        ] {
            assert!(err.to_string().len() > 25, "{err:?}");
        }
    }
}
