use futures_util::StreamExt;
use serde::{Deserialize, Deserializer};

use crate::types::{ChatResponse, ToolCall, Usage};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum StreamEvent {
  Content(String),
  Reasoning(String),
  ToolCalling,
}

#[derive(Debug, Deserialize)]
struct StreamChunk {
  #[serde(default, deserialize_with = "deserialize_default_on_null")]
  choices: Vec<StreamChoice>,
  usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
  #[serde(default, deserialize_with = "deserialize_default_on_null")]
  delta: StreamDelta,
}

#[derive(Debug, Deserialize, Default)]
struct StreamDelta {
  #[serde(default)]
  content: Option<String>,
  #[serde(default)]
  reasoning_content: Option<String>,
  #[serde(default, deserialize_with = "deserialize_default_on_null")]
  tool_calls: Vec<DeltaToolCall>,
}

#[derive(Debug, Deserialize)]
struct DeltaToolCall {
  index: usize,
  #[serde(default, deserialize_with = "deserialize_default_on_null")]
  id: String,
  #[serde(
    rename = "type",
    default,
    deserialize_with = "deserialize_default_on_null"
  )]
  kind: String,
  #[serde(default, deserialize_with = "deserialize_default_on_null")]
  function: DeltaFunctionCall,
}

#[derive(Debug, Deserialize, Default)]
struct DeltaFunctionCall {
  #[serde(default)]
  name: Option<String>,
  #[serde(default)]
  arguments: Option<String>,
}

fn deserialize_default_on_null<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
  D: Deserializer<'de>,
  T: Deserialize<'de> + Default,
{
  Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, thiserror::Error)]
pub enum SseError {
  #[error("sse stream read failed")]
  Read(#[source] reqwest::Error),
  #[error("sse stream ended without [DONE] sentinel")]
  TruncatedStream,
  #[error("sse line contained invalid UTF-8")]
  InvalidUtf8,
  #[error("sse chunk is not valid JSON")]
  JsonParse {
    data: String,
    #[source]
    source: serde_json::Error,
  },
}

impl SseError {
  pub fn is_retryable(&self) -> bool {
    matches!(self, Self::Read(_) | Self::TruncatedStream)
  }
}

/// Low-level SSE line classification.
enum SseLine {
  /// Decoded JSON string from a `data:` line (not `[DONE]`).
  Data(String),
  /// The `[DONE]` end-of-stream sentinel.
  Done,
  /// Empty lines, comments, or any non-`data:` line — caller should skip.
  Other,
}

/// Classify a single raw SSE line (bytes up to but not including `\n`).
///
/// Strips a trailing `\r` so that `\r\n`-terminated streams are handled
/// identically to `\n`-terminated ones. Returns `Err(SseError::InvalidUtf8)`
/// if the bytes are not valid UTF-8; all other errors are structural and
/// expressed through the `SseLine` variants.
fn decode_sse_line(line: &[u8]) -> Result<SseLine, SseError> {
  let line = line.strip_suffix(b"\r").unwrap_or(line);
  let s = std::str::from_utf8(line).map_err(|_| SseError::InvalidUtf8)?;
  let data = match s.strip_prefix("data:") {
    Some(d) => d.trim_start(),
    None => return Ok(SseLine::Other),
  };
  if data == "[DONE]" {
    return Ok(SseLine::Done);
  }
  Ok(SseLine::Data(data.to_string()))
}

#[derive(Default)]
struct AccToolCall {
  id: String,
  kind: String,
  name: String,
  arguments: String,
}

impl AccToolCall {
  fn into_tool_call(self) -> ToolCall {
    ToolCall::function(self.id, self.name, self.arguments)
  }
}

#[derive(Default)]
struct ChatAccumulator {
  response: ChatResponse,
  tool_calls: Vec<AccToolCall>,
  emitted_tool_calling: bool,
}

impl ChatAccumulator {
  fn apply(&mut self, chunk: StreamChunk) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    if let Some(usage) = chunk.usage {
      self.response.usage = usage;
    }
    for choice in chunk.choices {
      if let Some(reasoning_content) = choice.delta.reasoning_content {
        events.push(StreamEvent::Reasoning(reasoning_content.clone()));
        self.response.reasoning_content.push_str(&reasoning_content);
      }
      if let Some(content) = choice.delta.content {
        events.push(StreamEvent::Content(content.clone()));
        self.response.content.push_str(&content);
      }
      if !choice.delta.tool_calls.is_empty() && !self.emitted_tool_calling {
        self.emitted_tool_calling = true;
        events.push(StreamEvent::ToolCalling);
      }
      for tc in choice.delta.tool_calls {
        if tc.index >= self.tool_calls.len() {
          self
            .tool_calls
            .resize_with(tc.index + 1, AccToolCall::default);
        }
        let a = &mut self.tool_calls[tc.index];
        if !tc.id.is_empty() {
          a.id = tc.id;
        }
        if !tc.kind.is_empty() {
          a.kind = tc.kind;
        }
        if let Some(name) = tc.function.name
          && !name.is_empty()
        {
          a.name = name;
        }
        if let Some(args) = tc.function.arguments {
          a.arguments.push_str(&args);
        }
      }
    }
    events
  }

