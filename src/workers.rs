use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use tokio::sync::{Mutex, Notify};

const WORKER_PROGRESS_PROMPT_SUFFIX: &str = r#"## Progress Reporting

When your task requires more than one tool call, write concise current progress before each tool call using the `state` tool:
- `action`: `write`
- `path`: `progress/current`
- `content`: short factual status

Update this value when the phase changes. Keep it brief and factual. Examples: "reading parser", "defining trait", "refactoring call sites", "running tests". Skip this for trivial one-shot answers.

## Result Reporting

If your task specifies an output format, that format overrides your role's default output format.

When the Director asks for the standard worker result format, return:

```txt
Status: completed | blocked | partial

Summary:

Changed files:

Evidence:

Verification:

Risks:

Open questions:

Next action: accept | revise | verify | block
```"#;

#[derive(Clone)]
pub struct WorkerRunArgs {
  pub system_prompt: String,
  pub task_prompt: String,
  pub parent_session_id: String,
  pub worker_id: String,
  pub profile_name: String,
  pub workspace_root: PathBuf,
  pub progress_sink: Arc<std::sync::Mutex<String>>,
  pub output_sink: Option<Arc<dyn crate::agent::AgentOutputSink>>,
}

#[derive(Debug, Clone, Default)]
pub struct WorkerRunResult {
  pub output: String,
  pub err: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DispatchWorkersArgs {
  pub workers: Vec<WorkerDispatch>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkerDispatch {
  pub role: String,
  pub task: String,
}

#[derive(Debug, Clone, Serialize)]
struct DispatchWorkerResult {
  batch_id: String,
  index: usize,
  role: String,
  worker_id: String,
  status: String,
  output: String,
  error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DispatchBatchResult {
  message: String,
  batch_id: String,
  workers: Vec<WorkerStatus>,
  completed: Vec<DispatchWorkerResult>,
}

#[derive(Debug, Clone, Serialize)]
struct WaitWorkersResult {
  message: String,
  completed: Vec<DispatchWorkerResult>,
  running: Vec<WorkerStatus>,
}

#[derive(Debug, Clone, Serialize)]
struct WorkerStatus {
  batch_id: String,
  index: usize,
  role: String,
  worker_id: String,
  status: String,
  progress: String,
}

pub struct WorkerManager {
  workspace: crate::workspace::Workspace,
  inner: Mutex<Inner>,
  notify: Arc<Notify>,
  runner: WorkerRunner,
  output_sink: Option<Arc<dyn crate::agent::AgentOutputSink>>,
}

type WorkerRunFuture = Pin<Box<dyn Future<Output = WorkerRunResult> + Send>>;
type WorkerRunner = Arc<dyn Fn(WorkerRunArgs) -> WorkerRunFuture + Send + Sync>;

struct Inner {
  next_id: usize,
  next_batch_id: usize,
  in_flight: Vec<InFlightWorker>,
}

pub(crate) struct InFlightWorker {
  batch_id: String,
  index: usize,
  role: String,
  worker_id: String,
  pub(crate) done: tokio::task::JoinHandle<WorkerRunResult>,
  progress_sink: Arc<std::sync::Mutex<String>>,
}

impl WorkerManager {
  pub fn new(parent_session_id: Option<&str>, workspace: crate::workspace::Workspace) -> Self {
    Self {
      workspace: workspace.clone(),
      inner: Mutex::new(Inner {
        next_id: parent_session_id
          .map(|id| next_worker_counter(&workspace, id))
          .unwrap_or(0),
        next_batch_id: 0,
        in_flight: Vec::new(),
      }),
      notify: Arc::new(Notify::new()),
      runner: Arc::new(|args| Box::pin(run_worker_agent(args))),
      output_sink: None,
    }
  }

  pub fn set_output_sink(&mut self, output_sink: Option<Arc<dyn crate::agent::AgentOutputSink>>) {
    self.output_sink = output_sink;
  }

  #[cfg(test)]
  pub(crate) fn new_for_test<F, Fut>(runner: F) -> Self
  where
    F: Fn(WorkerRunArgs) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = WorkerRunResult> + Send + 'static,
  {
    Self {
      workspace: crate::workspace::Workspace::from_current_dir(),
      inner: Mutex::new(Inner {
        next_id: 0,
        next_batch_id: 0,
        in_flight: Vec::new(),
      }),
      notify: Arc::new(Notify::new()),
      runner: Arc::new(move |args| Box::pin(runner(args))),
      output_sink: None,
    }
  }

  pub async fn dispatch(
    &self,
    args: DispatchWorkersArgs,
    parent_session_id: &str,
    profile_name: &str,
  ) -> Result<String> {
    if args.workers.is_empty() {
      bail!("workers must contain at least one worker");
    }

    let batch_id = {
      let mut inner = self.inner.lock().await;
      inner.next_batch_id += 1;
      format!("batch-{}", inner.next_batch_id)
    };
    let mut workers = Vec::with_capacity(args.workers.len());
    let mut completed = Vec::new();

    for (index, worker) in args.workers.into_iter().enumerate() {
      let worker_id = {
        let mut inner = self.inner.lock().await;
        inner.next_id += 1;
        format!("worker-{}", inner.next_id)
      };

      let role = worker.role.trim().to_string();
      if role.is_empty() {
        completed.push(DispatchWorkerResult {
          batch_id: batch_id.clone(),
          index,
          role,
          worker_id,
          status: "failed".to_string(),
          output: String::new(),
          error: Some("workers[index].role is required".to_string()),
        });
        continue;
      }
      if worker.task.trim().is_empty() {
        completed.push(DispatchWorkerResult {
          batch_id: batch_id.clone(),
          index,
          role,
          worker_id,
          status: "failed".to_string(),
          output: String::new(),
          error: Some("workers[index].task is required".to_string()),
        });
        continue;
      }

      let (system_prompt, task_prompt) = match resolve_worker_prompts(&role, &worker.task, "").await
      {
        Ok(prompts) => prompts,
        Err(err) => {
          completed.push(DispatchWorkerResult {
            batch_id: batch_id.clone(),
            index,
            role,
            worker_id,
            status: "failed".to_string(),
            output: String::new(),
            error: Some(err.to_string()),
          });
          continue;
        }
      };

      let progress_sink = Arc::new(std::sync::Mutex::new(default_worker_progress()));
      let run_args = WorkerRunArgs {
        system_prompt,
        task_prompt,
        parent_session_id: parent_session_id.to_string(),
        worker_id: worker_id.clone(),
        profile_name: profile_name.to_string(),
        workspace_root: self.workspace.root().to_path_buf(),
        progress_sink: Arc::clone(&progress_sink),
        output_sink: self.output_sink.clone(),
      };
      let notify = Arc::clone(&self.notify);
      let runner = Arc::clone(&self.runner);
      let done = tokio::spawn(async move {
        let result = runner(run_args).await;
        notify.notify_waiters();
        result
      });
      workers.push(WorkerStatus {
        batch_id: batch_id.clone(),
        index,
        role: role.clone(),
        worker_id: worker_id.clone(),
        status: "running".to_string(),
        progress: default_worker_progress(),
      });
      let in_flight = InFlightWorker {
        batch_id: batch_id.clone(),
        index,
        role,
        worker_id,
        done,
        progress_sink,
      };
      self.inner.lock().await.in_flight.push(in_flight);
    }

    completed.sort_by_key(|r| (r.batch_id.clone(), r.index));
    Ok(serde_json::to_string(&DispatchBatchResult {
      message: dispatch_message(workers.len()),
      batch_id,
      workers,
      completed,
    })?)
  }

  pub async fn wait(&self) -> Result<String> {
    self
      .wait_with_timeout(std::time::Duration::from_secs(15))
      .await
  }

  async fn wait_with_timeout(&self, timeout: std::time::Duration) -> Result<String> {
    let mut finished = self.take_finished().await;
    let mut running = self.running_workers().await;

    if finished.is_empty() && !running.is_empty() {
      let notified = self.notify.notified();
      tokio::pin!(notified);

      finished = self.take_finished().await;
      if finished.is_empty() {
        tokio::select! {
          _ = &mut notified => {}
          _ = tokio::time::sleep(timeout) => {}
        }
        finished = self.take_finished().await;
      }
    }

    let mut completed = collect_results(finished).await;
    completed.extend(collect_results(self.take_finished().await).await);
    completed.sort_by_key(|r| (r.batch_id.clone(), r.index));
    running = self.running_workers().await;
    Ok(serde_json::to_string(&WaitWorkersResult {
      message: wait_message(!completed.is_empty(), !running.is_empty()),
      completed,
      running,
    })?)
  }

  async fn take_finished(&self) -> Vec<InFlightWorker> {
    let mut inner = self.inner.lock().await;
    let mut finished = Vec::new();
    let mut i = 0;
    while i < inner.in_flight.len() {
      if inner.in_flight[i].done.is_finished() {
        finished.push(inner.in_flight.remove(i));
      } else {
        i += 1;
      }
    }
    finished
  }

  async fn running_workers(&self) -> Vec<WorkerStatus> {
    let inner = self.inner.lock().await;
    inner
      .in_flight
      .iter()
      .map(|worker| {
        let p = match worker.progress_sink.lock() {
          Ok(progress) => progress.trim().to_string(),
          Err(poisoned) => poisoned.into_inner().trim().to_string(),
        };
        WorkerStatus {
          progress: if p.is_empty() {
            "Starting".to_string()
          } else {
            p
          },
          batch_id: worker.batch_id.clone(),
          index: worker.index,
          role: worker.role.clone(),
          worker_id: worker.worker_id.clone(),
          status: "running".to_string(),
        }
      })
      .collect()
  }

  pub async fn cancel(&self, worker_ids: Vec<String>) -> (Vec<String>, Vec<String>) {
    let mut inner = self.inner.lock().await;
    let mut cancelled = Vec::new();
    let mut not_found = Vec::new();
    for id in worker_ids {
      if let Some(pos) = inner.in_flight.iter().position(|w| w.worker_id == id) {
        let worker = inner.in_flight.remove(pos);
        worker.done.abort();
        cancelled.push(id);
      } else {
        not_found.push(id);
      }
    }
    (cancelled, not_found)
  }
}

fn next_worker_counter(workspace: &crate::workspace::Workspace, parent_session_id: &str) -> usize {
  let workers_root = crate::session::session_dir_in(workspace, parent_session_id).join("workers");
  let mut max_seen = 0usize;
  let Ok(entries) = std::fs::read_dir(workers_root) else {
    return 0;
  };
  for entry in entries.flatten() {
    let Some(name) = entry.file_name().to_str().map(str::to_string) else {
      continue;
    };
    let Some(num) = name.strip_prefix("worker-") else {
      continue;
    };
    let Ok(id) = num.parse::<usize>() else {
      continue;
    };
    max_seen = max_seen.max(id);
  }
  max_seen
}

fn default_worker_progress() -> String {
  "Starting".to_string()
}

async fn collect_results(workers: Vec<InFlightWorker>) -> Vec<DispatchWorkerResult> {
  let mut results = Vec::with_capacity(workers.len());
  for worker in workers {
    let result = worker.done.await.unwrap_or_else(|e| WorkerRunResult {
      err: Some(e.to_string()),
      ..Default::default()
    });
    let status = if result.err.is_some() {
      "failed"
    } else {
      "completed"
    };
    results.push(DispatchWorkerResult {
      batch_id: worker.batch_id,
      index: worker.index,
      role: worker.role,
      worker_id: worker.worker_id,
      status: status.to_string(),
      output: result.output,
      error: result.err,
    });
  }
  results
}

fn dispatch_message(running_count: usize) -> String {
  if running_count == 0 {
    return "No workers are running. Inspect `completed` for dispatch-time failures.".to_string();
  }
  "Workers dispatched successfully. Their results are not available yet. Next action: call `wait_workers`. `wait_workers` returns completed worker results as soon as any worker finishes; if none finish within about 15 seconds, it reports that workers are still running.".to_string()
}

fn wait_message(has_completed: bool, has_running: bool) -> String {
  match (has_completed, has_running) {
    (true, true) => {
      "Completed workers are available. Some workers are still running; call `wait_workers` again before depending on unfinished workers.".to_string()
    }
    (true, false) => {
      "All available worker results have been returned. No workers are still running.".to_string()
    }
    (false, true) => {
      "No workers completed after waiting about 15 seconds. Workers are still running; call `wait_workers` again to continue waiting.".to_string()
    }
    (false, false) => "No workers are running and no new worker results are available.".to_string(),
  }
}

pub async fn run_worker_agent(args: WorkerRunArgs) -> WorkerRunResult {
  match run_worker_agent_inner(args).await {
    Ok(output) => WorkerRunResult { output, err: None },
    Err(e) => WorkerRunResult {
      output: String::new(),
      err: Some(e.to_string()),
    },
  }
}

async fn run_worker_agent_inner(args: WorkerRunArgs) -> Result<String> {
  let profile_name = args.profile_name.clone();
  let config = crate::config::Config::default();
  let profile = config
    .get_profile(&profile_name)
    .with_context(|| format!("unknown profile: {profile_name}"))?;
  let provider = config
    .provider_for(profile)
    .context("missing provider config for profile")?;
  let client = crate::providers::new_client(profile, provider)?;
  let workspace = crate::workspace::Workspace::from_root(args.workspace_root);
  let messages = build_worker_messages(
    &args.system_prompt,
    &args.task_prompt,
    &args.parent_session_id,
  );
  let compact = crate::agent::CompactState::new(0.80, profile.context_limit);
  let meta = crate::session::SessionMeta {
    session_id: args.parent_session_id.clone(),
    parent_session: None,
    title: None,
    profile: profile_name.clone(),
    mode: "worker".to_string(),
    flags: crate::session::SessionFlags {
      steer: false,
      worker: true,
      autocompact: 80,
      resume: false,
      temp: false,
    },
    usage: crate::session::SessionUsage { total_tokens: 0 },
    draft_input: None,
    start_ts: Some(crate::session::timestamp_ms()),
    end_ts: None,
  };
  let mut agent = crate::agent::Agent::new(
    workspace,
    client,
    messages,
    crate::tools::configured_worker_tools(),
    compact,
    meta,
    Some(args.parent_session_id),
    Some(args.worker_id),
    crate::config::Config::default(),
  );
  agent.dirty = true;
  agent.progress_sink = Some(args.progress_sink);
  agent.output_sink = args.output_sink.clone();
  agent.worker_manager.set_output_sink(args.output_sink);
  let loop_result = agent.run_loop().await;
  if let Err(e) = loop_result {
    agent.persist_if_dirty()?;
    return Err(e.into());
  }
  agent.persist_if_dirty()?;
  Ok(agent.last_assistant_message().unwrap_or_default())
}

pub(crate) fn build_worker_messages(
  system_prompt: &str,
  prompt: &str,
  session_id: &str,
) -> Vec<crate::types::Message> {
  vec![
    crate::types::Message {
      role: "system".into(),
      content: system_prompt.to_string(),
      origin: crate::types::MessageOrigin::Internal,
      ..Default::default()
    },
    crate::types::Message {
      role: "user".into(),
      content: format!("[session: {session_id}]\n\n{prompt}"),
      origin: crate::types::MessageOrigin::Human,
      ..Default::default()
    },
  ]
}

static ARCHITECT_CLIENT: OnceLock<Result<crate::client::Client, String>> = OnceLock::new();

fn get_architect_client() -> Result<&'static crate::client::Client> {
  let result = ARCHITECT_CLIENT.get_or_init(|| {
    let config = crate::config::Config::default();
    let profile = config
      .get_profile("ds-flash")
      .ok_or_else(|| "architect profile 'ds-flash' not found".to_string())?;
    let provider = config
      .provider_for(profile)
      .ok_or_else(|| "missing provider config for architect profile 'ds-flash'".to_string())?;
    crate::providers::new_client(profile, provider).map_err(|e| e.to_string())
  });
  match result {
    Ok(client) => Ok(client),
    Err(e) => bail!("architect client init: {e}"),
  }
}

pub async fn resolve_worker_prompts(
  role: &str,
  task: &str,
  context: &str,
) -> Result<(String, String)> {
  let requested_role = normalize_role(role);
  if let Some(builtin) = crate::prompts::get_builtin_worker_prompt(requested_role) {
    let context_section = format!("## Context\n\n{}", context.trim());
    let system_prompt = compose_worker_system_prompt(builtin, Some(&context_section));
    return Ok((system_prompt, task.trim().to_string()));
  }

  let client = get_architect_client()?;
  let user_content = format!(
    "## Desired Role\n\n{requested_role}\n\n## Hiring Request\n\n{}\n\n## Context\n\n{}",
    task.trim(),
    context.trim()
  );
  let messages = vec![
    crate::types::Message {
      role: "system".into(),
      content: architect_prompt_for_role(requested_role).to_string(),
      origin: crate::types::MessageOrigin::Internal,
      ..Default::default()
    },
    crate::types::Message {
      role: "user".into(),
      content: user_content,
      origin: crate::types::MessageOrigin::Human,
      ..Default::default()
    },
  ];
  let resp = client
    .chat_json(&messages, &[])
    .await
    .context("architect LLM call failed")?;
  let (system_prompt, task_prompt) = parse_architect_output(&resp.content)?;
  Ok((
    compose_worker_system_prompt(&system_prompt, None),
    task_prompt,
  ))
}

fn compose_worker_system_prompt(base_prompt: &str, extra_section: Option<&str>) -> String {
  let mut sections = vec![base_prompt.trim().to_string()];
  if let Some(extra) = extra_section
    && !extra.trim().is_empty()
  {
    sections.push(extra.trim().to_string());
  }
  sections.push(WORKER_PROGRESS_PROMPT_SUFFIX.to_string());
  sections.join("\n\n")
}

fn normalize_role(role: &str) -> &str {
  let role = role.trim();
  if role.is_empty() { "factory" } else { role }
}

fn architect_prompt_for_role(_role: &str) -> &'static str {
  crate::prompts::CONTRACTOR_FACTORY
}

fn parse_architect_output(text: &str) -> Result<(String, String)> {
  let sys =
    extract_tag(text, "system_prompt").context("architect output missing <system_prompt> block")?;
  let task =
    extract_tag(text, "task_prompt").context("architect output missing <task_prompt> block")?;
  if sys.is_empty() {
    bail!("architect produced empty system_prompt");
  }
  if task.is_empty() {
    bail!("architect produced empty task_prompt");
  }
  Ok((sys, task))
}

fn extract_tag(text: &str, tag: &str) -> Option<String> {
  let open = format!("<{tag}>");
  let close = format!("</{tag}>");
  let start = text.find(&open)? + open.len();
  let end = text[start..].find(&close)? + start;
  Some(text[start..end].trim().to_string())
}

#[cfg(test)]
mod tests {
  use super::*;

