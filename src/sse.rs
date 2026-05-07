use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::Deserialize;
use std::collections::BTreeMap;

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

pub async fn parse_sse_response(
  resp: reqwest::Response,
  cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<ChatResponse> {
  let mut result = ChatResponse::default();
  let mut acc: BTreeMap<usize, AccToolCall> = BTreeMap::new();
  let mut stream = resp.bytes_stream();
  let mut buf = String::new();

  while let Some(item) = stream.next().await {
    if cancel.map(|c| c.is_cancelled()).unwrap_or(false) {
      for (_, a) in std::mem::take(&mut acc) {
        result.tool_calls.push(ToolCall {
          id: a.id,
          kind: a.kind,
          function: FunctionCall {
            name: a.name,
            arguments: a.arguments,
          },
        });
      }
      return Err(crate::types::ChatAbortedError { resp: result }.into());
    }
    let bytes = item.context("read sse")?;
    buf.push_str(&String::from_utf8_lossy(&bytes));
    while let Some(pos) = buf.find('\n') {
      let line = buf[..pos].trim_end_matches('\r').to_string();
      buf = buf[pos + 1..].to_string();
      process_line(&line, &mut result, &mut acc)?;
    }
  }
  for (_, a) in acc {
    result.tool_calls.push(ToolCall {
      id: a.id,
      kind: a.kind,
      function: FunctionCall {
        name: a.name,
        arguments: a.arguments,
      },
    });
  }
  Ok(result)
}

fn process_line(
  line: &str,
  result: &mut ChatResponse,
  acc: &mut BTreeMap<usize, AccToolCall>,
) -> Result<()> {
  let Some(data) = line.strip_prefix("data:") else {
    return Ok(());
  };
  let data = data.trim_start();
  if data == "[DONE]" {
    return Ok(());
  }
  let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) else {
    return Ok(());
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
      let a = acc.entry(tc.index).or_default();
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
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn process_line_accumulates_tool_args() {
    let mut resp = ChatResponse::default();
    let mut acc = BTreeMap::new();
    process_line(r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","type":"function","function":{"name":"bash","arguments":"{\"command\""}}]}}]}"#, &mut resp, &mut acc).unwrap();
    process_line(r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"ls\"}"}}]}}]}"#, &mut resp, &mut acc).unwrap();
    assert_eq!(acc.get(&0).unwrap().arguments, r#"{"command":"ls"}"#);
  }

  #[test]
  fn process_line_accepts_null_content_chunks() {
    let mut resp = ChatResponse::default();
    let mut acc = BTreeMap::new();
    process_line(
      r#"data: {"choices":[{"delta":{"content":null,"reasoning_content":"thinking"}}]}"#,
      &mut resp,
      &mut acc,
    )
    .unwrap();
    process_line(
      r#"data: {"choices":[{"delta":{"content":"hello","reasoning_content":null}}]}"#,
      &mut resp,
      &mut acc,
    )
    .unwrap();
    assert_eq!(resp.reasoning_content, "thinking");
    assert_eq!(resp.content, "hello");
  }
}
