use crate::client::{Client, ClientError};
use crate::session;
use crate::sse::{SseError, StreamEvent};
use crate::steer::{AgentState, SteerChannel, SteerEvent};
use crate::tools::{ToolContext, execute_tool, is_read_only_tool};
use crate::types::{ChatResponse, Message, MessageOrigin, Tool, ToolCall};
use crate::workers::WorkerManager;
use crate::workspace::Workspace;
use std::io::{self, Write};

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
  #[error("client error")]
  Client(#[from] ClientError),
  #[error(transparent)]
  Other(#[from] anyhow::Error),
}

enum SteerAction {
  Continue,
  Exit,
  Restart,
}

#[derive(Default)]
pub(crate) enum CompactPending {
  #[default]
  None,
  NoFocus,
  WithFocus(String),
}

enum SteerState {
  Idle {
    wait_for_input: bool,
  },
  StartTurn,
  InFlight {
    chat: tokio::task::JoinHandle<std::result::Result<ChatResponse, ClientError>>,
    cancel: tokio_util::sync::CancellationToken,
    cancelled: bool,
    steer_msg: Option<String>,
    stream_rx: tokio::sync::mpsc::Receiver<StreamEvent>,
    tool_calling: bool,
  },
  ProcessResult(ChatResponse),
  FinishTurn {
    has_more: bool,
  },
  Exit(Vec<Message>),
}

struct SteerCtx;

impl SteerState {
  async fn step(
    self,
    agent: &mut Agent,
    steer: &mut impl SteerChannel,
    _ctx: &mut SteerCtx,
  ) -> Result<Self, AgentError> {
    match self {
      Self::Exit(msgs) => Ok(Self::Exit(msgs)),

      Self::Idle { wait_for_input } => {
        steer.set_state(AgentState::Idle);
        let wait_baseline_len = agent.messages.len();
        let mut wait = wait_for_input;

        while let Ok(event) = steer.try_recv_event() {
          if let Some(next) = Self::process_idle_event(agent, steer, _ctx, event)? {
            return Ok(next);
          }
          if agent.messages.len() > wait_baseline_len
            && matches!(agent.messages.last().map(|m| m.role.as_str()), Some("user"))
          {
            wait = false;
          }
        }

        if wait {
          loop {
            let Some(event) = steer.recv_event().await else {
              continue;
            };
            if let Some(next) = Self::process_idle_event(agent, steer, _ctx, event)? {
              return Ok(next);
            }
            if agent.messages.len() > wait_baseline_len
              && matches!(agent.messages.last().map(|m| m.role.as_str()), Some("user"))
            {
              break;
            }
          }
        }

        Ok(Self::StartTurn)
      }

      Self::StartTurn => {
        steer.set_tokens(agent.total_tokens);

        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel.clone();
        let client = agent.client.clone();
        let messages = agent.messages.clone();
        let tools = agent.tools.clone();
        let (stream_tx, stream_rx) = tokio::sync::mpsc::channel::<StreamEvent>(1);
        let chat = tokio::spawn(async move {
          client
            .chat(&messages, &tools, Some(&cancel_clone), Some(stream_tx))
            .await
        });

        steer.log_start_stream();

        Ok(Self::InFlight {
          chat,
          cancel,
          cancelled: false,
          steer_msg: None,
          stream_rx,
          tool_calling: false,
        })
      }

      Self::InFlight {
        mut chat,
        cancel,
        mut cancelled,
        mut steer_msg,
        mut stream_rx,
        mut tool_calling,
      } => {
        let chat_result = 'select: loop {
          tokio::select! {
            r = &mut chat => break 'select r,
            Some(ev) = stream_rx.recv() => {
              match ev {
                StreamEvent::Content(chunk) => {
                  steer.log_append_stream_chunk(&chunk);
                  if !tool_calling {
                    steer.set_state(AgentState::Replying);
                  }
                }
                StreamEvent::Reasoning(chunk) => {
                  steer.log_append_reasoning_chunk(&chunk);
                  if !tool_calling {
                    steer.set_state(AgentState::Reasoning);
                  }
                }
                StreamEvent::ToolCalling => {
                  tool_calling = true;
                  steer.set_state(AgentState::Working);
                }
              }
            }
            maybe_event = steer.recv_event(), if !cancelled && steer_msg.is_none() => {
              let Some(event) = maybe_event else { continue; };
              match event {
                SteerEvent::Cancel => {
                  cancel.cancel();
                  cancelled = true;
                }
                SteerEvent::Message(content) => {
                  cancel.cancel();
                  steer_msg = Some(content);
                }
                SteerEvent::New => {
                  cancel.cancel();
                  agent.apply_steer_event(SteerEvent::New, steer)?;
                  chat.abort();
                  return Ok(Self::Idle { wait_for_input: true });
                }
                SteerEvent::Exit(exit_msg) => {
                  agent.apply_steer_event(SteerEvent::Exit(exit_msg), steer)?;
                  cancel.cancel();
                  chat.abort();
                  return Ok(Self::Exit(agent.messages.clone()));
                }
                other => {
                  match agent.apply_steer_event(other, steer)? {
                    SteerAction::Exit => {
                      cancel.cancel();
                      chat.abort();
                      return Ok(Self::Exit(agent.messages.clone()));
                    }
                    SteerAction::Restart => {
                      cancel.cancel();
                      chat.abort();
                      return Ok(Self::Idle { wait_for_input: true });
                    }
                    SteerAction::Continue => {}
                  }
                }
              }
            }
          }
        };

        while let Ok(ev) = stream_rx.try_recv() {
          match ev {
            StreamEvent::Content(chunk) => steer.log_append_stream_chunk(&chunk),
            StreamEvent::Reasoning(chunk) => steer.log_append_reasoning_chunk(&chunk),
            StreamEvent::ToolCalling => {}
          }
        }

        let resp = match chat_result {
          Ok(Ok(resp)) => resp,
          Ok(Err(ClientError::Aborted { resp }))
          | Ok(Err(ClientError::Sse(SseError::Aborted { resp }))) => {
            if !resp.content.is_empty()
              || !resp.reasoning_content.is_empty()
              || !resp.tool_calls.is_empty()
            {
              agent.total_tokens = resp.usage.total_tokens as u64;
              agent.push_msg(assistant_msg_full(
                resp.content.clone(),
                resp.reasoning_content.clone(),
                resp.tool_calls.clone(),
              ));
            }
            if cancelled {
              return Ok(Self::Idle {
                wait_for_input: true,
              });
            }
            if let Some(msg) = steer_msg {
              agent.push_msg(human_user_msg(msg.clone()));
              steer.log_push(format!("[user] {}", truncate(&msg, 200)));
              return Ok(Self::StartTurn);
            }
            return Ok(Self::Exit(agent.messages.clone()));
          }
          Ok(Err(e)) => return Err(AgentError::Client(e)),
          Err(join_err) => return Err(AgentError::Other(join_err.into())),
        };

        Ok(Self::ProcessResult(resp))
      }

      Self::ProcessResult(resp) => {
        let has_more = agent
          .handle_turn_response_with_log(resp, Some(steer), true)
          .await?;
        steer.log_end_stream();
        steer.set_state(AgentState::Idle);
        steer.set_tokens(agent.total_tokens);
        Ok(Self::FinishTurn { has_more })
      }

      Self::FinishTurn { mut has_more } => {
        agent.finish_turn(&mut has_more).await?;

        let pending = std::mem::take(&mut agent.pending_compact);
        if !matches!(pending, CompactPending::None) {
          let task_prompt = match pending {
            CompactPending::NoFocus => None,
            CompactPending::WithFocus(s) => Some(s),
            CompactPending::None => unreachable!(),
          };
          let handoff = agent
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .map(|m| m.content.clone())
            .unwrap_or_default();

          if handoff.is_empty() {
            steer.log_push("[compact] model returned empty response, not compacting".to_string());
          } else {
            let parent_id = agent.meta.session_id.clone();

            if !agent.meta.flags.temp {
              agent.meta.usage.total_tokens = agent.total_tokens;
              session::write_meta_in(&agent.workspace, &agent.meta)?;
              session::persist_session_in(
                &agent.workspace,
                &agent.messages,
                &agent.meta.session_id,
              )?;
            }

            let mut new_messages = crate::prompts::build_messages("");
            let mut content = format!(
              "[handoff from session {}]\n\n{}\n\n\
               If the summary is unclear or you need specific details, \
               read the full transcript:\n\
               .ogent/sessions/{}/messages.jsonl",
              parent_id, handoff, parent_id,
            );
            if let Some(ref tp) = task_prompt {
              content.push_str(&format!("\n\nFocus task: {}", tp));
            }
            new_messages.push(Message {
              role: "user".into(),
              content,
              origin: MessageOrigin::Internal,
              ..Default::default()
            });
            crate::prompts::enrich_initial_messages(&mut new_messages);

            agent.meta.session_id = session::generate_session_id();
            agent.meta.parent_session = Some(parent_id.clone());
            agent.meta.start_ts = Some(session::timestamp_ms());
            agent.meta.end_ts = None;
            agent.meta.usage = session::SessionUsage { total_tokens: 0 };
            agent.messages = new_messages;
            agent.total_tokens = 0;
            agent.dirty = true;

            agent.compact.compacting = false;
            agent.compact.urgency = 0;

            if !agent.meta.flags.temp {
              session::write_meta_in(&agent.workspace, &agent.meta)?;
              session::persist_session_in(
                &agent.workspace,
                &agent.messages,
                &agent.meta.session_id,
              )?;
            }

            steer.log_clear();
            steer.log_push(format!(
              "[compact] {} → {} (parent preserved)",
              parent_id, agent.meta.session_id
            ));
            steer.set_tokens(0);
          }

          return Ok(Self::Idle {
            wait_for_input: true,
          });
        }

        if !has_more {
          return Ok(Self::Idle {
            wait_for_input: true,
          });
        }

        Ok(Self::StartTurn)
      }
    }
  }

  fn process_idle_event(
    agent: &mut Agent,
    steer: &mut impl SteerChannel,
    _ctx: &mut SteerCtx,
    event: SteerEvent,
  ) -> Result<Option<Self>, AgentError> {
    match agent.apply_steer_event(event, steer)? {
      SteerAction::Exit => Ok(Some(Self::Exit(agent.messages.clone()))),
      SteerAction::Restart => Ok(Some(Self::Idle {
        wait_for_input: true,
      })),
      SteerAction::Continue => Ok(None),
    }
  }
}