  use serde_json::Value;
  use std::future;
  use std::path::PathBuf;

  #[test]
  fn parse_architect_output_extracts_tags() {
    let text = r#"Some preamble

<system_prompt>
Act as a specialist.
</system_prompt>

<task_prompt>
Review the code.
</task_prompt>

Some trailing text"#;
    let (sys, task) = parse_architect_output(text).unwrap();
    assert_eq!(sys, "Act as a specialist.");
    assert_eq!(task, "Review the code.");
  }

  #[test]
  fn parse_architect_output_rejects_missing_tags() {
    assert!(parse_architect_output("no tags here").is_err());
    assert!(parse_architect_output("<system_prompt>hello</system_prompt>").is_err());
  }

  #[test]
  fn extract_tag_returns_none_for_missing() {
    assert!(extract_tag("no tags", "system_prompt").is_none());
  }

  #[test]
  fn factory_role_uses_contractor_factory_prompt() {
    assert_eq!(
      architect_prompt_for_role("factory"),
      crate::prompts::CONTRACTOR_FACTORY
    );
    assert_eq!(
      architect_prompt_for_role("unknown-role"),
      crate::prompts::CONTRACTOR_FACTORY
    );
  }

  #[tokio::test]
  async fn resolve_worker_prompts_uses_reviewer_builtin() {
    let (sys, task) =
      resolve_worker_prompts("reviewer", "review src/lib.rs", "## Files\n- src/lib.rs")
        .await
        .unwrap();
    assert!(sys.contains("judge whether work satisfies the contract"));
    assert!(sys.contains("## Context"));
    assert!(sys.contains("src/lib.rs"));
    assert!(sys.contains("## Progress Reporting"));
    assert!(sys.contains("`path`: `progress/current`"));
    assert_eq!(task, "review src/lib.rs");
  }

