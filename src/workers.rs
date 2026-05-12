use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::OnceLock;

use tokio::process::Command;
use tokio::sync::Mutex;

use crate::prompts::WORKER_SUMMARY_PROMPT;

#[derive(Debug, Clone)]
pub struct WorkerProcessArgs {
  pub system_prompt: String,
  pub task_prompt: String,
  pub stream_stderr: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WorkerProcessResult {
  pub report: String,
  pub output: String,
  pub err: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DispatchWorkerArgs {
  pub task: String,
  #[serde(default = "default_template")]
  pub template: String,
  #[serde(default)]
  pub context: String,
}

fn default_template() -> String {
  "generic".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct AsyncCoworkerArgs {
  #[serde(default)]
  pub name: String,
  pub task: String,
  #[serde(default = "default_template")]
  pub template: String,
  #[serde(default)]
  pub context: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StartWorkersArgs {
  pub coworkers: Vec<AsyncCoworkerArgs>,
}

pub struct WorkerManager {
  inner: Mutex<Inner>,
}

struct Inner {
  next_id: usize,
  batches: usize,
  workers: Vec<Worker>,
}

struct Worker {
  id: String,
  batch_id: String,
  name: String,
  order: usize,
  done: tokio::task::JoinHandle<WorkerProcessResult>,
}

impl WorkerManager {
  pub fn new() -> Self {
    Self {
      inner: Mutex::new(Inner {
        next_id: 0,
        batches: 0,
        workers: Vec::new(),
      }),
    }
  }

  pub async fn start(&self, args: StartWorkersArgs) -> Result<String> {
    validate_start_workers_args(&args)?;
    let mut resolved = Vec::new();
    for (i, coworker) in args.coworkers.iter().enumerate() {
      let (sys, task) =
        resolve_worker_prompts(&coworker.template, &coworker.task, &coworker.context)
          .await
          .with_context(|| format!("architect failed for coworkers[{i}]"))?;
      let name = if coworker.name.trim().is_empty() {
        format!("coworker-{}", i + 1)
      } else {
        coworker.name.trim().to_string()
      };
      resolved.push((name, sys, task));
    }
    let mut inner = self.inner.lock().await;
    inner.batches += 1;
    let batch_id = format!("batch-{}", inner.batches);
    let mut started = Vec::new();
    for (name, system_prompt, task_prompt) in resolved {
      inner.next_id += 1;
      let id = format!("worker-{}", inner.next_id);
      let run_args = WorkerProcessArgs {
        system_prompt,
        task_prompt,
        stream_stderr: false,
      };
      let done = tokio::spawn(async move { run_worker_process(run_args).await });
      let order = inner.next_id;
      inner.workers.push(Worker {
        id: id.clone(),
        batch_id: batch_id.clone(),
        name: name.clone(),
        order,
        done,
      });
      started.push((id, name));
    }
    let mut out = format!(
      "Started {} async coworker(s) in {batch_id}:\n",
      started.len()
    );
    for (id, name) in started {
      out.push_str(&format!("- {id} ({name})\n"));
    }
    Ok(out)
  }

  pub async fn check(&self) -> String {
    let workers = {
      let mut inner = self.inner.lock().await;
      if inner.workers.is_empty() {
        return "No async coworkers are running or waiting to be collected.".to_string();
      }
      let mut workers = Vec::new();
      std::mem::swap(&mut workers, &mut inner.workers);
      workers
    };
    let mut sorted = workers;
    sorted.sort_by_key(|w| w.order);
    let mut out = format!("Async coworker summaries ({})\n", sorted.len());
    for worker in sorted {
      let result = worker.done.await.unwrap_or_else(|e| WorkerProcessResult {
        err: Some(e.to_string()),
        ..Default::default()
      });
      out.push_str(&format!(
        "\n## {} ({})\n- Batch: {}\n",
        worker.id, worker.name, worker.batch_id
      ));
      if let Some(err) = result.err {
        out.push_str(&format!("- Status: failed: {err}\n"));
        if !result.output.is_empty() {
          out.push_str(&format!("\nOutput:\n{}\n", result.output));
        }
      } else if !result.report.is_empty() {
        out.push_str(&format!(
          "- Status: completed\n\nSummary:\n{}\n",
          result.report
        ));
      } else {
        out.push_str("- Status: completed without summary\n");
        if !result.output.is_empty() {
          out.push_str(&format!("\nOutput:\n{}\n", result.output));
        }
      }
    }
    out
  }

  #[cfg(test)]
  async fn insert_finished_for_test(&self, name: &str, result: WorkerProcessResult) {
    let mut inner = self.inner.lock().await;
    inner.next_id += 1;
    inner.batches += 1;
    let id = format!("worker-{}", inner.next_id);
    let batch_id = format!("batch-{}", inner.batches);
    let order = inner.next_id;
    let done = tokio::spawn(async move { result });
    inner.workers.push(Worker {
      id,
      batch_id,
      name: name.to_string(),
      order,
      done,
    });
  }

  pub async fn status_message(&self) -> Option<String> {
    let inner = self.inner.lock().await;
    if inner.workers.is_empty() {
      return None;
    }
    if inner.workers.iter().any(|w| !w.done.is_finished()) {
      Some("Async coworkers are still running. Continue parent-owned work, or call `check_workers` if blocked or ready to integrate.".to_string())
    } else {
      Some(
        "All async coworkers finished. Call `check_workers` to collect reports before finalizing."
          .to_string(),
      )
    }
  }
}

pub async fn run_worker_process(args: WorkerProcessArgs) -> WorkerProcessResult {
  let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("ogent"));
  let mut cmd = Command::new(exe);
  cmd.arg("--worker");
  cmd.arg(&args.task_prompt);
  cmd.current_dir(crate::workspace::workspace_root());
  cmd.stdin(std::process::Stdio::piped());
  cmd.stdout(std::process::Stdio::piped());
  cmd.stderr(std::process::Stdio::piped());
  let mut child = match cmd.spawn() {
    Ok(child) => child,
    Err(e) => {
      return WorkerProcessResult {
        err: Some(format!("start worker: {e}")),
        ..Default::default()
      };
    }
  };
  if let Some(mut stdin) = child.stdin.take() {
    let prompt = args.system_prompt + WORKER_SUMMARY_PROMPT;
    tokio::spawn(async move {
      use tokio::io::AsyncWriteExt;
      let _ = stdin.write_all(prompt.as_bytes()).await;
    });
  }

  let Some(mut stdout) = child.stdout.take() else {
    return WorkerProcessResult {
      err: Some("worker stdout pipe unavailable after spawn".into()),
      ..Default::default()
    };
  };
  let Some(mut stderr) = child.stderr.take() else {
    return WorkerProcessResult {
      err: Some("worker stderr pipe unavailable after spawn".into()),
      ..Default::default()
    };
  };

  let stdout_task = tokio::spawn(async move {
    let mut buf = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut buf)
      .await
      .ok();
    String::from_utf8_lossy(&buf).to_string()
  });

  let stream_stderr = args.stream_stderr;
  let stderr_task = tokio::spawn(async move {
    let mut buf = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut buf)
      .await
      .ok();
    let s = String::from_utf8_lossy(&buf).to_string();
    if stream_stderr {
      let _ = tokio::io::AsyncWriteExt::write_all(&mut tokio::io::stderr(), s.as_bytes()).await;
    }
    s
  });

  let status = match child.wait().await {
    Ok(s) => s,
    Err(e) => {
      let out = stdout_task.await.unwrap_or_default();
      let err = stderr_task.await.unwrap_or_default();
      return WorkerProcessResult {
        output: format!("{out}{err}").trim().to_string(),
        err: Some(e.to_string()),
        ..Default::default()
      };
    }
  };

  let out = stdout_task.await.unwrap_or_default();
  let err = stderr_task.await.unwrap_or_default();

  if !status.success() {
    let combined = format!("{out}{err}");
    return WorkerProcessResult {
      output: combined.trim().to_string(),
      err: Some(status.to_string()),
      ..Default::default()
    };
  }
  WorkerProcessResult {
    report: out.trim().to_string(),
    output: err.trim().to_string(),
    err: None,
  }
}

pub fn format_dispatch_worker_result(result: WorkerProcessResult) -> Result<String> {
  match result.err {
    Some(err) if result.output.is_empty() => bail!("worker failed with no output: {err}"),
    Some(err) => Ok(format!("WORKER FAILED ({err}):\n\n{}", result.output)),
    None if result.report.is_empty() => Ok(format!(
      "Worker completed without summary. Output:\n\n{}",
      result.output
    )),
    None => Ok(format!("Worker completed. Summary:\n\n{}", result.report)),
  }
}

pub fn validate_start_workers_args(args: &StartWorkersArgs) -> Result<()> {
  if args.coworkers.is_empty() {
    bail!("coworkers must contain at least one worker");
  }
  let mut seen = HashSet::new();
  for (i, c) in args.coworkers.iter().enumerate() {
    if c.task.trim().is_empty() {
      bail!("coworkers[{i}].task is required");
    }
    let name = c.name.trim();
    if !name.is_empty() && !seen.insert(name.to_string()) {
      bail!("duplicate coworker name: {name}");
    }
  }
  Ok(())
}

static ARCHITECT_CLIENT: OnceLock<Result<crate::client::Client, String>> = OnceLock::new();

fn get_architect_client() -> Result<&'static crate::client::Client> {
  let result = ARCHITECT_CLIENT.get_or_init(|| {
    let profile = crate::profiles::get_profile("ds-flash")
      .ok_or_else(|| "architect profile 'ds-flash' not found".to_string())?;
    crate::providers::new_client(profile).map_err(|e| e.to_string())
  });
  match result {
    Ok(client) => Ok(client),
    Err(e) => bail!("architect client init: {e}"),
  }
}

