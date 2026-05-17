use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use crate::agent::Agent;
use crate::hashline::{EditOp, apply_anchor_edits, render_hashlines};
use crate::types::{Tool, ToolFunction};

pub struct ToolContext<'a> {
  pub agent: Option<&'a mut Agent>,
}

pub async fn execute_tool(mut ctx: ToolContext<'_>, name: &str, args: &str) -> Result<String> {
  match name {
    "read_file" => read_file(args),
    "write_file" => write_file(args),
    "bash" => {
      let director_mode = ctx
        .agent
        .as_ref()
        .is_some_and(|agent| !agent.meta.flags.worker);
      if director_mode {
        director_bash(args).await
      } else {
        bash(args).await
      }
    }
    "repo_map" => repo_map(args),
    "read_hash_anchors" => read_hash_anchors(args),
    "edit_hash_anchors" => edit_hash_anchors(args),
    "web_search" => web_search(args).await,
    "web_read" => web_read(args).await,
    "web_code_context" => web_code_context(args).await,
    "load_skill" => load_skill(args),
    "state" => {
      let agent = ctx
        .agent
        .as_deref_mut()
        .context("state requires an active agent")?;
      state(agent, args)
    }
    "dispatch_workers" => {
      let agent = ctx
        .agent
        .as_deref_mut()
        .context("dispatch_workers requires an active agent")?;
      dispatch_workers(agent, args).await
    }
    "wait_workers" => {
      let agent = ctx
        .agent
        .as_deref_mut()
        .context("wait_workers requires an active agent")?;
      wait_workers(agent, args).await
    }
    _ => bail!("unknown tool: {name}"),
  }
}

static DIRECTOR_TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();
static WORKER_TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();

pub fn configured_director_tools() -> Vec<Tool> {
  DIRECTOR_TOOLS.get_or_init(build_director_tools).clone()
}

pub fn configured_worker_tools() -> Vec<Tool> {
  WORKER_TOOLS.get_or_init(build_worker_tools).clone()
}

fn state_schema_parameters() -> Value {
  json!({
    "type": "object",
    "properties": {
      "action": {"type": "string", "enum": ["read", "write", "append", "list"]},
      "path": {"type": "string"},
      "content": {"type": "string"}
    },
    "required": ["action"],
    "allOf": [
      {
        "if": {"properties": {"action": {"const": "read"}}, "required": ["action"]},
        "then": {"required": ["path"]}
      },
      {
        "if": {"properties": {"action": {"enum": ["write", "append"]}}, "required": ["action"]},
        "then": {"required": ["path", "content"]}
      }
    ],
    "additionalProperties": false
  })
}

fn build_director_tools() -> Vec<Tool> {
  vec![
    schema(
      "bash",
      "Execute one plain search command in the workspace root and return stdout/stderr. Director mode only allows `colgrep` or `rg`; it is not for reading files or shell scripting. Do not use cat/find/ls/head/pipes/redirection/fallbacks. Default timeout is 120s if omitted or 0; max is 600s.",
      json!({"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer","description":"Max seconds. Default: 120 if 0 or omitted. Max: 600."}},"required":["command"],"additionalProperties":false}),
    ),
    schema(
      "repo_map",
      "Display a tree map of the repository directory structure. path defaults to the workspace root; levels defaults to 3.",
      json!({"type":"object","properties":{"path":{"type":"string","description":"Directory path relative to workspace root. Default: \".\""},"levels":{"type":"integer","description":"Max depth to descend. Default: 3 if 0 or omitted."}},"additionalProperties":false}),
    ),
    schema(
      "load_skill",
      "Load a skill from .ogent/skills/ or ~/.ogent/skills/.",
      json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}),
    ),
    schema(
      "state",
      "Read/write/list scoped runtime state in states.json. list accepts an empty path. read requires path. write/append require path and content.",
      state_schema_parameters(),
    ),
    schema(
      "dispatch_workers",
      "Spawn a worker batch and return worker ids immediately. Results are not available yet; call wait_workers next to receive completed worker results. Arguments must be exactly an object with one workers array. Example: {\"workers\":[{\"role\":\"implementer\",\"task\":\"# Task\\nEdit README.md only.\\n\\n# Required output\\nSummary and verification.\"}]}.",
      json!({"type":"object","properties":{"workers":{"type":"array","minItems":1,"items":{"type":"object","properties":{"role":{"type":"string"},"task":{"type":"string"}},"required":["role","task"],"additionalProperties":false}}},"required":["workers"],"additionalProperties":false}),
    ),
    schema(
      "wait_workers",
      "Wait for worker results. Returns immediately if any worker has completed; otherwise waits about 10 seconds before reporting still-running workers. Use after dispatch_workers and repeat until all needed worker results are returned.",
      json!({"type":"object","properties":{},"additionalProperties":false}),
    ),
  ]
}

