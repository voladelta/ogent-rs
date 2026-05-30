use anyhow::Result;
use std::io::Write;
use std::sync::{Arc, OnceLock};

use crate::client::{Client, ClientError};
use crate::session;
use crate::sse::StreamEvent;
use crate::tools::ToolContext;
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
  fn message(&self, actor_id: &str, message: &Message);

  fn stream_event(&self, _actor_id: &str, _verbose: bool, _event: &StreamEvent) {}

  fn tool_call(&self, _actor_id: &str, _verbose: bool, _tool_call: &ToolCall) {}

  fn tool_result(
    &self,
    _actor_id: &str,
    _verbose: bool,
    _tool_name: &str,
    _content: &str,
    _failed: bool,
  ) {
  }

  fn task_update(&self, _actor_id: &str, _status: &str, _summary: &str) {}
}

fn print_actor_text(actor_id: &str, text: &str) {
  static STATE: OnceLock<std::sync::Mutex<(String, bool)>> = OnceLock::new();
  let lock = STATE.get_or_init(|| std::sync::Mutex::new((String::new(), true)));
  let mut guard = lock.lock().unwrap();
  let (last_actor, at_line_start) = &mut *guard;

  if !*at_line_start && last_actor != actor_id {
    println!();
    *at_line_start = true;
  }
  *last_actor = actor_id.to_string();

  for (i, part) in text.split('\n').enumerate() {
    if i > 0 {
      println!();
      *at_line_start = true;
    }
    if part.is_empty() {
      continue;
    }
    if *at_line_start {
      print!("[{actor_id}] ");
      *at_line_start = false;
    }
    print!("{part}");
  }
  let _ = std::io::stdout().flush();
}

struct CliOutputSink;

impl AgentOutputSink for CliOutputSink {
  fn message(&self, actor_id: &str, message: &Message) {
    if message.role == Role::Assistant && !message.content.trim().is_empty() {
      print_actor_text(actor_id, &format!("{}\n", message.content));
    }
  }

  fn stream_event(&self, actor_id: &str, verbose: bool, event: &StreamEvent) {
    match event {
      StreamEvent::Content(content) => {
        print_actor_text(actor_id, content);
      }
      StreamEvent::Reasoning(content) => {
        if verbose {
          let tagged_actor = format!("{actor_id}][thinking");
          print_actor_text(&tagged_actor, content);
        }
      }
      StreamEvent::ToolCalling => {}
    }
  }

  fn tool_call(&self, actor_id: &str, verbose: bool, tool_call: &ToolCall) {
    let name = &tool_call.function.name;
    if (name == "exec" || name == "eval")
      && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&tool_call.function.arguments)
    {
      let reason = parsed.get("reason").and_then(|r| r.as_str()).unwrap_or("");
      let code = parsed.get("code").and_then(|c| c.as_str()).unwrap_or("");
      if !reason.is_empty() {
        print_actor_text(actor_id, &format!("[{name}] {reason}\n"));
      } else {
        print_actor_text(actor_id, &format!("[{name}]\n"));
      }
      if verbose && !code.is_empty() {
        print_actor_text(actor_id, &format!("-- Code:\n{code}\n"));
      }
      return;
    }
    let args = truncate_for_cli(&tool_call.function.arguments, 180);
    if verbose {
      print_actor_text(actor_id, &format!("[tool] {name} {args}\n"));
    }
  }

  fn tool_result(
    &self,
    actor_id: &str,
    verbose: bool,
    tool_name: &str,
    content: &str,
    failed: bool,
  ) {
    if tool_name == "exec" || tool_name == "eval" {
      if failed {
        print_actor_text(
          actor_id,
          &format!("--- Tool Execution Failed ---\n{}\n", content),
        );
      } else if verbose {
        let lines: Vec<&str> = content.lines().collect();
        let display = lines.iter().take(5).copied().collect::<Vec<_>>().join("\n");
        print_actor_text(
          actor_id,
          &format!("--- Tool Execution Result ---\n{}\n", display),
        );
        if lines.len() > 5 {
          print_actor_text(actor_id, "... (truncated)\n");
        }
      }
      return;
    }
    if failed {
      print_actor_text(
        actor_id,
        &format!("[tool:error] {tool_name}: {}\n", first_line(content)),
      );
    } else if verbose {
      print_actor_text(
        actor_id,
        &format!("[tool:ok] {tool_name} ({} bytes)\n", content.len()),
      );
    }
  }

  fn task_update(&self, actor_id: &str, status: &str, summary: &str) {
    print_actor_text(actor_id, &format!("task_update({status}): {summary}\n"));
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
  pub lua_session: Arc<parking_lot::Mutex<Option<mlua::Lua>>>,
  pub actor_id: String,
  pub verbose: bool,
  /// Subagent nesting depth: 0 for the root agent, incremented by each `agent{}` spawn.
  pub agent_depth: u32,
}

