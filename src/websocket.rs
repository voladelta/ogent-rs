use crate::agent::{Agent, AgentOutputSink, CompactState};
use crate::profiles;
use crate::prompts;
use crate::providers;
use crate::session;
use crate::steer::{AgentState, SteerChannel, SteerEvent};
use crate::tools;
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{WebSocketStream, accept_async};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum InboundEvent {
  Start {
    repo: String,
    temp: Option<bool>,
    profile: Option<String>,
    autocompact: Option<i32>,
  },
  Fork {
    repo: String,
    session: String,
    temp: Option<bool>,
    profile: Option<String>,
    autocompact: Option<i32>,
  },
  Resume {
    repo: String,
    session: String,
    temp: Option<bool>,
    profile: Option<String>,
    autocompact: Option<i32>,
  },
  Message {
    content: String,
  },
  Cancel,
  New,
  Compact {
    focus: Option<String>,
  },
  Profile {
    profile: String,
  },
  Exit,
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutboundEvent {
  Session {
    status: String,
    session_id: String,
    profile: String,
    mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
  },
  Status {
    state: String,
    tokens: u64,
    profile: String,
    model: String,
  },
  Message {
    source: String,
    role: String,
    content: String,
    reasoning_content: String,
    tool_calls: Vec<crate::types::ToolCall>,
    tool_call_id: String,
  },
  Error {
    code: String,
    message: String,
  },
}

#[derive(Debug)]
enum ConnCommand {
  Event(InboundEvent),
  Disconnected,
}

#[derive(Debug)]
struct SetupConfig {
  mode: String,
  session_id: String,
  profile_name: String,
  profile_model: String,
  repo: Option<String>,
  workspace: crate::workspace::Workspace,
  compact: CompactState,
  messages: Vec<crate::types::Message>,
  meta: session::SessionMeta,
}

#[derive(Clone)]
struct WsSteerState {
  profile: String,
  model: String,
  tokens: u64,
  state: AgentState,
}

pub struct WsSteerHandle {
  rx: mpsc::UnboundedReceiver<SteerEvent>,
  tx: mpsc::UnboundedSender<OutboundEvent>,
  state: Arc<Mutex<WsSteerState>>,
}

impl WsSteerHandle {
  fn new(
    rx: mpsc::UnboundedReceiver<SteerEvent>,
    tx: mpsc::UnboundedSender<OutboundEvent>,
    profile: String,
    model: String,
  ) -> Self {
    Self {
      rx,
      tx,
      state: Arc::new(Mutex::new(WsSteerState {
        profile,
        model,
        tokens: 0,
        state: AgentState::Idle,
      })),
    }
  }

  fn emit_status(&self) {
    let tx = self.tx.clone();
    let state = self.state.clone();
    tokio::spawn(async move {
      let snapshot = state.lock().await.clone();
      let _ = tx.send(OutboundEvent::Status {
        state: format!("{:?}", snapshot.state).to_lowercase(),
        tokens: snapshot.tokens,
        profile: snapshot.profile,
        model: snapshot.model,
      });
    });
  }
}

impl SteerChannel for WsSteerHandle {
  fn try_recv_event(
    &mut self,
  ) -> std::result::Result<SteerEvent, tokio::sync::mpsc::error::TryRecvError> {
    self.rx.try_recv()
  }

  fn recv_event(&mut self) -> Pin<Box<dyn Future<Output = Option<SteerEvent>> + Send + '_>> {
    Box::pin(self.rx.recv())
  }

  fn set_state(&self, state: AgentState) {
    let store = self.state.clone();
    let tx = self.tx.clone();
    tokio::spawn(async move {
      let mut s = store.lock().await;
      s.state = state;
      let _ = tx.send(OutboundEvent::Status {
        state: format!("{:?}", s.state).to_lowercase(),
        tokens: s.tokens,
        profile: s.profile.clone(),
        model: s.model.clone(),
      });
    });
  }

  fn set_tokens(&self, tokens: u64) {
    let store = self.state.clone();
    let tx = self.tx.clone();
    tokio::spawn(async move {
      let mut s = store.lock().await;
      s.tokens = tokens;
      let _ = tx.send(OutboundEvent::Status {
        state: format!("{:?}", s.state).to_lowercase(),
        tokens: s.tokens,
        profile: s.profile.clone(),
        model: s.model.clone(),
      });
    });
  }

  fn set_profile(&self, profile: String, model: String) {
    let store = self.state.clone();
    let tx = self.tx.clone();
    tokio::spawn(async move {
      let mut s = store.lock().await;
      s.profile = profile;
      s.model = model;
      let _ = tx.send(OutboundEvent::Status {
        state: format!("{:?}", s.state).to_lowercase(),
        tokens: s.tokens,
        profile: s.profile.clone(),
        model: s.model.clone(),
      });
    });
  }

  fn log_push(&self, line: String) {
    let _ = line;
  }

  fn log_push_assistant_markdown(&self, content: &str) {
    let _ = content;
  }

  fn log_clear(&self) {}

  fn log_start_stream(&self) {}

  fn log_append_stream_chunk(&self, chunk: &str) {
    let _ = chunk;
  }

  fn log_append_reasoning_chunk(&self, chunk: &str) {
    let _ = chunk;
  }

  fn log_end_stream(&self) {}
}

