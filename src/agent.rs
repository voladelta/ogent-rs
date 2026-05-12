use crate::client::{Client, ClientError};
use crate::session;
use crate::sse::{SseError, StreamEvent};
use crate::task_tracker::{TaskTracker, is_tracking_tool_name};
use crate::tools::{ToolContext, execute_tool, is_read_only_tool};
use crate::tui::{AgentState, SteerEvent, TuiHandle};
use crate::types::{ChatResponse, Message, Tool, ToolCall};
use crate::workers::WorkerManager;
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
    tui: &mut TuiHandle,
    _ctx: &mut SteerCtx,
  ) -> Result<Self, AgentError> {
    match self {
      Self::Exit(msgs) => Ok(Self::Exit(msgs)),

      Self::Idle { wait_for_input } => {
        tui.status.set_state(AgentState::Idle);
        let wait_baseline_len = agent.messages.len();
        let mut wait = wait_for_input;

        while let Ok(event) = tui.rx.try_recv() {
          if let Some(next) = Self::process_idle_event(agent, tui, _ctx, event)? {
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
            let Some(event) = tui.rx.recv().await else {
              continue;
            };
            if let Some(next) = Self::process_idle_event(agent, tui, _ctx, event)? {
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
        tui.status.set_tokens(agent.total_tokens);
        agent.refresh_workflow_reminder();

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

        tui.log.start_stream();

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
                  tui.log.append_stream_chunk(&chunk);
                  if !tool_calling {
                    tui.status.set_state(AgentState::Replying);
                  }
                }
                StreamEvent::Reasoning(chunk) => {
                  tui.log.append_reasoning_chunk(&chunk);
                  if !tool_calling {
                    tui.status.set_state(AgentState::Reasoning);
                  }
                }
                StreamEvent::ToolCalling => {
                  tool_calling = true;
                  tui.status.set_state(AgentState::Working);
                }
              }
            }
            maybe_event = tui.rx.recv(), if !cancelled && steer_msg.is_none() => {
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
                SteerEvent::Complete => {
                  cancel.cancel();
                  steer_msg = Some(MANUAL_COMPLETE_REMINDER.to_string());
                }
                SteerEvent::New => {
                  cancel.cancel();
                  agent.apply_steer_event(SteerEvent::New, tui)?;
                  chat.abort();
                  tui.log.push(STEER_COMMANDS.to_string());
                  return Ok(Self::Idle { wait_for_input: true });
                }
                SteerEvent::Exit => {
                  cancel.cancel();
                  chat.abort();
                  return Ok(Self::Exit(agent.messages.clone()));
                }
                other => {
                  match agent.apply_steer_event(other, tui)? {
                    SteerAction::Exit => {
                      cancel.cancel();
                      chat.abort();
                      return Ok(Self::Exit(agent.messages.clone()));
                    }
                    SteerAction::Restart => {
                      cancel.cancel();
                      chat.abort();
                      tui.log.push(STEER_COMMANDS.to_string());
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
            StreamEvent::Content(chunk) => tui.log.append_stream_chunk(&chunk),
            StreamEvent::Reasoning(chunk) => tui.log.append_reasoning_chunk(&chunk),
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
              agent.push_msg(user_msg(msg.clone()));
              tui.log.push(format!("[steer] {}", truncate(&msg, 200)));
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
          .handle_turn_response_with_log(resp, Some(&tui.log), true)
          .await?;
        tui.log.end_stream();
        tui.status.set_state(AgentState::Idle);
        tui.status.set_tokens(agent.total_tokens);
        Ok(Self::FinishTurn { has_more })
      }

      Self::FinishTurn { mut has_more } => {
        if agent.completion_summary.is_some() {
          tui
            .log
            .push("[steer] task complete; send a message to continue or /q to quit".to_string());
          agent.completion_summary = None;

          if agent.compact.compacting && agent.compact.threshold > 0.0 {
            let ratio = agent.total_tokens as f64 / agent.compact.context_limit as f64;
            if ratio >= agent.compact.threshold {
              let mut compact_msg = String::from(
                "Context budget exhausted. Produce a handoff brief now:\n\
                 - ## Goal\n\
                 - ## What was done\n\
                 - ## Current state\n\
                 - ## Relevant excerpts\n\
                 - ## Next steps\n\n\
                 Be concise. Specific facts only.",
              );
              if let Some(tracker) = &agent.task_tracker {
                compact_msg.push_str(&format!(
                  "\n\n## Task Plan (include verbatim)\n{}",
                  serde_json::to_string_pretty(tracker).unwrap_or_default(),
                ));
              }
              agent.push_msg(user_msg(compact_msg));
              tui
                .log
                .push("[compact] autocompact triggered, requesting handoff brief...");
              agent.pending_compact = CompactPending::NoFocus;
              return Ok(Self::StartTurn);
            }
          }

          return Ok(Self::Idle {
            wait_for_input: true,
          });
        }

        let should_exit = agent.finish_turn(&mut has_more, Some(&tui.log)).await?;

        if should_exit {
          return Ok(Self::Exit(agent.messages.clone()));
        }

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
            tui
              .log
              .push("[compact] model returned empty response, not compacting");
          } else {
            let parent_id = agent.meta.session_id.clone();

            if !agent.meta.flags.temp {
              agent.meta.usage.total_tokens = agent.total_tokens;
              session::write_meta(&agent.meta)?;
              session::persist_session(&agent.messages, &agent.meta.session_id)?;
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
              ..Default::default()
            });

            agent.meta.session_id = session::generate_session_id();
            agent.meta.parent_session = Some(parent_id.clone());
            agent.meta.start_ts = Some(session::timestamp_ms());
            agent.meta.end_ts = None;
            agent.meta.usage = session::SessionUsage { total_tokens: 0 };
            if task_prompt.is_some() {
              agent.meta.prompt = task_prompt;
            }
            agent.messages = new_messages;
            agent.total_tokens = 0;
            agent.dirty = true;

            agent.compact.compacting = false;
            agent.compact.urgency = 0;
            agent.completion_summary = None;
            agent.complete_open_work_warned = false;

            if !agent.meta.flags.temp {
              session::write_meta(&agent.meta)?;
              session::persist_session(&agent.messages, &agent.meta.session_id)?;
            }

            tui.log.clear();
            tui.log.push(format!(
              "[compact] {} → {} (parent preserved)",
              parent_id, agent.meta.session_id
            ));
            tui.status.set_tokens(0);
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
    tui: &mut TuiHandle,
    _ctx: &mut SteerCtx,
    event: SteerEvent,
  ) -> Result<Option<Self>, AgentError> {
    match agent.apply_steer_event(event, tui)? {
      SteerAction::Exit => Ok(Some(Self::Exit(agent.messages.clone()))),
      SteerAction::Restart => {
        tui.log.push(STEER_COMMANDS.to_string());
        Ok(Some(Self::Idle {
          wait_for_input: true,
        }))
      }
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
  pub client: Client,
  pub messages: Vec<Message>,
  pub tools: Vec<Tool>,
  pub worker_manager: WorkerManager,
  pub total_tokens: u64,
  pub compact: CompactState,
  pub completion_summary: Option<String>,
  pub task_tracker: Option<TaskTracker>,
  pub workflow_state: Option<crate::workflow::WorkflowState>,
  pub complete_open_work_warned: bool,
  pub meta: session::SessionMeta,
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
  pub fn new(
    client: Client,
    messages: Vec<Message>,
    tools: Vec<Tool>,
    compact: CompactState,
    task_tracker: Option<TaskTracker>,
    workflow_state: Option<crate::workflow::WorkflowState>,
    meta: session::SessionMeta,
  ) -> Self {
    Self {
      client,
      messages,
      tools,
      worker_manager: WorkerManager::new(),
      total_tokens: 0,
      compact,
      completion_summary: None,
      task_tracker,
      workflow_state,
      complete_open_work_warned: false,
      meta,
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
      self.meta.usage.total_tokens = self.total_tokens;
      session::write_meta(&self.meta)?;
      session::persist_session(&self.messages, &self.meta.session_id)?;
    }
    Ok(())
  }

  fn refresh_workflow_reminder(&mut self) {
    const WORKFLOW_MARKER: &str = "\n\n[Workflow]";
    if let Some(ref ws) = self.workflow_state {
      let reminder = ws.reminder_text();
      if let Some(first) = self.messages.first_mut()
        && first.role == "system"
      {
        if let Some(idx) = first.content.find(WORKFLOW_MARKER) {
          first.content.truncate(idx);
        }
        std::fmt::Write::write_fmt(
          &mut first.content,
          format_args!("{WORKFLOW_MARKER}\n{reminder}"),
        )
        .unwrap();
      }
    }
  }

  pub async fn run_loop(&mut self) -> Result<Vec<Message>, AgentError> {
    loop {
      self.refresh_workflow_reminder();
      let resp = self
        .client
        .chat(&self.messages, &self.tools, None, None)
        .await?;

      let mut has_more = self.handle_turn_response(resp).await?;
      if self.finish_turn(&mut has_more, None).await? {
        return Ok(self.messages.clone());
      }
      if !has_more {
        return Ok(self.messages.clone());
      }
    }
  }

  pub async fn steer_loop(
    &mut self,
    mut tui: TuiHandle,
    wait_for_input: bool,
  ) -> Result<Vec<Message>, AgentError> {
    tui.log.push(STEER_COMMANDS);
    let mut state = SteerState::Idle { wait_for_input };
    let mut ctx = SteerCtx;

    loop {
      state = match state.step(self, &mut tui, &mut ctx).await? {
        SteerState::Exit(msgs) => return Ok(msgs),
        next => next,
      };
    }
  }

  async fn finish_turn(
    &mut self,
    has_more: &mut bool,
    ui_log: Option<&crate::tui::UiLog>,
  ) -> Result<bool, AgentError> {
    if self.completion_summary.is_some() {
      return Ok(true);
    }
    if let Some(msg) = self.worker_manager.status_message().await {
      if let Some(log) = ui_log {
        self.push_msg(user_msg(msg.clone()));
        log.push(format!("[workers] {}", truncate(&msg, 200)));
      } else {
        self.push_msg(user_msg(msg));
      }
      *has_more = true;
    }
    if *has_more {
      self.check_compact();
      self.push_task_tracking_reminder();
    }
    Ok(false)
  }

  fn apply_steer_event(
    &mut self,
    event: SteerEvent,
    tui: &TuiHandle,
  ) -> Result<SteerAction, AgentError> {
    match event {
      SteerEvent::Message(content) => {
        if self.meta.prompt.is_none() {
          self.meta.prompt = Some(content.clone());
        }
        if self.meta.start_ts.is_none() {
          self.meta.start_ts = Some(session::timestamp_ms());
        }
        self.push_msg(user_msg(content.clone()));
        tui.log.push(format!("[user] {}", truncate(&content, 200)));
      }
      SteerEvent::Cancel => {
        tui.log.push("[steer] no in-flight request to cancel");
      }
      SteerEvent::Complete => {
        let has_assistant = self.messages.iter().any(|m| m.role == "assistant");
        if has_assistant {
          let content = MANUAL_COMPLETE_REMINDER.to_string();
          self.push_msg(user_msg(content));
          tui.log.push("[steer] complete requested");
        } else {
          tui
            .log
            .push("[steer] nothing to complete; session is empty");
        }
      }
      SteerEvent::New => {
        if self.dirty && !self.meta.flags.temp {
          self.meta.usage.total_tokens = self.total_tokens;
          session::write_meta(&self.meta)?;
          session::persist_session(&self.messages, &self.meta.session_id)?;
        }
        let old_id = self.meta.session_id.clone();
        self.meta.session_id = session::generate_session_id();
        self.meta.parent_session = Some(old_id);
        self.meta.usage = session::SessionUsage { total_tokens: 0 };
        self.meta.prompt = None;
        self.meta.start_ts = None;
        self.meta.end_ts = None;
        let mut messages = crate::prompts::build_messages("");
        crate::prompts::enrich_initial_messages(&mut messages);
        self.messages = messages;
        self.dirty = false;
        self.workflow_state = None;
        self.total_tokens = 0;
        self.worker_manager = WorkerManager::new();
        self.completion_summary = None;
        self.complete_open_work_warned = false;
        self.compact.compacting = false;
        self.compact.urgency = 0;
        self.pending_compact = CompactPending::None;
        tui.log.clear();
        tui.status.set_tokens(0);
        tui.log.push("[steer] new session started");
        return Ok(SteerAction::Restart);
      }
      SteerEvent::Compact(task_prompt) => {
        if !self.dirty {
          tui.log.push("[steer] nothing to compact; session is empty");
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
          if let Some(tracker) = &self.task_tracker {
            compact_msg.push_str(&format!(
              "\n\n## Task Plan (include verbatim)\n\
               Preserve the full task plan so set_goal/update_phase/update_todo can be resumed:\n{}",
              serde_json::to_string_pretty(tracker).unwrap_or_default(),
            ));
          }
          if let Some(ref prompt) = task_prompt {
            compact_msg.push_str(&format!("\n\nFocus the new session on: {}", prompt));
          }
          self.push_msg(user_msg(compact_msg));
          tui.log.push("[compact] requesting handoff brief...");
          self.pending_compact = match task_prompt {
            Some(p) => CompactPending::WithFocus(p),
            None => CompactPending::NoFocus,
          };
        }
      }
      SteerEvent::Profile(name) => match crate::profiles::get_profile(&name) {
        Some(p) => {
          self.client = crate::providers::new_client(p)?;
          self.meta.profile = name.clone();
          self.compact.context_limit = p.context_limit;
          tui.status.set_profile(name, p.model.to_string());
          tui
            .log
            .push(format!("[steer] profile → {}", self.meta.profile));
        }
        None => {
          tui.log.push(format!("[steer] unknown profile: {name}"));
        }
      },
      SteerEvent::Exit => return Ok(SteerAction::Exit),
    }
    Ok(SteerAction::Continue)
  }

  async fn handle_turn_response(&mut self, resp: ChatResponse) -> Result<bool, AgentError> {
    self.handle_turn_response_with_log(resp, None, false).await
  }

  async fn handle_turn_response_with_log(
    &mut self,
    resp: ChatResponse,
    ui_log: Option<&crate::tui::UiLog>,
    streamed: bool,
  ) -> Result<bool, AgentError> {
    self.meta.end_ts = Some(session::timestamp_ms());
    self.total_tokens = resp.usage.total_tokens as u64;
    if !resp.reasoning_content.is_empty() && !streamed {
      if let Some(log) = ui_log {
        log.push(format!(
          "reasoning: {}",
          truncate(&resp.reasoning_content, 300)
        ));
      } else {
        eprintln!("reasoning: {}", truncate(&resp.reasoning_content, 300));
      }
    }
    if !resp.content.is_empty() && !streamed {
      if let Some(log) = ui_log {
        log.push_assistant_markdown(&resp.content);
      } else {
        eprintln!("content: {}", truncate(&resp.content, 200));
      }
    }

    if resp.tool_calls.is_empty() {
      self.push_msg(assistant_msg_with_reasoning(
        resp.content.clone(),
        resp.reasoning_content,
      ));
      if ui_log.is_none() {
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
        log.push(format!(
          "tool: {}({}) -> {}",
          r.name,
          truncate(&r.args, 120),
          indicator
        ));
        if !r.success {
          log.push(format!("  => {}", truncate(&r.output, 200)));
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
        results.extend(run_read_only_batch(&read_only_batch).await?);
        read_only_batch.clear();
      }
      let (output, success) = self.run_tool_call(tc).await;
      results.push(ToolResult {
        name: tc.function.name.clone(),
        args: tc.function.arguments.clone(),
        output,
        success,
      });
      if self.completion_summary.is_some() {
        break;
      }
    }

    if !read_only_batch.is_empty() {
      results.extend(run_read_only_batch(&read_only_batch).await?);
    }

    for (tc, r) in resp.tool_calls.iter().zip(results.iter()) {
      self.push_msg(tool_msg(r.output.clone(), tc.id.clone()));
    }
    self.record_task_tracking_turn(&results);
    Ok(results)
  }

  async fn run_tool_call(&mut self, tc: &ToolCall) -> (String, bool) {
    let (output, success) = format_tool_result(
      execute_tool(
        ToolContext { agent: Some(self) },
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
        "Context budget at {pct}%.\nEXHAUSTED.\nDo not write more files, delegate, or start new work.\nCall `complete` IMMEDIATELY with a summary of completed files, current state, verification state, blockers, and next steps."
      ),
    };
    self.push_msg(user_msg(format!("Reminder: [context_budget] {body}")));
  }

  fn report_tokens(&self) {
    eprintln!("\n\ntokens: {}", self.total_tokens);
  }

  fn record_task_tracking_turn(&mut self, results: &[ToolResult]) {
    let Some(tracker) = self.task_tracker.as_mut() else {
      return;
    };
    let mut saw_tracking_update = false;
    let mut saw_meaningful_non_tracking = false;
    for result in results {
      if !result.success {
        continue;
      }
      if is_tracking_tool_name(&result.name) {
        saw_tracking_update = true;
      } else {
        saw_meaningful_non_tracking = true;
      }
    }
    tracker.note_tool_turn(saw_tracking_update, saw_meaningful_non_tracking);
  }

  fn push_task_tracking_reminder(&mut self) {
    if let Some(tracker) = self.task_tracker.as_mut()
      && let Some(reminder) = tracker.take_reminder()
    {
      self.push_msg(user_msg(reminder));
    }
  }
}

fn user_msg(content: impl Into<String>) -> Message {
  Message {
    role: "user".into(),
    content: content.into(),
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
    reasoning_content: reasoning.into(),
    tool_calls,
    ..Default::default()
  }
}

fn tool_msg(content: impl Into<String>, tool_call_id: impl Into<String>) -> Message {
  Message {
    role: "tool".into(),
    content: content.into(),
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

async fn run_read_only_batch(batch: &[&ToolCall]) -> Result<Vec<ToolResult>, AgentError> {
  let futs = batch.iter().map(|tc| async {
    let (output, success) = format_tool_result(
      execute_tool(
        ToolContext { agent: None },
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

const STEER_COMMANDS: &str =
  "[steer] commands: /complete /cancel /new /compact [/compact <focus>] /q";

const MANUAL_COMPLETE_REMINDER: &str = r#"Reminder: [manual_complete]
The user requested completion from steer mode.

Summarize the current session retrospectively and call `complete` with structured Markdown:
- task summary
- what changed / what you did
- what you learned
- what to do better next time
- optional evidence: files touched, tests run, git head"#;

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
      prompt: None,
      start_ts: None,
      end_ts: None,
    }
  }

  fn dummy_agent() -> Agent {
    Agent::new(
      dummy_client(),
      crate::prompts::build_messages(""),
      Vec::new(),
      CompactState::disabled(),
      None,
      None,
      dummy_meta(),
    )
  }

  #[test]
  fn agent_starts_clean() {
    let agent = dummy_agent();
    assert!(!agent.dirty);
  }

  #[test]
  fn push_msg_sets_dirty() {
    let mut agent = dummy_agent();
    agent.push_msg(user_msg("hello"));
    assert!(agent.dirty);
    assert_eq!(agent.messages.len(), 3); // system + initial user + "hello"
  }

  #[tokio::test]
  async fn first_message_sets_prompt_and_start_ts() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    let action = agent
      .apply_steer_event(SteerEvent::Message("fix bug".into()), &tui)
      .unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(agent.dirty);
    assert_eq!(agent.meta.prompt, Some("fix bug".into()));
    assert!(agent.meta.start_ts.is_some());
  }

  #[tokio::test]
  async fn second_message_preserves_prompt() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    agent
      .apply_steer_event(SteerEvent::Message("fix bug".into()), &tui)
      .unwrap();
    let start_ts = agent.meta.start_ts;
    agent
      .apply_steer_event(SteerEvent::Message("more context".into()), &tui)
      .unwrap();
    assert_eq!(agent.meta.prompt, Some("fix bug".into()));
    assert_eq!(agent.meta.start_ts, start_ts);
  }

  #[tokio::test]
  async fn cancel_does_not_change_dirty() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    let action = agent.apply_steer_event(SteerEvent::Cancel, &tui).unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(!agent.dirty);
  }

  #[tokio::test]
  async fn complete_on_empty_session_stays_clean() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    let action = agent.apply_steer_event(SteerEvent::Complete, &tui).unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(!agent.dirty);
    assert_eq!(agent.messages.len(), 2); // no extra message pushed
  }

  #[tokio::test]
  async fn complete_with_assistant_makes_dirty() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    agent.push_msg(assistant_msg_with_reasoning("ok", ""));
    assert!(agent.dirty);
    let action = agent.apply_steer_event(SteerEvent::Complete, &tui).unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(agent.dirty);
    assert_eq!(agent.messages.len(), 4); // system + user + assistant + complete reminder
  }

  #[tokio::test]
  async fn exit_returns_exit_action() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    let action = agent.apply_steer_event(SteerEvent::Exit, &tui).unwrap();
    assert!(matches!(action, SteerAction::Exit));
  }

  #[tokio::test]
  async fn new_on_clean_resets_without_files() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    let old_id = agent.meta.session_id.clone();
    let action = agent.apply_steer_event(SteerEvent::New, &tui).unwrap();
    assert!(matches!(action, SteerAction::Restart));
    assert!(!agent.dirty);
    assert_eq!(agent.meta.prompt, None);
    assert_eq!(agent.meta.start_ts, None);
    assert_eq!(agent.meta.end_ts, None);
    assert_eq!(agent.meta.parent_session, Some(old_id.clone()));
    assert_ne!(agent.meta.session_id, old_id);
  }

  #[tokio::test]
  async fn new_on_dirty_persists_old_then_resets() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    agent.push_msg(user_msg("hello"));
    let old_id = agent.meta.session_id.clone();

    let action = agent.apply_steer_event(SteerEvent::New, &tui).unwrap();
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
    assert_eq!(agent.meta.prompt, None);
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
  async fn compact_on_empty_is_noop() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    let old_id = agent.meta.session_id.clone();
    let action = agent
      .apply_steer_event(SteerEvent::Compact(None), &tui)
      .unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(!agent.dirty);
    assert_eq!(agent.meta.session_id, old_id);
    assert!(matches!(agent.pending_compact, CompactPending::None));
  }

  #[tokio::test]
  async fn compact_pushes_handoff_message() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    agent.push_msg(user_msg("hello"));
    let old_id = agent.meta.session_id.clone();
    let old_len = agent.messages.len();

    let action = agent
      .apply_steer_event(SteerEvent::Compact(None), &tui)
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
    let tui = crate::tui::TuiHandle::test_handle();
    agent.push_msg(user_msg("hello"));

    let action = agent
      .apply_steer_event(SteerEvent::Compact(Some("fix auth".into())), &tui)
      .unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(matches!(agent.pending_compact, CompactPending::WithFocus(ref s) if s == "fix auth"));
    let last = agent.messages.last().unwrap();
    assert!(last.content.contains("fix auth"));
  }
}