  #[tokio::test]
  async fn resolve_worker_prompts_uses_implementer_builtin() {
    let (sys, task) = resolve_worker_prompts(
      "implementer",
      "edit src/lib.rs",
      "## Write Scope\n- src/lib.rs",
    )
    .await
    .unwrap();
    assert!(sys.contains("produce the requested artifact or code change"));
    assert!(sys.contains("## Context"));
    assert!(sys.contains("src/lib.rs"));
    assert!(sys.contains("## Progress Reporting"));
    assert_eq!(task, "edit src/lib.rs");
  }

  #[test]
  fn compose_worker_system_prompt_adds_progress_nudge_for_custom_factory_path() {
    let sys = compose_worker_system_prompt("Custom system prompt", None);
    assert!(sys.contains("Custom system prompt"));
    assert!(sys.contains("## Progress Reporting"));
    assert!(sys.contains("`action`: `write`"));
    assert!(sys.contains("`path`: `progress/current`"));
  }

  #[tokio::test]
  async fn resolve_worker_prompts_uses_architecture_builtins() {
    let (db_sys, db_task) = resolve_worker_prompts(
      "database_architect",
      "design seat-based subscriptions",
      "## Context\n- subscription schemas",
    )
    .await
    .unwrap();
    assert!(db_sys.contains("Database Architect"));
    assert!(db_sys.contains("schema shape and normalization tradeoffs"));
    assert_eq!(db_task, "design seat-based subscriptions");

    let (system_sys, system_task) = resolve_worker_prompts(
      "system_architect",
      "design seat assignment flow",
      "## Context\n- existing subscription system",
    )
    .await
    .unwrap();
    assert!(system_sys.contains("System Architect"));
    assert!(system_sys.contains("service, module, and API boundaries"));
    assert_eq!(system_task, "design seat assignment flow");
  }