  fn finish(mut self) -> ChatResponse {
    self.response.tool_calls.extend(
      std::mem::take(&mut self.tool_calls)
        .into_iter()
        .map(AccToolCall::into_tool_call),
    );
    self.response
  }
}

async fn send_event(tx: &mut Option<tokio::sync::mpsc::Sender<StreamEvent>>, ev: StreamEvent) {
  if let Some(t) = tx
    && t.send(ev).await.is_err()
  {
    *tx = None;
  }
}

/// Parse an OpenAI-compatible SSE response into a [`ChatResponse`].
///
/// The byte stream is accumulated in a `Vec<u8>` and split on `b'\n'`
/// boundaries so that multi-byte UTF-8 characters that span network chunk
/// boundaries are never corrupted by premature string conversion.
///
/// Returns [`SseError::TruncatedStream`] if the connection closes before the
/// `[DONE]` sentinel is received, and [`SseError::JsonParse`] if a `data:`
/// line cannot be decoded as a [`StreamChunk`].
pub async fn parse_sse_response(
  resp: reqwest::Response,
  mut stream_tx: Option<tokio::sync::mpsc::Sender<StreamEvent>>,
) -> Result<ChatResponse, SseError> {
  let mut accumulator = ChatAccumulator::default();
  let mut stream = resp.bytes_stream();
  let mut buf: Vec<u8> = Vec::new();
  let mut done = false;

  'stream: while let Some(item) = stream.next().await {
    let bytes = item.map_err(SseError::Read)?;
    buf.extend_from_slice(&bytes);
    let mut consumed = 0;
    loop {
      let Some(rel_pos) = buf[consumed..].iter().position(|&b| b == b'\n') else {
        break;
      };
      let abs_end = consumed + rel_pos;
      match decode_sse_line(&buf[consumed..abs_end])? {
        SseLine::Done => {
          done = true;
          break 'stream;
        }
        SseLine::Data(json) => {
          let chunk = serde_json::from_str::<StreamChunk>(&json)
            .map_err(|source| SseError::JsonParse { data: json.clone(), source })?;
          for ev in accumulator.apply(chunk) {
            send_event(&mut stream_tx, ev).await;
          }
        }
        SseLine::Other => {}
      }
      consumed = abs_end + 1;
    }
    if consumed > 0 {
      buf.drain(..consumed);
    }
  }

  // Process any trailing bytes (last line without a terminating newline).
  if !done && !buf.is_empty() {
    match decode_sse_line(&buf)? {
      SseLine::Done => done = true,
      SseLine::Data(json) => {
        let chunk = serde_json::from_str::<StreamChunk>(&json)
          .map_err(|source| SseError::JsonParse { data: json.clone(), source })?;
        for ev in accumulator.apply(chunk) {
          send_event(&mut stream_tx, ev).await;
        }
      }
      SseLine::Other => {}
    }
  }

  if !done {
    return Err(SseError::TruncatedStream);
  }

  Ok(accumulator.finish())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn apply_data_line(acc: &mut ChatAccumulator, line: &str) -> Vec<StreamEvent> {
    let SseLine::Data(json) = decode_sse_line(line.as_bytes()).expect("valid SSE line") else {
      return vec![];
    };
    let chunk = serde_json::from_str::<StreamChunk>(&json).expect("valid JSON");
    acc.apply(chunk)
  }

  // --- decode_sse_line unit tests ---

  #[test]
  fn decode_sse_line_emits_data() {
    let payload = r#"{"choices":[]}"#;
    let line = format!("data: {payload}");
    let SseLine::Data(s) = decode_sse_line(line.as_bytes()).unwrap() else {
      panic!("expected SseLine::Data");
    };
    assert_eq!(s, payload);
  }

  #[test]
  fn decode_sse_line_emits_done() {
    assert!(matches!(
      decode_sse_line(b"data: [DONE]").unwrap(),
      SseLine::Done
    ));
    // No space after colon is also valid SSE.
    assert!(matches!(
      decode_sse_line(b"data:[DONE]").unwrap(),
      SseLine::Done
    ));
  }

  #[test]
  fn decode_sse_line_skips_non_data_lines() {
    assert!(matches!(decode_sse_line(b"").unwrap(), SseLine::Other));
    assert!(matches!(
      decode_sse_line(b": heartbeat").unwrap(),
      SseLine::Other
    ));
    assert!(matches!(
      decode_sse_line(b"event: ping").unwrap(),
      SseLine::Other
    ));
  }

  #[test]
  fn decode_sse_line_strips_trailing_cr() {
    assert!(matches!(
      decode_sse_line(b"data: [DONE]\r").unwrap(),
      SseLine::Done
    ));
  }

  #[test]
  fn decode_sse_line_rejects_invalid_utf8() {
    // 0xFF is not valid UTF-8
    assert!(matches!(
      decode_sse_line(b"data: \xff"),
      Err(SseError::InvalidUtf8)
    ));
  }

  // --- ChatAccumulator tests (use apply_data_line helper) ---

  #[tokio::test]
  async fn accumulator_applies_tool_call_chunks() {
    let mut acc = ChatAccumulator::default();
    apply_data_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","type":"function","function":{"name":"bash","arguments":"{\"command\""}}]}}]}"#,
    );
    apply_data_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"ls\""}}]}}]}"#,
    );
    assert_eq!(
      acc.tool_calls.first().unwrap().arguments,
      "{\"command\":\"ls\""
    );
  }

  #[tokio::test]
  async fn accumulator_accepts_null_content_chunks() {
    let mut acc = ChatAccumulator::default();
    apply_data_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"content":null,"reasoning_content":"thinking"}}]}"#,
    );
    apply_data_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"content":"hello","reasoning_content":null}}]}"#,
    );
    assert_eq!(acc.response.reasoning_content, "thinking");
    assert_eq!(acc.response.content, "hello");
  }

  #[tokio::test]
  async fn accumulator_accepts_null_function_fields() {
    let mut acc = ChatAccumulator::default();
    apply_data_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","type":"function","function":{"name":"read_file","arguments":""}}]}}]}"#,
    );
    apply_data_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":null,"arguments":"{\"path\": \"README.md"}}]}}]}"#,
    );
    apply_data_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":null,"arguments":"\"}"}}]}}]}"#,
    );
    let resp = acc.finish();
    let tc = resp.tool_calls.first().unwrap();
    assert_eq!(tc.function.name, "read_file");
    assert_eq!(tc.function.arguments, "{\"path\": \"README.md\"}");
  }

  #[tokio::test]
  async fn accumulator_accepts_null_tool_calls() {
    let mut acc = ChatAccumulator::default();
    apply_data_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"content":"hello","reasoning_content":"thinking","tool_calls":null}}]}"#,
    );
    assert_eq!(acc.response.reasoning_content, "thinking");
    assert_eq!(acc.response.content, "hello");
    assert!(acc.tool_calls.is_empty());
    assert!(!acc.emitted_tool_calling);
  }

  #[tokio::test]
  async fn accumulator_accepts_null_function_object() {
    let mut acc = ChatAccumulator::default();
    apply_data_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","type":"function","function":null}]}}]}"#,
    );
    let resp = acc.finish();
    let tc = resp.tool_calls.first().unwrap();
    assert_eq!(tc.id, "x");
    assert_eq!(tc.kind, "function");
    assert_eq!(tc.function.name, "");
    assert_eq!(tc.function.arguments, "");
  }
}