struct WsMessageSink {
  tx: mpsc::UnboundedSender<OutboundEvent>,
}

impl AgentOutputSink for WsMessageSink {
  fn message(&self, source: &str, message: &crate::types::Message) {
    let _ = self.tx.send(OutboundEvent::Message {
      source: source.to_string(),
      role: message.role.clone(),
      content: message.content.clone(),
      reasoning_content: message.reasoning_content.clone(),
      tool_calls: message.tool_calls.clone(),
      tool_call_id: message.tool_call_id.clone(),
    });
  }

  fn session(&self, workspace: &crate::workspace::Workspace, meta: &session::SessionMeta) {
    let _ = self.tx.send(OutboundEvent::Session {
      status: "updated".to_string(),
      session_id: meta.session_id.clone(),
      profile: meta.profile.clone(),
      mode: meta.mode.clone(),
      title: meta.title.clone(),
      repo: Some(workspace.root().to_string_lossy().to_string()),
    });
  }
}

pub async fn serve(addr: &str, profile_name: &str, autocompact: i32, temp: bool) -> Result<()> {
  profiles::get_profile(profile_name)
    .with_context(|| format!("unknown profile: {profile_name}"))?;

  let listener = TcpListener::bind(addr)
    .await
    .with_context(|| format!("failed to bind websocket server at {addr}"))?;
  let active_sessions = Arc::new(Mutex::new(HashSet::<String>::new()));
  eprintln!("[serve] websocket listening on ws://{addr}");
  loop {
    let (stream, peer) = listener.accept().await?;
    let profile_name = profile_name.to_string();
    let active_sessions = active_sessions.clone();
    tokio::spawn(async move {
      if let Err(err) =
        handle_connection(stream, &profile_name, autocompact, temp, active_sessions).await
      {
        eprintln!("[serve] connection {peer} failed: {err}");
      }
    });
  }
}

