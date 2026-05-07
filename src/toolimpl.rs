use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use crate::hashline::{EditOp, apply_anchor_edits, render_hashlines};
use crate::tools::{ToolContext, parse_args, require_nonempty};

pub async fn execute_tool(mut ctx: ToolContext<'_>, name: &str, args: &str) -> Result<String> {
  match name {
    "read_file" => read_file(args),
    "write_file" => write_file(args),
    "bash" => bash(args).await,
    "repo_map" => repo_map(args).await,
    "read_hash_anchors" => read_hash_anchors(args),
    "edit_hash_anchors" => edit_hash_anchors(args),
    "web_search" => web_search(args).await,
    "web_read" => web_read(args).await,
    "code_web_context" => code_web_context(args).await,
    "handoff" => handoff(ctx.agent.as_deref_mut(), args),
    "load_skill" => load_skill(args),
    "dispatch_worker" => dispatch_worker(args).await,
    "start_workers" => start_workers(ctx.agent.as_deref_mut(), args).await,
    "check_workers" => check_workers(ctx.agent.as_deref_mut(), args).await,
    "question" => bail!("interactive mode required"),
    "worker_question" => worker_question(args),
    "complete" => complete(ctx.agent.as_deref_mut(), args),
    _ => bail!("unknown tool: {name}"),
  }
}

#[derive(Deserialize)]
struct ReadFileArgs {
  path: String,
  start: Option<usize>,
  end: Option<usize>,
}

fn read_file(args: &str) -> Result<String> {
  let args: ReadFileArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  let path = crate::workspace::readable_path(&args.path)?;
  let meta = fs::metadata(&path).with_context(|| format!("stat {}", args.path))?;
  if meta.len() > (1 << 20) {
    bail!(
      "file {} exceeds size limit ({} > {} bytes)",
      args.path,
      meta.len(),
      1 << 20
    );
  }
  let content = fs::read_to_string(&path).with_context(|| format!("read {}", args.path))?;
  if args.start.is_none() && args.end.is_none() {
    return Ok(content);
  }
  let lines: Vec<&str> = content.split('\n').collect();
  let start = args.start.unwrap_or(0).min(lines.len());
  let end = args.end.unwrap_or(lines.len()).min(lines.len());
  if start > end {
    bail!("start line {start} exceeds end line {end}");
  }
  Ok(lines[start..end].join("\n"))
}

#[derive(Deserialize)]
struct WriteFileArgs {
  path: String,
  content: String,
  #[serde(default)]
  overwrite_existing: bool,
}

fn write_file(args: &str) -> Result<String> {
  let args: WriteFileArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  let path = crate::workspace::workspace_path(&args.path)?;
  if path.exists() && !args.overwrite_existing {
    bail!(
      "file {} already exists; use edit_hash_anchors for anchored edits or set overwrite_existing=true for intentional full-file replacement",
      args.path
    );
  }
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
  }
  fs::write(&path, &args.content).with_context(|| format!("write {}", args.path))?;
  Ok(format!(
    "Wrote {} bytes to {}",
    args.content.len(),
    args.path
  ))
}

#[derive(Deserialize)]
struct BashArgs {
  command: String,
  #[serde(default)]
  timeout_seconds: u64,
}

async fn bash(args: &str) -> Result<String> {
  let args: BashArgs = parse_args(args)?;
  require_nonempty(&args.command, "command")?;
  validate_bash_command(&args.command)?;
  let secs = if args.timeout_seconds == 0 {
    120
  } else {
    args.timeout_seconds
  };
  if secs > 600 {
    bail!("timeout_seconds must be <= 600");
  }
  let mut cmd = Command::new("sh");
  cmd
    .arg("-c")
    .arg(&args.command)
    .current_dir(crate::workspace::workspace_root())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
  let output = timeout(Duration::from_secs(secs), cmd.output()).await;
  match output {
    Err(_) => bail!("command timed out after {secs}s"),
    Ok(Err(e)) => bail!("exec: {e}"),
    Ok(Ok(out)) => {
      let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
      );
      if !out.status.success() {
        bail!("exit err: {}\n{}", out.status, combined);
      }
      Ok(combined)
    }
  }
}

