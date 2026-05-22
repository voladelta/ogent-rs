use anyhow::Result;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

use crate::client::{Client, ClientError};
use crate::session;
use crate::sse::StreamEvent;
use crate::tools::{ToolContext, execute_tool};
use crate::types::{Message, MessageOrigin, Tool, ToolCall};
use crate::workspace::Workspace;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
  #[error("client error")]
  Client(#[from] ClientError),
  #[error(transparent)]
  Other(#[from] anyhow::Error),
}

pub trait AgentOutputSink: Send + Sync {
  fn message(&self, source: &str, message: &Message);

  fn stream_event(&self, _source: &str, _event: &StreamEvent) {}

  fn tool_call(&self, _source: &str, _tool_call: &ToolCall) {}

  fn tool_result(&self, _source: &str, _tool_name: &str, _content: &str, _failed: bool) {}
}

struct CliOutputSink;

impl AgentOutputSink for CliOutputSink {
  fn message(&self, source: &str, message: &Message) {
    if source == "worker" && message.role == "assistant" && !message.content.trim().is_empty() {
      println!("{}", message.content);
    }
  }

  fn stream_event(&self, source: &str, event: &StreamEvent) {
    if source != "worker" {
      return;
    }
    match event {
      StreamEvent::Content(content) => {
        print!("{content}");
        let _ = std::io::stdout().flush();
      }
      StreamEvent::Reasoning(content) => {
        eprint!("{content}");
        let _ = std::io::stderr().flush();
      }
      StreamEvent::ToolCalling => {
        eprintln!();
      }
    }
  }

  fn tool_call(&self, source: &str, tool_call: &ToolCall) {
    if source != "worker" {
      return;
    }
    let args = truncate_for_cli(&tool_call.function.arguments, 180);
    eprintln!("[tool] {} {args}", tool_call.function.name);
  }

  fn tool_result(&self, source: &str, tool_name: &str, content: &str, failed: bool) {
    if source != "worker" {
      return;
    }
    if failed {
      eprintln!("[tool:error] {tool_name}: {}", first_line(content));
    } else {
      eprintln!("[tool:ok] {tool_name} ({} bytes)", content.len());
    }
  }
}

pub fn cli_output_sink() -> Arc<dyn AgentOutputSink> {
  static SINK: OnceLock<Arc<dyn AgentOutputSink>> = OnceLock::new();
  SINK
    .get_or_init(|| Arc::new(CliOutputSink) as Arc<dyn AgentOutputSink>)
    .clone()
}

pub struct Agent {
  pub workspace: Workspace,
  pub client: Client,
  pub messages: Vec<Message>,
  pub tools: Vec<Tool>,
  pub session_id: String,
  pub temp: bool,
  pub progress_sink: Option<Arc<Mutex<String>>>,
  pub dirty: bool,
  output_sink: Option<Arc<dyn AgentOutputSink>>,
}

impl Agent {
  pub fn new(
    workspace: Workspace,
    client: Client,
    messages: Vec<Message>,
    tools: Vec<Tool>,
    session_id: String,
    temp: bool,
  ) -> Self {
    Self {
      workspace,
      client,
      messages,
      tools,
      session_id,
      temp,
      progress_sink: None,
      dirty: false,
      output_sink: None,
    }
  }

  pub fn set_output_sink(&mut self, sink: Option<Arc<dyn AgentOutputSink>>) {
    self.output_sink = sink;
  }

  fn emit_message(&self, message: &Message) {
    if let Some(sink) = &self.output_sink {
      sink.message(self.source_label(), message);
    }
  }

  fn emit_tool_call(&self, tool_call: &ToolCall) {
    if let Some(sink) = &self.output_sink {
      sink.tool_call(self.source_label(), tool_call);
    }
  }

  fn emit_tool_result(&self, tool_name: &str, content: &str, failed: bool) {
    if let Some(sink) = &self.output_sink {
      sink.tool_result(self.source_label(), tool_name, content, failed);
    }
  }

  fn stream_events_to_sink(
    &self,
  ) -> Option<(
    tokio::sync::mpsc::Sender<StreamEvent>,
    tokio::task::JoinHandle<()>,
  )> {
    let sink = self.output_sink.clone()?;
    let source = self.source_label().to_string();
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let handle = tokio::spawn(async move {
      while let Some(event) = rx.recv().await {
        sink.stream_event(&source, &event);
      }
    });
    Some((tx, handle))
  }

  fn source_label(&self) -> &str {
    "worker"
  }

  pub async fn run_loop(&mut self) -> Result<(), AgentError> {
    loop {
      let stream = self.stream_events_to_sink();
      let streaming_to_sink = stream.is_some();
      let (stream_tx, stream_handle) = match stream {
        Some((tx, handle)) => (Some(tx), Some(handle)),
        None => (None, None),
      };
      let resp = self
        .client
        .chat(&self.messages, &self.tools, stream_tx)
        .await?;
      if let Some(handle) = stream_handle {
        let _ = handle.await;
      }

      let assistant = Message {
        role: "assistant".to_string(),
        content: resp.content,
        origin: MessageOrigin::Model,
        reasoning_content: resp.reasoning_content,
        tool_calls: resp.tool_calls,
        tool_call_id: String::new(),
      };
      self.messages.push(assistant.clone());
      if streaming_to_sink && !assistant.content.is_empty() {
        if !assistant.content.ends_with('\n') {
          println!();
        }
      } else {
        self.emit_message(&assistant);
      }
      self.dirty = true;

      if assistant.tool_calls.is_empty() {
        break;
      }

      for tool_call in assistant.tool_calls {
        self.emit_tool_call(&tool_call);
        let workspace = self.workspace.clone();
        let result = execute_tool(
          ToolContext {
            agent: Some(self),
            workspace,
          },
          &tool_call.function.name,
          &tool_call.function.arguments,
        )
        .await;
        let failed = result.is_err();
        let content = tool_result_content(&tool_call.function.name, result);
        self.emit_tool_result(&tool_call.function.name, &content, failed);
        let tool_message = Message {
          role: "tool".to_string(),
          content,
          origin: MessageOrigin::Tool,
          reasoning_content: String::new(),
          tool_calls: Vec::new(),
          tool_call_id: tool_call.id,
        };
        self.messages.push(tool_message.clone());
        self.emit_message(&tool_message);
      }
      self.dirty = true;
    }

    Ok(())
  }

  pub fn persist_if_dirty(&self) -> Result<()> {
    if !self.dirty || self.temp {
      return Ok(());
    }
    session::persist_session_in(&self.workspace, &self.messages, &self.session_id)
  }
}

fn tool_result_content(tool_name: &str, result: Result<String>) -> String {
  match result {
    Ok(content) => content,
    Err(err) => format!(
      "tool `{tool_name}` failed:\n{err}\n\nUse this tool error as evidence, then adjust the next tool call or report the failure."
    ),
  }
}

fn truncate_for_cli(s: &str, limit: usize) -> String {
  let compact = s.split_whitespace().collect::<Vec<_>>().join(" ");
  if compact.len() <= limit {
    return compact;
  }
  let end = compact.floor_char_boundary(limit);
  format!("{}...", &compact[..end])
}

fn first_line(s: &str) -> &str {
  s.lines().next().unwrap_or("")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn tool_result_content_reports_errors_to_model() {
    let content = tool_result_content("bash", Err(anyhow::anyhow!("exit err: exit status: 127")));

    assert!(content.contains("tool `bash` failed"));
    assert!(content.contains("exit status: 127"));
    assert!(content.contains("adjust the next tool call"));
  }

  #[test]
  fn truncate_for_cli_keeps_short_text() {
    assert_eq!(truncate_for_cli("hello   world", 20), "hello world");
  }
}
