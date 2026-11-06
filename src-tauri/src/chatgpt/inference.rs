//! Model listing and the streaming Responses call (Models and inference, Preview limitations).

use std::time::Duration;

use futures_util::StreamExt;
use reqwest::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::Serialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::errors::{Error, ErrorKind, Result, api_error};
use super::oauth::header;
use super::secret::Secret;

pub const API_BASE: &str = "https://api.openai.com/v1";
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub slug: String,
    pub display_name: String,
}

pub async fn list_models(http: &Client, access: &Secret) -> Result<Vec<Model>> {
    let response = http
        .get(format!("{API_BASE}/models"))
        .bearer_auth(access.expose())
        .header(ACCEPT, "application/json")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| Error::network())?;
    let status = response.status();
    let request_id = header(&response, "x-request-id");
    let body: Option<Value> = response
        .bytes()
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    if !status.is_success() {
        return Err(api_error(
            body.as_ref(),
            Some(status.as_u16()),
            request_id.as_deref(),
        ));
    }
    parse_models(body.as_ref())
}

/// `{"models": [{slug, display_name, visibility}]}`: keep `visibility == "list"`, in server order.
pub fn parse_models(body: Option<&Value>) -> Result<Vec<Model>> {
    let invalid = || {
        Error::new(
            ErrorKind::Temporary,
            "invalid_model_catalog",
            "ChatGPT returned an unexpected model list. Try again.",
        )
    };
    let models = body
        .and_then(|b| b.get("models"))
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    let text = |model: &Value, key: &str| -> Option<String> {
        model
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty() && v.len() <= 200)
            .map(str::to_owned)
    };
    models
        .iter()
        .filter(|model| model.get("visibility").and_then(Value::as_str) == Some("list"))
        .map(|model| {
            Ok(Model {
                slug: text(model, "slug").ok_or_else(invalid)?,
                display_name: text(model, "display_name").ok_or_else(invalid)?,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Server-sent events
// ---------------------------------------------------------------------------------------------

/// Incremental SSE parser. Returns the `data` of each complete event.
#[derive(Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    data: String,
    has_data: bool,
}

impl SseParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=newline).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line);
            if line.is_empty() {
                if self.has_data {
                    events.push(std::mem::take(&mut self.data));
                    self.has_data = false;
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line.as_ref(), ""),
            };
            if field == "data" {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
        }
        events
    }

    /// Data of a final event that wasn't followed by a blank line.
    pub fn finish(&mut self) -> Option<String> {
        let tail = std::mem::take(&mut self.buffer);
        let mut events = self.push(&tail);
        events.extend(self.push(b"\n\n"));
        events.pop()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Delta(String),
    Completed,
    Continue,
}

pub fn interpret_event(data: &str, status: u16, request_id: Option<&str>) -> Result<Step> {
    if data == "[DONE]" {
        return Ok(Step::Continue);
    }
    let event: Value = serde_json::from_str(data).map_err(|_| invalid_stream())?;
    match event.get("type").and_then(Value::as_str) {
        Some("response.output_text.delta") => Ok(event
            .get("delta")
            .and_then(Value::as_str)
            .map_or(Step::Continue, |delta| Step::Delta(delta.to_owned()))),
        // The only signal that counts as success.
        Some("response.completed") => Ok(Step::Completed),
        // A usage limit can arrive here after streaming has started.
        Some("response.failed") => Err(api_error(event.get("response"), Some(status), request_id)),
        Some("error") => Err(api_error(Some(&event), Some(status), request_id)),
        Some("response.incomplete") => Err(Error::new(
            ErrorKind::Temporary,
            "response_incomplete",
            "ChatGPT stopped before finishing the response. You can try again.",
        )),
        _ => Ok(Step::Continue),
    }
}

fn invalid_stream() -> Error {
    Error::new(
        ErrorKind::Temporary,
        "invalid_stream",
        "ChatGPT sent a response Readback couldn't read. Try again.",
    )
}

fn interrupted() -> Error {
    Error::new(
        ErrorKind::Network,
        "stream_interrupted",
        "The connection to ChatGPT was interrupted before the response finished. Try again.",
    )
}

/// The request body. `store: false` and `stream: true` are mandatory for plan usage; input is a
/// list of messages; no other options are sent.
pub fn request_body(model: &str, prompt: &str) -> Value {
    json!({
        "model": model,
        "input": [{ "role": "user", "content": prompt }],
        "store": false,
        "stream": true,
    })
}

