use futures_util::StreamExt;
use serde::Deserialize;

use crate::types::{ChatResponse, FunctionCall, ToolCall, Usage};

#[derive(Debug, Clone)]
pub enum StreamEvent {
  Content(String),
  Reasoning(String),
  ToolCalling,
}

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

async fn send_event(tx: &mut Option<tokio::sync::mpsc::Sender<StreamEvent>>, ev: StreamEvent) {
  if let Some(t) = tx {
    if t.send(ev).await.is_err() {
      *tx = None;
    }
  }
}

pub async fn parse_sse_response(
  resp: reqwest::Response,
  cancel: Option<&tokio_util::sync::CancellationToken>,
  mut stream_tx: Option<tokio::sync::mpsc::Sender<StreamEvent>>,
) -> Result<ChatResponse, SseError> {
  let mut result = ChatResponse::default();
  let mut acc: Vec<AccToolCall> = Vec::new();
  let mut tool_calling = false;
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
      process_line(
        line,
        &mut result,
        &mut acc,
        &mut stream_tx,
        &mut tool_calling,
      )
      .await;
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
      &mut stream_tx,
      &mut tool_calling,
    )
    .await;
  }
  flush_tool_calls(&mut acc, &mut result);
  Ok(result)
}

async fn process_line(
  line: &str,
  result: &mut ChatResponse,
  acc: &mut Vec<AccToolCall>,
  stream_tx: &mut Option<tokio::sync::mpsc::Sender<StreamEvent>>,
  tool_calling: &mut bool,
) {
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
      send_event(stream_tx, StreamEvent::Reasoning(reasoning_content.clone())).await;
      result.reasoning_content.push_str(&reasoning_content);
    }
    if let Some(content) = choice.delta.content {
      send_event(stream_tx, StreamEvent::Content(content.clone())).await;
      result.content.push_str(&content);
    }
    if !choice.delta.tool_calls.is_empty() && !*tool_calling {
      *tool_calling = true;
      send_event(stream_tx, StreamEvent::ToolCalling).await;
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
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn process_line_accumulates_tool_args() {
    let mut resp = ChatResponse::default();
    let mut acc = Vec::new();
    let mut tc = false;
    process_line(
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","type":"function","function":{"name":"bash","arguments":"{\"command\""}}]}}]}"#,
      &mut resp,
      &mut acc,
      &mut None,
      &mut tc,
    ).await;
    process_line(
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"ls\""}}]}}]}"#,
      &mut resp,
      &mut acc,
      &mut None,
      &mut tc,
    ).await;
    assert_eq!(acc.first().unwrap().arguments, "{\"command\":\"ls\"");
  }

  #[tokio::test]
  async fn process_line_accepts_null_content_chunks() {
    let mut resp = ChatResponse::default();
    let mut acc = Vec::new();
    let mut tc = false;
    process_line(
      r#"data: {"choices":[{"delta":{"content":null,"reasoning_content":"thinking"}}]}"#,
      &mut resp,
      &mut acc,
      &mut None,
      &mut tc,
    )
    .await;
    process_line(
      r#"data: {"choices":[{"delta":{"content":"hello","reasoning_content":null}}]}"#,
      &mut resp,
      &mut acc,
      &mut None,
      &mut tc,
    )
    .await;
    assert_eq!(resp.reasoning_content, "thinking");
    assert_eq!(resp.content, "hello");
  }

  #[tokio::test]
  async fn process_line_accepts_null_function_fields() {
    let mut resp = ChatResponse::default();
    let mut acc = Vec::new();
    let mut tc = false;
    process_line(
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","type":"function","function":{"name":"read_file","arguments":""}}]}}]}"#,
      &mut resp,
      &mut acc,
      &mut None,
      &mut tc,
    ).await;
    process_line(
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":null,"arguments":"{\"path\": \"README.md"}}]}}]}"#,
      &mut resp,
      &mut acc,
      &mut None,
      &mut tc,
    ).await;
    process_line(
      r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":null,"arguments":"\"}"}}]}}]}"#,
      &mut resp,
      &mut acc,
      &mut None,
      &mut tc,
    ).await;
    flush_tool_calls(&mut acc, &mut resp);
    let tc = resp.tool_calls.first().unwrap();
    assert_eq!(tc.function.name, "read_file");
    assert_eq!(tc.function.arguments, "{\"path\": \"README.md\"}");
  }
}
