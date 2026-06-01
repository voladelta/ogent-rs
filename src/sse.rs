use futures_util::StreamExt;
use serde::{Deserialize, Deserializer};

use crate::types::{ChatResponse, ToolCall, Usage};

#[derive(Debug, Clone)]
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

enum SseLine {
  Data(String),
  Done,
  Other,
}

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
struct AccumulatedToolCall {
  id: String,
  kind: String,
  name: String,
  arguments: String,
}

impl AccumulatedToolCall {
  fn into_tool_call(self) -> ToolCall {
    ToolCall::function(self.id, self.name, self.arguments)
  }
}

#[derive(Default)]
struct ChatAccumulator {
  response: ChatResponse,
  tool_calls: Vec<AccumulatedToolCall>,
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
            .resize_with(tc.index + 1, AccumulatedToolCall::default);
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
        .map(AccumulatedToolCall::into_tool_call),
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

async fn parse_sse_byte_stream<S, B>(
  stream: S,
  mut stream_tx: Option<tokio::sync::mpsc::Sender<StreamEvent>>,
  require_done: bool,
) -> Result<ChatResponse, SseError>
where
  S: futures_util::Stream<Item = Result<B, SseError>>,
  B: AsRef<[u8]>,
{
  let mut accumulator = ChatAccumulator::default();
  let mut buf: Vec<u8> = Vec::new();
  let mut done = false;

  futures_util::pin_mut!(stream);
  'stream: while let Some(item) = stream.next().await {
    let bytes = item?;
    buf.extend_from_slice(bytes.as_ref());
    let mut consumed = 0;
    while let Some(rel_pos) = buf[consumed..].iter().position(|&b| b == b'\n') {
      let abs_end = consumed + rel_pos;
      match decode_sse_line(&buf[consumed..abs_end])? {
        SseLine::Done => {
          done = true;
          break 'stream;
        }
        SseLine::Data(json) => {
          let chunk =
            serde_json::from_str::<StreamChunk>(&json).map_err(|source| SseError::JsonParse {
              data: json.clone(),
              source,
            })?;
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

  if !done && !buf.is_empty() {
    match decode_sse_line(&buf)? {
      SseLine::Done => done = true,
      SseLine::Data(json) => {
        let chunk =
          serde_json::from_str::<StreamChunk>(&json).map_err(|source| SseError::JsonParse {
            data: json.clone(),
            source,
          })?;
        for ev in accumulator.apply(chunk) {
          send_event(&mut stream_tx, ev).await;
        }
      }
      SseLine::Other => {}
    }
  }

  if !done && require_done {
    return Err(SseError::TruncatedStream);
  }

  Ok(accumulator.finish())
}

pub async fn parse_sse_response(
  resp: reqwest::Response,
  stream_tx: Option<tokio::sync::mpsc::Sender<StreamEvent>>,
  require_done: bool,
) -> Result<ChatResponse, SseError> {
  let stream = resp.bytes_stream().map(|item| item.map_err(SseError::Read));
  parse_sse_byte_stream(stream, stream_tx, require_done).await
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
    assert!(matches!(
      decode_sse_line(b"data: \xff"),
      Err(SseError::InvalidUtf8)
    ));
  }

  #[test]
  fn accumulator_applies_tool_call_chunks() {
    let mut acc = ChatAccumulator::default();
    apply_data_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","type":"function","function":{"name":"shell","arguments":"{\"command\""}}]}}]}"#,
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

  #[test]
  fn accumulator_accepts_null_content_chunks() {
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

  #[test]
  fn accumulator_accepts_null_function_fields() {
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

  #[test]
  fn accumulator_accepts_null_tool_calls() {
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

  #[test]
  fn accumulator_accepts_null_function_object() {
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

  async fn sse_response(body: Vec<u8>) -> reqwest::Response {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
      let (mut sock, _) = listener.accept().await.unwrap();
      let mut buf = [0u8; 2048];
      let _ = sock.read(&mut buf).await;
      let headers =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
      let _ = sock.write_all(headers).await;
      let _ = sock.write_all(&body).await;
    });
    reqwest::get(format!("http://127.0.0.1:{}", addr.port()))
      .await
      .unwrap()
  }

  #[tokio::test]
  async fn parse_sse_response_ok_with_done() {
    let body =
      b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n".to_vec();
    let resp = sse_response(body).await;
    let result = parse_sse_response(resp, None, true).await;
    assert!(result.is_ok(), "unexpected error: {result:?}");
    assert_eq!(result.unwrap().content, "hi");
  }

  #[tokio::test]
  async fn parse_sse_response_truncated_stream_without_done() {
    let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n".to_vec();
    let resp = sse_response(body).await;
    let result = parse_sse_response(resp, None, true).await;
    assert!(
      matches!(result, Err(SseError::TruncatedStream)),
      "expected TruncatedStream, got {result:?}"
    );
  }

  #[tokio::test]
  async fn parse_sse_response_ok_without_done_when_not_required() {
    let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n".to_vec();
    let resp = sse_response(body).await;
    let result = parse_sse_response(resp, None, false).await;
    assert!(result.is_ok(), "unexpected error: {result:?}");
    assert_eq!(result.unwrap().content, "hi");
  }

  #[tokio::test]
  async fn parse_sse_response_json_parse_error_on_invalid_data() {
    let body = b"data: not-valid-json\n\ndata: [DONE]\n\n".to_vec();
    let resp = sse_response(body).await;
    let result = parse_sse_response(resp, None, true).await;
    assert!(
      matches!(result, Err(SseError::JsonParse { .. })),
      "expected JsonParse, got {result:?}"
    );
  }

  #[tokio::test]
  async fn parse_sse_byte_stream_multibyte_utf8_split_across_chunks() {
    // "café" — the é (U+00E9) is encoded as 0xC3 0xA9 (two bytes).
    // We split the SSE line right between those two bytes.
    let prefix = b"data: {\"choices\":[{\"delta\":{\"content\":\"caf".to_vec();
    let first_byte_of_e = b"\xc3".to_vec(); // first byte of é
    let suffix = b"\xa9\"}}]}\n\ndata: [DONE]\n\n".to_vec(); // second byte + rest
    let stream = futures_util::stream::iter(
      vec![prefix, first_byte_of_e, suffix]
        .into_iter()
        .map(Ok::<Vec<u8>, SseError>),
    );
    let result = parse_sse_byte_stream(stream, None, true).await;
    assert!(result.is_ok(), "unexpected error: {result:?}");
    assert_eq!(result.unwrap().content, "café");
  }
}