/// Streams a response, calling `on_delta` for each text delta. Never retried: usage-limit and
/// other plan errors are reported once.
pub async fn stream_response(
    http: &Client,
    access: &Secret,
    model: &str,
    prompt: &str,
    cancel: &CancellationToken,
    mut on_delta: impl FnMut(&str),
) -> Result<()> {
    let work = async {
        let response = http
            .post(format!("{API_BASE}/responses"))
            .bearer_auth(access.expose())
            .header(ACCEPT, "text/event-stream")
            .json(&request_body(model, prompt))
            .timeout(RESPONSE_TIMEOUT)
            .send()
            .await
            .map_err(|_| Error::network())?;
        let status = response.status();
        let request_id = header(&response, "x-request-id");
        if !status.is_success() {
            let body: Option<Value> = response
                .bytes()
                .await
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
            return Err(api_error(
                body.as_ref(),
                Some(status.as_u16()),
                request_id.as_deref(),
            ));
        }
        // The direct route may omit Content-Type; events are validated either way.
        if let Some(content_type) = header(&response, CONTENT_TYPE.as_str()) {
            let essence = content_type.split(';').next().unwrap_or_default().trim();
            if !essence.eq_ignore_ascii_case("text/event-stream") {
                return Err(invalid_stream());
            }
        }

        let mut stream = response.bytes_stream();
        let mut parser = SseParser::default();
        let mut received = 0usize;
        let mut handle = |data: String| -> Result<bool> {
            match interpret_event(&data, status.as_u16(), request_id.as_deref())? {
                Step::Delta(text) => {
                    received += text.len();
                    if received > MAX_RESPONSE_BYTES {
                        return Err(Error::new(
                            ErrorKind::Request,
                            "response_too_large",
                            "The response was too large. Try a shorter request.",
                        ));
                    }
                    on_delta(&text);
                    Ok(false)
                }
                Step::Completed => Ok(true),
                Step::Continue => Ok(false),
            }
        };
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| interrupted())?;
            for data in parser.push(&chunk) {
                if handle(data)? {
                    return Ok(());
                }
            }
        }
        if let Some(data) = parser.finish()
            && handle(data)?
        {
            return Ok(());
        }
        // Ending without response.completed is an error.
        Err(interrupted())
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(Error::cancelled()),
        result = work => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_body_follows_plan_usage_rules() {
        let body = request_body("some-model", "Hello");
        assert_eq!(body["store"], json!(false));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(
            body["input"],
            json!([{ "role": "user", "content": "Hello" }])
        );
        assert_eq!(body.as_object().unwrap().len(), 4);
    }

    #[test]
    fn models_keep_listed_entries_in_server_order() {
        let body = json!({ "models": [
            { "slug": "b", "display_name": "B", "visibility": "list" },
            { "slug": "hidden", "display_name": "H", "visibility": "hide" },
            { "slug": "a", "display_name": "A", "visibility": "list" },
        ]});
        let models = parse_models(Some(&body)).unwrap();
        assert_eq!(
            models.iter().map(|m| m.slug.as_str()).collect::<Vec<_>>(),
            ["b", "a"]
        );
        assert!(parse_models(Some(&json!({ "data": [] }))).is_err());
        assert!(
            parse_models(Some(
                &json!({ "models": [{ "slug": "x", "visibility": "list" }] })
            ))
            .is_err()
        );
    }

    #[test]
    fn sse_parser_handles_split_chunks_and_crlf() {
        let mut parser = SseParser::default();
        assert!(
            parser
                .push(b"event: response.output_text.delta\r\ndata: {\"a\"")
                .is_empty()
        );
        assert_eq!(
            parser.push(b":1}\r\n\r\ndata: x\n"),
            vec!["{\"a\":1}".to_string()]
        );
        assert_eq!(
            parser.push(b"data: y\n\n: comment\n\n"),
            vec!["x\ny".to_string()]
        );
        assert!(parser.push(b"data: tail").is_empty());
        assert_eq!(parser.finish(), Some("tail".to_string()));
    }

    #[test]
    fn sse_parser_keeps_split_utf8_intact() {
        let mut parser = SseParser::default();
        let bytes = "data: café\n\n".as_bytes();
        let (first, second) = bytes.split_at(10); // splits the 2-byte "é"
        assert!(parser.push(first).is_empty());
        assert_eq!(parser.push(second), vec!["café".to_string()]);
    }

    #[test]
    fn events_are_interpreted() {
        let delta = r#"{"type":"response.output_text.delta","delta":"Hi"}"#;
        assert_eq!(
            interpret_event(delta, 200, None).unwrap(),
            Step::Delta("Hi".into())
        );
        assert_eq!(
            interpret_event(r#"{"type":"response.completed"}"#, 200, None).unwrap(),
            Step::Completed
        );
        assert_eq!(
            interpret_event(r#"{"type":"response.created"}"#, 200, None).unwrap(),
            Step::Continue
        );

        let failed = r#"{"type":"response.failed","response":{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"x"}}}"#;
        let error = interpret_event(failed, 200, Some("req_1")).unwrap_err();
        assert_eq!(error.kind, ErrorKind::UsageLimit);
        assert!(!error.retryable);

        let incomplete = r#"{"type":"response.incomplete"}"#;
        assert_eq!(
            interpret_event(incomplete, 200, None).unwrap_err().code,
            "response_incomplete"
        );
        assert_eq!(
            interpret_event("not json", 200, None).unwrap_err().code,
            "invalid_stream"
        );
    }
}