async fn handle_connection(
  stream: TcpStream,
  default_profile_name: &str,
  default_autocompact: i32,
  default_temp: bool,
  active_sessions: Arc<Mutex<HashSet<String>>>,
) -> Result<()> {
  let ws: WebSocketStream<TcpStream> = accept_async(stream).await.context("websocket handshake")?;
  let (mut ws_tx, mut ws_rx) = ws.split();
  let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<ConnCommand>();
  let (out_tx, mut out_rx) = mpsc::unbounded_channel::<OutboundEvent>();

  let writer = tokio::spawn(async move {
    while let Some(event) = out_rx.recv().await {
      let payload = serde_json::to_string(&event).unwrap_or_else(|_| {
        "{\"type\":\"error\",\"code\":\"serialization_failed\",\"message\":\"serialization failed\"}"
          .to_string()
      });
      if ws_tx.send(WsMessage::Text(payload)).await.is_err() {
        return;
      }
    }
    let _ = ws_tx.send(WsMessage::Close(None)).await;
  });

  let reader_cmd_tx = cmd_tx.clone();
  let reader_out_tx = out_tx.clone();
  let reader = tokio::spawn(async move {
    while let Some(msg) = ws_rx.next().await {
      match msg {
        Ok(WsMessage::Text(text)) => {
          let parsed: Result<InboundEvent, _> = serde_json::from_str(&text);
          match parsed {
            Ok(event) => {
              if reader_cmd_tx.send(ConnCommand::Event(event)).is_err() {
                break;
              }
            }
            Err(err) => {
              emit_error(
                &reader_out_tx,
                "invalid_event",
                format!("invalid event: {err}"),
              );
            }
          }
        }
        Ok(WsMessage::Close(_)) => break,
        Ok(_) => {}
        Err(err) => {
          emit_error(
            &reader_out_tx,
            "websocket_read_error",
            format!("websocket read error: {err}"),
          );
          break;
        }
      }
    }
    let _ = reader_cmd_tx.send(ConnCommand::Disconnected);
  });

  let mut session_key_for_cleanup: Option<String> = None;
  let mut agent_tx: Option<mpsc::UnboundedSender<SteerEvent>> = None;
  let mut agent_join: Option<tokio::task::JoinHandle<Result<()>>> = None;

  while let Some(cmd) = cmd_rx.recv().await {
    match cmd {
      ConnCommand::Disconnected => {
        if let Some(tx) = &agent_tx {
          let _ = tx.send(SteerEvent::Exit(None));
        }
        break;
      }
      ConnCommand::Event(event) => {
        if agent_tx.is_none() {
          match event {
            InboundEvent::Start {
              repo,
              temp,
              profile,
              autocompact,
            } => {
              match build_start_setup(
                repo,
                temp,
                profile,
                autocompact,
                default_profile_name,
                default_autocompact,
                default_temp,
              ) {
                Ok(cfg) => {
                  let repo = cfg.repo.clone().unwrap_or_default();
                  let sid = register_new_session(active_sessions.clone(), &repo).await;
                  let run_cfg = SetupConfig {
                    session_id: sid,
                    ..cfg
                  };
                  if let Err(err) = launch_agent(
                    &out_tx,
                    &mut agent_tx,
                    &mut agent_join,
                    run_cfg,
                    &mut session_key_for_cleanup,
                  ) {
                    if let Some(session_key) = session_key_for_cleanup.take() {
                      unregister_session(active_sessions.clone(), &session_key).await;
                    }
                    emit_error(&out_tx, "setup_failed", err.to_string());
                  }
                }
                Err(err) => emit_error(&out_tx, "setup_failed", err.to_string()),
              }
            }
            InboundEvent::Fork {
              repo,
              session,
              temp,
              profile,
              autocompact,
            } => match build_fork_or_resume_setup(
              "fork",
              repo,
              session,
              temp,
              profile,
              autocompact,
              default_profile_name,
              default_autocompact,
            ) {
              Ok(cfg) => {
                let repo = cfg.repo.clone().unwrap_or_default();
                let sid = register_new_session(active_sessions.clone(), &repo).await;
                let run_cfg = SetupConfig {
                  session_id: sid,
                  ..cfg
                };
                if let Err(err) = launch_agent(
                  &out_tx,
                  &mut agent_tx,
                  &mut agent_join,
                  run_cfg,
                  &mut session_key_for_cleanup,
                ) {
                  if let Some(session_key) = session_key_for_cleanup.take() {
                    unregister_session(active_sessions.clone(), &session_key).await;
                  }
                  emit_error(&out_tx, "setup_failed", err.to_string());
                }
              }
              Err(err) => emit_error(&out_tx, "setup_failed", err.to_string()),
            },
            InboundEvent::Resume {
              repo,
              session,
              temp,
              profile,
              autocompact,
            } => match build_fork_or_resume_setup(
              "resume",
              repo,
              session,
              temp,
              profile,
              autocompact,
              default_profile_name,
              default_autocompact,
            ) {
              Ok(cfg) => {
                let key = scoped_session_key(cfg.repo.as_deref().unwrap_or(""), &cfg.session_id);
                if !register_specific_session(active_sessions.clone(), &key).await {
                  emit_error(
                    &out_tx,
                    "session_active",
                    format!("session {} is already active", cfg.session_id),
                  );
                  continue;
                }
                if let Err(err) = launch_agent(
                  &out_tx,
                  &mut agent_tx,
                  &mut agent_join,
                  cfg,
                  &mut session_key_for_cleanup,
                ) {
                  if let Some(session_key) = session_key_for_cleanup.take() {
                    unregister_session(active_sessions.clone(), &session_key).await;
                  }
                  emit_error(&out_tx, "setup_failed", err.to_string());
                }
              }
              Err(err) => emit_error(&out_tx, "setup_failed", err.to_string()),
            },
            _ => emit_error(
              &out_tx,
              "not_initialized",
              "connection is not initialized; send start, fork, or resume first",
            ),
          }
          continue;
        }

        if let Some(tx) = &agent_tx {
          let mut exit_requested = false;
          let steer = match event {
            InboundEvent::Message { content } => Some(SteerEvent::Message(content)),
            InboundEvent::Cancel => Some(SteerEvent::Cancel),
            InboundEvent::New => Some(SteerEvent::New),
            InboundEvent::Compact { focus } => Some(SteerEvent::Compact(focus)),
            InboundEvent::Profile { profile } => Some(SteerEvent::Profile(profile)),
            InboundEvent::Exit => {
              exit_requested = true;
              Some(SteerEvent::Exit(None))
            }
            InboundEvent::Start { .. }
            | InboundEvent::Fork { .. }
            | InboundEvent::Resume { .. } => {
              emit_error(
                &out_tx,
                "already_initialized",
                "session is already initialized",
              );
              None
            }
          };
          if let Some(steer) = steer {
            let _ = tx.send(steer);
          }
          if exit_requested {
            break;
          }
        }
      }
    }
  }

  if let Some(tx) = &agent_tx {
    let _ = tx.send(SteerEvent::Exit(None));
  }
  if let Some(join) = agent_join
    && let Ok(Err(err)) = join.await
  {
    emit_error(&out_tx, "agent_error", err.to_string());
  }
  if let Some(session_key) = session_key_for_cleanup.take() {
    unregister_session(active_sessions, &session_key).await;
  }
  reader.abort();
  drop(out_tx);
  let _ = writer.await;
  Ok(())
}

