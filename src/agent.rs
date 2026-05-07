use anyhow::{Result, bail};

use crate::client::Client;
use crate::tools::{ToolContext, execute_tool, is_read_only_tool, remove_question};
use crate::tui::{SteerEvent, TuiHandle};
use crate::types::{ChatResponse, Message, Tool, ToolCall};
use crate::workers::WorkerManager;

#[derive(Debug, thiserror::Error)]
#[error("interactive mode required")]
struct InteractiveRequiredError;

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
}

pub struct ToolResult {
  name: String,
  args: String,
  output: String,
}

impl Agent {
  pub fn new(
    client: Client,
    messages: Vec<Message>,
    tools: Vec<Tool>,
    compact: CompactState,
  ) -> Self {
    Self {
      client,
      messages,
      tools,
      worker_manager: WorkerManager::new(),
      total_prompt: 0,
      total_completion: 0,
      compact,
    }
  }

  pub async fn run_loop(
    &mut self,
    max_turns: i32,
    question_available_on_first_turn: bool,
    auto_continue: bool,
  ) -> Result<Vec<Message>> {
    let mut turn = 1;
    loop {
      if max_turns > 0 && turn > max_turns {
        self.report_tokens();
        bail!("exceeded max turns ({max_turns})");
      }
      eprintln!(
        "\n--- turn {turn} | tokens: {} ---",
        self.total_prompt + self.total_completion
      );
      let resp = self.client.chat(&self.messages, &self.tools, None).await?;
      let mut has_more = match self.handle_turn_response(resp).await {
        Ok(hm) => hm,
        Err(e) if e.is::<InteractiveRequiredError>() => return Ok(self.messages.clone()),
        Err(e) => return Err(e),
      };
      if self.finish_turn(&mut has_more, auto_continue, None).await? {
        return Ok(self.messages.clone());
      }
      if turn == 1 && question_available_on_first_turn {
        remove_question(&mut self.tools);
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
    mut auto_continue: bool,
    mut tui: TuiHandle,
    mut wait_for_input: bool,
  ) -> Result<Vec<Message>> {
    tui.log.push("[steer] commands: /auto /stop /cancel /q");
    if wait_for_input {
      tui.log.push("[steer] waiting for your first message");
    }
    let mut turn = 1;
    loop {
      let wait_baseline_len = self.messages.len();
      while let Ok(event) = tui.rx.try_recv() {
        if self
          .apply_steer_event(event, &mut auto_continue, &tui)
          .await?
        {
          return Ok(self.messages.clone());
        }
        if self.messages.len() > wait_baseline_len
          && matches!(self.messages.last().map(|m| m.role.as_str()), Some("user"))
        {
          wait_for_input = false;
        }
      }

      while wait_for_input {
        let Some(event) = tui.rx.recv().await else {
          continue;
        };
        if self
          .apply_steer_event(event, &mut auto_continue, &tui)
          .await?
        {
          return Ok(self.messages.clone());
        }
        if self.messages.len() > wait_baseline_len
          && matches!(self.messages.last().map(|m| m.role.as_str()), Some("user"))
        {
          wait_for_input = false;
        }
      }

      if max_turns > 0 && turn > max_turns {
        tui.log.push(format!(
          "[steer] reached max turns ({max_turns}); exiting cleanly"
        ));
        return Ok(self.messages.clone());
      }
      tui
        .status
        .set_turn_tokens(turn, self.total_prompt + self.total_completion);
      tui.log.push(format!("--- turn {turn} ---"));

      let cancel = tokio_util::sync::CancellationToken::new();
      let client = self.client.clone();
      let messages = self.messages.clone();
      let tools = self.tools.clone();
      let chat_cancel = cancel.clone();
      let mut chat =
        tokio::spawn(async move { client.chat(&messages, &tools, Some(&chat_cancel)).await });
      let mut cancelled_turn = false;
      let mut steer_msg: Option<String> = None;

      let chat_result = 'chat: loop {
        tokio::select! {
          r = &mut chat => break 'chat r,
          maybe_event = tui.rx.recv(), if !cancelled_turn && steer_msg.is_none() => {
            let Some(event) = maybe_event else { continue; };
            match event {
              SteerEvent::Cancel => {
                cancel.cancel();
                cancelled_turn = true;
              }
              SteerEvent::Message(content) => {
                cancel.cancel();
                steer_msg = Some(content);
              }
              SteerEvent::Exit => {
                cancel.cancel();
                break 'chat chat.await;
              }
              other => {
                if self.apply_steer_event(other, &mut auto_continue, &tui).await? {
                  cancel.cancel();
                  break 'chat chat.await;
                }
              }
            }
          }
        }
      };

      let resp = match chat_result {
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => {
          if let Some(aborted) = e.downcast_ref::<crate::types::ChatAbortedError>() {
            let resp = aborted.resp.clone();
            if !resp.content.is_empty()
              || !resp.reasoning_content.is_empty()
              || !resp.tool_calls.is_empty()
            {
              self.total_prompt += resp.usage.prompt_tokens;
              self.total_completion += resp.usage.completion_tokens;
              self.messages.push(Message {
                role: "assistant".into(),
                content: resp.content.clone(),
                reasoning_content: resp.reasoning_content.clone(),
                tool_calls: resp.tool_calls.clone(),
                ..Default::default()
              });
            }
            if cancelled_turn {
              wait_for_input = true;
              continue;
            }
            if let Some(msg) = steer_msg {
              self.messages.push(Message {
                role: "user".into(),
                content: msg.clone(),
                ..Default::default()
              });
              tui.log.push(format!("[steer] {}", truncate(&msg, 200)));
              turn += 1;
              continue;
            }
            return Ok(self.messages.clone());
          }
          return Err(e);
        }
        Err(join_err) => return Err(join_err.into()),
      };

      let mut has_more = self
        .handle_turn_response_with_log(resp, Some(&tui.log))
        .await?;
      if self
        .finish_turn(&mut has_more, auto_continue, Some(&tui.log))
        .await?
      {
        return Ok(self.messages.clone());
      }
      if !has_more && !auto_continue {
        tui.log.push("[steer] turn complete; waiting for input");
        turn += 1;
        wait_for_input = true;
        continue;
      }
      turn += 1;
    }
  }

  async fn finish_turn(
    &mut self,
    has_more: &mut bool,
    auto_continue: bool,
    ui_log: Option<&crate::tui::UiLog>,
  ) -> Result<bool> {
    if !self.compact.last_handoff_path.is_empty() {
      if self.handle_handoff().await? {
        return Ok(true);
      }
      *has_more = true;
    }
    if let Some(msg) = self.worker_manager.status_message().await {
      if let Some(log) = ui_log {
        self.messages.push(Message {
          role: "user".into(),
          content: msg.clone(),
          ..Default::default()
        });
        log.push(format!("[workers] {}", truncate(&msg, 200)));
      } else {
        self.messages.push(Message {
          role: "user".into(),
          content: msg,
          ..Default::default()
        });
      }
      *has_more = true;
    } else if *has_more {
      self.check_compact();
      if auto_continue && !self.compact.compacting {
        self.messages.push(Message {
          role: "user".into(),
          content: auto_continue_reminder(),
          ..Default::default()
        });
      }
    }
    Ok(false)
  }

  async fn apply_steer_event(
    &mut self,
    event: SteerEvent,
    auto_continue: &mut bool,
    tui: &TuiHandle,
  ) -> Result<bool> {
    match event {
      SteerEvent::Message(content) => {
        self.messages.push(Message {
          role: "user".into(),
          content: content.clone(),
          ..Default::default()
        });
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
      SteerEvent::Exit => return Ok(true),
    }
    Ok(false)
  }

  async fn handle_turn_response(&mut self, resp: ChatResponse) -> Result<bool> {
    self.handle_turn_response_with_log(resp, None).await
  }

  async fn handle_turn_response_with_log(
    &mut self,
    resp: ChatResponse,
    ui_log: Option<&crate::tui::UiLog>,
  ) -> Result<bool> {
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
      self.messages.push(Message {
        role: "assistant".into(),
        content: resp.content.clone(),
        reasoning_content: resp.reasoning_content,
        ..Default::default()
      });
      if ui_log.is_none() {
        print!("{}", resp.content);
        self.report_tokens();
      }
      return Ok(false);
    }

    let results = self.process_tool_calls(&resp).await?;
    for r in results {
      if let Some(log) = ui_log {
        log.push(format!("tool: {}({})", r.name, truncate(&r.args, 120)));
        log.push(format!("  => {}", truncate(&r.output, 200)));
      } else {
        eprintln!("tool: {}({})", r.name, truncate(&r.args, 120));
        eprintln!("  => {}", truncate(&r.output, 200));
      }
    }
    Ok(true)
  }

  async fn process_tool_calls(&mut self, resp: &ChatResponse) -> Result<Vec<ToolResult>> {
    self.messages.push(Message {
      role: "assistant".into(),
      content: resp.content.clone(),
      reasoning_content: resp.reasoning_content.clone(),
      tool_calls: resp.tool_calls.clone(),
      ..Default::default()
    });

    let mut results = Vec::with_capacity(resp.tool_calls.len());

    let mut i = 0;
    while i < resp.tool_calls.len() {
      if !is_read_only_tool(&resp.tool_calls[i].function.name) {
        let output = self.run_tool_call(&resp.tool_calls[i]).await;
        let is_interactive = output == "ERROR: interactive mode required";
        results.push(ToolResult {
          name: resp.tool_calls[i].function.name.clone(),
          args: resp.tool_calls[i].function.arguments.clone(),
          output,
        });
        if is_interactive {
          return Err(InteractiveRequiredError.into());
        }
        i += 1;
        continue;
      }

      let start = i;
      while i < resp.tool_calls.len() && is_read_only_tool(&resp.tool_calls[i].function.name) {
        i += 1;
      }

      let group = &resp.tool_calls[start..i];
      let futs = group.iter().map(|tc| {
        let name = tc.function.name.clone();
        let args = tc.function.arguments.clone();
        async move {
          let output = match execute_tool(ToolContext { agent: None }, &name, &args).await {
            Ok(out) => out,
            Err(e) if e.to_string() == "interactive mode required" => {
              "ERROR: interactive mode required".to_string()
            }
            Err(e) => format!("ERROR: {e}"),
          };
          ToolResult { name, args, output }
        }
      });
      let group_results = futures_util::future::join_all(futs).await;
      for r in group_results {
        if r.output == "ERROR: interactive mode required" {
          return Err(InteractiveRequiredError.into());
        }
        results.push(r);
      }
    }

    for (tc, r) in resp.tool_calls.iter().zip(results.iter()) {
      self.messages.push(Message {
        role: "tool".into(),
        tool_call_id: tc.id.clone(),
        content: r.output.clone(),
        ..Default::default()
      });
    }
    Ok(results)
  }

  async fn run_tool_call(&mut self, tc: &ToolCall) -> String {
    let _read_only = is_read_only_tool(&tc.function.name);
    match execute_tool(
      ToolContext { agent: Some(self) },
      &tc.function.name,
      &tc.function.arguments,
    )
    .await
    {
      Ok(out) => out,
      Err(e) if e.to_string() == "interactive mode required" => {
        "ERROR: interactive mode required".to_string()
      }
      Err(e) => format!("ERROR: {e}"),
    }
  }

  fn check_compact(&mut self) {
    if self.compact.threshold < 0.0 || self.compact.context_limit == 0 {
      self.compact.compacting = false;
      self.compact.urgency = 0;
      return;
    }
    let total = (self.total_prompt + self.total_completion) as usize;
    if total as f64 / (self.compact.context_limit as f64) < self.compact.threshold {
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
        "Context budget at {pct}%.\nApproaching the limit. Finish only critical in-progress work.\nWrite a checkpoint if it will preserve important state, then call `handoff` as soon as possible."
      ),
      _ => format!(
        "Context budget at {pct}%.\nEXHAUSTED.\nDo not write more files or start new work.\nCall `handoff` IMMEDIATELY with completed files, current state, verification state, blockers, and next steps."
      ),
    };
    self.messages.push(Message {
      role: "user".into(),
      content: format!(
        "<system_reminder urgency=\"{}\" kind=\"context_budget\">\n{}\n</system_reminder>",
        self.compact.urgency, body
      ),
      ..Default::default()
    });
  }