pub async fn resolve_worker_prompts(
  template: &str,
  task: &str,
  context: &str,
) -> Result<(String, String)> {
  let requested_template = normalize_template(template);
  if let Some(builtin) = crate::prompts::get_builtin_worker_prompt(requested_template) {
    let system_prompt = format!("{builtin}\n\n## Context\n\n{}", context.trim());
    return Ok((system_prompt, task.trim().to_string()));
  }

  let client = get_architect_client()?;
  let template_body = crate::prompts::get_worker_template(requested_template);
  let user_content = format!(
    "## Requested Template/Role\n\n{requested_template}\n\n## Template\n\n{template_body}\n\n## Task\n\n{}\n\n## Context\n\n{}",
    task.trim(),
    context.trim()
  );
  let messages = vec![
    crate::types::Message {
      role: "system".into(),
      content: crate::prompts::ARCHITECT_PROMPT.to_string(),
      ..Default::default()
    },
    crate::types::Message {
      role: "user".into(),
      content: user_content,
      ..Default::default()
    },
  ];
  let resp = client
    .chat_json(&messages, &[])
    .await
    .context("architect LLM call failed")?;
  parse_architect_output(&resp.content)
}

fn normalize_template(template: &str) -> &str {
  let template = template.trim();
  if template.is_empty() {
    "generic"
  } else {
    template
  }
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

  #[test]
  fn start_workers_rejects_empty_coworker_list() {
    assert!(validate_start_workers_args(&StartWorkersArgs { coworkers: vec![] }).is_err());
  }

  #[test]
  fn start_workers_rejects_duplicate_names() {
    let args = StartWorkersArgs {
      coworkers: vec![
        AsyncCoworkerArgs {
          name: "a".into(),
          task: "do something".into(),
          template: "generic".into(),
          context: String::new(),
        },
        AsyncCoworkerArgs {
          name: "a".into(),
          task: "do something else".into(),
          template: "generic".into(),
          context: String::new(),
        },
      ],
    };
    assert!(validate_start_workers_args(&args).is_err());
  }

  #[tokio::test]
  async fn check_workers_returns_reports_and_clears_workers() {
    let manager = WorkerManager::new();
    manager
      .insert_finished_for_test(
        "alpha",
        WorkerProcessResult {
          report: "report body".into(),
          ..Default::default()
        },
      )
      .await;

    let first = manager.check().await;
    assert!(first.contains("report body"));
    let second = manager.check().await;
    assert!(second.contains("No async coworkers"));
  }

  #[test]
  fn dispatch_worker_reports_subprocess_failure_with_captured_output() {
    let out = format_dispatch_worker_result(WorkerProcessResult {
      output: "stderr text".into(),
      err: Some("exit status: 1".into()),
      ..Default::default()
    })
    .unwrap();
    assert!(out.contains("WORKER FAILED"));
    assert!(out.contains("stderr text"));
  }

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

  #[tokio::test]
  async fn resolve_worker_prompts_uses_builtin_without_architect() {
    let (sys, task) =
      resolve_worker_prompts("reviewer", "review src/lib.rs", "## Files\n- src/lib.rs")
        .await
        .unwrap();
    assert!(sys.contains("code reviewer"));
    assert!(sys.contains("## Context"));
    assert!(sys.contains("src/lib.rs"));
    assert_eq!(task, "review src/lib.rs");
  }
}