  #[tokio::test]
  async fn resolve_worker_prompts_uses_visual_designer_builtin() {
    let (sys, task) = resolve_worker_prompts(
      "visual_designer",
      "design subscription management UI",
      "## Context\n- billing dashboard",
    )
    .await
    .unwrap();
    assert!(sys.contains("Visual Designer"));
    assert!(sys.contains("visual style"));
    assert_eq!(task, "design subscription management UI");
  }

  #[tokio::test]
  async fn dispatch_rejects_empty_worker_list() {
    let manager = WorkerManager::new(None, crate::workspace::Workspace::from_current_dir());
    let err = manager
      .dispatch(
        DispatchWorkersArgs { workers: vec![] },
        "parent-session",
        "ds-flash",
      )
      .await
      .expect_err("empty list should fail");
    assert!(err.to_string().contains("at least one worker"));
  }

  #[tokio::test]
  async fn wait_returns_finished_workers() {
    let manager = WorkerManager::new(None, crate::workspace::Workspace::from_current_dir());
    let done = tokio::spawn(async {
      WorkerRunResult {
        output: "done".to_string(),
        err: None,
      }
    });
    tokio::task::yield_now().await;
    manager.inner.lock().await.in_flight.push(InFlightWorker {
      batch_id: "batch-1".to_string(),
      index: 0,
      role: "implementer".to_string(),
      worker_id: "worker-1".to_string(),
      done,
      progress_sink: Arc::new(std::sync::Mutex::new(String::new())),
    });

    let out = manager
      .wait_with_timeout(std::time::Duration::from_secs(30))
      .await
      .unwrap();

    assert!(out.contains("\"worker_id\":\"worker-1\""));
    assert!(out.contains("\"status\":\"completed\""));
    assert!(out.contains("\"output\":\"done\""));
    assert!(out.contains("\"running\":[]"));
  }