#[derive(Debug, Clone)]
pub struct CompactState {
  pub threshold: f64,
  pub context_limit: usize,
  pub compacting: bool,
  pub urgency: usize,
}

impl CompactState {
  pub fn disabled() -> Self {
    Self {
      threshold: -1.0,
      context_limit: 0,
      compacting: false,
      urgency: 0,
    }
  }

  pub fn new(threshold: f64, context_limit: usize) -> Self {
    Self {
      threshold,
      context_limit,
      compacting: false,
      urgency: 0,
    }
  }
}

pub struct Agent {
  pub workspace: Workspace,
  pub client: Client,
  pub messages: Vec<Message>,
  pub tools: Vec<Tool>,
  pub worker_manager: WorkerManager,
  pub total_tokens: u64,
  pub compact: CompactState,
  pub meta: session::SessionMeta,
  pub worker_parent_session_id: Option<String>,
  pub worker_id: Option<String>,
  pub dirty: bool,

  pub pending_compact: CompactPending,
}

pub struct ToolResult {
  name: String,
  args: String,
  output: String,
  success: bool,
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
      workspace: workspace.clone(),
      client,
      messages,
      tools,
      worker_manager: WorkerManager::new(Some(&meta.session_id), workspace),
      total_tokens: 0,
      compact,
      meta,
      worker_parent_session_id,
      worker_id,
      dirty: false,