fn emit_error(tx: &mpsc::UnboundedSender<OutboundEvent>, code: &str, message: impl Into<String>) {
  let _ = tx.send(OutboundEvent::Error {
    code: code.to_string(),
    message: message.into(),
  });
}

fn canonicalize_repo(repo: &str) -> Result<String> {
  let path = PathBuf::from(repo);
  if !path.exists() {
    anyhow::bail!("repo path does not exist: {repo}");
  }
  if !path.is_dir() {
    anyhow::bail!("repo path is not a directory: {repo}");
  }
  let canonical = std::fs::canonicalize(&path)?;
  Ok(canonical.to_string_lossy().to_string())
}

fn resolved_profile_name(requested: Option<String>, fallback: &str) -> String {
  requested.unwrap_or_else(|| fallback.to_string())
}

fn build_start_setup(
  repo: String,
  temp: Option<bool>,
  profile: Option<String>,
  autocompact: Option<i32>,
  default_profile_name: &str,
  default_autocompact: i32,
  default_temp: bool,
) -> Result<SetupConfig> {
  let repo = canonicalize_repo(&repo)?;
  let workspace = crate::workspace::Workspace::from_root(PathBuf::from(&repo));
  let profile_name = resolved_profile_name(profile, default_profile_name);
  let profile_cfg = profiles::get_profile(&profile_name)
    .with_context(|| format!("unknown profile: {profile_name}"))?;
  let autocompact = autocompact.unwrap_or(default_autocompact);
  let temp = temp.unwrap_or(default_temp);
  let compact = if autocompact >= 0 {
    CompactState::new(f64::from(autocompact) / 100.0, profile_cfg.context_limit)
  } else {
    CompactState::disabled()
  };
  let mut messages = prompts::build_messages("");
  prompts::enrich_initial_messages(&mut messages);
  let meta = session::SessionMeta {
    session_id: String::new(),
    parent_session: None,
    title: None,
    profile: profile_name.clone(),
    mode: "serve".to_string(),
    flags: session::SessionFlags {
      steer: true,
      worker: false,
      autocompact,
      resume: false,
      temp,
    },
    usage: session::SessionUsage { total_tokens: 0 },
    draft_input: None,
    start_ts: None,
    end_ts: None,
  };
  Ok(SetupConfig {
    mode: "start".to_string(),
    session_id: String::new(),
    profile_name,
    profile_model: profile_cfg.model.to_string(),
    repo: Some(repo),
    workspace,
    compact,
    messages,
    meta,
  })
}

