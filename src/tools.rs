use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;
use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use crate::agent::Agent;
use crate::hashline::{EditOp, apply_anchor_edits, render_hashlines};
use crate::task_tracker::{Complexity, GoalState, PhaseUpdate, Status, TaskTracker, TodoUpdate};
use crate::types::{Tool, ToolFunction};

pub struct ToolContext<'a> {
  pub agent: Option<&'a mut Agent>,
}

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
    "set_goal" => set_goal(ctx.agent.as_deref_mut(), args),
    "revise_goal" => revise_goal(ctx.agent.as_deref_mut(), args),
    "update_phase" => update_phase(ctx.agent.as_deref_mut(), args),
    "update_todo" => update_todo(ctx.agent.as_deref_mut(), args),
    "load_skill" => load_skill(ctx.agent.as_deref_mut(), args),
    "load_worker_template" => load_worker_template(args),
    "dispatch_worker" => dispatch_worker(args).await,
    "start_workers" => start_workers(ctx.agent.as_deref_mut(), args).await,
    "check_workers" => check_workers(ctx.agent.as_deref_mut(), args).await,
    "question" => bail!("interactive mode required"),
    "worker_question" => worker_question(args),
    "worker_complete" => worker_complete(ctx.agent.as_deref_mut(), args),
    "complete" => complete(ctx.agent.as_deref_mut(), args),
    _ => bail!("unknown tool: {name}"),
  }
}

static CODER_TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();
static WORKER_TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();

pub fn configured_coder_tools(_steer: bool) -> Vec<Tool> {
  CODER_TOOLS.get_or_init(build_coder_tools).clone()
}

pub fn configured_worker_tools() -> Vec<Tool> {
  WORKER_TOOLS.get_or_init(build_worker_tools).clone()
}

const WORKER_EXCLUDED: &[&str] = &[
  "dispatch_worker",
  "start_workers",
  "check_workers",
  "handoff",
  "complete",
  "question",
  "set_goal",
  "revise_goal",
  "update_phase",
  "update_todo",
  "load_worker_template",
];