  #[tokio::test]
  async fn wait_reports_default_progress_for_running_worker_without_state() {
    let parent_session_id = format!("test-progress-missing-{}", crate::session::timestamp_ms());
    cleanup_session_dir(&parent_session_id);

    let manager = WorkerManager::new(None, crate::workspace::Workspace::from_current_dir());
    let done = tokio::spawn(async { future::pending::<WorkerRunResult>().await });
    manager.inner.lock().await.in_flight.push(InFlightWorker {
      batch_id: parent_session_id.clone(),
      index: 0,
      role: "implementer".to_string(),
      worker_id: "worker-1".to_string(),
      done,
      progress_sink: Arc::new(std::sync::Mutex::new("Starting".to_string())),
    });

    let out = manager
      .wait_with_timeout(std::time::Duration::from_millis(20))
      .await
      .unwrap();
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["running"][0]["progress"], "Starting");

    abort_all_in_flight(&manager).await;
    cleanup_session_dir(&parent_session_id);
  }

  #[tokio::test]
  async fn wait_reports_progress_for_running_worker_with_progress_state() {
    let parent_session_id = format!("test-progress-present-{}", crate::session::timestamp_ms());
    let worker_id = "worker-1";
    cleanup_session_dir(&parent_session_id);

    let manager = WorkerManager::new(None, crate::workspace::Workspace::from_current_dir());
    let done = tokio::spawn(async { future::pending::<WorkerRunResult>().await });
    manager.inner.lock().await.in_flight.push(InFlightWorker {
      batch_id: parent_session_id.clone(),
      index: 0,
      role: "implementer".to_string(),
      worker_id: worker_id.to_string(),
      done,
      progress_sink: Arc::new(std::sync::Mutex::new("indexing files".to_string())),
    });

    let out = manager
      .wait_with_timeout(std::time::Duration::from_millis(20))
      .await
      .unwrap();
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
      json["running"][0]["progress"],
      Value::String("indexing files".into())
    );

