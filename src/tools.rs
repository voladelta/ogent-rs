use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use crate::agent::Agent;
use crate::hashline::{EditOp, apply_anchor_edits, render_hashlines};
use crate::task_tracker::{Complexity, GoalState, PhaseUpdate, Status, TaskTracker, TodoUpdate};
use crate::types::{Tool, ToolFunction};
use crate::workflow::{CheckStatus, ManualCheckInput};

pub struct ToolContext<'a> {
  pub agent: Option<&'a mut Agent>,
}

pub async fn execute_tool(mut ctx: ToolContext<'_>, name: &str, args: &str) -> Result<String> {
  match name {
    "read_file" => read_file(args),
    "write_file" => write_file(args),
    "bash" => bash(args).await,
    "repo_map" => repo_map(args),
    "read_hash_anchors" => read_hash_anchors(args),
    "edit_hash_anchors" => edit_hash_anchors(args),
    "web_search" => web_search(args).await,
    "web_read" => web_read(args).await,
    "code_web_context" => code_web_context(args).await,

    "set_goal" => {
      let agent = ctx
        .agent
        .as_deref_mut()
        .context("set_goal requires an active agent")?;
      set_goal(agent, args)
    }
    "revise_goal" => {
      let agent = ctx
        .agent
        .as_deref_mut()
        .context("revise_goal requires an active agent")?;
      revise_goal(agent, args)
    }
    "update_phase" => {
      let agent = ctx
        .agent
        .as_deref_mut()
        .context("update_phase requires an active agent")?;
      update_phase(agent, args)
    }
    "update_todo" => {
      let agent = ctx
        .agent
        .as_deref_mut()
        .context("update_todo requires an active agent")?;
      update_todo(agent, args)
    }
    "workflow_status" => {
      let agent = ctx
        .agent
        .as_deref_mut()
        .context("workflow_status requires an active agent")?;
      workflow_status(agent, args)
    }
    "workflow_enter_step" => {
      let agent = ctx
        .agent
        .as_deref_mut()
        .context("workflow_enter_step requires an active agent")?;
      workflow_enter_step(agent, args)
    }
    "workflow_record_check" => {
      let agent = ctx
        .agent
        .as_deref_mut()
        .context("workflow_record_check requires an active agent")?;
      workflow_record_check(agent, args)
    }
    "workflow_run_check" => {
      let agent = ctx
        .agent
        .as_deref_mut()
        .context("workflow_run_check requires an active agent")?;
      workflow_run_check(agent, args).await
    }
    "load_skill" => load_skill(ctx.agent.as_deref_mut(), args),
    "dispatch_worker" => dispatch_worker(args).await,
    "start_workers" => {
      start_workers(
        ctx
          .agent
          .as_deref_mut()
          .context("start_workers requires an active agent")?,
        args,
      )
      .await
    }
    "check_workers" => {
      check_workers(
        ctx
          .agent
          .as_deref_mut()
          .context("check_workers requires an active agent")?,
        args,
      )
      .await
    }
    "worker_complete" => {
      let agent = ctx
        .agent
        .as_deref_mut()
        .context("worker_complete requires an active agent")?;
      worker_complete(agent, args)
    }
    "complete" => {
      let agent = ctx
        .agent
        .as_deref_mut()
        .context("complete requires an active agent")?;
      complete(agent, args)
    }
    _ => bail!("unknown tool: {name}"),
  }
}

static CODER_TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();
static CODER_TOOLS_WITH_WORKFLOW: OnceLock<Vec<Tool>> = OnceLock::new();
static WORKER_TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();

pub fn configured_coder_tools(workflow_enabled: bool) -> Vec<Tool> {
  if workflow_enabled {
    CODER_TOOLS_WITH_WORKFLOW
      .get_or_init(|| build_coder_tools(true))
      .clone()
  } else {
    CODER_TOOLS.get_or_init(|| build_coder_tools(false)).clone()
  }
}

pub fn configured_worker_tools() -> Vec<Tool> {
  WORKER_TOOLS.get_or_init(build_worker_tools).clone()
}

const WORKER_EXCLUDED: &[&str] = &[
  "dispatch_worker",
  "start_workers",
  "check_workers",
  "complete",
  "set_goal",
  "revise_goal",
  "update_phase",
  "update_todo",
  "workflow_status",
  "workflow_enter_step",
  "workflow_record_check",
  "workflow_run_check",
];