fn build_coder_tools() -> Vec<Tool> {
  vec![
    schema(
      "read_file",
      "Read a file from the local filesystem.",
      json!({"type":"object","properties":{"path":{"type":"string"},"start":{"type":"integer"},"end":{"type":"integer"}},"required":["path"],"additionalProperties":false}),
    ),
    schema(
      "write_file",
      "Write content to a new file. For existing files, prefer edit_hash_anchors.",
      json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},"overwrite_existing":{"type":"boolean"}},"required":["path","content"],"additionalProperties":false}),
    ),
    schema(
      "bash",
      "Execute a shell command in the workspace and return stdout and stderr combined.",
      json!({"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer"}},"required":["command"],"additionalProperties":false}),
    ),
    schema(
      "repo_map",
      "Display a tree map of the repository directory structure.",
      json!({"type":"object","properties":{"path":{"type":"string"},"levels":{"type":"integer"}},"additionalProperties":false}),
    ),
    schema(
      "read_hash_anchors",
      "Read a file returning each line prefixed as line:hash|content.",
      json!({"type":"object","properties":{"path":{"type":"string"},"start":{"type":"integer"},"end":{"type":"integer"}},"required":["path"],"additionalProperties":false}),
    ),
    schema(
      "edit_hash_anchors",
      "Edit a file using hashline anchors from read_hash_anchors.",
      json!({"type":"object","properties":{"path":{"type":"string"},"ops":{"type":"array","items":{"type":"object","properties":{"anchor":{"type":"string"},"end_anchor":{"type":"string"},"action":{"type":"string","enum":["replace","before","after"]},"new_string":{"type":"string"}},"required":["anchor","action","new_string"]}}},"required":["path","ops"],"additionalProperties":false}),
    ),
    schema(
      "web_search",
      "Search the web using Exa.",
      json!({"type":"object","properties":{"query":{"type":"string"},"num_results":{"type":"integer"},"type":{"type":"string","enum":["auto","deep-reasoning"]}},"required":["query"],"additionalProperties":false}),
    ),
    schema(
      "web_read",
      "Read the content of one or more URLs using Exa.",
      json!({"type":"object","properties":{"urls":{"type":"array","items":{"type":"string"}},"mode":{"type":"string","enum":["text","highlights"]}},"required":["urls"],"additionalProperties":false}),
    ),
    schema(
      "code_web_context",
      "Search for code examples and practical implementation context.",
      json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}),
    ),
    schema(
      "dispatch_worker",
      "Hire a specialist coworker. system_prompt shapes worker behavior/scope; task states the concrete assignment. The worker runs as a separate process and returns a Markdown summary.",
      json!({"type":"object","properties":{"system_prompt":{"type":"string","description":"Complete behavior-shaping system prompt for the worker: role, permissions, read/write scope, constraints, commands, and summary format"},"task":{"type":"string","description":"Concrete task-shaping user prompt for the worker: exact assignment, expected output, success criteria, and immediate next step"},"max_turns":{"type":"integer","description":"Optional max turns for the worker (-1=unlimited). If omitted, worker has no turn limit."}},"required":["system_prompt","task"],"additionalProperties":false}),
    ),
    schema(
      "start_workers",
      "Start a batch of specialist coworkers asynchronously and return immediately with worker IDs.",
      json!({"type":"object","properties":{"coworkers":{"type":"array","minItems":1,"items":{"type":"object","properties":{"name":{"type":"string","description":"Optional short unique label for status"},"system_prompt":{"type":"string","description":"Behavior-shaping system prompt: role, permissions, read/write scope, constraints, commands, and summary format"},"task_prompt":{"type":"string","description":"Concrete task prompt: assignment, expected output, success criteria, and immediate next step"},"max_turns":{"type":"integer","description":"Optional max turns for this worker. If omitted or <=0, worker has no turn limit."}},"required":["system_prompt","task_prompt"],"additionalProperties":false}}},"required":["coworkers"],"additionalProperties":false}),
    ),
    schema(
      "check_workers",
      "Wait for all active async coworkers and return reports.",
      json!({"type":"object","properties":{},"additionalProperties":false}),
    ),
    schema(
      "handoff",
      "Write a session handoff brief to disk.",
      json!({"type":"object","properties":{"brief":{"type":"string"}},"required":["brief"],"additionalProperties":false}),
    ),
    schema(
      "set_goal",
      "Initialize runtime task tracking with one Goal.",
      json!({"type":"object","properties":{"goal":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed","blocked","skipped"]},"complexity":{"type":"string","enum":["simple","medium","complex"]},"success_criteria":{"type":"array","items":{"type":"string"}},"notes":{"type":"string"}},"required":["goal","status","complexity"],"additionalProperties":false}),
    ),
    schema(
      "revise_goal",
      "Rarely revise the Goal and record the prior Goal plus reason.",
      json!({"type":"object","properties":{"goal":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed","blocked","skipped"]},"complexity":{"type":"string","enum":["simple","medium","complex"]},"success_criteria":{"type":"array","items":{"type":"string"}},"reason":{"type":"string"},"notes":{"type":"string"}},"required":["goal","status","complexity","reason"],"additionalProperties":false}),
    ),
    schema(
      "update_phase",
      "Upsert one Phase under the current Goal.",
      json!({"type":"object","properties":{"phase_id":{"type":"string"},"title":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed","blocked","skipped"]},"complexity":{"type":"string","enum":["simple","medium","complex"]},"notes":{"type":"string"}},"required":["phase_id","title","status","complexity"],"additionalProperties":false}),
    ),
    schema(
      "update_todo",
      "Upsert one Todo under an existing Phase.",
      json!({"type":"object","properties":{"phase_id":{"type":"string"},"todo_id":{"type":"string"},"title":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed","blocked","skipped"]},"complexity":{"type":"string","enum":["simple","medium","complex"]},"notes":{"type":"string"}},"required":["phase_id","todo_id","title","status","complexity"],"additionalProperties":false}),
    ),
    schema(
      "load_skill",
      "Load a skill from .ogent/skills/ or .skills/.",
      json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}),
    ),
    schema(
      "load_worker_template",
      "Load a built-in worker template (generic, tester, reviewer). Returns the template content with placeholders. Fill placeholders before using as system_prompt.",
      json!({"type":"object","properties":{"name":{"type":"string","enum":["generic","tester","reviewer"],"description":"Built-in worker template name"}},"required":["name"],"additionalProperties":false}),
    ),
    schema(
      "complete",
      "Mark the current task complete and provide a retrospective structured Markdown session summary.",
      json!({"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"],"additionalProperties":false}),
    ),
    schema(
      "question",
      "Ask the user a question. Only available on the first turn.",
      json!({"type":"object","properties":{"question":{"type":"string"}},"required":["question"],"additionalProperties":false}),
    ),
  ]
}

fn build_worker_tools() -> Vec<Tool> {
  let mut tools: Vec<Tool> = build_coder_tools()
    .into_iter()
    .filter(|t| !WORKER_EXCLUDED.contains(&t.function.name.as_str()))
    .collect();
  tools.push(schema("worker_question", "Ask the parent coder agent a question when blocked.", json!({"type":"object","properties":{"question":{"type":"string"}},"required":["question"],"additionalProperties":false})));
  tools.push(schema("worker_complete", "Finish this worker subprocess and return a concise Markdown summary to the parent coder.", json!({"type":"object","properties":{"summary":{"type":"string","description":"Concise Markdown summary for the parent coder"}},"required":["summary"],"additionalProperties":false})));
  tools
}

pub fn remove_question(tools: &mut Vec<Tool>) {
  tools.retain(|t| t.function.name != "question");
}

pub fn is_read_only_tool(name: &str) -> bool {
  matches!(
    name,
    "read_file"
      | "read_hash_anchors"
      | "repo_map"
      | "web_search"
      | "web_read"
      | "code_web_context"
      | "load_worker_template"
  )
}

pub fn parse_args<T: serde::de::DeserializeOwned>(args: &str) -> Result<T> {
  serde_json::from_str(args).context("bad args")
}

pub fn require_nonempty(value: &str, name: &str) -> Result<()> {
  if value.trim().is_empty() {
    bail!("{name} is required");
  }
  Ok(())
}

fn schema(name: &str, description: &str, parameters: Value) -> Tool {
  Tool {
    kind: "function".to_string(),
    function: ToolFunction {
      name: name.to_string(),
      description: description.to_string(),
      parameters,
    },
  }
}

fn require_agent<'a>(
  agent: Option<&'a mut crate::agent::Agent>,
  tool: &str,
) -> Result<&'a mut crate::agent::Agent> {
  agent.with_context(|| format!("{tool} requires an active agent"))
}

fn exa_client() -> &'static reqwest::Client {
  static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
  CLIENT.get_or_init(reqwest::Client::new)
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
      let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
      combined.push_str(&String::from_utf8_lossy(&out.stderr));
      if !out.status.success() {
        bail!("exit err: {}\n{combined}", out.status);
      }
      Ok(combined)
    }
  }
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
    let _ = writeln!(out, "{}{}", "  ".repeat(depth), name.to_string_lossy());
  }
  if path.is_dir() && depth < max_depth {
    let mut entries: Vec<_> = fs::read_dir(path)?.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
      let name = entry.file_name();
      let name = name.to_string_lossy();
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
  let search_type = if args.search_type.is_empty() {
    "auto"
  } else {
    &args.search_type
  };
  let body = json!({"query": args.query, "type": search_type, "numResults": n, "contents": {"highlights": true}});
  let v = exa_post("https://api.exa.ai/search", body).await?;
  let mut out = String::new();
  for (i, r) in v["results"].as_array().into_iter().flatten().enumerate() {
    let _ = writeln!(out, "{}. {}", i + 1, r["title"].as_str().unwrap_or(""));
    let _ = writeln!(out, "   {}", r["url"].as_str().unwrap_or(""));
    if let Some(highlights) = r["highlights"].as_array() {
      for h in highlights {
        let _ = writeln!(out, "   > {}", h.as_str().unwrap_or(""));
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
  for r in v["results"].as_array().into_iter().flatten() {
    let _ = writeln!(out, "--- {} ---", r["title"].as_str().unwrap_or(""));
    let _ = writeln!(out, "{}", r["url"].as_str().unwrap_or(""));
    out.push('\n');
    if mode == "text" {
      out.push_str(r["text"].as_str().unwrap_or(""));
      out.push_str("\n\n");
    } else if let Some(highlights) = r["highlights"].as_array() {
      for h in highlights {
        let _ = writeln!(out, "> {}", h.as_str().unwrap_or(""));
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

fn exa_api_key() -> Result<&'static str> {
  static KEY: OnceLock<String> = OnceLock::new();
  let key = KEY.get_or_init(|| std::env::var("EXA_API_KEY").unwrap_or_default());
  if key.is_empty() {
    bail!("EXA_API_KEY not set");
  }
  Ok(key)
}

async fn exa_post(url: &str, body: Value) -> Result<Value> {
  let key = exa_api_key()?;
  let resp = exa_client()
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
  let mut body = args.brief.trim_end().to_string();
  if let Some(tracker) = agent.as_ref().and_then(|agent| agent.task_tracker.as_ref()) {
    let appendix = tracker.render_handoff_appendix();
    if !appendix.is_empty() {
      if !body.is_empty() {
        body.push_str("\n\n");
      }
      body.push_str(&appendix);
    }
  }
  fs::write(&path, body)?;
  if let Some(agent) = agent {
    agent.compact.last_handoff_path = path.clone();
  }
  Ok(format!("Handoff written to {path}"))
}

#[derive(Deserialize)]
struct SetGoalArgs {
  goal: String,
  status: Status,
  complexity: Complexity,
  #[serde(default)]
  success_criteria: Vec<String>,
  #[serde(default)]
  notes: String,
}

fn set_goal(agent: Option<&mut crate::agent::Agent>, args: &str) -> Result<String> {
  let args: SetGoalArgs = parse_args(args)?;
  require_nonempty(&args.goal, "goal")?;
  let agent = require_agent(agent, "set_goal")?;
  if agent.task_tracker.is_some() {
    bail!("set_goal can only be called once; use revise_goal for goal changes");
  }
  agent.task_tracker = Some(TaskTracker::new(GoalState {
    title: args.goal.trim().to_string(),
    status: args.status,
    complexity: args.complexity,
    success_criteria: clean_strings(args.success_criteria),
    notes: args.notes.trim().to_string(),
  }));
  Ok(
    agent
      .task_tracker
      .as_ref()
      .map(crate::task_tracker::TaskTracker::render_tool_snapshot)
      .unwrap_or_else(|| "Goal initialized.".to_string()),
  )
}

#[derive(Deserialize)]
struct ReviseGoalArgs {
  goal: String,
  status: Status,
  complexity: Complexity,
  #[serde(default)]
  success_criteria: Vec<String>,
  reason: String,
  #[serde(default)]
  notes: String,
}

fn revise_goal(agent: Option<&mut crate::agent::Agent>, args: &str) -> Result<String> {
  let args: ReviseGoalArgs = parse_args(args)?;
  require_nonempty(&args.goal, "goal")?;
  require_nonempty(&args.reason, "reason")?;
  let agent = require_agent(agent, "revise_goal")?;
  let Some(tracker) = agent.task_tracker.as_mut() else {
    bail!("set_goal must be called before revise_goal");
  };
  tracker.revise_goal(
    GoalState {
      title: args.goal.trim().to_string(),
      status: args.status,
      complexity: args.complexity,
      success_criteria: clean_strings(args.success_criteria),
      notes: args.notes.trim().to_string(),
    },
    args.reason.trim().to_string(),
  );
  Ok(tracker.render_tool_snapshot())
}

#[derive(Deserialize)]
struct UpdatePhaseArgs {
  phase_id: String,
  title: String,
  status: Status,
  complexity: Complexity,
  #[serde(default)]
  notes: String,
}

fn update_phase(agent: Option<&mut crate::agent::Agent>, args: &str) -> Result<String> {
  let args: UpdatePhaseArgs = parse_args(args)?;
  require_nonempty(&args.phase_id, "phase_id")?;
  require_nonempty(&args.title, "title")?;
  let agent = require_agent(agent, "update_phase")?;
  let Some(tracker) = agent.task_tracker.as_mut() else {
    bail!("set_goal must be called before update_phase");
  };
  if let Some(ref mut ws) = agent.workflow_state {
    if args.status == Status::InProgress {
      ws.transition_to(&args.phase_id)?;
    } else if args.status == Status::Completed
      && ws.current_phase.as_deref() != Some(&args.phase_id)
    {
      // Agent may mark a terminal phase completed without ever setting it in_progress.
      // Transition workflow state so complete/terminal checks align.
      let _ = ws.transition_to(&args.phase_id);
    }
  }
  tracker.update_phase(PhaseUpdate {
    id: args.phase_id.trim().to_string(),
    title: args.title.trim().to_string(),
    status: args.status,
    complexity: args.complexity,
    notes: args.notes.trim().to_string(),
  });
  Ok(tracker.render_tool_snapshot())
}

#[derive(Deserialize)]
struct UpdateTodoArgs {
  phase_id: String,
  todo_id: String,
  title: String,
  status: Status,
  complexity: Complexity,
  #[serde(default)]
  notes: String,
}

fn update_todo(agent: Option<&mut crate::agent::Agent>, args: &str) -> Result<String> {
  let args: UpdateTodoArgs = parse_args(args)?;
  require_nonempty(&args.phase_id, "phase_id")?;
  require_nonempty(&args.todo_id, "todo_id")?;
  require_nonempty(&args.title, "title")?;
  let agent = require_agent(agent, "update_todo")?;
  let Some(tracker) = agent.task_tracker.as_mut() else {
    bail!("set_goal must be called before update_todo");
  };
  tracker.update_todo(TodoUpdate {
    phase_id: args.phase_id.trim().to_string(),
    id: args.todo_id.trim().to_string(),
    title: args.title.trim().to_string(),
    status: args.status,
    complexity: args.complexity,
    notes: args.notes.trim().to_string(),
  })?;
  Ok(tracker.render_tool_snapshot())
}

#[derive(Deserialize)]
struct LoadSkillArgs {
  name: String,
}

fn load_skill(agent: Option<&mut crate::agent::Agent>, args: &str) -> Result<String> {
  let args: LoadSkillArgs = parse_args(args)?;
  require_nonempty(&args.name, "name")?;
  let (name, root, body, workflow) = crate::prompts::load_skill_content(&args.name)?;
  if let Some(agent) = agent
    && let Some(wf) = workflow
  {
    agent.workflow_state = Some(crate::workflow::WorkflowState::new(wf));
  }
  Ok(format!(
    "<skill name=\"{name}\" root=\"{root}\">\n{body}\n</skill>"
  ))
}

#[derive(Deserialize)]
struct LoadWorkerTemplateArgs {
  name: String,
}

fn load_worker_template(args: &str) -> Result<String> {
  let args: LoadWorkerTemplateArgs = parse_args(args)?;
  require_nonempty(&args.name, "name")?;
  let template = crate::prompts::get_worker_template(&args.name).with_context(|| {
    format!(
      "unknown worker template: {}. Use generic, tester, or reviewer.",
      args.name
    )
  })?;
  Ok(format!(
    "<worker_template name=\"{}\">\n{}\n</worker_template>",
    args.name, template
  ))
}

#[derive(Deserialize)]
struct DispatchWorkerArgs {
  system_prompt: String,
  task: String,
  #[serde(default)]
  max_turns: i32,
}

async fn dispatch_worker(args: &str) -> Result<String> {
  let args: DispatchWorkerArgs = parse_args(args)?;
  require_nonempty(&args.system_prompt, "system_prompt")?;
  require_nonempty(&args.task, "task")?;
  let result = crate::workers::run_worker_process(crate::workers::WorkerProcessArgs {
    system_prompt: args.system_prompt,
    task_prompt: args.task,
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

async fn check_workers(agent: Option<&mut crate::agent::Agent>, _args: &str) -> Result<String> {
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
  let agent = require_agent(agent, "complete")?;
  // Workflow gate
  if let Some(ref ws) = agent.workflow_state
    && let Some(ref phase) = ws.current_phase
    && let Some(def) = ws.definition.phases.get(phase)
    && !def.terminal
  {
    if !agent.complete_open_work_warned {
      agent.complete_open_work_warned = true;
      return Ok(format!(
        "WARNING: Workflow not complete. Current phase '{}' is not terminal. Allowed exits: {:?}. Call complete again with explicit Limitation and Intent if you must stop.",
        phase, def.next
      ));
    }
    if !summary_has_limitation_and_intent(&args.summary) {
      bail!("Workflow incomplete; second complete requires explicit Limitation and Intent");
    }
  }
  if agent
    .task_tracker
    .as_ref()
    .is_some_and(crate::task_tracker::TaskTracker::open_phase_or_todo_exists)
  {
    if !agent.complete_open_work_warned {
      agent.complete_open_work_warned = true;
      return Ok("WARNING: tracked work is still open. Call complete again only if you intend to stop now, and include explicit \"Limitation\" and \"Intent\" text in summary.".to_string());
    }
    if !summary_has_limitation_and_intent(&args.summary) {
      bail!(
        "tracked work is still open; second complete requires explicit Limitation and Intent in summary"
      );
    }
  }
  agent.completion_summary = Some(args.summary.trim().to_string());
  Ok("Task marked complete.".to_string())
}

fn worker_complete(agent: Option<&mut crate::agent::Agent>, args: &str) -> Result<String> {
  let args: CompleteArgs = parse_args(args)?;
  require_nonempty(&args.summary, "summary")?;
  let agent = require_agent(agent, "worker_complete")?;
  agent.completion_summary = Some(args.summary.trim().to_string());
  Ok("Worker marked complete.".to_string())
}

fn summary_has_limitation_and_intent(summary: &str) -> bool {
  let s = summary.to_lowercase();
  s.contains("limitation") && s.contains("intent")
}

fn clean_strings(values: Vec<String>) -> Vec<String> {
  values
    .into_iter()
    .map(|mut s| {
      let trimmed = s.trim();
      if trimmed.len() != s.len() {
        s = trimmed.to_string();
      }
      s
    })
    .filter(|s| !s.is_empty())
    .collect()
}

#[cfg(test)]
mod complete_tests {
  use super::summary_has_limitation_and_intent;

  #[test]
  fn summary_requires_limitation_and_intent() {
    assert!(summary_has_limitation_and_intent(
      "## Limitation\nx\n## Intent\ny"
    ));
    assert!(!summary_has_limitation_and_intent("## Limitation\nx"));
    assert!(!summary_has_limitation_and_intent("## Intent\ny"));
  }
}