      pending_compact: CompactPending::None,
    }
  }

  fn push_msg(&mut self, msg: Message) {
    self.dirty = true;
    self.messages.push(msg);
  }

  pub fn persist_if_dirty(&mut self) -> anyhow::Result<()> {
    if self.dirty && !self.meta.flags.temp {
      if let (Some(parent_session_id), Some(worker_id)) = (
        self.worker_parent_session_id.as_deref(),
        self.worker_id.as_deref(),
      ) {
        session::persist_worker_session_in(
          &self.workspace,
          &self.messages,
          parent_session_id,
          worker_id,
        )?;
      } else {
        self.meta.usage.total_tokens = self.total_tokens;
        session::write_meta_in(&self.workspace, &self.meta)?;
        session::persist_session_in(&self.workspace, &self.messages, &self.meta.session_id)?;
      }
    }
    Ok(())
  }

  pub async fn run_loop(&mut self) -> Result<Vec<Message>, AgentError> {
    loop {
      let resp = self
        .client
        .chat(&self.messages, &self.tools, None, None)
        .await?;

      let mut has_more = self.handle_turn_response(resp).await?;
      self.finish_turn(&mut has_more).await?;
      if !has_more {
        return Ok(self.messages.clone());
      }
    }
  }

  pub async fn steer_loop(
    &mut self,
    mut steer: impl SteerChannel,
    wait_for_input: bool,
  ) -> Result<Vec<Message>, AgentError> {
    self.replay_messages_to_steer_log(&steer);
    let mut state = SteerState::Idle { wait_for_input };
    let mut ctx = SteerCtx;

    loop {
      state = match state.step(self, &mut steer, &mut ctx).await? {
        SteerState::Exit(msgs) => return Ok(msgs),
        next => next,
      };
    }
  }

  async fn finish_turn(&mut self, has_more: &mut bool) -> Result<(), AgentError> {
    if *has_more {
      self.check_compact();
    }
    Ok(())
  }

  fn apply_steer_event(
    &mut self,
    event: SteerEvent,
    steer: &impl SteerChannel,
  ) -> Result<SteerAction, AgentError> {
    match event {
      SteerEvent::Message(content) => {
        self.meta.draft_input = None;
        if self.meta.start_ts.is_none() {
          self.meta.start_ts = Some(session::timestamp_ms());
        }
        self.push_msg(human_user_msg(content.clone()));
        steer.log_push(format!("[user] {}", truncate(&content, 200)));
      }
      SteerEvent::Cancel => {
        self.meta.draft_input = None;
        steer.log_push("[control] no in-flight request to cancel".to_string());
      }
      SteerEvent::New => {
        self.meta.draft_input = None;
        if self.dirty && !self.meta.flags.temp {
          self.meta.usage.total_tokens = self.total_tokens;
          session::write_meta_in(&self.workspace, &self.meta)?;
          session::persist_session_in(&self.workspace, &self.messages, &self.meta.session_id)?;
        }
        let old_id = self.meta.session_id.clone();
        self.meta.session_id = session::generate_session_id();
        self.meta.parent_session = Some(old_id);
        self.meta.usage = session::SessionUsage { total_tokens: 0 };
        self.meta.draft_input = None;
        self.meta.start_ts = None;
        self.meta.end_ts = None;
        let mut messages = crate::prompts::build_messages("");
        crate::prompts::enrich_initial_messages(&mut messages);
        self.messages = messages;
        self.dirty = false;
        self.tools = crate::tools::configured_director_tools();
        self.total_tokens = 0;
        self.worker_manager =
          WorkerManager::new(Some(&self.meta.session_id), self.workspace.clone());
        self.compact.compacting = false;
        self.compact.urgency = 0;
        self.pending_compact = CompactPending::None;
        steer.log_clear();
        steer.set_tokens(0);
        steer.log_push("[control] new session started".to_string());
        return Ok(SteerAction::Restart);
      }
      SteerEvent::Compact(task_prompt) => {
        self.meta.draft_input = None;
        let has_assistant = self.messages.iter().any(|m| m.role == "assistant");
        if !has_assistant {
          steer.log_push("[control] nothing to compact; no assistant response yet".to_string());
        } else {
          let mut compact_msg = String::from(
            "Produce a handoff brief for continuing this work in a fresh context window.\n\n\
             Include:\n\
             - ## Goal — what was the user trying to accomplish\n\
             - ## What was done — key actions, files modified, decisions (specific paths)\n\
             - ## Current state — what's in progress, what's blocked\n\
             - ## Relevant excerpts — critical code, errors, outputs\n\
             - ## Next steps\n\n\
             Be concise. Specific facts only. Omit anything that won't matter for continuing.",
          );
          if let Some(ref prompt) = task_prompt {
            compact_msg.push_str(&format!("\n\nFocus the new session on: {}", prompt));
          }
          self.push_msg(internal_user_msg(compact_msg));
          steer.log_push("[compact] requesting handoff brief...".to_string());
          self.pending_compact = match task_prompt {
            Some(p) => CompactPending::WithFocus(p),
            None => CompactPending::NoFocus,
          };
        }
      }
      SteerEvent::Profile(name) => match crate::profiles::get_profile(&name) {
        Some(p) => {
          self.meta.draft_input = None;
          self.client = crate::providers::new_client(p)?;
          self.meta.profile = name.clone();
          self.compact.context_limit = p.context_limit;
          steer.set_profile(name, p.model.to_string());
          steer.log_push(format!("[control] profile → {}", self.meta.profile));
        }
        None => {
          self.meta.draft_input = None;
          steer.log_push(format!("[control] unknown profile: {name}"));
        }
      },
      SteerEvent::Exit(exit_msg) => {
        match exit_msg {
          Some(content) => {
            self.meta.draft_input = Some(content);
            self.dirty = true;
          }
          None => {
            if self.meta.draft_input.is_some() {
              self.meta.draft_input = None;
              self.dirty = true;
            }
          }
        }
        return Ok(SteerAction::Exit);
      }
    }
    Ok(SteerAction::Continue)
  }

  fn replay_messages_to_steer_log(&self, steer: &impl SteerChannel) {
    for msg in &self.messages {
      match msg.role.as_str() {
        "system" => {}
        "user" if msg.origin != MessageOrigin::Internal => {
          steer.log_push(format!("[user] {}", truncate(&msg.content, 200)));
        }
        "user" => {}
        "assistant" => {
          if !msg.reasoning_content.is_empty() {
            steer.log_push(format!(
              "reasoning: {}",
              truncate(&msg.reasoning_content, 300)
            ));
          }
          if !msg.content.is_empty() {
            steer.log_push_assistant_markdown(&msg.content);
          }
          for tc in &msg.tool_calls {
            steer.log_push(format!(
              "tool: {}({})",
              tc.function.name,
              truncate(&tc.function.arguments, 120)
            ));
          }
        }
        "tool" => {
          steer.log_push(format!("tool_result: {}", truncate(&msg.content, 200)));
        }
        _ => {}
      }
    }
  }

  async fn handle_turn_response(&mut self, resp: ChatResponse) -> Result<bool, AgentError> {
    self
      .handle_turn_response_with_log::<crate::steer::NoopSteer>(resp, None, false)
      .await
  }

  async fn handle_turn_response_with_log<S: SteerChannel + ?Sized>(
    &mut self,
    resp: ChatResponse,
    ui_log: Option<&S>,
    streamed: bool,
  ) -> Result<bool, AgentError> {
    self.meta.end_ts = Some(session::timestamp_ms());
    self.total_tokens = resp.usage.total_tokens as u64;
    if !resp.reasoning_content.is_empty() && !streamed {
      if let Some(log) = ui_log {
        log.log_push(format!(
          "reasoning: {}",
          truncate(&resp.reasoning_content, 300)
        ));
      } else {
        eprintln!("reasoning: {}", truncate(&resp.reasoning_content, 300));
      }
    }
    if !resp.content.is_empty() && !streamed {
      if let Some(log) = ui_log {
        log.log_push_assistant_markdown(&resp.content);
      } else {
        eprintln!("content: {}", truncate(&resp.content, 200));
      }
    }

    if resp.tool_calls.is_empty() {
      self.push_msg(assistant_msg_with_reasoning(
        resp.content.clone(),
        resp.reasoning_content,
      ));
      if ui_log.is_none() && !self.meta.flags.worker {
        print!("{}", resp.content);
        io::stdout().flush().map_err(anyhow::Error::from)?;
        self.report_tokens();
      }
      return Ok(false);
    }

    let results = self.process_tool_calls(&resp).await?;
    for r in results {
      let indicator = if r.success { "ok" } else { "failed" };
      if let Some(log) = ui_log {
        log.log_push(format!(
          "tool: {}({}) -> {}",
          r.name,
          truncate(&r.args, 120),
          indicator
        ));
        if !r.success {
          log.log_push(format!("  => {}", truncate(&r.output, 200)));
        }
      } else {
        eprintln!(
          "tool: {}({}) -> {}",
          r.name,
          truncate(&r.args, 120),
          indicator
        );
        if !r.success {
          eprintln!("  => {}", truncate(&r.output, 200));
        }
      }
    }
    Ok(true)
  }

  async fn process_tool_calls(
    &mut self,
    resp: &ChatResponse,
  ) -> Result<Vec<ToolResult>, AgentError> {
    self.push_msg(assistant_msg_full(
      resp.content.clone(),
      resp.reasoning_content.clone(),
      resp.tool_calls.clone(),
    ));

    let mut results = Vec::with_capacity(resp.tool_calls.len());
    let mut read_only_batch: Vec<&ToolCall> = Vec::new();

    for tc in &resp.tool_calls {
      if is_read_only_tool(&tc.function.name) {
        read_only_batch.push(tc);
        continue;
      }
      if !read_only_batch.is_empty() {
        results.extend(run_read_only_batch(&self.workspace, &read_only_batch).await?);
        read_only_batch.clear();
      }
      let (output, success) = self.run_tool_call(tc).await;
      results.push(ToolResult {
        name: tc.function.name.clone(),
        args: tc.function.arguments.clone(),
        output,
        success,
      });
    }

    if !read_only_batch.is_empty() {
      results.extend(run_read_only_batch(&self.workspace, &read_only_batch).await?);
    }

    for (tc, r) in resp.tool_calls.iter().zip(results.iter()) {
      self.push_msg(tool_msg(r.output.clone(), tc.id.clone()));
    }
    Ok(results)
  }

  async fn run_tool_call(&mut self, tc: &ToolCall) -> (String, bool) {
    let workspace = self.workspace.clone();
    let (output, success) = format_tool_result(
      execute_tool(
        ToolContext {
          agent: Some(self),
          workspace,
        },
        &tc.function.name,
        &tc.function.arguments,
      )
      .await,
    );
    (output, success)
  }

  fn check_compact(&mut self) {
    if self.compact.threshold < 0.0 || self.compact.context_limit == 0 {
      return;
    }
    let total = self.total_tokens as usize;
    let ratio = total as f64 / self.compact.context_limit as f64;
    if ratio < self.compact.threshold {
      self.compact.compacting = false;
      self.compact.urgency = 0;
      return;
    }
    self.compact.compacting = true;
    self.compact.urgency += 1;
    let pct = total * 100 / self.compact.context_limit;
    let body = match self.compact.urgency {
      1 => format!(
        "Context budget at {pct}%.\nFinish the current chunk. Do not start unrelated work.\nIf useful state may be lost, write a checkpoint before continuing."
      ),
      2 => format!(
        "Context budget at {pct}%.\nApproaching the limit. Finish only critical in-progress work.\nDo not delegate new work.\nWrite a checkpoint if it will preserve important state."
      ),
      _ => format!(
        "Context budget at {pct}%.\nEXHAUSTED.\nDo not write more files, delegate, or start new work.\nSet terminal `state` key `status` to done/blocked/failed/partial and provide a final assistant summary."
      ),
    };
    self.push_msg(internal_user_msg(format!(
      "Reminder: [context_budget] {body}"
    )));
  }

  fn report_tokens(&self) {
    eprintln!("\n\ntokens: {}", self.total_tokens);
  }

  pub fn last_assistant_message(&self) -> Option<String> {
    self
      .messages
      .iter()
      .rev()
      .find(|m| m.role == "assistant")
      .map(|m| m.content.clone())
  }
}

