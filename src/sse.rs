use anyhow::{Context, Result};
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
  name: String,
  #[serde(default)]
  arguments: String,
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
) -> Result<ChatResponse> {
  let mut result = ChatResponse::default();
  let mut acc: Vec<AccToolCall> = Vec::new();
  let mut stream = resp.bytes_stream();
  let mut buf = String::new();

  while let Some(item) = stream.next().await {
    if cancel.is_some_and(|c| c.is_cancelled()) {
      flush_tool_calls(&mut acc, &mut result);
      return Err(crate::types::ChatAbortedError { resp: result }.into());
    }
    let bytes = item.context("read sse")?;
    buf.push_str(&String::from_utf8_lossy(&bytes));
    while let Some(pos) = buf.find('\n') {
      let line = buf[..pos].trim_end_matches('\r');
      process_line(line, &mut result, &mut acc);
      buf.drain(..=pos);
    }
  }
  flush_tool_calls(&mut acc, &mut result);
  Ok(result)
}

fn process_line(
  line: &str,
  result: &mut ChatResponse,
  acc: &mut Vec<AccToolCall>,
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
      if !tc.function.name.is_empty() {
        a.name = tc.function.name;
      }
      a.arguments.push_str(&tc.function.arguments);
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
    process_line(r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","type":"function","function":{"name":"bash","arguments":"{\"command\""}}]}}]}"#, &mut resp, &mut acc);
    process_line(r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"ls\""}}]}}]}"#, &mut resp, &mut acc);
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
}