#[allow(clippy::too_many_arguments)]
fn build_fork_or_resume_setup(
  mode: &str,
  repo: String,
  session_id: String,
  temp: Option<bool>,
  profile: Option<String>,
  autocompact: Option<i32>,
  default_profile_name: &str,
  default_autocompact: i32,
) -> Result<SetupConfig> {
  if temp.is_some() {
    anyhow::bail!("temp is only valid for start");
  }
  let repo = canonicalize_repo(&repo)?;
  let workspace = crate::workspace::Workspace::from_root(PathBuf::from(&repo));
  let profile_name = resolved_profile_name(profile, default_profile_name);
  let profile_cfg = profiles::get_profile(&profile_name)
    .with_context(|| format!("unknown profile: {profile_name}"))?;
  let autocompact = autocompact.unwrap_or(default_autocompact);
  let compact = if autocompact >= 0 {
    CompactState::new(f64::from(autocompact) / 100.0, profile_cfg.context_limit)
  } else {
    CompactState::disabled()
  };
  let mut messages = session::load_session_in(&workspace, &session_id)?;
  messages.retain(|m| {
    !(m.role == "user"
      && m.content.is_empty()
      && m.reasoning_content.is_empty()
      && m.tool_calls.is_empty()
      && m.tool_call_id.is_empty())
  });
  let old_meta = session::read_meta_in(&workspace, &session_id).ok();
  let mut meta = session::SessionMeta {
    session_id: session_id.clone(),
    parent_session: None,
    title: None,
    profile: profile_name.clone(),
    mode: "serve".to_string(),
    flags: session::SessionFlags {
      steer: true,
      worker: false,
      autocompact,
      resume: mode == "resume",
      temp: false,
    },
    usage: session::SessionUsage { total_tokens: 0 },
    draft_input: None,
    start_ts: None,
    end_ts: None,
  };
  if mode == "fork" {
    meta.parent_session = Some(session_id.clone());
  } else if let Some(ref old) = old_meta {
    meta.parent_session = old.parent_session.clone();
    meta.title = old.title.clone();
    meta.start_ts = old.start_ts;
    meta.end_ts = old.end_ts;
    meta.draft_input = old.draft_input.clone();
  }
  Ok(SetupConfig {
    mode: mode.to_string(),
    session_id,
    profile_name,
    profile_model: profile_cfg.model.to_string(),
    repo: Some(repo),
    workspace,
    compact,
    messages,
    meta,
  })
}

fn scoped_session_key(repo: &str, session_id: &str) -> String {
  format!("{repo}::{session_id}")
}

