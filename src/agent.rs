use anyhow::Result;
use std::io::Write;
use std::sync::{Arc, OnceLock};

use crate::client::{Client, ClientError};
use crate::session;
use crate::sse::StreamEvent;
use crate::tools::{ToolContext, execute_tool};
use crate::types::{Message, Role, Tool, ToolCall};
use crate::workspace::Workspace;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
  #[error("client error")]
  Client(#[from] ClientError),
  #[error(transparent)]
  Other(#[from] anyhow::Error),
}

pub trait AgentOutputSink: Send + Sync {
  fn message(&self, message: &Message);

  fn stream_event(&self, _event: &StreamEvent) {}

  fn tool_call(&self, _tool_call: &ToolCall) {}

  fn tool_result(&self, _tool_name: &str, _content: &str, _failed: bool) {}
}

struct CliOutputSink;

impl AgentOutputSink for CliOutputSink {
  fn message(&self, message: &Message) {
    if message.role == Role::Assistant && !message.content.trim().is_empty() {
      println!("{}", message.content);
    }
  }

  fn stream_event(&self, event: &StreamEvent) {
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

  fn tool_call(&self, tool_call: &ToolCall) {
    let args = truncate_for_cli(&tool_call.function.arguments, 180);
    eprintln!("[tool] {} {args}", tool_call.function.name);
  }

  fn tool_result(&self, tool_name: &str, content: &str, failed: bool) {
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
  output_sink: Option<Arc<dyn AgentOutputSink>>,
  pub skill_store: Arc<crate::skills::SkillStore>,
}

impl Agent {
  pub fn new(
    workspace: Workspace,
    client: Client,
    messages: Vec<Message>,
    tools: Vec<Tool>,
    session_id: String,
    skill_store: Arc<crate::skills::SkillStore>,
  ) -> Self {
    Self {
      workspace,
      client,
      messages,
      tools,
      session_id,
      output_sink: None,
      skill_store,
    }
  }

  pub fn set_output_sink(&mut self, sink: Option<Arc<dyn AgentOutputSink>>) {
    self.output_sink = sink;
  }

  fn emit_message(&self, message: &Message) {
    if let Some(sink) = &self.output_sink {
      sink.message(message);
    }
  }

  fn emit_tool_call(&self, tool_call: &ToolCall) {
    if let Some(sink) = &self.output_sink {
      sink.tool_call(tool_call);
    }
  }

  fn emit_tool_result(&self, tool_name: &str, content: &str, failed: bool) {
    if let Some(sink) = &self.output_sink {
      sink.tool_result(tool_name, content, failed);
    }
  }

  fn stream_events_to_sink(
    &self,
  ) -> Option<(
    tokio::sync::mpsc::Sender<StreamEvent>,
    tokio::task::JoinHandle<()>,
  )> {
    let sink = self.output_sink.clone()?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let handle = tokio::spawn(async move {
      while let Some(event) = rx.recv().await {
        sink.stream_event(&event);
      }
    });
    Some((tx, handle))
  }

  pub async fn run_loop(&mut self) -> Result<(), AgentError> {
    loop {
      let (stream_tx, stream_handle) = match self.stream_events_to_sink() {
        Some((tx, handle)) => (Some(tx), Some(handle)),
        None => (None, None),
      };
      let streaming_to_sink = stream_handle.is_some();
      let resp = self
        .client
        .chat(&self.messages, &self.tools, stream_tx)
        .await?;
      if let Some(handle) = stream_handle {
        let _ = handle.await;
      }

      let assistant = Message::assistant(resp);
      self.messages.push(assistant.clone());
      if streaming_to_sink && !assistant.content.is_empty() {
        if !assistant.content.ends_with('\n')
          && let Some(sink) = &self.output_sink
        {
          sink.stream_event(&StreamEvent::Content("\n".to_string()));
        }
      } else {
        self.emit_message(&assistant);
      }

      if assistant.tool_calls.is_empty() {
        break;
      }

      for tool_call in assistant.tool_calls {
        self.emit_tool_call(&tool_call);
        let workspace = self.workspace.clone();
        let skill_store = self.skill_store.clone();
        let result = execute_tool(
          ToolContext {
            workspace,
            skill_store,
          },
          &tool_call.function.name,
          &tool_call.function.arguments,
        )
        .await;
        let failed = result.is_err();
        let content = tool_result_content(&tool_call.function.name, result);
        self.emit_tool_result(&tool_call.function.name, &content, failed);
        let tool_message = Message::tool_result(tool_call.id, content);
        self.messages.push(tool_message.clone());
        self.emit_message(&tool_message);
      }
    }

    Ok(())
  }

  pub fn persist(&self) -> Result<()> {
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
  use crate::client::Client;
  use crate::types::MessageOrigin;

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

  #[test]
  fn persist_writes_messages_jsonl() {
    let root = std::env::temp_dir().join(format!(
      "ogent-agent-persist-test-{}",
      crate::session::timestamp_ms()
    ));
    let workspace = Workspace::from_root(root.clone());
    let session_id = "persist-test";
    let client = Client::new(
      "http://localhost",
      "dummy".into(),
      |_, _| Ok(serde_json::Value::Null),
      30,
    )
    .unwrap();
    let skill_store =
      std::sync::Arc::new(crate::skills::SkillStore::new(workspace.root(), Vec::new()));
    let agent = Agent::new(
      workspace.clone(),
      client,
      vec![Message::user("hello", MessageOrigin::Human)],
      Vec::new(),
      session_id.to_string(),
      skill_store,
    );

    agent.persist().unwrap();

    let path = crate::session::session_file_in(&workspace, session_id);
    assert!(path.exists());
    let data = std::fs::read_to_string(&path).unwrap();
    assert!(data.contains("\"content\":\"hello\""));
    let _ = std::fs::remove_dir_all(root);
  }

  #[tokio::test]
  async fn test_run_loop_streaming_newline_redirect() {
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    tokio::spawn(async move {
      let (mut socket, _) = listener.accept().await.unwrap();
      let mut buf = [0u8; 1024];
      let _ = socket.read(&mut buf).await;
      let response = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n\
                      data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n\
                      data: [DONE]\n\n";
      let _ = socket.write_all(response.as_bytes()).await;
    });

    let root = std::env::temp_dir().join(format!(
      "ogent-agent-newline-test-{}",
      crate::session::timestamp_ms()
    ));
    let workspace = Workspace::from_root(root.clone());
    let session_id = "newline-test";
    let client = Client::new(&url, "dummy".into(), |_, _| Ok(serde_json::Value::Null), 30).unwrap();
    let skill_store =
      std::sync::Arc::new(crate::skills::SkillStore::new(workspace.root(), Vec::new()));
    let mut agent = Agent::new(
      workspace.clone(),
      client,
      vec![Message::user("hello", MessageOrigin::Human)],
      Vec::new(),
      session_id.to_string(),
      skill_store,
    );

    struct MockSink {
      events: Arc<Mutex<Vec<StreamEvent>>>,
    }
    impl AgentOutputSink for MockSink {
      fn message(&self, _message: &Message) {}
      fn stream_event(&self, event: &StreamEvent) {
        self.events.lock().unwrap().push(event.clone());
      }
    }

    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::new(MockSink {
      events: events.clone(),
    });
    agent.set_output_sink(Some(sink));

    let run_res = agent.run_loop().await;
    assert!(run_res.is_ok());

    let recorded = events.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    match &recorded[0] {
      StreamEvent::Content(c) => assert_eq!(c, "hello"),
      _ => panic!("Expected first event to be Content"),
    }
    match &recorded[1] {
      StreamEvent::Content(c) => assert_eq!(c, "\n"),
      _ => panic!("Expected second event to be Content"),
    }

    let _ = std::fs::remove_dir_all(root);
  }
}
