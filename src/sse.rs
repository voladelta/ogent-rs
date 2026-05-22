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
    ToolCall::function(
      self.id,
      self.name,
      repair_tool_arguments_json(&self.arguments),
    )
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

fn repair_tool_arguments_json(arguments: &str) -> String {
  if arguments.trim().is_empty() || serde_json::from_str::<serde_json::Value>(arguments).is_ok() {
    return arguments.to_string();
  }

  let mut stack = Vec::new();
  let mut in_string = false;
  let mut escaped = false;

  for ch in arguments.chars() {
    if in_string {
      if escaped {
        escaped = false;
      } else if ch == '\\' {
        escaped = true;
      } else if ch == '"' {
        in_string = false;
      }
      continue;
    }

    match ch {
      '"' => in_string = true,
      '{' => stack.push('}'),
      '[' => stack.push(']'),
      '}' | ']' if stack.pop() != Some(ch) => {
        return arguments.to_string();
      }
      _ => {}
    }
  }

  if in_string || stack.is_empty() {
    return arguments.to_string();
  }

  let mut repaired = arguments.to_string();
  while let Some(ch) = stack.pop() {
    repaired.push(ch);
  }

  if serde_json::from_str::<serde_json::Value>(&repaired).is_ok() {
    repaired
  } else {
    arguments.to_string()
  }
}

fn truncate_for_log(s: &str, limit: usize) -> String {
  if s.len() <= limit {
    return s.to_string();
  }
  let end = s.floor_char_boundary(limit);
  let mut out = s[..end].to_string();
  out.push_str("...");
  out
}

async fn send_event(tx: &mut Option<tokio::sync::mpsc::Sender<StreamEvent>>, ev: StreamEvent) {
  if let Some(t) = tx
    && t.send(ev).await.is_err()
  {
    *tx = None;
  }
}

fn parse_sse_data_line(line: &str) -> Option<StreamChunk> {
  let data = line.strip_prefix("data:")?.trim_start();
  if data == "[DONE]" {
    return None;
  }
  match serde_json::from_str::<StreamChunk>(data) {
    Ok(chunk) => Some(chunk),
    Err(_) => {
      eprintln!(
        "failed to parse sse data chunk: {}",
        truncate_for_log(data, 240)
      );
      None
    }
  }
}

pub async fn parse_sse_response(
  resp: reqwest::Response,
  mut stream_tx: Option<tokio::sync::mpsc::Sender<StreamEvent>>,
) -> Result<ChatResponse, SseError> {
  let mut accumulator = ChatAccumulator::default();
  let mut stream = resp.bytes_stream();
  let mut buf = String::new();
  let mut consumed = 0;

  while let Some(item) = stream.next().await {
    let bytes = item.map_err(SseError::Read)?;
    buf.push_str(&String::from_utf8_lossy(&bytes));
    while let Some(pos) = buf[consumed..].find('\n') {
      let abs_pos = consumed + pos;
      let line = buf[consumed..abs_pos].trim_end_matches('\r');
      if let Some(chunk) = parse_sse_data_line(line) {
        for ev in accumulator.apply(chunk) {
          send_event(&mut stream_tx, ev).await;
        }
      }
      consumed = abs_pos + 1;
    }
    if consumed > 0 {
      buf.drain(..consumed);
      consumed = 0;
    }
  }
  if !buf[consumed..].is_empty() {
    if let Some(chunk) = parse_sse_data_line(buf[consumed..].trim_end_matches('\r')) {
      for ev in accumulator.apply(chunk) {
        send_event(&mut stream_tx, ev).await;
      }
    }
  }
  Ok(accumulator.finish())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn apply_line(acc: &mut ChatAccumulator, line: &str) -> Vec<StreamEvent> {
    let chunk = parse_sse_data_line(line).expect("valid test line");
    acc.apply(chunk)
  }

  #[tokio::test]
  async fn accumulator_applies_tool_call_chunks() {
    let mut acc = ChatAccumulator::default();
    apply_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","type":"function","function":{"name":"bash","arguments":"{\"command\""}}]}}]}"#,
    );
    apply_line(
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
    apply_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"content":null,"reasoning_content":"thinking"}}]}"#,
    );
    apply_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"content":"hello","reasoning_content":null}}]}"#,
    );
    assert_eq!(acc.response.reasoning_content, "thinking");
    assert_eq!(acc.response.content, "hello");
  }

  #[tokio::test]
  async fn accumulator_accepts_null_function_fields() {
    let mut acc = ChatAccumulator::default();
    apply_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","type":"function","function":{"name":"read_file","arguments":""}}]}}]}"#,
    );
    apply_line(
      &mut acc,
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":null,"arguments":"{\"path\": \"README.md"}}]}}]}"#,
    );
    apply_line(
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
    apply_line(
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
    apply_line(
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

  #[test]
  fn finish_repairs_missing_closing_delimiters() {
    let acc = ChatAccumulator {
      response: ChatResponse::default(),
      tool_calls: vec![AccToolCall {
        id: "x".to_string(),
        kind: "function".to_string(),
        name: "write_file".to_string(),
        arguments: r#"{"path":"out.txt","content":"fix it""#.to_string(),
      }],
      emitted_tool_calling: false,
    };

    let resp = acc.finish();

    let tc = resp.tool_calls.first().unwrap();
    assert_eq!(
      tc.function.arguments,
      "{\"path\":\"out.txt\",\"content\":\"fix it\"}"
    );
  }

  #[test]
  fn repair_tool_arguments_json_ignores_brackets_inside_strings() {
    let repaired =
      repair_tool_arguments_json(r#"{"command":"printf '] }'","items":[{"path":"src/sse.rs"}"#);

    assert_eq!(
      repaired,
      r#"{"command":"printf '] }'","items":[{"path":"src/sse.rs"}]}"#
    );
  }

  #[test]
  fn repair_tool_arguments_json_leaves_mismatched_json_unchanged() {
    let arguments = r#"{"items":[}]"#;

    assert_eq!(repair_tool_arguments_json(arguments), arguments);
  }
}
