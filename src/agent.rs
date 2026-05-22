use anyhow::Result;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

use crate::client::{Client, ClientError};
use crate::session;
use crate::sse::StreamEvent;
use crate::tools::{ToolContext, execute_tool};
use crate::types::{Message, MessageOrigin, Tool};
use crate::workspace::Workspace;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
  #[error("client error")]
  Client(#[from] ClientError),
  #[error(transparent)]
  Other(#[from] anyhow::Error),
}

#[derive(Debug, Clone, Copy)]
pub struct CompactState {
  enabled: bool,
  #[allow(dead_code)]
  threshold: f64,
  #[allow(dead_code)]
  context_limit: usize,
}

impl CompactState {
  pub fn new(threshold: f64, context_limit: usize) -> Self {
    Self {
      enabled: true,
      threshold,
      context_limit,
    }
  }

  pub fn disabled() -> Self {
    Self {
      enabled: false,
      threshold: 1.0,
      context_limit: 0,
    }
  }
}

pub trait AgentOutputSink: Send + Sync {
  fn message(&self, source: &str, message: &Message);

  fn stream_event(&self, _source: &str, _event: &StreamEvent) {}
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
  pub compact: CompactState,
  pub meta: session::SessionMeta,
  pub worker_parent_session_id: Option<String>,
  pub worker_id: Option<String>,
  pub progress_sink: Option<Arc<Mutex<String>>>,
  pub dirty: bool,
  output_sink: Option<Arc<dyn AgentOutputSink>>,
}

impl Agent {
  #[allow(clippy::too_many_arguments)]
  pub fn new(
    workspace: Workspace,
    client: Client,
    messages: Vec<Message>,
    tools: Vec<Tool>,
    compact: CompactState,
    meta: session::SessionMeta,
    worker_parent_session_id: Option<String>,
    worker_id: Option<String>,
  ) -> Self {
    Self {
      workspace,
      client,
      messages,
      tools,
      compact,
      meta,
      worker_parent_session_id,
      worker_id,
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
    self.worker_id.as_deref().unwrap_or("worker")
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
        .chat(&self.messages, &self.tools, None, stream_tx)
        .await?;
      if let Some(handle) = stream_handle {
        let _ = handle.await;
      }

      self.meta.usage.total_tokens = self
        .meta
        .usage
        .total_tokens
        .saturating_add(resp.usage.total_tokens.max(0) as u64);

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
        let content = tool_result_content(&tool_call.function.name, result);
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

    let _ = self.compact.enabled;
    Ok(())
  }

  pub fn persist_if_dirty(&self) -> Result<()> {
    if !self.dirty || self.meta.flags.temp {
      return Ok(());
    }
    session::write_meta_in(&self.workspace, &self.meta)?;
    if let (Some(parent), Some(worker_id)) = (&self.worker_parent_session_id, &self.worker_id) {
      session::persist_worker_session_in(&self.workspace, &self.messages, parent, worker_id)
    } else {
      session::persist_session_in(&self.workspace, &self.messages, &self.meta.session_id)
    }
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
}
