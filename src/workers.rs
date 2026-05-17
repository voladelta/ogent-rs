use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use tokio::process::Command;
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct WorkerProcessArgs {
  pub system_prompt: String,
  pub task_prompt: String,
  pub stream_stderr: bool,
  pub parent_session_id: String,
  pub worker_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct WorkerProcessResult {
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
  index: usize,
  role: String,
  worker_id: String,
  status: String,
  output: String,
  error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DispatchBatchResult {
  results: Vec<DispatchWorkerResult>,
}

pub struct WorkerManager {
  inner: Mutex<Inner>,
}

struct Inner {
  next_id: usize,
}

struct InFlightWorker {
  index: usize,
  role: String,
  worker_id: String,
  done: tokio::task::JoinHandle<WorkerProcessResult>,
}

impl WorkerManager {
  pub fn new() -> Self {
    Self {
      inner: Mutex::new(Inner { next_id: 0 }),
    }
  }

  pub async fn dispatch(
    &self,
    args: DispatchWorkersArgs,
    parent_session_id: &str,
  ) -> Result<String> {
    if args.workers.is_empty() {
      bail!("workers must contain at least one worker");
    }

    let mut in_flight = Vec::new();
    let mut results = Vec::with_capacity(args.workers.len());

    for (index, worker) in args.workers.into_iter().enumerate() {
      let worker_id = {
        let mut inner = self.inner.lock().await;
        inner.next_id += 1;
        format!("worker-{}", inner.next_id)
      };

      let role = worker.role.trim().to_string();
      if role.is_empty() {
        results.push(DispatchWorkerResult {
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
        results.push(DispatchWorkerResult {
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
          results.push(DispatchWorkerResult {
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

      let run_args = WorkerProcessArgs {
        system_prompt,
        task_prompt,
        stream_stderr: false,
        parent_session_id: parent_session_id.to_string(),
        worker_id: worker_id.clone(),
      };
      let done = tokio::spawn(async move { run_worker_process(run_args).await });
      in_flight.push(InFlightWorker {
        index,
        role,
        worker_id,
        done,
      });
    }

    for worker in in_flight {
      let result = worker.done.await.unwrap_or_else(|e| WorkerProcessResult {
        err: Some(e.to_string()),
        ..Default::default()
      });
      let status = if result.err.is_some() {
        "failed"
      } else {
        "completed"
      };
      results.push(DispatchWorkerResult {
        index: worker.index,
        role: worker.role,
        worker_id: worker.worker_id,
        status: status.to_string(),
        output: result.output,
        error: result.err,
      });
    }

    results.sort_by_key(|r| r.index);
    Ok(serde_json::to_string(&DispatchBatchResult { results })?)
  }
}

pub async fn run_worker_process(args: WorkerProcessArgs) -> WorkerProcessResult {
  let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("ogent"));
  let mut cmd = Command::new(exe);
  cmd.arg(format!("--worker={}", args.parent_session_id));
  cmd.arg(&args.task_prompt);
  cmd.current_dir(crate::workspace::workspace_root());
  cmd.env("OGENT_WORKER_ID", &args.worker_id);
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
    let prompt = args.system_prompt;
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
        output: out.trim().to_string(),
        err: Some(format!("{e}\n{err}")),
      };
    }
  };

  let out = stdout_task.await.unwrap_or_default();
  let err = stderr_task.await.unwrap_or_default();

  if !status.success() {
    return WorkerProcessResult {
      output: out.trim().to_string(),
      err: Some(if err.trim().is_empty() {
        status.to_string()
      } else {
        format!("{}\n{}", status, err.trim())
      }),
    };
  }
  WorkerProcessResult {
    output: out.trim().to_string(),
    err: None,
  }
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
  role: &str,
  task: &str,
  context: &str,
) -> Result<(String, String)> {
  let requested_role = normalize_role(role);
  if let Some(builtin) = crate::prompts::get_builtin_worker_prompt(requested_role) {
    let system_prompt = format!("{builtin}\n\n## Context\n\n{}", context.trim());
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
  parse_architect_output(&resp.content)
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
    assert_eq!(task, "edit src/lib.rs");
  }

  #[tokio::test]
  async fn dispatch_rejects_empty_worker_list() {
    let manager = WorkerManager::new();
    let err = manager
      .dispatch(DispatchWorkersArgs { workers: vec![] }, "parent-session")
      .await
      .expect_err("empty list should fail");
    assert!(err.to_string().contains("at least one worker"));
  }
}
