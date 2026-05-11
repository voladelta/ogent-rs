use crate::client::{Client, ClientError};
use crate::session;
use crate::task_tracker::{TaskTracker, is_tracking_tool_name};
use crate::tools::{ToolContext, execute_tool, is_read_only_tool};
use crate::tui::{SteerEvent, TuiHandle};
use crate::types::{ChatResponse, Message, Tool, ToolCall};
use crate::workers::WorkerManager;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
  #[error("interactive mode required")]
  InteractiveRequired,
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
  },
  ProcessResult(ChatResponse),
  FinishTurn {
    has_more: bool,
  },
  Exit(Vec<Message>),
}

struct SteerCtx {
  turn: i32,
  max_turns: i32,
  auto_continue: bool,
}

impl SteerState {
  async fn step(
    self,
    agent: &mut Agent,
    tui: &mut TuiHandle,
    ctx: &mut SteerCtx,
  ) -> Result<Self, AgentError> {
    match self {
      Self::Exit(msgs) => Ok(Self::Exit(msgs)),

      Self::Idle { wait_for_input } => {
        if agent.next_turn_reset {
          ctx.turn = 1;
          agent.next_turn_reset = false;
        }

        let wait_baseline_len = agent.messages.len();
        let mut wait = wait_for_input;

        while let Ok(event) = tui.rx.try_recv() {
          match agent.apply_steer_event(event, &mut ctx.auto_continue, tui)? {
            SteerAction::Exit => {
              return Ok(Self::Exit(agent.messages.clone()));
            }
            SteerAction::Restart => {
              ctx.turn = 1;
              tui
                .log
                .push("[steer] commands: /auto /stop /complete /cancel /new /fork /q".to_string());
              return Ok(Self::Idle {
                wait_for_input: true,
              });
            }
            SteerAction::Continue => {}
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
            match agent.apply_steer_event(event, &mut ctx.auto_continue, tui)? {
              SteerAction::Exit => {
                return Ok(Self::Exit(agent.messages.clone()));
              }
              SteerAction::Restart => {
                ctx.turn = 1;
                tui.log.push(
                  "[steer] commands: /auto /stop /complete /cancel /new /fork /q".to_string(),
                );
                return Ok(Self::Idle {
                  wait_for_input: true,
                });
              }
              SteerAction::Continue => {}
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
        if ctx.max_turns > 0 && ctx.turn > ctx.max_turns {
          tui.log.push(format!(
            "[steer] reached max turns ({}); exiting cleanly. Resume with ogent --resume.",
            ctx.max_turns
          ));
          return Ok(Self::Exit(agent.messages.clone()));
        }

        agent.meta.turn = ctx.turn;
        tui
          .status
          .set_turn_tokens(ctx.turn, agent.total_prompt + agent.total_completion);
        tui.log.push(format!("--- turn {} ---", ctx.turn));
        agent.push_turn_budget_reminder(ctx.max_turns, ctx.turn);
        agent.refresh_workflow_reminder();

        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel.clone();
        let client = agent.client.clone();
        let messages = agent.messages.clone();
        let tools = agent.tools.clone();
        let chat =
          tokio::spawn(async move { client.chat(&messages, &tools, Some(&cancel_clone)).await });

        Ok(Self::InFlight {
          chat,
          cancel,
          cancelled: false,
          steer_msg: None,
        })
      }

      Self::InFlight {
        mut chat,
        cancel,
        mut cancelled,
        mut steer_msg,
      } => {
        let chat_result = 'select: loop {
          tokio::select! {
            r = &mut chat => break 'select r,
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
                  agent.apply_steer_event(SteerEvent::New, &mut ctx.auto_continue, tui)?;
                  chat.abort();
                  ctx.turn = 1;
                  tui.log.push("[steer] commands: /auto /stop /complete /cancel /new /fork /q".to_string());
                  return Ok(Self::Idle { wait_for_input: true });
                }
                SteerEvent::Exit => {
                  cancel.cancel();
                  chat.abort();
                  return Ok(Self::Exit(agent.messages.clone()));
                }
                other => {
                  match agent.apply_steer_event(other, &mut ctx.auto_continue, tui)? {
                    SteerAction::Exit => {
                      cancel.cancel();
                      chat.abort();
                      return Ok(Self::Exit(agent.messages.clone()));
                    }
                    SteerAction::Restart => {
                      cancel.cancel();
                      chat.abort();
                      ctx.turn = 1;
                      tui.log.push("[steer] commands: /auto /stop /complete /cancel /new /fork /q".to_string());
                      return Ok(Self::Idle { wait_for_input: true });
                    }
                    SteerAction::Continue => {}
                  }
                }
              }
            }
          }
        };

        let resp = match chat_result {
          Ok(Ok(resp)) => resp,
          Ok(Err(ClientError::Aborted { resp })) => {
            if !resp.content.is_empty()
              || !resp.reasoning_content.is_empty()
              || !resp.tool_calls.is_empty()
            {
              agent.total_prompt += resp.usage.prompt_tokens;
              agent.total_completion += resp.usage.completion_tokens;
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
              ctx.turn += 1;
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
          .handle_turn_response_with_log(resp, Some(&tui.log))
          .await?;
        Ok(Self::FinishTurn { has_more })
      }

      Self::FinishTurn { mut has_more } => {
        if agent.completion_summary.is_some()
          && !agent.compact.last_handoff_path.is_empty()
          && agent.handle_handoff().await?
        {
          return Ok(Self::Exit(agent.messages.clone()));
        }
        if agent.completion_summary.is_some() {
          tui
            .log
            .push("[steer] task complete; send a message to continue or /q to quit".to_string());
          agent.completion_summary = None;
          return Ok(Self::Idle {
            wait_for_input: true,
          });
        }

        let should_exit = agent
          .finish_turn(
            &mut has_more,
            ctx.auto_continue,
            Some(&tui.log),
            ctx.max_turns,
            ctx.turn,
          )
          .await?;

        if should_exit {
          return Ok(Self::Exit(agent.messages.clone()));
        }

        if !has_more && !ctx.auto_continue {
          ctx.turn += 1;
          return Ok(Self::Idle {
            wait_for_input: true,
          });
        }

        ctx.turn += 1;
        Ok(Self::StartTurn)
      }
    }
  }
}

#[derive(Debug, Clone)]
pub struct CompactState {
  pub threshold: f64,
  pub exit_after: bool,
  pub context_limit: usize,
  pub compacting: bool,
  pub urgency: usize,
  pub last_handoff_path: String,
}

impl CompactState {
  pub fn disabled() -> Self {
    Self {
      threshold: -1.0,
      exit_after: false,
      context_limit: 0,
      compacting: false,
      urgency: 0,
      last_handoff_path: String::new(),
    }
  }

  pub fn new(threshold: f64, exit_after: bool, context_limit: usize) -> Self {
    Self {
      threshold,
      exit_after,
      context_limit,
      compacting: false,
      urgency: 0,
      last_handoff_path: String::new(),
    }
  }
}

pub struct Agent {
  pub client: Client,
  pub messages: Vec<Message>,
  pub tools: Vec<Tool>,
  pub worker_manager: WorkerManager,
  pub total_prompt: i32,
  pub total_completion: i32,
  pub compact: CompactState,
  pub completion_summary: Option<String>,
  pub task_tracker: Option<TaskTracker>,
  pub workflow_state: Option<crate::workflow::WorkflowState>,
  pub complete_open_work_warned: bool,
  last_turn_budget_reminder_turn: Option<i32>,
  pub meta: session::SessionMeta,
  pub dirty: bool,
  pub next_turn_reset: bool,
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
      total_prompt: 0,
      total_completion: 0,
      compact,
      completion_summary: None,
      task_tracker,
      workflow_state,
      complete_open_work_warned: false,
      last_turn_budget_reminder_turn: None,
      meta,
      dirty: false,
      next_turn_reset: false,
    }
  }