fn human_user_msg(content: impl Into<String>) -> Message {
  Message {
    role: "user".into(),
    content: content.into(),
    origin: MessageOrigin::Human,
    ..Default::default()
  }
}

fn internal_user_msg(content: impl Into<String>) -> Message {
  Message {
    role: "user".into(),
    content: content.into(),
    origin: MessageOrigin::Internal,
    ..Default::default()
  }
}

fn assistant_msg_with_reasoning(
  content: impl Into<String>,
  reasoning: impl Into<String>,
) -> Message {
  Message {
    role: "assistant".into(),
    content: content.into(),
    origin: MessageOrigin::Model,
    reasoning_content: reasoning.into(),
    ..Default::default()
  }
}

fn assistant_msg_full(
  content: impl Into<String>,
  reasoning: impl Into<String>,
  tool_calls: Vec<ToolCall>,
) -> Message {
  Message {
    role: "assistant".into(),
    content: content.into(),
    origin: MessageOrigin::Model,
    reasoning_content: reasoning.into(),
    tool_calls,
    ..Default::default()
  }
}

fn tool_msg(content: impl Into<String>, tool_call_id: impl Into<String>) -> Message {
  Message {
    role: "tool".into(),
    content: content.into(),
    origin: MessageOrigin::Tool,
    tool_call_id: tool_call_id.into(),
    ..Default::default()
  }
}