  async fn handle_handoff(&mut self) -> Result<bool> {
    let path = std::mem::take(&mut self.compact.last_handoff_path);
    if self.compact.exit_after {
      eprintln!("\nHandoff written to {path}");
      return Ok(true);
    }
    let data = tokio::fs::read_to_string(&path)
      .await
      .unwrap_or_else(|_| "(handoff read error)".into());
    let system = self
      .messages
      .first()
      .filter(|m| m.role == "system")
      .map(|m| m.content.clone())
      .unwrap_or_default();
    self.messages = vec![
      Message {
        role: "system".into(),
        content: system,
        ..Default::default()
      },
      Message {
        role: "user".into(),
        content: format!(
          "## Previous Session Handoff\n\n{data}\n\nPlease process this handoff brief and continue the work."
        ),
        ..Default::default()
      },
    ];
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
}

fn truncate(s: &str, n: usize) -> String {
  let escaped = s.replace('\n', "\\n");
  if escaped.len() <= n {
    escaped
  } else {
    format!("{}...", &escaped[..n])
  }
}

fn auto_continue_reminder() -> String {
  r#"<system_reminder kind="auto_continue">
Auto mode is enabled. Continue only if useful work remains.

Before continuing:
- Re-check the current goal, latest tool results, worker status, and context budget.
- If the next step is clear, proceed.
- If a command or edit fails, inspect the failure and make one focused retry when justified.
- If blocked by missing expertise, uncertainty, or parallelizable review, dispatch a scoped worker with exact paths, evidence, success criteria, and artifact path.
- If context is getting large, write a checkpoint for yourself and prefer finishing the current chunk over starting new work.
- If continuation would be speculative or unsafe, stop and report the current state.
</system_reminder>"#
    .to_string()
}