async fn register_new_session(active_sessions: Arc<Mutex<HashSet<String>>>, repo: &str) -> String {
  loop {
    let session_id = session::generate_session_id();
    let key = scoped_session_key(repo, &session_id);
    if register_specific_session(active_sessions.clone(), &key).await {
      return session_id;
    }
  }
}

async fn register_specific_session(
  active_sessions: Arc<Mutex<HashSet<String>>>,
  session_id: &str,
) -> bool {
  let mut guard = active_sessions.lock().await;
  if guard.contains(session_id) {
    return false;
  }
  guard.insert(session_id.to_string());
  true
}

async fn unregister_session(active_sessions: Arc<Mutex<HashSet<String>>>, session_id: &str) {
  let mut guard = active_sessions.lock().await;
  guard.remove(session_id);
}

fn launch_agent(
  out_tx: &mpsc::UnboundedSender<OutboundEvent>,
  agent_tx: &mut Option<mpsc::UnboundedSender<SteerEvent>>,
  agent_join: &mut Option<tokio::task::JoinHandle<Result<()>>>,
  mut cfg: SetupConfig,
  session_key_for_cleanup: &mut Option<String>,
) -> Result<()> {
  let session_id = cfg.session_id.clone();
  cfg.meta.session_id = session_id.clone();
  let client = providers::new_client(
    profiles::get_profile(&cfg.profile_name)
      .with_context(|| format!("unknown profile: {}", cfg.profile_name))?,
  )?;
  let (in_tx, in_rx) = mpsc::unbounded_channel::<SteerEvent>();
  let steer = WsSteerHandle::new(
    in_rx,
    out_tx.clone(),
    cfg.profile_name.clone(),
    cfg.profile_model.clone(),
  );
  steer.emit_status();
  let mut agent = Agent::new(
    cfg.workspace.clone(),
    client,
    cfg.messages,
    tools::configured_director_tools(),
    cfg.compact,
    cfg.meta,
    None,
    None,
  );
  agent.set_output_sink(Some(Arc::new(WsMessageSink { tx: out_tx.clone() })));
  agent.dirty = true;
  let _ = out_tx.send(OutboundEvent::Session {
    status: "ok".to_string(),
    session_id: session_id.clone(),
    profile: cfg.profile_name.clone(),
    mode: cfg.mode.clone(),
    title: agent.meta.title.clone(),
    repo: cfg.repo.clone(),
  });
  *agent_join = Some(tokio::spawn(async move {
    let loop_result = agent.steer_loop(steer, true).await;
    if let Err(err) = loop_result {
      return Err(err.into());
    }
    agent.persist_if_dirty()?;
    Ok(())
  }));
  *agent_tx = Some(in_tx);
  let repo = cfg.repo.clone().unwrap_or_default();
  *session_key_for_cleanup = Some(scoped_session_key(&repo, &session_id));
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn start_requires_directory_repo() {
    let err = canonicalize_repo("/definitely/not/a/repo").unwrap_err();
    assert!(err.to_string().contains("does not exist"));
  }

  #[test]
  fn fork_resume_reject_temp() {
    let err = build_fork_or_resume_setup(
      "fork",
      "/tmp".to_string(),
      "missing".to_string(),
      Some(true),
      None,
      None,
      "ds-flash",
      80,
    )
    .unwrap_err();
    assert!(err.to_string().contains("temp is only valid for start"));
  }

  #[test]
  fn session_event_serializes_title_when_present() {
    let event = OutboundEvent::Session {
      status: "updated".to_string(),
      session_id: "abc".to_string(),
      profile: "test".to_string(),
      mode: "serve".to_string(),
      title: Some("Fix websocket title".to_string()),
      repo: Some("/tmp/repo".to_string()),
    };
    let json = serde_json::to_value(event).unwrap();
    assert_eq!(json["type"], "session");
    assert_eq!(json["title"], "Fix websocket title");
  }
}