fn format_tool_result(result: anyhow::Result<String>) -> (String, bool) {
  match result {
    Ok(out) => (out, true),
    Err(e) => (format!("ERROR: {e}"), false),
  }
}

async fn run_read_only_batch(
  workspace: &Workspace,
  batch: &[&ToolCall],
) -> Result<Vec<ToolResult>, AgentError> {
  let workspace = workspace.clone();
  let futs = batch.iter().map(|tc| async {
    let (output, success) = format_tool_result(
      execute_tool(
        ToolContext {
          agent: None,
          workspace: workspace.clone(),
        },
        &tc.function.name,
        &tc.function.arguments,
      )
      .await,
    );
    ToolResult {
      name: tc.function.name.clone(),
      args: tc.function.arguments.clone(),
      output,
      success,
    }
  });
  let results = futures_util::future::join_all(futs).await;
  Ok(results)
}

fn truncate(s: &str, n: usize) -> String {
  if s.len() <= n && !s.contains('\n') {
    return s.to_string();
  }
  let mut escaped = String::with_capacity(s.len());
  for c in s.chars() {
    if c == '\n' {
      escaped.push_str("\\n");
    } else {
      escaped.push(c);
    }
  }
  if escaped.len() <= n {
    escaped
  } else {
    let end = escaped.floor_char_boundary(n);
    escaped.truncate(end);
    escaped.push_str("...");
    escaped
  }
}