  fn push_msg(&mut self, msg: Message) {
    self.dirty = true;
    self.messages.push(msg);
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
        let _ = std::fmt::Write::write_fmt(
          &mut first.content,
          format_args!("{WORKFLOW_MARKER}\n{reminder}"),
        );
      }
    }
  }

  pub async fn run_loop(
    &mut self,
    max_turns: i32,
    auto_continue: bool,
  ) -> Result<Vec<Message>, AgentError> {
    let mut turn = 1;
    loop {
      self.meta.turn = turn;
      if max_turns > 0 && turn > max_turns {
        self.report_tokens();
        eprintln!("\nReached max turns ({max_turns}). Session saved. Resume with ogent --resume.");
        return Ok(self.messages.clone());
      }
      eprintln!(
        "\n--- turn {turn} | tokens: {} ---",
        self.total_prompt + self.total_completion
      );
      self.push_turn_budget_reminder(max_turns, turn);
      self.refresh_workflow_reminder();
      let resp = self.client.chat(&self.messages, &self.tools, None).await?;

      let mut has_more = match self.handle_turn_response(resp).await {
        Ok(hm) => hm,
        Err(AgentError::InteractiveRequired) => return Ok(self.messages.clone()),
        Err(e) => return Err(e),
      };
      if self
        .finish_turn(&mut has_more, auto_continue, None, max_turns, turn)
        .await?
      {
        return Ok(self.messages.clone());
      }
      if !has_more {
        return Ok(self.messages.clone());
      }
      turn += 1;
    }
  }

  pub async fn steer_loop(
    &mut self,
    max_turns: i32,
    auto_continue: bool,
    mut tui: TuiHandle,
    wait_for_input: bool,
  ) -> Result<Vec<Message>, AgentError> {
    tui
      .log
      .push("[steer] commands: /auto /stop /complete /cancel /new /fork /q");
    let mut state = SteerState::Idle { wait_for_input };
    let mut ctx = SteerCtx {
      turn: 1,
      max_turns,
      auto_continue,
    };

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
    auto_continue: bool,
    ui_log: Option<&crate::tui::UiLog>,
    max_turns: i32,
    turn: i32,
  ) -> Result<bool, AgentError> {
    if !self.compact.last_handoff_path.is_empty() {
      if self.handle_handoff().await? {
        return Ok(true);
      }
      *has_more = true;
    }
    if self.completion_summary.is_some() {
      return Ok(true);
    }
    let mut pushed_worker_status = false;
    if let Some(msg) = self.worker_manager.status_message().await {
      if let Some(log) = ui_log {
        self.push_msg(user_msg(msg.clone()));
        log.push(format!("[workers] {}", truncate(&msg, 200)));
      } else {
        self.push_msg(user_msg(msg));
      }
      pushed_worker_status = true;
      *has_more = true;
    }
    if *has_more {
      self.check_compact();
      self.push_task_tracking_reminder();
      self.push_turn_budget_reminder(max_turns, turn + 1);
      if auto_continue && !self.compact.compacting && !pushed_worker_status {
        self.push_msg(user_msg(AUTO_CONTINUE_REMINDER.to_string()));
      }
    }
    Ok(false)
  }

  fn apply_steer_event(
    &mut self,
    event: SteerEvent,
    auto_continue: &mut bool,
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
      SteerEvent::Auto => {
        *auto_continue = true;
        tui.status.set_auto(true);
        tui.log.push("[steer] auto on");
      }
      SteerEvent::Stop => {
        *auto_continue = false;
        tui.status.set_auto(false);
        tui.log.push("[steer] auto off");
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
        if self.dirty {
          self.meta.usage.prompt_tokens = self.total_prompt;
          self.meta.usage.completion_tokens = self.total_completion;
          session::write_meta(&self.meta)?;
          session::persist_session(&self.messages, &self.meta.session_id)?;
        }
        let old_id = self.meta.session_id.clone();
        self.meta.session_id = session::generate_session_id();
        self.meta.parent_session = Some(old_id);
        self.meta.turn = 0;
        self.meta.usage = session::SessionUsage {
          prompt_tokens: 0,
          completion_tokens: 0,
        };
        self.meta.prompt = None;
        self.meta.start_ts = None;
        self.meta.end_ts = None;
        let mut messages = crate::prompts::build_messages("");
        let workflow_state = crate::prompts::enrich_initial_messages(&mut messages);
        self.messages = messages;
        self.dirty = false;
        self.workflow_state = workflow_state;
        self.total_prompt = 0;
        self.total_completion = 0;
        self.worker_manager = WorkerManager::new();
        self.completion_summary = None;
        self.complete_open_work_warned = false;
        self.last_turn_budget_reminder_turn = None;
        self.compact.last_handoff_path = String::new();
        self.compact.compacting = false;
        self.compact.urgency = 0;
        tui.log.clear();
        tui.status.set_turn_tokens(0, 0);
        tui.log.push("[steer] new session started");
        return Ok(SteerAction::Restart);
      }
      SteerEvent::Fork => {
        if !self.dirty {
          tui.log.push("[steer] nothing to fork; session is empty");
        } else {
          self.meta.usage.prompt_tokens = self.total_prompt;
          self.meta.usage.completion_tokens = self.total_completion;
          session::write_meta(&self.meta)?;
          session::persist_session(&self.messages, &self.meta.session_id)?;
          let parent_id = self.meta.session_id.clone();
          self.meta.session_id = session::generate_session_id();
          self.meta.parent_session = Some(parent_id.clone());
          self.meta.start_ts = Some(session::timestamp_ms());
          self.meta.end_ts = None;
          self.next_turn_reset = true;
          self.dirty = true;
          session::write_meta(&self.meta)?;
          session::persist_session(&self.messages, &self.meta.session_id)?;
          tui.log.push(format!(
            "[steer] forked to {}; parent is {}. Resume parent with --resume-session {}",
            self.meta.session_id, parent_id, parent_id
          ));
        }
      }
      SteerEvent::Exit => return Ok(SteerAction::Exit),
    }
    Ok(SteerAction::Continue)
  }

  async fn handle_turn_response(&mut self, resp: ChatResponse) -> Result<bool, AgentError> {
    self.handle_turn_response_with_log(resp, None).await
  }

  async fn handle_turn_response_with_log(
    &mut self,
    resp: ChatResponse,
    ui_log: Option<&crate::tui::UiLog>,
  ) -> Result<bool, AgentError> {
    self.meta.end_ts = Some(session::timestamp_ms());
    self.total_prompt += resp.usage.prompt_tokens;
    self.total_completion += resp.usage.completion_tokens;
    if !resp.reasoning_content.is_empty() {
      if let Some(log) = ui_log {
        log.push(format!(
          "reasoning: {}",
          truncate(&resp.reasoning_content, 300)
        ));
      } else {
        eprintln!("reasoning: {}", truncate(&resp.reasoning_content, 300));
      }
    }
    if !resp.content.is_empty() {
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
      let is_interactive = output == INTERACTIVE_ERR;
      results.push(ToolResult {
        name: tc.function.name.clone(),
        args: tc.function.arguments.clone(),
        output,
        success,
      });
      if is_interactive {
        return Err(AgentError::InteractiveRequired);
      }
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
    let (output, success, is_interactive) = format_tool_result(
      execute_tool(
        ToolContext { agent: Some(self) },
        &tc.function.name,
        &tc.function.arguments,
      )
      .await,
    );
    if is_interactive {
      return (INTERACTIVE_ERR.to_string(), false);
    }
    (output, success)
  }

  fn check_compact(&mut self) {
    if self.compact.threshold < 0.0 || self.compact.context_limit == 0 {
      return;
    }
    let total = (self.total_prompt + self.total_completion) as usize;
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
        "Context budget at {pct}%.\nFinish the current chunk. Do not start unrelated work.\nIf useful state may be lost, write a checkpoint before continuing.\nIf between chunks, call `handoff`."
      ),
      2 => format!(
        "Context budget at {pct}%.\nApproaching the limit. Finish only critical in-progress work.\nDo not delegate new work.\nWrite a checkpoint if it will preserve important state, then call `handoff` as soon as possible."
      ),
      _ => format!(
        "Context budget at {pct}%.\nEXHAUSTED.\nDo not write more files, delegate, or start new work.\nCall `handoff` IMMEDIATELY with completed files, current state, verification state, blockers, and next steps."
      ),
    };
    self.push_msg(user_msg(format!(
      "<system_reminder urgency=\"{}\" kind=\"context_budget\">\n{body}\n</system_reminder>",
      self.compact.urgency
    )));
  }

  async fn handle_handoff(&mut self) -> Result<bool, AgentError> {
    let path = std::mem::take(&mut self.compact.last_handoff_path);
    if self.compact.exit_after {
      self.meta.usage.prompt_tokens = self.total_prompt;
      self.meta.usage.completion_tokens = self.total_completion;
      session::write_meta(&self.meta)?;
      eprintln!("\nHandoff written to {path}");
      return Ok(true);
    }
    let data = tokio::fs::read_to_string(&path)
      .await
      .unwrap_or_else(|_| "(handoff read error)".into());
    if let Some(mut tracker) = TaskTracker::from_handoff_text(&data) {
      tracker.mark_restored();
      self.task_tracker = Some(tracker);
    }
    let stripped = TaskTracker::strip_handoff_state_block(&data);
    let system = self
      .messages
      .first()
      .filter(|m| m.role == "system")
      .map(|m| m.content.clone())
      .unwrap_or_default();
    let old_messages = std::mem::take(&mut self.messages);
    self.messages = vec![
      system_msg(system),
      user_msg(format!(
        "## Previous Session Handoff\n\n{stripped}\n\nPlease process this handoff brief and continue the work."
      )),
    ];
    self.dirty = false;
    self.meta.usage.prompt_tokens = self.total_prompt;
    self.meta.usage.completion_tokens = self.total_completion;
    session::write_meta(&self.meta)?;
    session::persist_session(&old_messages, &self.meta.session_id)?;
    let parent_id = self.meta.session_id.clone();
    self.meta.session_id = session::generate_session_id();
    self.meta.parent_session = Some(parent_id);
    self.meta.turn = 0;
    self.meta.usage = session::SessionUsage {
      prompt_tokens: 0,
      completion_tokens: 0,
    };
    self.meta.prompt = None;
    self.meta.start_ts = None;
    self.meta.end_ts = None;
    session::write_meta(&self.meta)?;
    self.push_task_tracking_reminder();
    self.compact.compacting = false;
    self.compact.urgency = 0;
    self.total_prompt = 0;
    self.total_completion = 0;
    Ok(false)
  }

  fn report_tokens(&self) {
    eprintln!(
      "\n\ntokens: prompt={} completion={} total={}",
      self.total_prompt,
      self.total_completion,
      self.total_prompt + self.total_completion
    );
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

  fn push_turn_budget_reminder(&mut self, max_turns: i32, turn: i32) {
    if self.last_turn_budget_reminder_turn == Some(turn) {
      return;
    }
    if let Some(reminder) = turn_budget_reminder(max_turns, turn) {
      self.push_msg(user_msg(reminder));
      self.last_turn_budget_reminder_turn = Some(turn);
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

fn system_msg(content: impl Into<String>) -> Message {
  Message {
    role: "system".into(),
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

const INTERACTIVE_ERR: &str = "ERROR: interactive mode required";

fn format_tool_result(result: anyhow::Result<String>) -> (String, bool, bool) {
  match result {
    Ok(out) => (out, true, false),
    Err(e) if e.to_string() == "interactive mode required" => {
      (INTERACTIVE_ERR.to_string(), false, true)
    }
    Err(e) => (format!("ERROR: {e}"), false, false),
  }
}

async fn run_read_only_batch(batch: &[&ToolCall]) -> Result<Vec<ToolResult>, AgentError> {
  let futs = batch.iter().map(|tc| async {
    let (output, success, _) = format_tool_result(
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
  if results.iter().any(|r| r.output == INTERACTIVE_ERR) {
    return Err(AgentError::InteractiveRequired);
  }
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

const AUTO_CONTINUE_REMINDER: &str = r#"<system_reminder kind="auto_continue">
Auto mode is enabled. Prefer action over extended analysis. Continue only if useful work remains.

Before continuing:
- Re-check the current goal, latest tool results, worker status, and context budget.
- If no useful work remains, call `complete` with a retrospective structured Markdown summary.
- If the next step is clear, proceed. If unclear on low-risk work, make your best call and proceed.
- Destructive, irreversible, or shared-system actions (force push, deleting branches, messaging, pushing to shared infra) still require user confirmation. Auto mode is not a license to destroy.
- If a command or edit fails, inspect the failure and make one focused retry when justified.
- If blocked by missing expertise, uncertainty, or parallelizable review, dispatch a scoped worker with exact paths, evidence, success criteria, and expected summary format.
- If context is getting large, write a checkpoint for yourself and prefer finishing the current chunk over starting new work.
- If continuation would be speculative or unsafe, call `complete` with the current state and limitation.
</system_reminder>"#;

const MANUAL_COMPLETE_REMINDER: &str = r#"<system_reminder kind="manual_complete">
The user requested completion from steer mode.

Summarize the current session retrospectively and call `complete` with structured Markdown:
- task summary
- what changed / what you did
- what you learned
- what to do better next time
- optional evidence: files touched, tests run, git head
</system_reminder>"#;

fn turn_budget_reminder(max_turns: i32, turn: i32) -> Option<String> {
  if max_turns <= 0 || turn <= 0 || turn > max_turns {
    return None;
  }
  let remaining = max_turns - turn + 1;

  let msg = match remaining {
    1 => {
      "This is the FINAL turn. If the task is done, call `complete`. Otherwise call `handoff` for the human to review and resume. Do not call tools that require follow-up verification."
    }
    2 => {
      "Two turns left. Do not start new work. Call `complete` if done, or `handoff` for the human to resume."
    }
    3 => {
      "Three turns left. If done, call `complete`. Otherwise finish the current chunk and prepare to `handoff` for human review and resume."
    }
    _ if max_turns >= 10 && remaining == max_turns / 2 => {
      "Half the turn budget is used. If useful work is parallelizable and delegatable, delegate coworkers now. Keep the critical path local."
    }
    _ if max_turns >= 10 && remaining == max_turns / 4 && max_turns / 4 >= 5 => {
      "Three-quarters of the turn budget is used. Focus on verification, tracking updates, completion, or a necessary handoff. Avoid new exploratory delegation."
    }
    _ if turn == 1 => {
      "Use turns deliberately. If useful work is parallelizable and delegatable, delegate coworkers now while keeping the critical path local."
    }
    _ => return None,
  };

  Some(format!(
    "<system_reminder kind=\"turn_budget\">\nYou are on turn {turn} of {max_turns}. {remaining} turn{} remain including this one.\n{msg}\n</system_reminder>",
    if remaining == 1 { "" } else { "s" }
  ))
}

#[cfg(test)]
mod turn_budget_tests {
  use super::turn_budget_reminder;

  #[test]
  fn turn_budget_emits_first_turn() {
    let r = turn_budget_reminder(20, 1).expect("first turn should be shown");
    assert!(r.contains("turn 1 of 20"));
    assert!(r.contains("Use turns deliberately"));
  }

  #[test]
  fn turn_budget_fires_50_percent() {
    let r = turn_budget_reminder(20, 11).expect("50% should fire");
    assert!(r.contains("Half the turn budget"));
    assert!(r.contains("delegate coworkers now"));
  }

  #[test]
  fn turn_budget_fires_25_percent() {
    let r = turn_budget_reminder(20, 16).expect("25% should fire");
    assert!(r.contains("Three-quarters of the turn budget"));
  }

  #[test]
  fn turn_budget_skips_mid_turns() {
    assert!(turn_budget_reminder(20, 10).is_none());
    assert!(turn_budget_reminder(20, 12).is_none());
    assert!(turn_budget_reminder(20, 14).is_none());
  }

  #[test]
  fn turn_budget_fires_3_2_1() {
    assert!(turn_budget_reminder(20, 18).is_some());
    assert!(turn_budget_reminder(20, 19).is_some());
    assert!(turn_budget_reminder(20, 20).is_some());
  }

  #[test]
  fn turn_budget_final_turn_is_explicit() {
    let r = turn_budget_reminder(3, 3).expect("final turn should be shown");
    assert!(r.contains("FINAL turn"));
    assert!(r.contains("handoff"));
  }

  #[test]
  fn turn_budget_two_left() {
    let r = turn_budget_reminder(5, 4).expect("two-left should fire");
    assert!(r.contains("Two turns left"));
    assert!(r.contains("handoff"));
  }

  #[test]
  fn turn_budget_ignores_unbounded_runs() {
    assert!(turn_budget_reminder(-1, 1).is_none());
    assert!(turn_budget_reminder(0, 1).is_none());
  }

  #[test]
  fn turn_budget_small_budget_no_percentages() {
    assert!(turn_budget_reminder(8, 4).is_none()); // would be 50% at remaining=4
    assert!(turn_budget_reminder(8, 6).is_some()); // remaining=3
    assert!(turn_budget_reminder(8, 8).is_some()); // remaining=1
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
      0,
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
      max_turns: -1,
      turn: 0,
      flags: session::SessionFlags {
        steer: true,
        auto: false,
        worker: false,
        autocompact: -1,
        handoff: false,
        retry: 0,
        continue_flag: false,
        resume: false,
      },
      usage: session::SessionUsage {
        prompt_tokens: 0,
        completion_tokens: 0,
      },
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
      .apply_steer_event(SteerEvent::Message("fix bug".into()), &mut false, &tui)
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
      .apply_steer_event(SteerEvent::Message("fix bug".into()), &mut false, &tui)
      .unwrap();
    let start_ts = agent.meta.start_ts;
    agent
      .apply_steer_event(SteerEvent::Message("more context".into()), &mut false, &tui)
      .unwrap();
    assert_eq!(agent.meta.prompt, Some("fix bug".into()));
    assert_eq!(agent.meta.start_ts, start_ts);
  }

  #[tokio::test]
  async fn auto_on_clean_stays_clean() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    let mut auto = false;
    let action = agent
      .apply_steer_event(SteerEvent::Auto, &mut auto, &tui)
      .unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(!agent.dirty);
    assert!(auto);
  }

  #[tokio::test]
  async fn stop_on_dirty_stays_dirty() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    let mut auto = true;
    agent.push_msg(user_msg("hello"));
    let action = agent
      .apply_steer_event(SteerEvent::Stop, &mut auto, &tui)
      .unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(agent.dirty);
    assert!(!auto);
  }

  #[tokio::test]
  async fn cancel_does_not_change_dirty() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    let action = agent
      .apply_steer_event(SteerEvent::Cancel, &mut false, &tui)
      .unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(!agent.dirty);
  }

  #[tokio::test]
  async fn complete_on_empty_session_stays_clean() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    let action = agent
      .apply_steer_event(SteerEvent::Complete, &mut false, &tui)
      .unwrap();
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
    let action = agent
      .apply_steer_event(SteerEvent::Complete, &mut false, &tui)
      .unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(agent.dirty);
    assert_eq!(agent.messages.len(), 4); // system + user + assistant + complete reminder
  }

  #[tokio::test]
  async fn exit_returns_exit_action() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    let action = agent
      .apply_steer_event(SteerEvent::Exit, &mut false, &tui)
      .unwrap();
    assert!(matches!(action, SteerAction::Exit));
  }

  #[tokio::test]
  async fn new_on_clean_resets_without_files() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    let old_id = agent.meta.session_id.clone();
    let action = agent
      .apply_steer_event(SteerEvent::New, &mut false, &tui)
      .unwrap();
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

    let action = agent
      .apply_steer_event(SteerEvent::New, &mut false, &tui)
      .unwrap();
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
      usage: crate::types::Usage {
        prompt_tokens: 10,
        completion_tokens: 5,
        total_tokens: 15,
      },
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
      agent.handle_turn_response(resp).await.unwrap();
    });
    assert!(agent.meta.end_ts.is_some());
    assert!(agent.dirty);
  }

  #[tokio::test]
  async fn handoff_normal_persists_old_and_resets_meta() {
    let mut agent = dummy_agent();
    agent.push_msg(user_msg("hello"));
    agent.meta.prompt = Some("test prompt".into());
    agent.meta.start_ts = Some(1000);
    agent.meta.end_ts = Some(2000);
    let old_id = agent.meta.session_id.clone();

    let handoff_path = ".ogent/handoffs/test-handoff.md";
    std::fs::create_dir_all(".ogent/handoffs").ok();
    std::fs::write(handoff_path, "# Handoff\n\nstate: done\n").unwrap();
    agent.compact.last_handoff_path = handoff_path.into();
    agent.compact.exit_after = false;

    let should_exit = agent.handle_handoff().await.unwrap();
    assert!(!should_exit);

    // old session persisted
    let old_dir = session::session_dir(&old_id);
    assert!(old_dir.join("meta.json").exists());
    assert!(old_dir.join("messages.jsonl").exists());

    // new session is clean with reset meta
    assert!(!agent.dirty);
    assert_eq!(agent.meta.prompt, None);
    assert_eq!(agent.meta.start_ts, None);
    assert_eq!(agent.meta.end_ts, None);
    assert_eq!(agent.meta.parent_session, Some(old_id.clone()));
    assert_ne!(agent.meta.session_id, old_id);
    assert_eq!(agent.messages.len(), 2); // system + handoff user

    // clean up
    let _ = std::fs::remove_dir_all(&old_dir);
    let _ = std::fs::remove_file(handoff_path);
  }

  #[tokio::test]
  async fn handoff_exit_after_writes_meta_and_returns_true() {
    let mut agent = dummy_agent();
    agent.push_msg(user_msg("hello"));
    agent.total_prompt = 100;
    agent.total_completion = 50;
    let old_id = agent.meta.session_id.clone();

    let handoff_path = ".ogent/handoffs/test-handoff-exit.md";
    std::fs::create_dir_all(".ogent/handoffs").ok();
    std::fs::write(handoff_path, "# Handoff\n").unwrap();
    agent.compact.last_handoff_path = handoff_path.into();
    agent.compact.exit_after = true;

    let should_exit = agent.handle_handoff().await.unwrap();
    assert!(should_exit);

    // session persisted with updated usage
    let dir = session::session_dir(&old_id);
    assert!(dir.join("meta.json").exists());
    let meta = session::read_meta(&old_id).unwrap();
    assert_eq!(meta.usage.prompt_tokens, 100);
    assert_eq!(meta.usage.completion_tokens, 50);

    // meta fields preserved (not a new session)
    assert_eq!(meta.prompt, None); // dummy_agent starts with None
    assert_eq!(agent.meta.session_id, old_id);
    assert!(agent.dirty); // still dirty from before

    // clean up
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(handoff_path);
  }

  #[tokio::test]
  async fn fork_on_empty_is_noop() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    let old_id = agent.meta.session_id.clone();
    let action = agent
      .apply_steer_event(SteerEvent::Fork, &mut false, &tui)
      .unwrap();
    assert!(matches!(action, SteerAction::Continue));
    assert!(!agent.dirty);
    assert_eq!(agent.meta.session_id, old_id);
    assert_eq!(agent.meta.parent_session, None);
    assert!(!agent.next_turn_reset);
  }

  #[tokio::test]
  async fn fork_persists_parent_and_creates_fork() {
    let mut agent = dummy_agent();
    let tui = crate::tui::TuiHandle::test_handle();
    agent.push_msg(user_msg("hello"));
    agent.meta.prompt = Some("test prompt".into());
    agent.meta.start_ts = Some(1000);
    agent.meta.end_ts = Some(2000);
    let parent_id = agent.meta.session_id.clone();

    let action = agent
      .apply_steer_event(SteerEvent::Fork, &mut false, &tui)
      .unwrap();
    assert!(matches!(action, SteerAction::Continue));

    // parent persisted
    let parent_dir = session::session_dir(&parent_id);
    assert!(parent_dir.join("meta.json").exists());
    assert!(parent_dir.join("messages.jsonl").exists());

    // fork has new identity
    assert!(agent.dirty);
    assert!(agent.next_turn_reset);
    assert_ne!(agent.meta.session_id, parent_id);
    assert_eq!(agent.meta.parent_session, Some(parent_id.clone()));
    assert_eq!(agent.meta.prompt, Some("test prompt".into()));
    assert!(agent.meta.start_ts.unwrap() >= 1000); // fork time >= original start
    assert_eq!(agent.meta.end_ts, None);
    assert_eq!(agent.messages.len(), 3); // system + initial user + "hello"

    // fork persisted immediately
    let fork_dir = session::session_dir(&agent.meta.session_id);
    assert!(fork_dir.join("meta.json").exists());
    assert!(fork_dir.join("messages.jsonl").exists());

    // clean up
    let _ = std::fs::remove_dir_all(&parent_dir);
    let _ = std::fs::remove_dir_all(&fork_dir);
  }
}