fn build_worker_tools() -> Vec<Tool> {
  vec![
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
      "web_code_context",
      "Search real code for syntax, APIs, and patterns to avoid hallucinating implementation details. Not for general web search or URL reading.",
      json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}),
    ),
    schema(
      "load_skill",
      "Load a skill from .ogent/skills/ or ~/.ogent/skills/.",
      json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}),
    ),
    schema(
      "state",
      "Read/write/list scoped runtime state in states.json. list accepts an empty path. read requires path. write/append require path and content.",
      state_schema_parameters(),
    ),
  ]
}

pub fn is_read_only_tool(name: &str) -> bool {
  matches!(
    name,
    "read_file"
      | "read_hash_anchors"
      | "repo_map"
      | "web_search"
      | "web_read"
      | "web_code_context"
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

fn check_director_bash_allowlist(command: &str) -> Result<()> {
  for forbidden in ["&&", "||", "|", ";", "\n", "\r", ">", "<", "`", "$("] {
    if command.contains(forbidden) {
      bail!("director bash only allows a single plain `colgrep` or `rg` command");
    }
  }

  let line = command.trim();
  if line.is_empty() || line.starts_with('#') {
    return Ok(());
  }
  let words: Vec<_> = line
    .split_whitespace()
    .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
    .collect();
  let mut i = 0;
  while i < words.len() && words[i].contains('=') && !words[i].contains('/') {
    i += 1;
  }
  let Some(first) = words.get(i).copied() else {
    return Ok(());
  };
  if first != "colgrep" && first != "rg" {
    bail!("director bash only allows `colgrep` and `rg` executables");
  }

  if first == "rg" {
    let lists_only = words
      .iter()
      .skip(i + 1)
      .any(|w| *w == "-l" || *w == "--files-with-matches");
    if !lists_only {
      let pattern = words.iter().skip(i + 1).find(|w| !w.starts_with('-'));
      if pattern.is_some_and(|p| matches!(*p, "" | "." | ".*" | "^" | "$")) {
        bail!("director `rg` cannot be used to dump file contents");
      }
    }
  }
  Ok(())
}

async fn bash(args: &str) -> Result<String> {
  bash_internal(args, false).await
}

pub async fn director_bash(args: &str) -> Result<String> {
  bash_internal(args, true).await
}

async fn bash_internal(args: &str, director_mode: bool) -> Result<String> {
  let args: BashArgs = parse_args(args)?;
  require_nonempty(&args.command, "command")?;
  check_bash_cds(&args.command)?;
  if director_mode {
    check_director_bash_allowlist(&args.command)?;
  }
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

async fn web_code_context(args: &str) -> Result<String> {
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
struct StateArgs {
  action: String,
  #[serde(default)]
  path: String,
  content: Option<String>,
}

fn state(agent: &mut Agent, args: &str) -> Result<String> {
  let args: StateArgs = parse_args(args)?;
  require_nonempty(&args.action, "action")?;
  let scope_path = if let (Some(parent_session_id), Some(worker_id)) = (
    agent.worker_parent_session_id.as_deref(),
    agent.worker_id.as_deref(),
  ) {
    crate::session::worker_state_path(parent_session_id, worker_id)
  } else {
    crate::session::state_path(&agent.meta.session_id)
  };

  let mut map = read_state_map(&scope_path)?;
  match args.action.as_str() {
    "read" => {
      require_nonempty(&args.path, "path")?;
      Ok(serde_json::to_string(&map.get(&args.path).cloned())?)
    }
    "list" => {
      let prefix = args.path.trim();
      let keys: Vec<String> = if prefix.is_empty() {
        map.keys().cloned().collect()
      } else {
        map
          .keys()
          .filter(|k| k.starts_with(prefix))
          .cloned()
          .collect()
      };
      Ok(serde_json::to_string(&keys)?)
    }
    "write" => {
      require_nonempty(&args.path, "path")?;
      let content = args
        .content
        .context("content is required for state write")?;
      map.insert(args.path, content);
      write_state_map(&scope_path, &map)?;
      Ok("ok".to_string())
    }
    "append" => {
      require_nonempty(&args.path, "path")?;
      let content = args
        .content
        .context("content is required for state append")?;
      let entry = map.entry(args.path).or_default();
      entry.push_str(&content);
      write_state_map(&scope_path, &map)?;
      Ok("ok".to_string())
    }
    _ => bail!("action must be one of: read, write, append, list"),
  }
}

fn read_state_map(path: &Path) -> Result<BTreeMap<String, String>> {
  if !path.exists() {
    return Ok(BTreeMap::new());
  }
  let data = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
  if data.trim().is_empty() {
    return Ok(BTreeMap::new());
  }
  serde_json::from_str(&data).with_context(|| format!("parse {}", path.display()))
}

fn write_state_map(path: &Path, map: &BTreeMap<String, String>) -> Result<()> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
  }
  let data = serde_json::to_string_pretty(map)?;
  fs::write(path, data).with_context(|| format!("write {}", path.display()))
}

async fn dispatch_workers(agent: &mut Agent, args: &str) -> Result<String> {
  if agent.meta.flags.worker {
    bail!("worker mode cannot dispatch workers");
  }
  let args: crate::workers::DispatchWorkersArgs =
    serde_json::from_str(args).with_context(|| {
      "bad dispatch_workers args; expected exactly: \
       {\"workers\":[{\"role\":\"implementer\",\"task\":\"# Task\\n...\"}]}. \
       `workers` must be an array of objects, each with string `role` and string `task`; \
       close the JSON with `]}` and do not include `sync`, `worker_ids`, or top-level `role`/`task`"
    })?;
  agent
    .worker_manager
    .dispatch(args, &agent.meta.session_id)
    .await
}

async fn wait_workers(agent: &mut Agent, args: &str) -> Result<String> {
  if agent.meta.flags.worker {
    bail!("worker mode cannot wait on workers");
  }
  let value: serde_json::Value = serde_json::from_str(args).context("bad wait_workers args")?;
  if !value.as_object().is_some_and(|obj| obj.is_empty()) {
    bail!("wait_workers takes no arguments; pass {{}}");
  }
  agent.worker_manager.wait().await
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::client::Client;
  use std::sync::atomic::{AtomicU64, Ordering};

  static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

  fn dummy_client() -> Client {
    Client::new(
      "http://localhost",
      "dummy".into(),
      |_, _| serde_json::Value::Null,
      30,
    )
    .unwrap()
  }

  fn dummy_agent(worker_scope: Option<(&str, &str)>) -> Agent {
    let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let (worker_parent_session_id, worker_id) = worker_scope
      .map(|(p, w)| (Some(p.to_string()), Some(w.to_string())))
      .unwrap_or((None, None));
    let meta = crate::session::SessionMeta {
      session_id: format!("tools-test-session-{id}"),
      parent_session: None,
      profile: "test".into(),
      mode: if worker_scope.is_some() {
        "worker".into()
      } else {
        "default".into()
      },
      flags: crate::session::SessionFlags {
        steer: false,
        worker: worker_scope.is_some(),
        autocompact: -1,
        resume: false,
        temp: true,
      },
      usage: crate::session::SessionUsage { total_tokens: 0 },
      draft_input: None,
      start_ts: None,
      end_ts: None,
    };
    Agent::new(
      dummy_client(),
      crate::prompts::build_messages(""),
      configured_director_tools(),
      crate::agent::CompactState::disabled(),
      meta,
      worker_parent_session_id,
      worker_id,
    )
  }

  #[test]
  fn is_read_only_tool_classification() {
    assert!(is_read_only_tool("read_file"));
    assert!(is_read_only_tool("read_hash_anchors"));
    assert!(is_read_only_tool("repo_map"));
    assert!(is_read_only_tool("web_search"));
    assert!(is_read_only_tool("web_read"));
    assert!(is_read_only_tool("web_code_context"));
    assert!(is_read_only_tool("load_skill"));
    assert!(!is_read_only_tool("write_file"));
    assert!(!is_read_only_tool("edit_hash_anchors"));
    assert!(!is_read_only_tool("bash"));
    assert!(!is_read_only_tool("state"));
  }

  #[test]
  fn configured_director_tools_includes_expected() {
    let tools = configured_director_tools();
    let names: Vec<_> = tools.iter().map(|t| t.function.name.as_str()).collect();
    assert!(names.contains(&"repo_map"));
    assert!(names.contains(&"bash"));
    assert!(names.contains(&"state"));
    assert!(names.contains(&"dispatch_workers"));
    assert!(names.contains(&"wait_workers"));
    assert!(names.contains(&"load_skill"));
    assert!(!names.contains(&"read_file"));
    assert!(!names.contains(&"web_search"));
    assert!(!names.contains(&"web_read"));
    assert!(!names.contains(&"web_code_context"));
    assert!(!names.contains(&"write_file"));
    assert!(!names.contains(&"edit_hash_anchors"));
  }

  #[test]
  fn configured_worker_tools_includes_expected() {
    let tools = configured_worker_tools();
    let names: Vec<_> = tools.iter().map(|t| t.function.name.as_str()).collect();
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"write_file"));
    assert!(names.contains(&"read_hash_anchors"));
    assert!(names.contains(&"edit_hash_anchors"));
    assert!(names.contains(&"state"));
    assert!(!names.contains(&"dispatch_workers"));
    assert!(!names.contains(&"wait_workers"));
  }

  #[test]
  fn check_director_bash_allowlist_accepts_rg_and_colgrep() {
    assert!(check_director_bash_allowlist("rg foo src").is_ok());
    assert!(check_director_bash_allowlist("colgrep \"search\" ./src").is_ok());
    assert!(check_director_bash_allowlist("rg -l \"\" docs").is_ok());
  }

  #[test]
  fn check_director_bash_allowlist_rejects_other_execs() {
    assert!(check_director_bash_allowlist("ls -la").is_err());
    assert!(check_director_bash_allowlist("rg foo src | head -n 1").is_err());
    assert!(check_director_bash_allowlist("rg foo src && colgrep bar ./src").is_err());
    assert!(check_director_bash_allowlist("rg -n \".\" docs/index.md").is_err());
    assert!(check_director_bash_allowlist("python -c 'print(1)'").is_err());
  }

  #[test]
  fn state_round_trip_write_read_list_append() {
    let mut agent = dummy_agent(None);
    let key = "foo/bar";

    state(
      &mut agent,
      &format!(r#"{{"action":"write","path":"{key}","content":"hello"}}"#),
    )
    .unwrap();
    let read = state(
      &mut agent,
      &format!(r#"{{"action":"read","path":"{key}"}}"#),
    )
    .unwrap();
    assert_eq!(read, "\"hello\"");

    state(
      &mut agent,
      &format!(r#"{{"action":"append","path":"{key}","content":" world"}}"#),
    )
    .unwrap();
    let read_after_append = state(
      &mut agent,
      &format!(r#"{{"action":"read","path":"{key}"}}"#),
    )
    .unwrap();
    assert_eq!(read_after_append, "\"hello world\"");

    let list = state(&mut agent, r#"{"action":"list","path":"foo"}"#).unwrap();
    assert!(list.contains(key));
  }

  #[test]
  fn state_write_and_append_require_content() {
    let mut agent = dummy_agent(None);

    let write_err = state(&mut agent, r#"{"action":"write","path":"goal"}"#).unwrap_err();
    assert!(write_err.to_string().contains("content is required"));

    let append_err = state(&mut agent, r#"{"action":"append","path":"goal"}"#).unwrap_err();
    assert!(append_err.to_string().contains("content is required"));
  }

  #[test]
  fn state_scope_uses_director_and_worker_paths() {
    let mut director = dummy_agent(None);
    state(
      &mut director,
      r#"{"action":"write","path":"status","content":"director"}"#,
    )
    .unwrap();
    let director_path = crate::session::state_path(&director.meta.session_id);
    assert!(director_path.exists());

    let mut worker = dummy_agent(Some((&director.meta.session_id, "worker-test-1")));
    state(
      &mut worker,
      r#"{"action":"write","path":"status","content":"worker"}"#,
    )
    .unwrap();
    let worker_path = crate::session::worker_state_path(&director.meta.session_id, "worker-test-1");
    assert!(worker_path.exists());
  }

  #[tokio::test]
  async fn execute_tool_unknown_returns_error() {
    let result = execute_tool(ToolContext { agent: None }, "nonexistent_tool", "{}").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("unknown tool"));
  }

  #[tokio::test]
  async fn dispatch_workers_bad_json_explains_expected_shape() {
    let mut agent = dummy_agent(None);
    let err = dispatch_workers(
      &mut agent,
      r#"{"workers":[{"role":"implementer","task":"missing array close"}"#,
    )
    .await
    .expect_err("malformed JSON should fail");
    let msg = err.to_string();
    assert!(msg.contains("bad dispatch_workers args"));
    assert!(msg.contains("\"workers\""));
    assert!(msg.contains("close the JSON"));
  }

  #[tokio::test]
  async fn wait_workers_without_running_workers_returns_immediately() {
    let mut agent = dummy_agent(None);
    let out = wait_workers(&mut agent, "{}").await.unwrap();
    assert!(out.contains("No workers are running"));
  }
}