#[cfg(test)]
mod truncate_tests {
  use super::truncate;

  #[test]
  fn truncate_keeps_short_ascii_unchanged() {
    assert_eq!(truncate("hello", 10), "hello");
  }

  #[test]
  fn truncate_does_not_split_utf8() {
    assert_eq!(truncate("x─y", 2), "x...");
  }
}

#[cfg(test)]
mod dirty_state_machine_tests {
  use super::*;
  use std::sync::atomic::{AtomicU64, Ordering};

  static TEST_SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

  fn dummy_client() -> Client {
    Client::new(
      "http://localhost",
      "dummy".into(),
      |_, _| serde_json::Value::Null,
      30,
    )
    .unwrap()
  }

  fn dummy_meta() -> session::SessionMeta {
    let id = TEST_SESSION_COUNTER.fetch_add(1, Ordering::SeqCst);
    session::SessionMeta {
      session_id: format!("test-session-{id}"),
      parent_session: None,
      profile: "test".into(),
      mode: "steer".into(),
      flags: session::SessionFlags {
        steer: true,
        worker: false,
        autocompact: -1,
        resume: false,
        temp: false,
      },
      usage: session::SessionUsage { total_tokens: 0 },
      draft_input: None,
      start_ts: None,
      end_ts: None,
    }
  }

  fn dummy_agent() -> Agent {
    Agent::new(
      Workspace::from_current_dir(),
      dummy_client(),
      crate::prompts::build_messages(""),
      Vec::new(),
      CompactState::disabled(),
      dummy_meta(),
      None,
      None,
    )
  }

  fn tool_response(id: &str, name: &str, arguments: &str) -> ChatResponse {
    ChatResponse {
      tool_calls: vec![ToolCall {
        id: id.to_string(),
        kind: "function".to_string(),
        function: crate::types::FunctionCall {
          name: name.to_string(),
          arguments: arguments.to_string(),
        },
      }],
      ..Default::default()
    }
  }