fn validate_bash_command(command: &str) -> Result<()> {
  for segment in command.split([';', '&', '|', '\n']) {
    let words: Vec<_> = segment.split_whitespace().collect();
    for pair in words.windows(2) {
      if pair[0] == "cd" && matches!(pair[1], "/" | "~" | ".." | "-") {
        bail!(
          "cd outside the workspace is not allowed; run commands in the current working directory"
        );
      }
    }
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn bash_error_includes_stdout_and_stderr() {
    let err = bash(r#"{"command":"printf stdout; printf stderr >&2; exit 7"}"#)
      .await
      .expect_err("command should fail");
    let msg = err.to_string();

    assert!(msg.contains("exit status: 7"));
    assert!(msg.contains("stdout"));
    assert!(msg.contains("stderr"));
  }
}

#[derive(Deserialize)]
struct RepoMapArgs {
  #[serde(default)]
  path: String,
  #[serde(default)]
  levels: usize,
}

async fn repo_map(args: &str) -> Result<String> {
  let args: RepoMapArgs = parse_args(args)?;
  let rel = if args.path.is_empty() {
    "."
  } else {
    &args.path
  };
  let path = crate::workspace::readable_path(rel)?;
  let levels = if args.levels == 0 { 3 } else { args.levels };
  let mut out = String::new();
  repo_map_walk(&path, &path, levels, 0, &mut out)?;
  Ok(out)
}

fn repo_map_walk(
  root: &Path,
  path: &Path,
  max_depth: usize,
  depth: usize,
  out: &mut String,
) -> Result<()> {
  if depth > max_depth {
    return Ok(());
  }
  let rel = path.strip_prefix(root).unwrap_or(path);
  if depth == 0 {
    out.push_str(".\n");
  } else if let Some(name) = rel.file_name() {
    out.push_str(&format!(
      "{}{}\n",
      "  ".repeat(depth),
      name.to_string_lossy()
    ));
  }
  if path.is_dir() && depth < max_depth {
    let mut entries: Vec<_> = fs::read_dir(path)?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
      let name = entry.file_name().to_string_lossy().to_string();
      if name.starts_with('.') || name == "node_modules" || name == "target" {
        continue;
      }
      repo_map_walk(root, &entry.path(), max_depth, depth + 1, out)?;
    }
  }
  Ok(())
}

fn read_hash_anchors(args: &str) -> Result<String> {
  let args: ReadFileArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  let path = crate::workspace::workspace_path(&args.path)?;
  let source = fs::read_to_string(&path).with_context(|| format!("read {}", args.path))?;
  Ok(render_hashlines(&source, 1, args.start, args.end))
}

#[derive(Deserialize)]
struct EditHashAnchorsArgs {
  path: String,
  ops: Vec<EditOp>,
}

fn edit_hash_anchors(args: &str) -> Result<String> {
  let args: EditHashAnchorsArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  if args.ops.is_empty() {
    bail!("ops array is required");
  }
  let path = crate::workspace::workspace_path(&args.path)?;
  let source = fs::read_to_string(&path).with_context(|| format!("read {}", args.path))?;
  let out = apply_anchor_edits(&source, &args.ops)?;
  fs::write(&path, out).with_context(|| format!("write {}", args.path))?;
  Ok(format!("Applied {} edits to {}", args.ops.len(), args.path))
}

#[derive(Deserialize)]
struct WebSearchArgs {
  query: String,
  #[serde(default)]
  num_results: usize,
  #[serde(default, rename = "type")]
  search_type: String,
}

async fn web_search(args: &str) -> Result<String> {
  let args: WebSearchArgs = parse_args(args)?;
  require_nonempty(&args.query, "query")?;
  let n = args.num_results.clamp(1, 100);
  let body = json!({"query": args.query, "type": if args.search_type.is_empty() { "auto" } else { &args.search_type }, "numResults": n, "contents": {"highlights": true}});
  let v = exa_post("https://api.exa.ai/search", body).await?;
  let mut out = String::new();
  for (i, r) in v["results"]
    .as_array()
    .unwrap_or(&Vec::new())
    .iter()
    .enumerate()
  {
    out.push_str(&format!(
      "{}. {}\n   {}\n",
      i + 1,
      r["title"].as_str().unwrap_or(""),
      r["url"].as_str().unwrap_or("")
    ));
    if let Some(highlights) = r["highlights"].as_array() {
      for h in highlights {
        out.push_str(&format!("   > {}\n", h.as_str().unwrap_or("")));
      }
    }
    out.push('\n');
  }
  Ok(out)
}

#[derive(Deserialize)]
struct WebReadArgs {
  urls: Vec<String>,
  #[serde(default)]
  mode: String,
}

async fn web_read(args: &str) -> Result<String> {
  let args: WebReadArgs = parse_args(args)?;
  if args.urls.is_empty() {
    bail!("urls is required");
  }
  let mode = if args.mode.is_empty() {
    "highlights"
  } else {
    &args.mode
  };
  let body = if mode == "text" {
    json!({"urls": args.urls, "text": true})
  } else {
    json!({"urls": args.urls, "highlights": true})
  };
  let v = exa_post("https://api.exa.ai/contents", body).await?;
  let mut out = String::new();
  for r in v["results"].as_array().unwrap_or(&Vec::new()) {
    out.push_str(&format!(
      "--- {} ---\n{}\n\n",
      r["title"].as_str().unwrap_or(""),
      r["url"].as_str().unwrap_or("")
    ));
    if mode == "text" {
      out.push_str(r["text"].as_str().unwrap_or(""));
      out.push_str("\n\n");
    } else if let Some(highlights) = r["highlights"].as_array() {
      for h in highlights {
        out.push_str(&format!("> {}\n", h.as_str().unwrap_or("")));
      }
      out.push('\n');
    }
  }
  Ok(out)
}

#[derive(Deserialize)]
struct CodeWebContextArgs {
  query: String,
}

async fn code_web_context(args: &str) -> Result<String> {
  let args: CodeWebContextArgs = parse_args(args)?;
  require_nonempty(&args.query, "query")?;
  let v = exa_post(
    "https://api.exa.ai/context",
    json!({"query": args.query, "tokensNum": "dynamic"}),
  )
  .await?;
  Ok(v["response"].as_str().unwrap_or("").to_string())
}

async fn exa_post(url: &str, body: Value) -> Result<Value> {
  let key = std::env::var("EXA_API_KEY").unwrap_or_default();
  if key.is_empty() {
    bail!("EXA_API_KEY not set");
  }
  let resp = reqwest::Client::new()
    .post(url)
    .header("x-api-key", key)
    .json(&body)
    .send()
    .await?;
  let status = resp.status();
  let text = resp.text().await?;
  if !status.is_success() {
    bail!("exa {}: {}", status.as_u16(), text.trim());
  }
  let v: Value = serde_json::from_str(&text).context("unmarshal exa response")?;
  if let Some(err) = v["error"].as_str().filter(|s| !s.is_empty()) {
    bail!("exa error: {err}");
  }
  Ok(v)
}

#[derive(Deserialize)]
struct HandoffArgs {
  brief: String,
}

fn handoff(agent: Option<&mut crate::agent::Agent>, args: &str) -> Result<String> {
  let args: HandoffArgs = parse_args(args)?;
  require_nonempty(&args.brief, "brief")?;
  fs::create_dir_all(".ogent/handoffs")?;
  let path = format!(".ogent/handoffs/{}.md", crate::session::timestamp());
  fs::write(&path, args.brief)?;
  if let Some(agent) = agent {
    agent.compact.last_handoff_path = path.clone();
  }
  Ok(format!("Handoff written to {path}"))
}

#[derive(Deserialize)]
struct LoadSkillArgs {
  name: String,
}

fn load_skill(args: &str) -> Result<String> {
  let args: LoadSkillArgs = parse_args(args)?;
  require_nonempty(&args.name, "name")?;
  let (name, root, body) = crate::prompts::load_skill_content(&args.name)?;
  Ok(format!(
    "<skill name=\"{name}\" root=\"{root}\">\n{body}\n</skill>"
  ))
}

#[derive(Deserialize)]
struct DispatchWorkerArgs {
  system_prompt: String,
  task: String,
  artifact_path: String,
  #[serde(default)]
  max_turns: i32,
}

async fn dispatch_worker(args: &str) -> Result<String> {
  let args: DispatchWorkerArgs = parse_args(args)?;
  require_nonempty(&args.system_prompt, "system_prompt")?;
  require_nonempty(&args.task, "task")?;
  require_nonempty(&args.artifact_path, "artifact_path")?;
  crate::workers::validate_worker_artifact_path(&args.artifact_path)
    .map_err(|e| anyhow::anyhow!("artifact_path: {e}"))?;
  let result = crate::workers::run_worker_process(crate::workers::WorkerProcessArgs {
    system_prompt: args.system_prompt,
    task_prompt: args.task,
    artifact_path: args.artifact_path,
    max_turns: args.max_turns,
    stream_stderr: true,
  })
  .await;
  crate::workers::format_dispatch_worker_result(result)
}

async fn start_workers(agent: Option<&mut crate::agent::Agent>, args: &str) -> Result<String> {
  let args: crate::workers::StartWorkersArgs = parse_args(args)?;
  match agent {
    Some(agent) => agent.worker_manager.start(args).await,
    None => crate::workers::WorkerManager::new().start(args).await,
  }
}

async fn check_workers(agent: Option<&mut crate::agent::Agent>, args: &str) -> Result<String> {
  let _: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
  Ok(match agent {
    Some(agent) => agent.worker_manager.check().await,
    None => crate::workers::WorkerManager::new().check().await,
  })
}

#[derive(Deserialize)]
struct QuestionArgs {
  question: String,
}

fn worker_question(args: &str) -> Result<String> {
  let args: QuestionArgs = parse_args(args)?;
  Ok(format!("[BLOCKER] Worker asks: {}", args.question))
}

#[derive(Deserialize)]
struct CompleteArgs {
  summary: String,
}

fn complete(agent: Option<&mut crate::agent::Agent>, args: &str) -> Result<String> {
  let args: CompleteArgs = parse_args(args)?;
  require_nonempty(&args.summary, "summary")?;
  let Some(agent) = agent else {
    bail!("complete requires an active agent");
  };
  agent.completion_summary = Some(args.summary.trim().to_string());
  Ok("Task marked complete.".to_string())
}
