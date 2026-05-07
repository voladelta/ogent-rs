use anyhow::{Result, bail};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use tokio::process::Command;
use tokio::sync::Mutex;

use crate::prompts::WORKER_SUMMARY_PROMPT;

#[derive(Debug, Clone)]
pub struct WorkerProcessArgs {
  pub system_prompt: String,
  pub task_prompt: String,
  pub artifact_path: String,
  pub max_turns: i32,
  pub stream_stderr: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WorkerProcessResult {
  pub report: String,
  pub output: String,
  pub err: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AsyncCoworkerArgs {
  #[serde(default)]
  pub name: String,
  pub system_prompt: String,
  pub task_prompt: String,
  #[serde(default)]
  pub artifact_path: String,
  #[serde(default)]
  pub max_turns: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StartWorkersArgs {
  pub coworkers: Vec<AsyncCoworkerArgs>,
}

#[derive(Clone)]
pub struct WorkerManager {
  inner: Arc<Mutex<Inner>>,
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
  artifact_path: String,
  order: usize,
  done: tokio::task::JoinHandle<WorkerProcessResult>,
}

impl Default for WorkerManager {
  fn default() -> Self {
    Self::new()
  }
}

impl WorkerManager {
  pub fn new() -> Self {
    Self {
      inner: Arc::new(Mutex::new(Inner {
        next_id: 0,
        batches: 0,
        workers: Vec::new(),
      })),
    }
  }

  pub async fn start(&self, args: StartWorkersArgs) -> Result<String> {
    validate_start_workers_args(&args)?;
    let mut inner = self.inner.lock().await;
    inner.batches += 1;
    let batch_id = format!("batch-{}", inner.batches);
    let mut started = Vec::new();
    for (i, coworker) in args.coworkers.into_iter().enumerate() {
      inner.next_id += 1;
      let id = format!("worker-{}", inner.next_id);
      let name = if coworker.name.trim().is_empty() {
        format!("coworker-{}", i + 1)
      } else {
        coworker.name.trim().to_string()
      };
      let artifact_path = if coworker.artifact_path.is_empty() {
        format!(".ogent/workers/{batch_id}-{id}.md")
      } else {
        coworker.artifact_path.clone()
      };
      let run_args = WorkerProcessArgs {
        system_prompt: coworker.system_prompt,
        task_prompt: coworker.task_prompt,
        artifact_path: artifact_path.clone(),
        max_turns: coworker.max_turns,
        stream_stderr: false,
      };
      let done = tokio::spawn(async move { run_worker_process(run_args).await });
      let order = inner.next_id;
      inner.workers.push(Worker {
        id: id.clone(),
        batch_id: batch_id.clone(),
        name: name.clone(),
        artifact_path: artifact_path.clone(),
        order,
        done,
      });
      started.push((id, name, artifact_path));
    }
    let mut out = format!(
      "Started {} async coworker(s) in {batch_id}:\n",
      started.len()
    );
    for (id, name, artifact) in started {
      out.push_str(&format!("- {id} ({name}): report={artifact}\n"));
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
    let mut out = format!("Async coworker reports ({}):\n", sorted.len());
    for worker in sorted {
      let result = worker.done.await.unwrap_or_else(|e| WorkerProcessResult {
        err: Some(e.to_string()),
        ..Default::default()
      });
      out.push_str(&format!(
        "\n## {} ({})\n- Batch: {}\n- Artifact: {}\n",
        worker.id, worker.name, worker.batch_id, worker.artifact_path
      ));
      if let Some(err) = result.err {
        out.push_str(&format!("- Status: failed: {err}\n"));
        if !result.output.is_empty() {
          out.push_str(&format!("\nOutput:\n{}\n", result.output));
        }
      } else if !result.report.is_empty() {
        out.push_str(&format!(
          "- Status: completed\n\nReport:\n{}\n",
          result.report
        ));
      } else {
        out.push_str("- Status: completed without report file\n");
        if !result.output.is_empty() {
          out.push_str(&format!("\nOutput:\n{}\n", result.output));
        }
      }
    }
    out
  }

  #[cfg(test)]
  async fn insert_finished_for_test(
    &self,
    name: &str,
    artifact_path: &str,
    result: WorkerProcessResult,
  ) {
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
      artifact_path: artifact_path.to_string(),
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
  if args.max_turns > 0 {
    cmd.arg(format!("--max-turns={}", args.max_turns));
  }
  cmd.arg(task_prompt_with_artifact(
    &args.task_prompt,
    &args.artifact_path,
  ));
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

  let mut stdout = child.stdout.take().unwrap();
  let mut stderr = child.stderr.take().unwrap();

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
      return WorkerProcessResult {
        err: Some(e.to_string()),
        ..Default::default()
      };
    }
  };

  let out = stdout_task.await.unwrap_or_default();
  let err = stderr_task.await.unwrap_or_default();
  let combined = format!("{out}{err}");

  if !status.success() {
    return WorkerProcessResult {
      output: combined,
      err: Some(status.to_string()),
      ..Default::default()
    };
  }
  match tokio::fs::read_to_string(&args.artifact_path).await {
    Ok(report) => WorkerProcessResult {
      report,
      output: combined,
      err: None,
    },
    Err(e) => WorkerProcessResult {
      output: combined,
      err: Some(format!(
        "worker completed but report not found at {}: {e}",
        args.artifact_path
      )),
      report: String::new(),
    },
  }
}

pub fn task_prompt_with_artifact(task_prompt: &str, artifact_path: &str) -> String {
  format!("{}\n\nArtifact path: {artifact_path}", task_prompt.trim())
}

pub fn format_dispatch_worker_result(result: WorkerProcessResult) -> Result<String> {
  if let Some(err) = result.err {
    if result.output.is_empty() {
      bail!("worker failed with no output: {err}");
    }
    return Ok(format!("WORKER FAILED ({err}):\n\n{}", result.output));
  }
  if result.report.is_empty() {
    return Ok(format!(
      "Worker completed (no report file) but produced output:\n\n{}",
      result.output
    ));
  }
  Ok(format!("Worker completed. Report:\n\n{}", result.report))
}

pub fn validate_start_workers_args(args: &StartWorkersArgs) -> Result<()> {
  if args.coworkers.is_empty() {
    bail!("coworkers must contain at least one worker");
  }
  let mut seen = HashSet::new();
  for (i, c) in args.coworkers.iter().enumerate() {
    if c.system_prompt.trim().is_empty() {
      bail!("coworkers[{i}].system_prompt is required");
    }
    if c.task_prompt.trim().is_empty() {
      bail!("coworkers[{i}].task_prompt is required");
    }
    let name = c.name.trim();
    if !name.is_empty() && !seen.insert(name.to_string()) {
      bail!("duplicate coworker name: {name}");
    }
    if !c.artifact_path.is_empty() {
      validate_worker_artifact_path(&c.artifact_path)
        .map_err(|e| anyhow::anyhow!("coworkers[{i}].artifact_path: {e}"))?;
    }
  }
  Ok(())
}

pub fn validate_worker_artifact_path(path: &str) -> Result<()> {
  let p = Path::new(path);
  if p.is_absolute() {
    bail!("absolute paths are not allowed");
  }
  if p
    .components()
    .any(|c| matches!(c, std::path::Component::ParentDir))
  {
    bail!("path traversal is not allowed");
  }
  let clean = p.components().collect::<std::path::PathBuf>();
  if clean.as_os_str().is_empty() || clean == Path::new(".") {
    bail!("path traversal is not allowed");
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn validate_worker_artifact_path_rejects_absolute_paths() {
    assert!(validate_worker_artifact_path("/tmp/report.md").is_err());
  }

  #[test]
  fn validate_worker_artifact_path_rejects_traversal() {
    assert!(validate_worker_artifact_path("../report.md").is_err());
    assert!(validate_worker_artifact_path("a/../report.md").is_err());
  }

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
          system_prompt: "s".into(),
          task_prompt: "t".into(),
          artifact_path: String::new(),
          max_turns: 0,
        },
        AsyncCoworkerArgs {
          name: "a".into(),
          system_prompt: "s".into(),
          task_prompt: "t".into(),
          artifact_path: String::new(),
          max_turns: 0,
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
        ".ogent/workers/alpha.md",
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
}