  fn first_message_index(
    messages: &[Message],
    predicate: impl Fn(&Message) -> bool,
  ) -> Option<usize> {
    messages.iter().position(predicate)
  }

  #[test]
  fn agent_starts_clean() {
    let agent = dummy_agent();
    assert!(!agent.dirty);
  }

  #[test]
  fn push_msg_sets_dirty() {
    let mut agent = dummy_agent();
    agent.push_msg(human_user_msg("hello"));
    assert!(agent.dirty);
    assert_eq!(agent.messages.len(), 2); // system + "hello"
  }

  #[tokio::test]
  async fn first_message_sets_start_ts() {
    let mut agent = dummy_agent();
    let steer = crate::steer::TestSteerHandle::new();
    let action = agent
      .apply_steer_event(SteerEvent::Message("fix bug".into()), &steer)
      .unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(agent.dirty);
    assert!(agent.meta.start_ts.is_some());
  }

  #[tokio::test]
  async fn second_message_preserves_start_ts() {
    let mut agent = dummy_agent();
    let steer = crate::steer::TestSteerHandle::new();
    agent
      .apply_steer_event(SteerEvent::Message("fix bug".into()), &steer)
      .unwrap();
    let start_ts = agent.meta.start_ts;
    agent
      .apply_steer_event(SteerEvent::Message("more context".into()), &steer)
      .unwrap();
    assert_eq!(agent.meta.start_ts, start_ts);
  }

  #[tokio::test]
  async fn cancel_does_not_change_dirty() {
    let mut agent = dummy_agent();
    let steer = crate::steer::TestSteerHandle::new();
    let action = agent.apply_steer_event(SteerEvent::Cancel, &steer).unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(!agent.dirty);
  }

  #[tokio::test]
  async fn exit_returns_exit_action() {
    let mut agent = dummy_agent();
    let steer = crate::steer::TestSteerHandle::new();
    let action = agent
      .apply_steer_event(SteerEvent::Exit(None), &steer)
      .unwrap();
    assert!(matches!(action, SteerAction::Exit));
    assert!(!agent.dirty);
  }

  #[tokio::test]
  async fn exit_with_message_sets_draft_only() {
    let mut agent = dummy_agent();
    let steer = crate::steer::TestSteerHandle::new();
    let before_len = agent.messages.len();
    let action = agent
      .apply_steer_event(SteerEvent::Exit(Some("save this".into())), &steer)
      .unwrap();
    assert!(matches!(action, SteerAction::Exit));
    assert_eq!(agent.meta.draft_input, Some("save this".into()));
    assert!(agent.dirty);
    assert_eq!(agent.messages.len(), before_len);
  }

  #[tokio::test]
  async fn new_on_clean_resets_without_files() {
    let mut agent = dummy_agent();
    let steer = crate::steer::TestSteerHandle::new();
    let old_id = agent.meta.session_id.clone();
    let action = agent.apply_steer_event(SteerEvent::New, &steer).unwrap();
    assert!(matches!(action, SteerAction::Restart));
    assert!(!agent.dirty);
    assert_eq!(agent.meta.start_ts, None);
    assert_eq!(agent.meta.end_ts, None);
    assert_eq!(agent.meta.parent_session, Some(old_id.clone()));
    assert_ne!(agent.meta.session_id, old_id);
  }

  #[tokio::test]
  async fn new_on_dirty_persists_old_then_resets() {
    let mut agent = dummy_agent();
    let steer = crate::steer::TestSteerHandle::new();
    agent.push_msg(human_user_msg("hello"));
    let old_id = agent.meta.session_id.clone();

    let action = agent.apply_steer_event(SteerEvent::New, &steer).unwrap();
    assert!(matches!(action, SteerAction::Restart));

    // old session should have been persisted
    let old_dir = session::session_dir(&old_id);
    assert!(
      old_dir.join("meta.json").exists(),
      "old meta should be persisted"
    );
    assert!(
      old_dir.join("messages.jsonl").exists(),
      "old messages should be persisted"
    );

    // new session should be clean
    assert!(!agent.dirty);
    assert_eq!(agent.meta.start_ts, None);
    assert_eq!(agent.meta.end_ts, None);
    assert_eq!(agent.meta.parent_session, Some(old_id.clone()));
    assert_ne!(agent.meta.session_id, old_id);

    // clean up
    let _ = std::fs::remove_dir_all(&old_dir);
  }