    abort_all_in_flight(&manager).await;
    cleanup_session_dir(&parent_session_id);
  }

  async fn abort_all_in_flight(manager: &WorkerManager) {
    let mut inner = manager.inner.lock().await;
    for worker in inner.in_flight.drain(..) {
      worker.done.abort();
    }
  }

  fn cleanup_session_dir(parent_session_id: &str) {
    let path = PathBuf::from(".ogent/sessions").join(parent_session_id);
    let _ = std::fs::remove_dir_all(path);
  }

  #[tokio::test]
  async fn cancel_workers_cancels_in_flight_worker() {
    let manager = WorkerManager::new(None, crate::workspace::Workspace::from_current_dir());
    let done = tokio::spawn(async { std::future::pending::<WorkerRunResult>().await });
    manager.inner.lock().await.in_flight.push(InFlightWorker {
      batch_id: "batch-1".to_string(),
      index: 0,
      role: "researcher".to_string(),
      worker_id: "worker-cancel-1".to_string(),
      done,
      progress_sink: Arc::new(std::sync::Mutex::new("Starting".to_string())),
    });

    let (cancelled, not_found) = manager.cancel(vec!["worker-cancel-1".to_string()]).await;
    assert_eq!(cancelled, vec!["worker-cancel-1"]);
    assert!(not_found.is_empty());
    assert!(manager.inner.lock().await.in_flight.is_empty());
  }

  #[tokio::test]
  async fn cancel_workers_reports_not_found_for_missing_ids() {
    let manager = WorkerManager::new(None, crate::workspace::Workspace::from_current_dir());
    let (cancelled, not_found) = manager.cancel(vec!["worker-nonexistent".to_string()]).await;
    assert!(cancelled.is_empty());
    assert_eq!(not_found, vec!["worker-nonexistent"]);
  }
}
