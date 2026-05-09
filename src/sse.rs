use futures_util::StreamExt;
use serde::Deserialize;

use crate::types::{ChatResponse, FunctionCall, ToolCall, Usage};

#[derive(Debug, Deserialize)]
struct StreamChunk {
  #[serde(default)]
  choices: Vec<StreamChoice>,
  usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
  delta: StreamDelta,
}

#[derive(Debug, Deserialize, Default)]
struct StreamDelta {
  #[serde(default)]
  content: Option<String>,
  #[serde(default)]
  reasoning_content: Option<String>,
  #[serde(default)]
  tool_calls: Vec<DeltaToolCall>,
}

#[derive(Debug, Deserialize)]
struct DeltaToolCall {
  index: usize,
  #[serde(default)]
  id: String,
  #[serde(rename = "type", default)]
  kind: String,
  #[serde(default)]
  function: DeltaFunctionCall,
}

#[derive(Debug, Deserialize, Default)]
struct DeltaFunctionCall {
  #[serde(default)]
  name: Option<String>,
  #[serde(default)]
  arguments: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SseError {
  #[error("chat aborted by context cancellation")]
  Aborted { resp: ChatResponse },
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
    ToolCall {
      id: self.id,
      kind: self.kind,
      function: FunctionCall {
        name: self.name,
        arguments: self.arguments,
      },
    }
  }
}

fn flush_tool_calls(acc: &mut Vec<AccToolCall>, result: &mut ChatResponse) {
  result.tool_calls.extend(
    std::mem::take(acc)
      .into_iter()
      .map(AccToolCall::into_tool_call),
  );
}

pub async fn parse_sse_response(
  resp: reqwest::Response,
  cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<ChatResponse, SseError> {
  let mut result = ChatResponse::default();
  let mut acc: Vec<AccToolCall> = Vec::new();
  let mut stream = resp.bytes_stream();
  let mut buf = String::new();
  let mut consumed = 0;

  while let Some(item) = stream.next().await {
    if cancel.is_some_and(tokio_util::sync::CancellationToken::is_cancelled) {
      flush_tool_calls(&mut acc, &mut result);
      return Err(SseError::Aborted { resp: result });
    }
    let bytes = item.map_err(SseError::Read)?;
    buf.push_str(&String::from_utf8_lossy(&bytes));
    while let Some(pos) = buf[consumed..].find('\n') {
      let abs_pos = consumed + pos;
      let line = buf[consumed..abs_pos].trim_end_matches('\r');
      process_line(line, &mut result, &mut acc);
      consumed = abs_pos + 1;
    }
    if consumed > 0 {
      buf.drain(..consumed);
      consumed = 0;
    }
  }
  if !buf[consumed..].is_empty() {
    process_line(
      buf[consumed..].trim_end_matches('\r'),
      &mut result,
      &mut acc,
    );
  }
  flush_tool_calls(&mut acc, &mut result);
  Ok(result)
}

fn process_line(line: &str, result: &mut ChatResponse, acc: &mut Vec<AccToolCall>) {
  let Some(data) = line.strip_prefix("data:") else {
    return;
  };
  let data = data.trim_start();
  if data == "[DONE]" {
    return;
  }
  let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) else {
    return;
  };
  if let Some(usage) = chunk.usage {
    result.usage = usage;
  }
  for choice in chunk.choices {
    if let Some(reasoning_content) = choice.delta.reasoning_content {
      result.reasoning_content.push_str(&reasoning_content);
    }
    if let Some(content) = choice.delta.content {
      result.content.push_str(&content);
    }
    for tc in choice.delta.tool_calls {
      if tc.index >= acc.len() {
        acc.resize_with(tc.index + 1, AccToolCall::default);
      }
      let a = &mut acc[tc.index];
      if !tc.id.is_empty() {
        a.id = tc.id;
      }
      if !tc.kind.is_empty() {
        a.kind = tc.kind;
      }
      if let Some(name) = tc.function.name {
        if !name.is_empty() {
          a.name = name;
        }
      }
      if let Some(args) = tc.function.arguments {
        a.arguments.push_str(&args);
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn process_line_accumulates_tool_args() {
    let mut resp = ChatResponse::default();
    let mut acc = Vec::new();
    process_line(
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","type":"function","function":{"name":"bash","arguments":"{\"command\""}}]}}]}"#,
      &mut resp,
      &mut acc,
    );
    process_line(
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"ls\""}}]}}]}"#,
      &mut resp,
      &mut acc,
    );
    assert_eq!(acc.first().unwrap().arguments, "{\"command\":\"ls\"");
  }

  #[test]
  fn process_line_accepts_null_content_chunks() {
    let mut resp = ChatResponse::default();
    let mut acc = Vec::new();
    process_line(
      r#"data: {"choices":[{"delta":{"content":null,"reasoning_content":"thinking"}}]}"#,
      &mut resp,
      &mut acc,
    );
    process_line(
      r#"data: {"choices":[{"delta":{"content":"hello","reasoning_content":null}}]}"#,
      &mut resp,
      &mut acc,
    );
    assert_eq!(resp.reasoning_content, "thinking");
    assert_eq!(resp.content, "hello");
  }

  #[test]
  fn process_line_accepts_null_function_fields() {
    let mut resp = ChatResponse::default();
    let mut acc = Vec::new();
    process_line(
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","type":"function","function":{"name":"read_file","arguments":""}}]}}]}"#,
      &mut resp,
      &mut acc,
    );
    process_line(
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":null,"arguments":"{\"path\": \"README.md"}}]}}]}"#,
      &mut resp,
      &mut acc,
    );
    process_line(
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":null,"arguments":"\"}"}}]}}]}"#,
      &mut resp,
      &mut acc,
    );
    flush_tool_calls(&mut acc, &mut resp);
    let tc = resp.tool_calls.first().unwrap();
    assert_eq!(tc.function.name, "read_file");
    assert_eq!(tc.function.arguments, "{\"path\": \"README.md\"}");
  }
}