  #[test]
  fn handle_turn_response_sets_end_ts() {
    let mut agent = dummy_agent();
    assert_eq!(agent.meta.end_ts, None);
    let resp = ChatResponse {
      content: "ok".into(),
      reasoning_content: String::new(),
      tool_calls: Vec::new(),
      usage: crate::types::Usage { total_tokens: 15 },
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
      agent.handle_turn_response(resp).await.unwrap();
    });
    assert!(agent.meta.end_ts.is_some());
    assert!(agent.dirty);
  }

  #[tokio::test]
  async fn persisted_trace_keeps_failed_dispatch_then_valid_wait() {
    let mut agent = dummy_agent();
    agent.worker_manager = WorkerManager::new_for_test(|args| async move {
      crate::workers::WorkerProcessResult {
        output: format!("finished {}", args.worker_id),
        err: None,
      }
    });

    let bad_dispatch = tool_response(
      "bad-dispatch",
      "dispatch_workers",
      r#"{"workers":[{"role":"implementer","task":"missing array close"}"#,
    );
    assert!(agent.handle_turn_response(bad_dispatch).await.unwrap());

    let valid_dispatch = tool_response(
      "valid-dispatch",
      "dispatch_workers",
      r#"{"workers":[{"role":"implementer","task":"complete quickly"}]}"#,
    );
    assert!(agent.handle_turn_response(valid_dispatch).await.unwrap());

    let wait = tool_response("wait-workers", "wait_workers", "{}");
    assert!(agent.handle_turn_response(wait).await.unwrap());

    agent.persist_if_dirty().unwrap();
    let messages = session::load_session(&agent.meta.session_id).unwrap();

    let bad_dispatch_idx = first_message_index(&messages, |m| {
      m.tool_calls
        .iter()
        .any(|tc| tc.id == "bad-dispatch" && tc.function.name == "dispatch_workers")
    })
    .unwrap();
    let bad_error_idx = first_message_index(&messages, |m| {
      m.role == "tool"
        && m.tool_call_id == "bad-dispatch"
        && m.content.contains("ERROR: bad dispatch_workers args")
    })
    .unwrap();
    let valid_dispatch_idx = first_message_index(&messages, |m| {
      m.tool_calls
        .iter()
        .any(|tc| tc.id == "valid-dispatch" && tc.function.name == "dispatch_workers")
    })
    .unwrap();
    let dispatch_result_idx = first_message_index(&messages, |m| {
      m.role == "tool"
        && m.tool_call_id == "valid-dispatch"
        && m.content.contains("Workers dispatched successfully")
    })
    .unwrap();
    let wait_idx = first_message_index(&messages, |m| {
      m.tool_calls
        .iter()
        .any(|tc| tc.id == "wait-workers" && tc.function.name == "wait_workers")
    })
    .unwrap();
    let wait_result_idx = first_message_index(&messages, |m| {
      m.role == "tool"
        && m.tool_call_id == "wait-workers"
        && m.content.contains("\"status\":\"completed\"")
        && m.content.contains("finished worker-1")
    })
    .unwrap();

    assert!(bad_dispatch_idx < bad_error_idx);
    assert!(bad_error_idx < valid_dispatch_idx);
    assert!(valid_dispatch_idx < dispatch_result_idx);
    assert!(dispatch_result_idx < wait_idx);
    assert!(wait_idx < wait_result_idx);
  }

  #[tokio::test]
  async fn compact_on_empty_is_noop() {
    let mut agent = dummy_agent();
    let steer = crate::steer::TestSteerHandle::new();
    let old_id = agent.meta.session_id.clone();
    let action = agent
      .apply_steer_event(SteerEvent::Compact(None), &steer)
      .unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(!agent.dirty);
    assert_eq!(agent.meta.session_id, old_id);
    assert!(matches!(agent.pending_compact, CompactPending::None));
  }

  #[tokio::test]
  async fn compact_pushes_handoff_message() {
    let mut agent = dummy_agent();
    let steer = crate::steer::TestSteerHandle::new();
    agent.push_msg(human_user_msg("hello"));
    agent.push_msg(assistant_msg_with_reasoning("ok", ""));
    let old_id = agent.meta.session_id.clone();
    let old_len = agent.messages.len();

    let action = agent
      .apply_steer_event(SteerEvent::Compact(None), &steer)
      .unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(agent.dirty);
    assert_eq!(agent.meta.session_id, old_id);
    assert!(matches!(agent.pending_compact, CompactPending::NoFocus));
    assert_eq!(agent.messages.len(), old_len + 1);
    let last = agent.messages.last().unwrap();
    assert_eq!(last.role, "user");
    assert!(last.content.contains("handoff brief"));
  }

  #[tokio::test]
  async fn compact_with_focus_includes_prompt() {
    let mut agent = dummy_agent();
    let steer = crate::steer::TestSteerHandle::new();
    agent.push_msg(human_user_msg("hello"));
    agent.push_msg(assistant_msg_with_reasoning("ok", ""));

    let action = agent
      .apply_steer_event(SteerEvent::Compact(Some("fix auth".into())), &steer)
      .unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(matches!(agent.pending_compact, CompactPending::WithFocus(ref s) if s == "fix auth"));
    let last = agent.messages.last().unwrap();
    assert!(last.content.contains("fix auth"));
  }
}