impl Agent {
  #[allow(clippy::too_many_arguments)]
  pub fn new(
    workspace: Workspace,
    client: Client,
    messages: Vec<Message>,
    tools: Vec<Tool>,
    session_id: String,
    skill_store: Arc<crate::skills::SkillStore>,
    actor_id: String,
    verbose: bool,
    agent_depth: u32,
  ) -> Self {
    Self {
      workspace,
      client,
      messages,
      tools,
      session_id,
      output_sink: None,
      skill_store,
      lua_session: Arc::new(parking_lot::Mutex::new(None)),
      actor_id,
      verbose,
      agent_depth,
    }
  }

  pub fn set_output_sink(&mut self, sink: Option<Arc<dyn AgentOutputSink>>) {
    self.output_sink = sink;
  }

  fn emit_message(&self, message: &Message) {
    if let Some(sink) = &self.output_sink {
      sink.message(&self.actor_id, message);
    }
  }

  fn emit_tool_call(&self, tool_call: &ToolCall) {
    if let Some(sink) = &self.output_sink {
      sink.tool_call(&self.actor_id, self.verbose, tool_call);
    }
  }

  fn emit_tool_result(&self, tool_name: &str, content: &str, failed: bool) {
    if let Some(sink) = &self.output_sink {
      sink.tool_result(&self.actor_id, self.verbose, tool_name, content, failed);
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
    let actor_id = self.actor_id.clone();
    let verbose = self.verbose;
    let handle = tokio::spawn(async move {
      while let Some(event) = rx.recv().await {
        sink.stream_event(&actor_id, verbose, &event);
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
        .await;
      // Always drain the streaming task before propagating errors: dropping
      // stream_tx (inside chat) closes the channel, so the task exits promptly.
      if let Some(handle) = stream_handle {
        handle
          .await
          .expect("streaming output task should not panic");
      }
      let resp = resp?;

      let assistant = Message::assistant(resp);
      self.messages.push(assistant.clone());
      if streaming_to_sink && !assistant.content.is_empty() {
        if !assistant.content.ends_with('\n')
          && let Some(sink) = &self.output_sink
        {
          sink.stream_event(
            &self.actor_id,
            self.verbose,
            &StreamEvent::Content("\n".to_string()),
          );
        }
      } else {
        self.emit_message(&assistant);
      }

      if assistant.tool_calls.is_empty() {
        break;
      }

      for tool_call in assistant.tool_calls {
        self.emit_tool_call(&tool_call);
        let ctx = ToolContext {
          workspace: self.workspace.clone(),
          skill_store: self.skill_store.clone(),
          lua_session: self.lua_session.clone(),
          client: self.client.clone(),
          output_sink: self.output_sink.clone(),
          verbose: self.verbose,
          actor_id: self.actor_id.clone(),
          agent_depth: self.agent_depth,
        };
        let lua_code: String =
          serde_json::from_str::<serde_json::Value>(&tool_call.function.arguments)
            .ok()
            .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(String::from))
            .unwrap_or_else(|| tool_call.function.arguments.clone());
        let result = match tool_call.function.name.as_str() {
          "exec" => crate::tools::exec(ctx, &lua_code).await,
          "eval" => crate::tools::eval(ctx, &lua_code).await,
          other => Err(anyhow::anyhow!("unknown tool: {other}")),
        };
        let failed = result.is_err();
        let content = match result {
          Ok(content) => content,
          Err(err) => format!(
            "tool `{}` failed:\n{err}\n\nUse this tool error as evidence, then adjust the next tool call or report the failure.",
            tool_call.function.name
          ),
        };
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
    let skill_store = std::sync::Arc::new(crate::skills::SkillStore::new(workspace.root()));
    let agent = Agent::new(
      workspace.clone(),
      client,
      vec![Message::user("hello", MessageOrigin::Human)],
      Vec::new(),
      session_id.to_string(),
      skill_store,
      "director".to_string(),
      false,
      0,
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
    let skill_store = std::sync::Arc::new(crate::skills::SkillStore::new(workspace.root()));
    let mut agent = Agent::new(
      workspace.clone(),
      client,
      vec![Message::user("hello", MessageOrigin::Human)],
      Vec::new(),
      session_id.to_string(),
      skill_store,
      "director".to_string(),
      false,
      0,
    );

    struct MockSink {
      events: Arc<Mutex<Vec<StreamEvent>>>,
    }
    impl AgentOutputSink for MockSink {
      fn message(&self, _actor_id: &str, _message: &Message) {}
      fn stream_event(&self, _actor_id: &str, _verbose: bool, event: &StreamEvent) {
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