fn build_coder_tools(workflow_enabled: bool) -> Vec<Tool> {
  let mut tools = vec![
    schema(
      "read_file",
      "Read a file from the local filesystem. Use start and end as 1-indexed line numbers; omit both for the full file.",
      json!({"type":"object","properties":{"path":{"type":"string"},"start":{"type":"integer","description":"1-indexed start line (inclusive)"},"end":{"type":"integer","description":"1-indexed end line (inclusive)"}},"required":["path"],"additionalProperties":false}),
    ),
    schema(
      "write_file",
      "Write content to a new file. For existing files, prefer edit_hash_anchors.",
      json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},"overwrite_existing":{"type":"boolean"}},"required":["path","content"],"additionalProperties":false}),
    ),
    schema(
      "bash",
      "Execute a shell command in the workspace root and return stdout and stderr combined. Default timeout is 120s if omitted or 0; max is 600s.",
      json!({"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer","description":"Max seconds. Default: 120 if 0 or omitted. Max: 600."}},"required":["command"],"additionalProperties":false}),
    ),
    schema(
      "repo_map",
      "Display a tree map of the repository directory structure. path defaults to the workspace root; levels defaults to 3.",
      json!({"type":"object","properties":{"path":{"type":"string","description":"Directory path relative to workspace root. Default: \".\""},"levels":{"type":"integer","description":"Max depth to descend. Default: 3 if 0 or omitted."}},"additionalProperties":false}),
    ),
    schema(
      "read_hash_anchors",
      "Read a file with each line prefixed as <line>:<hash>|content, where the 4-char hash is derived from line content. Use before edit_hash_anchors to generate stable anchors.",
      json!({"type":"object","properties":{"path":{"type":"string"},"start":{"type":"integer","description":"1-indexed start line (inclusive)"},"end":{"type":"integer","description":"1-indexed end line (inclusive)"}},"required":["path"],"additionalProperties":false}),
    ),
    schema(
      "edit_hash_anchors",
      "Edit a file using hashline anchors from read_hash_anchors. Anchors must be <line>:<4-char-hash> (e.g., \"15:af63\"); use end_anchor for multi-line ranges. new_string replaces the entire anchored line(s).",
      json!({"type":"object","properties":{"path":{"type":"string"},"ops":{"type":"array","items":{"type":"object","properties":{"anchor":{"type":"string","description":"Anchor in <line-number>:<4-char-hash> format (e.g., 15:af63)"},"end_anchor":{"type":"string","description":"Optional end anchor in <line-number>:<4-char-hash> format for range replacement"},"action":{"type":"string","enum":["replace","insert_before","insert_after"]},"new_string":{"type":"string"}},"required":["anchor","action","new_string"]}}},"required":["path","ops"],"additionalProperties":false}),
    ),
    schema(
      "web_search",
      "Search the web for relevant excerpts. Use type=auto for quick facts and deep-reasoning for complex or niche topics.",
      json!({"type":"object","properties":{"query":{"type":"string"},"num_results":{"type":"integer"},"type":{"type":"string","enum":["auto","deep-reasoning"]}},"required":["query"],"additionalProperties":false}),
    ),
    schema(
      "web_read",
      "Read key excerpts from one or more URLs. Set mode=text for full text or highlights for key excerpts.",
      json!({"type":"object","properties":{"urls":{"type":"array","items":{"type":"string"}},"mode":{"type":"string","enum":["text","highlights"],"description":"text for full page text, highlights for key excerpts. Default: highlights."}},"required":["urls"],"additionalProperties":false}),
    ),
    schema(
      "code_web_context",
      "Search real code for syntax, APIs, and patterns to avoid hallucinating implementation details. Not for general web search or URL reading.",
      json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}),
    ),
    schema(
      "dispatch_worker",
      "Hire a specialist coworker. ogent generates the worker's system prompt via an architect LLM call using the template and context you provide. Built-in templates bypass architect generation. The worker runs as a separate process and returns a Markdown summary.",
      json!({"type":"object","properties":{"task":{"type":"string","description":"What the worker should accomplish — exact assignment, expected output, success criteria"},"template":{"type":"string","description":"Worker template or concise custom role: generic, coder, tester, reviewer, validator, etc. Default: generic."},"context":{"type":"string","description":"Markdown context for the worker: project info, files, commands, constraints, known facts"}},"required":["task"],"additionalProperties":false}),
    ),
    schema(
      "start_workers",
      "Start a batch of specialist coworkers asynchronously. ogent generates each worker's system prompt via an architect LLM call unless a built-in template is used.",
      json!({"type":"object","properties":{"coworkers":{"type":"array","minItems":1,"items":{"type":"object","properties":{"name":{"type":"string","description":"Optional short unique label for status"},"task":{"type":"string","description":"What the worker should accomplish"},"template":{"type":"string","description":"Worker template or concise custom role: generic, coder, tester, reviewer, validator, etc. Default: generic."},"context":{"type":"string","description":"Markdown context: project info, files, commands, constraints, known facts"}},"required":["task"],"additionalProperties":false}}},"required":["coworkers"],"additionalProperties":false}),
    ),
    schema(
      "check_workers",
      "Wait for all active async coworkers and return their reports.",
      json!({"type":"object","properties":{},"additionalProperties":false}),
    ),
    schema(
      "set_goal",
      "Initialize the single top-level Goal for this session. Call once at the start of complex tasks to enable progress tracking.",
      json!({"type":"object","properties":{"goal":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed","blocked","skipped"]},"complexity":{"type":"string","enum":["simple","medium","complex"]},"success_criteria":{"type":"array","items":{"type":"string"}},"notes":{"type":"string"}},"required":["goal","status","complexity"],"additionalProperties":false}),
    ),
    schema(
      "revise_goal",
      "Rarely revise the Goal and record the prior Goal plus reason.",
      json!({"type":"object","properties":{"goal":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed","blocked","skipped"]},"complexity":{"type":"string","enum":["simple","medium","complex"]},"success_criteria":{"type":"array","items":{"type":"string"}},"reason":{"type":"string"},"notes":{"type":"string"}},"required":["goal","status","complexity","reason"],"additionalProperties":false}),
    ),
    schema(
      "update_phase",
      "Add or update a Phase under the current Goal.",
      json!({"type":"object","properties":{"phase_id":{"type":"string"},"title":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed","blocked","skipped"]},"complexity":{"type":"string","enum":["simple","medium","complex"]},"notes":{"type":"string"},"contracts":{"type":"array","description":"Optional validation contracts for this phase. Define behavioral assertions before implementing.","items":{"type":"object","properties":{"id":{"type":"string"},"assertion":{"type":"string"},"command":{"type":"string"}},"required":["id","assertion"],"additionalProperties":false}}},"required":["phase_id","title","status","complexity"],"additionalProperties":false}),
    ),
    schema(
      "update_todo",
      "Add or update a Todo under an existing Phase.",
      json!({"type":"object","properties":{"phase_id":{"type":"string"},"todo_id":{"type":"string"},"title":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed","blocked","skipped"]},"complexity":{"type":"string","enum":["simple","medium","complex"]},"notes":{"type":"string"}},"required":["phase_id","todo_id","title","status","complexity"],"additionalProperties":false}),
    ),
    schema(
      "load_skill",
      "Load a skill from .ogent/skills/ or .skills/.",
      json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}),
    ),
    schema(
      "complete",
      "Mark the current task complete with a Markdown summary. If work is still open, first call returns a warning; call again with explicit Limitation and Intent to force stop.",
      json!({"type":"object","properties":{"summary":{"type":"string","description":"Markdown retrospective. Include Limitation and Intent if forcing early stop."}},"required":["summary"],"additionalProperties":false}),
    ),
  ];
  if workflow_enabled {
    tools.extend(build_workflow_tools());
  }
  tools
}

fn build_workflow_tools() -> Vec<Tool> {
  vec![
    schema(
      "workflow_status",
      "Show the active workflow state.",
      json!({"type":"object","properties":{},"additionalProperties":false}),
    ),
    schema(
      "workflow_enter_step",
      "Move to a workflow step. Enforces start, allowed next transitions, gates, required checks, and max_visits. If a goal tracker exists, mirrors the step as an in-progress phase.",
      json!({"type":"object","properties":{"step_id":{"type":"string"},"reason":{"type":"string","description":"Required when leaving a gated step; otherwise optional."}},"required":["step_id"],"additionalProperties":false}),
    ),
    schema(
      "workflow_record_check",
      "Record manual workflow check evidence. Passed/failed checks require evidence. Waived checks require waiver_reason and waiver_risk.",
      json!({"type":"object","properties":{"step_id":{"type":"string"},"check_id":{"type":"string"},"status":{"type":"string","enum":["passed","failed","waived"]},"evidence":{"type":"string"},"waiver_reason":{"type":"string"},"waiver_risk":{"type":"string"}},"required":["step_id","check_id","status"],"additionalProperties":false}),
    ),
    schema(
      "workflow_run_check",
      "Run a command workflow check and record command, exit code, output excerpt, and pass/fail status. Uses the check's configured command unless command is supplied.",
      json!({"type":"object","properties":{"step_id":{"type":"string"},"check_id":{"type":"string"},"command":{"type":"string"},"timeout_seconds":{"type":"integer","description":"Max seconds. Default: 120 if 0 or omitted. Max: 600."}},"required":["step_id","check_id"],"additionalProperties":false}),
    ),
  ]
}

fn build_worker_tools() -> Vec<Tool> {
  let mut tools: Vec<Tool> = build_coder_tools(false)
    .into_iter()
    .filter(|t| !WORKER_EXCLUDED.contains(&t.function.name.as_str()))
    .collect();
  tools.push(schema("worker_complete", "Finish this worker subprocess and return a concise Markdown summary to the parent coder.", json!({"type":"object","properties":{"summary":{"type":"string","description":"Concise Markdown summary for the parent coder"}},"required":["summary"],"additionalProperties":false})));
  tools
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
      | "load_skill"
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
  if args.start == Some(0) {
    bail!("start line must be >= 1 (lines are 1-indexed)");
  }
  if args.end == Some(0) {
    bail!("end line must be >= 1 (lines are 1-indexed)");
  }
  let start = args.start.unwrap_or(1);
  let end = args.end.unwrap_or(lines.len());
  let slice_start = (start - 1).min(lines.len());
  let slice_end = end.min(lines.len());
  if slice_start > slice_end {
    bail!("start line {start} exceeds end line {end}");
  }
  Ok(lines[slice_start..slice_end].join("\n"))
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

fn check_bash_cds(command: &str) -> Result<()> {
  let mut cmd = command.to_string();
  for sep in ["&&", "||", "|", ";", "\n", "\r"] {
    cmd = cmd.replace(sep, "\n");
  }
  let base = crate::workspace::workspace_root();
  for line in cmd.split('\n') {
    let mut words = line.split_whitespace();
    if words.next() == Some("cd") {
      let path = words.next().unwrap_or("");
      if path.is_empty() {
        bail!(
          "cd without argument is not allowed (would go to $HOME). Use a relative path within the workspace (e.g., ./foo) or /tmp."
        );
      }
      let target = if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
          PathBuf::from(home)
        } else {
          bail!(
            "cd to ~ is not allowed. Use a relative path within the workspace (e.g., ./foo) or /tmp."
          );
        }
      } else if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
          PathBuf::from(home).join(rest)
        } else {
          bail!(
            "cd to ~/... is not allowed. Use a relative path within the workspace (e.g., ./foo) or /tmp."
          );
        }
      } else if path.starts_with('/') {
        PathBuf::from(path)
      } else {
        base.join(path)
      };
      let norm = crate::workspace::normalize(&target);
      let in_workspace = norm.starts_with(base);
      let in_tmp = norm.starts_with(Path::new("/tmp"));
      if !in_workspace && !in_tmp {
        bail!(
          "cd to {path} is not allowed. You cannot cd outside the workspace or /tmp. Use relative paths within the workspace (e.g., ./foo or foo)."
        );
      }
    }
  }
  Ok(())
}

async fn bash(args: &str) -> Result<String> {
  let args: BashArgs = parse_args(args)?;
  require_nonempty(&args.command, "command")?;
  check_bash_cds(&args.command)?;
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

async fn run_workflow_command(command: &str, secs: u64) -> Result<(i32, String)> {
  require_nonempty(command, "command")?;
  check_bash_cds(command)?;
  let mut cmd = Command::new("sh");
  cmd
    .arg("-c")
    .arg(command)
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
      Ok((out.status.code().unwrap_or(-1), combined))
    }
  }
}

#[derive(Deserialize)]
struct RepoMapArgs {
  #[serde(default)]
  path: String,
  #[serde(default)]
  levels: usize,
}

fn repo_map(args: &str) -> Result<String> {
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
    writeln!(out, "{}{}", "  ".repeat(depth), name.to_string_lossy()).unwrap();
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
  if args.start == Some(0) || args.end == Some(0) {
    bail!("start and end line numbers must be >= 1 (1-indexed)");
  }
  let path = crate::workspace::workspace_path(&args.path)?;
  let source = fs::read_to_string(&path).with_context(|| format!("read {}", args.path))?;
  Ok(render_hashlines(&source, args.start, args.end))
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
    writeln!(out, "{}. {}", i + 1, r["title"].as_str().unwrap_or("")).unwrap();
    writeln!(out, "   {}", r["url"].as_str().unwrap_or("")).unwrap();
    if let Some(highlights) = r["highlights"].as_array() {
      for h in highlights {
        writeln!(out, "   > {}", h.as_str().unwrap_or("")).unwrap();
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
    writeln!(out, "--- {} ---", r["title"].as_str().unwrap_or("")).unwrap();
    writeln!(out, "{}", r["url"].as_str().unwrap_or("")).unwrap();
    out.push('\n');
    if mode == "text" {
      out.push_str(r["text"].as_str().unwrap_or(""));
      out.push_str("\n\n");
    } else if let Some(highlights) = r["highlights"].as_array() {
      for h in highlights {
        writeln!(out, "> {}", h.as_str().unwrap_or("")).unwrap();
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
struct SetGoalArgs {
  goal: String,
  status: Status,
  complexity: Complexity,
  #[serde(default)]
  success_criteria: Vec<String>,
  #[serde(default)]
  notes: String,
}

fn set_goal(agent: &mut crate::agent::Agent, args: &str) -> Result<String> {
  let args: SetGoalArgs = parse_args(args)?;
  require_nonempty(&args.goal, "goal")?;
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

fn revise_goal(agent: &mut crate::agent::Agent, args: &str) -> Result<String> {
  let args: ReviseGoalArgs = parse_args(args)?;
  require_nonempty(&args.goal, "goal")?;
  require_nonempty(&args.reason, "reason")?;
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
  #[serde(default)]
  contracts: Option<Vec<crate::task_tracker::ValidationContract>>,
}

fn update_phase(agent: &mut crate::agent::Agent, args: &str) -> Result<String> {
  let args: UpdatePhaseArgs = parse_args(args)?;
  require_nonempty(&args.phase_id, "phase_id")?;
  require_nonempty(&args.title, "title")?;
  let Some(tracker) = agent.task_tracker.as_mut() else {
    bail!("set_goal must be called before update_phase");
  };
  tracker.update_phase(PhaseUpdate {
    id: args.phase_id.trim().to_string(),
    title: args.title.trim().to_string(),
    status: args.status,
    complexity: args.complexity,
    notes: args.notes.trim().to_string(),
    contracts: args.contracts,
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

fn update_todo(agent: &mut crate::agent::Agent, args: &str) -> Result<String> {
  let args: UpdateTodoArgs = parse_args(args)?;
  require_nonempty(&args.phase_id, "phase_id")?;
  require_nonempty(&args.todo_id, "todo_id")?;
  require_nonempty(&args.title, "title")?;
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

fn workflow_status(agent: &mut crate::agent::Agent, _args: &str) -> Result<String> {
  Ok(
    agent
      .workflow_state
      .as_ref()
      .map(crate::workflow::WorkflowState::render_status)
      .unwrap_or_else(|| "No active workflow.".to_string()),
  )
}

#[derive(Deserialize)]
struct WorkflowEnterStepArgs {
  step_id: String,
  #[serde(default)]
  reason: String,
}

fn workflow_enter_step(agent: &mut crate::agent::Agent, args: &str) -> Result<String> {
  let args: WorkflowEnterStepArgs = parse_args(args)?;
  require_nonempty(&args.step_id, "step_id")?;
  let Some(ws) = agent.workflow_state.as_mut() else {
    bail!("no active workflow; start ogent with --workflow to enable workflow enforcement");
  };
  ws.enter_step(&args.step_id, &args.reason, crate::session::timestamp_ms())?;
  if let Some(tracker) = agent.task_tracker.as_mut()
    && let Some(step) = ws.definition.steps.get(args.step_id.trim())
  {
    tracker.update_phase(PhaseUpdate {
      id: args.step_id.trim().to_string(),
      title: if step.title.trim().is_empty() {
        args.step_id.trim().to_string()
      } else {
        step.title.trim().to_string()
      },
      status: Status::InProgress,
      complexity: Complexity::Medium,
      notes: step.instructions.trim().to_string(),
      contracts: None,
    });
  }
  Ok(ws.render_status())
}

#[derive(Deserialize)]
struct WorkflowRecordCheckArgs {
  step_id: String,
  check_id: String,
  status: CheckStatus,
  #[serde(default)]
  evidence: String,
  #[serde(default)]
  waiver_reason: String,
  #[serde(default)]
  waiver_risk: String,
}

fn workflow_record_check(agent: &mut crate::agent::Agent, args: &str) -> Result<String> {
  let args: WorkflowRecordCheckArgs = parse_args(args)?;
  require_nonempty(&args.step_id, "step_id")?;
  require_nonempty(&args.check_id, "check_id")?;
  let Some(ws) = agent.workflow_state.as_mut() else {
    bail!("no active workflow; start ogent with --workflow to enable workflow enforcement");
  };
  ws.record_check(ManualCheckInput {
    step_id: args.step_id.trim(),
    check_id: args.check_id.trim(),
    status: args.status,
    evidence: &args.evidence,
    waiver_reason: &args.waiver_reason,
    waiver_risk: &args.waiver_risk,
    timestamp_ms: crate::session::timestamp_ms(),
  })?;
  Ok(ws.render_status())
}

#[derive(Deserialize)]
struct WorkflowRunCheckArgs {
  step_id: String,
  check_id: String,
  #[serde(default)]
  command: String,
  #[serde(default)]
  timeout_seconds: u64,
}

async fn workflow_run_check(agent: &mut crate::agent::Agent, args: &str) -> Result<String> {
  let args: WorkflowRunCheckArgs = parse_args(args)?;
  require_nonempty(&args.step_id, "step_id")?;
  require_nonempty(&args.check_id, "check_id")?;
  let command = {
    let Some(ws) = agent.workflow_state.as_ref() else {
      bail!("no active workflow; start ogent with --workflow to enable workflow enforcement");
    };
    if args.command.trim().is_empty() {
      ws.command_for_check(args.step_id.trim(), args.check_id.trim())?
        .context("workflow check has no configured command; supply command")?
    } else {
      args.command.trim().to_string()
    }
  };
  let secs = if args.timeout_seconds == 0 {
    120
  } else {
    args.timeout_seconds
  };
  if secs > 600 {
    bail!("timeout_seconds must be <= 600");
  }
  let (exit_code, output) = run_workflow_command(&command, secs).await?;
  let output_excerpt = truncate_output(&output, 4000);
  let evidence = if output_excerpt.trim().is_empty() {
    format!("command `{command}` exited with code {exit_code} and produced no output")
  } else {
    format!("command `{command}` exited with code {exit_code}\n{output_excerpt}")
  };
  let Some(ws) = agent.workflow_state.as_mut() else {
    bail!("no active workflow; start ogent with --workflow to enable workflow enforcement");
  };
  ws.record_command_check(
    args.step_id.trim(),
    args.check_id.trim(),
    &command,
    exit_code,
    &evidence,
    crate::session::timestamp_ms(),
  )?;
  Ok(format!(
    "{}\n\nCommand exit_code={exit_code}\n{}",
    ws.render_status(),
    output_excerpt
  ))
}

#[derive(Deserialize)]
struct LoadSkillArgs {
  name: String,
}

fn load_skill(agent: Option<&mut crate::agent::Agent>, args: &str) -> Result<String> {
  let args: LoadSkillArgs = parse_args(args)?;
  require_nonempty(&args.name, "name")?;
  let _ = agent;
  let (name, root, body) = crate::prompts::load_skill_content(&args.name)?;
  Ok(format!(
    "<skill name=\"{name}\" root=\"{root}\">\n{body}\n</skill>"
  ))
}

async fn dispatch_worker(args: &str) -> Result<String> {
  let args: crate::workers::DispatchWorkerArgs = parse_args(args)?;
  require_nonempty(&args.task, "task")?;
  let (system_prompt, task_prompt) =
    crate::workers::resolve_worker_prompts(&args.template, &args.task, &args.context)
      .await
      .context("architect failed for dispatch_worker")?;
  let result = crate::workers::run_worker_process(crate::workers::WorkerProcessArgs {
    system_prompt,
    task_prompt,
    stream_stderr: true,
  })
  .await;
  crate::workers::format_dispatch_worker_result(result)
}

async fn start_workers(agent: &mut crate::agent::Agent, args: &str) -> Result<String> {
  let args: crate::workers::StartWorkersArgs = parse_args(args)?;
  agent.worker_manager.start(args).await
}

async fn check_workers(agent: &mut crate::agent::Agent, _args: &str) -> Result<String> {
  Ok(agent.worker_manager.check().await)
}

#[derive(Deserialize)]
struct CompleteArgs {
  summary: String,
}

fn complete(agent: &mut crate::agent::Agent, args: &str) -> Result<String> {
  let args: CompleteArgs = parse_args(args)?;
  require_nonempty(&args.summary, "summary")?;
  if let Some(ref ws) = agent.workflow_state {
    ws.ensure_current_step_is_terminal()?;
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

fn worker_complete(agent: &mut crate::agent::Agent, args: &str) -> Result<String> {
  let args: CompleteArgs = parse_args(args)?;
  require_nonempty(&args.summary, "summary")?;
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
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .collect()
}

fn truncate_output(s: &str, n: usize) -> String {
  if s.len() <= n {
    return s.to_string();
  }
  let mut out = s.to_string();
  let end = out.floor_char_boundary(n);
  out.truncate(end);
  out.push_str("\n...[truncated]");
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn summary_requires_limitation_and_intent() {
    assert!(summary_has_limitation_and_intent(
      "## Limitation\nx\n## Intent\ny"
    ));
    assert!(!summary_has_limitation_and_intent("## Limitation\nx"));
    assert!(!summary_has_limitation_and_intent("## Intent\ny"));
  }

  #[test]
  fn is_read_only_tool_classification() {
    assert!(is_read_only_tool("read_file"));
    assert!(is_read_only_tool("read_hash_anchors"));
    assert!(is_read_only_tool("repo_map"));
    assert!(is_read_only_tool("web_search"));
    assert!(is_read_only_tool("web_read"));
    assert!(is_read_only_tool("code_web_context"));
    assert!(is_read_only_tool("load_skill"));
    assert!(!is_read_only_tool("write_file"));
    assert!(!is_read_only_tool("edit_hash_anchors"));
    assert!(!is_read_only_tool("bash"));
    assert!(!is_read_only_tool("complete"));
  }

  #[test]
  fn configured_coder_tools_includes_expected() {
    let tools = configured_coder_tools(false);
    let names: Vec<_> = tools.iter().map(|t| t.function.name.as_str()).collect();
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"write_file"));
    assert!(names.contains(&"bash"));
    assert!(names.contains(&"complete"));
    assert!(names.contains(&"set_goal"));
    assert!(names.contains(&"update_phase"));
    assert!(names.contains(&"update_todo"));
    assert!(!names.contains(&"workflow_status"));
  }

  #[test]
  fn workflow_tools_are_conditional() {
    let tools = configured_coder_tools(true);
    let names: Vec<_> = tools.iter().map(|t| t.function.name.as_str()).collect();
    assert!(names.contains(&"workflow_status"));
    assert!(names.contains(&"workflow_enter_step"));
    assert!(names.contains(&"workflow_record_check"));
    assert!(names.contains(&"workflow_run_check"));
  }

  #[test]
  fn configured_worker_tools_excludes_coder_only() {
    let tools = configured_worker_tools();
    let names: Vec<_> = tools.iter().map(|t| t.function.name.as_str()).collect();
    assert!(!names.contains(&"dispatch_worker"));
    assert!(!names.contains(&"start_workers"));
    assert!(!names.contains(&"check_workers"));
    assert!(!names.contains(&"complete"));
    assert!(!names.contains(&"set_goal"));
    assert!(names.contains(&"worker_complete"));
    assert!(names.contains(&"read_file"));
  }

  #[test]
  fn tool_names_unique_within_coder_tools() {
    let tools = configured_coder_tools(true);
    let mut seen = std::collections::HashSet::new();
    for t in &tools {
      assert!(
        seen.insert(t.function.name.clone()),
        "duplicate coder tool: {}",
        t.function.name
      );
    }
  }

  #[test]
  fn tool_names_unique_within_worker_tools() {
    let tools = configured_worker_tools();
    let mut seen = std::collections::HashSet::new();
    for t in &tools {
      assert!(
        seen.insert(t.function.name.clone()),
        "duplicate worker tool: {}",
        t.function.name
      );
    }
  }

  #[tokio::test]
  async fn execute_tool_unknown_returns_error() {
    let result = execute_tool(ToolContext { agent: None }, "nonexistent_tool", "{}").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("unknown tool"));
  }

  #[test]
  fn read_file_schema_has_path_required() {
    let tools = configured_coder_tools(false);
    let t = tools
      .iter()
      .find(|t| t.function.name == "read_file")
      .unwrap();
    let params = &t.function.parameters;
    assert_eq!(params["type"], "object");
    assert!(params["properties"]["path"].is_object());
    let required: Vec<String> = serde_json::from_value(params["required"].clone()).unwrap();
    assert!(required.contains(&"path".to_string()));
  }

  #[test]
  fn edit_hash_anchors_schema_has_ops_array() {
    let tools = configured_coder_tools(false);
    let t = tools
      .iter()
      .find(|t| t.function.name == "edit_hash_anchors")
      .unwrap();
    let params = &t.function.parameters;
    assert_eq!(params["properties"]["ops"]["type"], "array");
    assert!(params["properties"]["ops"]["items"]["properties"]["anchor"].is_object());
    let action = &params["properties"]["ops"]["items"]["properties"]["action"];
    let enum_vals: Vec<String> = serde_json::from_value(action["enum"].clone()).unwrap();
    assert_eq!(enum_vals, vec!["replace", "insert_before", "insert_after"]);
  }

  #[test]
  fn update_phase_schema_includes_contracts() {
    let tools = configured_coder_tools(false);
    let t = tools
      .iter()
      .find(|t| t.function.name == "update_phase")
      .unwrap();
    let params = &t.function.parameters;
    assert!(params["properties"]["contracts"].is_object());
    assert_eq!(params["properties"]["contracts"]["type"], "array");
    let item = &params["properties"]["contracts"]["items"];
    assert!(item["properties"]["id"].is_object());
    assert!(item["properties"]["assertion"].is_object());
    let required: Vec<String> = serde_json::from_value(item["required"].clone()).unwrap();
    assert!(required.contains(&"id".to_string()));
    assert!(required.contains(&"assertion".to_string()));
  }

  #[test]
  fn bash_schema_has_command_and_timeout() {
    let tools = configured_coder_tools(false);
    let t = tools.iter().find(|t| t.function.name == "bash").unwrap();
    let params = &t.function.parameters;
    assert!(params["properties"]["command"].is_object());
    assert!(params["properties"]["timeout_seconds"].is_object());
    let required: Vec<String> = serde_json::from_value(params["required"].clone()).unwrap();
    assert!(required.contains(&"command".to_string()));
  }

  #[test]
  fn dispatch_worker_schema_has_task_template_and_context() {
    let tools = configured_coder_tools(false);
    let t = tools
      .iter()
      .find(|t| t.function.name == "dispatch_worker")
      .unwrap();
    let params = &t.function.parameters;
    assert!(params["properties"]["task"].is_object());
    assert!(params["properties"]["template"].is_object());
    assert!(params["properties"]["context"].is_object());
    let required: Vec<String> = serde_json::from_value(params["required"].clone()).unwrap();
    assert!(required.contains(&"task".to_string()));
    assert!(!required.contains(&"template".to_string()));
    assert!(!required.contains(&"context".to_string()));
  }

  #[test]
  fn complete_schema_has_summary_required() {
    let tools = configured_coder_tools(false);
    let t = tools
      .iter()
      .find(|t| t.function.name == "complete")
      .unwrap();
    let params = &t.function.parameters;
    assert!(params["properties"]["summary"].is_object());
    let required: Vec<String> = serde_json::from_value(params["required"].clone()).unwrap();
    assert!(required.contains(&"summary".to_string()));
  }

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

  #[test]
  fn check_bash_cds_allows_workspace_relative() {
    assert!(check_bash_cds("cd src").is_ok());
    assert!(check_bash_cds("cd ./src").is_ok());
    assert!(check_bash_cds("cd .").is_ok());
  }

  #[test]
  fn check_bash_cds_allows_tmp() {
    assert!(check_bash_cds("cd /tmp").is_ok());
    assert!(check_bash_cds("cd /tmp/foo").is_ok());
  }

  #[test]
  fn check_bash_cds_rejects_outside_workspace_and_tmp() {
    assert!(check_bash_cds("cd /etc").is_err());
    assert!(check_bash_cds("cd /").is_err());
    assert!(check_bash_cds("cd ..").is_err());
    assert!(check_bash_cds("cd ../..").is_err());
  }

  #[test]
  fn check_bash_cds_rejects_home() {
    assert!(check_bash_cds("cd ~").is_err());
    assert!(check_bash_cds("cd ~/projects").is_err());
  }

  #[test]
  fn check_bash_cds_rejects_bare_cd() {
    assert!(check_bash_cds("cd").is_err());
  }

  #[test]
  fn check_bash_cds_checks_all_separated_commands() {
    assert!(check_bash_cds("cd src && cd /etc").is_err());
    assert!(check_bash_cds("cd src; cd /etc").is_err());
    assert!(check_bash_cds("cd src || cd /etc").is_err());
    assert!(check_bash_cds("cd src | cd /etc").is_err());
    assert!(check_bash_cds("cd src\ncd /etc").is_err());
    assert!(check_bash_cds("cd src && cd .").is_ok());
    assert!(check_bash_cds("cd /tmp; cd /etc").is_err());
  }
}
