use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use crate::hashline::{EditOp, apply_anchor_edits, render_hashlines};
use crate::types::{Tool, ToolFunction};
use crate::workspace::Workspace;

pub struct ToolContext {
  pub workspace: Workspace,
}

pub async fn execute_tool(ctx: ToolContext, name: &str, args: &str) -> Result<String> {
  match name {
    "read_file" => read_file(&ctx.workspace, args),
    "write_file" => write_file(&ctx.workspace, args),
    "bash" => bash(&ctx.workspace, args).await,
    "repo_map" => repo_map(&ctx.workspace, args),
    "code_map" => code_map(&ctx.workspace, args),
    "read_hash_anchors" => read_hash_anchors(&ctx.workspace, args),
    "edit_hash_anchors" => edit_hash_anchors(&ctx.workspace, args),
    "web_search" => web_search(args).await,
    "web_read" => web_read(args).await,
    "web_code_context" => web_code_context(args).await,
    "load_skill" => load_skill(args),
    _ => bail!("unknown tool: {name}"),
  }
}

static WORKER_TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();

pub fn configured_worker_tools() -> Vec<Tool> {
  WORKER_TOOLS.get_or_init(build_worker_tools).clone()
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
      "code_map",
      "Display a symbol map of source files (Rust and Go), showing structs, enums, traits, impls, functions, interfaces, types, and modules with line ranges. Use to understand the shape and contents of source files before deciding which files or line ranges to read. For a single file, pass its path; for a directory, pass the directory path to map all .rs and .go files inside. Use before read_file to target exact line ranges.",
      json!({"type":"object","properties":{"path":{"type":"string","description":"File or directory path relative to workspace root. Default: \".\""}},"additionalProperties":false}),
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
  ]
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

fn exa_client() -> Result<&'static reqwest::Client> {
  static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
  if let Some(client) = CLIENT.get() {
    return Ok(client);
  }
  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(60))
    .build()
    .context("build exa client")?;
  Ok(CLIENT.get_or_init(|| client))
}

#[derive(Deserialize)]
struct ReadFileArgs {
  path: String,
  start: Option<usize>,
  end: Option<usize>,
}

fn read_file(workspace: &Workspace, args: &str) -> Result<String> {
  let args: ReadFileArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  let path = workspace.readable_path(&args.path)?;
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

fn write_file(workspace: &Workspace, args: &str) -> Result<String> {
  let args: WriteFileArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  let path = workspace.workspace_path(&args.path)?;
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

fn check_bash_cds(workspace: &Workspace, command: &str) -> Result<()> {
  let cmd = strip_heredoc_bodies(command);
  let cmd = split_shell_separators(&cmd);
  let base = workspace.root();
  let tmp = Path::new("/tmp");
  let mut cwd = base.to_path_buf();
  for line in cmd.split('\n') {
    let mut words = line.split_whitespace();
    if words.next() == Some("cd") {
      let path = words.next().unwrap_or("");
      if path.is_empty() {
        bail!(
          "cd without argument is not allowed (would go to $HOME). Use a relative path within the workspace (e.g., ./foo) or /tmp."
        );
      }
      let target = resolve_cd_target(&cwd, path)?;
      let norm = crate::workspace::normalize(&target);
      let in_workspace = norm.starts_with(base);
      let in_tmp = norm.starts_with(tmp);
      if !in_workspace && !in_tmp {
        bail!(
          "cd to {path} is not allowed. You cannot cd outside the workspace or /tmp. Use relative paths within the workspace (e.g., ./foo or foo)."
        );
      }
      cwd = norm;
    }
  }
  Ok(())
}

fn split_shell_separators(command: &str) -> String {
  let mut cmd = command.to_string();
  for sep in ["&&", "||", "|", ";", "\n", "\r"] {
    cmd = cmd.replace(sep, "\n");
  }
  cmd
}

fn strip_heredoc_bodies(command: &str) -> String {
  let mut out = String::new();
  let mut lines = command.lines();
  while let Some(line) = lines.next() {
    out.push_str(line);
    out.push('\n');

    let Some(marker) = heredoc_marker(line) else {
      continue;
    };

    for body_line in lines.by_ref() {
      if body_line.trim() == marker {
        out.push_str(body_line);
        out.push('\n');
        break;
      }
    }
  }
  out
}

fn heredoc_marker(line: &str) -> Option<String> {
  let marker = line.split_once("<<")?.1.trim_start();
  let marker = marker
    .split_whitespace()
    .next()?
    .trim_matches(|c| matches!(c, '\'' | '"'));
  if marker.is_empty() {
    None
  } else {
    Some(marker.to_string())
  }
}

fn resolve_cd_target(base: &Path, path: &str) -> Result<PathBuf> {
  if path == "~" {
    return std::env::var_os("HOME").map(PathBuf::from).context(
      "cd to ~ is not allowed. Use a relative path within the workspace (e.g., ./foo) or /tmp.",
    );
  }
  if let Some(rest) = path.strip_prefix("~/") {
    let home = std::env::var_os("HOME").context(
      "cd to ~/... is not allowed. Use a relative path within the workspace (e.g., ./foo) or /tmp.",
    )?;
    return Ok(PathBuf::from(home).join(rest));
  }
  if path.starts_with('/') {
    return Ok(PathBuf::from(path));
  }
  Ok(base.join(path))
}

async fn bash(workspace: &Workspace, args: &str) -> Result<String> {
  let args: BashArgs = parse_args(args)?;
  require_nonempty(&args.command, "command")?;
  check_bash_cds(workspace, &args.command)?;
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
    .current_dir(workspace.root())
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

fn repo_map(workspace: &Workspace, args: &str) -> Result<String> {
  let args: RepoMapArgs = parse_args(args)?;
  let rel = if args.path.is_empty() {
    "."
  } else {
    &args.path
  };
  let path = workspace.readable_path(rel)?;
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
    out.push_str(&"  ".repeat(depth));
    out.push_str(&name.to_string_lossy());
    out.push('\n');
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

#[derive(Deserialize)]
struct CodeMapArgs {
  #[serde(default)]
  path: String,
}

fn code_map(workspace: &Workspace, args: &str) -> Result<String> {
  let args: CodeMapArgs = parse_args(args)?;
  let rel = if args.path.is_empty() {
    "."
  } else {
    &args.path
  };
  let path = workspace.readable_path(rel)?;
  crate::symbol_tree::format_path(&path)
}

fn read_hash_anchors(workspace: &Workspace, args: &str) -> Result<String> {
  let args: ReadFileArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  if args.start == Some(0) || args.end == Some(0) {
    bail!("start and end line numbers must be >= 1 (1-indexed)");
  }
  let path = workspace.workspace_path(&args.path)?;
  let source = fs::read_to_string(&path).with_context(|| format!("read {}", args.path))?;
  Ok(render_hashlines(&source, args.start, args.end))
}

#[derive(Deserialize)]
struct EditHashAnchorsArgs {
  path: String,
  ops: Vec<EditOp>,
}

fn edit_hash_anchors(workspace: &Workspace, args: &str) -> Result<String> {
  let args: EditHashAnchorsArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  if args.ops.is_empty() {
    bail!("ops array is required");
  }
  let path = workspace.workspace_path(&args.path)?;
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
    out.push_str(&format!(
      "{}. {}\n",
      i + 1,
      r["title"].as_str().unwrap_or("")
    ));
    out.push_str(&format!("   {}\n", r["url"].as_str().unwrap_or("")));
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
  for r in v["results"].as_array().into_iter().flatten() {
    out.push_str(&format!("--- {} ---\n", r["title"].as_str().unwrap_or("")));
    out.push_str(&format!("{}\n", r["url"].as_str().unwrap_or("")));
    out.push('\n');
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

fn exa_api_key() -> String {
  std::env::var("EXA_API_KEY").unwrap_or_default()
}

pub fn ensure_exa_api_key_set() -> Result<()> {
  let key = std::env::var("EXA_API_KEY").unwrap_or_default();
  if key.trim().is_empty() {
    bail!("EXA_API_KEY is not set. Set EXA_API_KEY before running ogent.");
  }
  Ok(())
}

async fn exa_post(url: &str, body: Value) -> Result<Value> {
  let key = exa_api_key();
  let resp = exa_client()?
    .post(url)
    .header("x-api-key", key)
    .json(&body)
    .send()
    .await?;
  let status = resp.status();
  let text = resp.text().await?;
  if !status.is_success() {
    eprintln!("exa request failed: {} {}", status.as_u16(), text.trim());
    bail!("exa {}: {}", status.as_u16(), text.trim());
  }
  let v: Value = serde_json::from_str(&text).context("unmarshal exa response")?;
  if let Some(err) = v["error"].as_str().filter(|s| !s.is_empty()) {
    eprintln!("exa returned error: {err}");
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



#[cfg(test)]
mod tests {
  use super::*;
  use std::path::PathBuf;

  fn test_workspace(root: &str) -> Workspace {
    Workspace::from_root(PathBuf::from(root))
  }

  #[test]
  fn check_bash_cds_tracks_cwd_after_tmp_cd() {
    let ws = test_workspace("/tmp/demo");

    let err = check_bash_cds(&ws, "cd /tmp && cd ..").unwrap_err();

    assert!(err.to_string().contains("cd to .. is not allowed"));
  }

  #[test]
  fn check_bash_cds_allows_relative_tmp_child_after_tmp_cd() {
    let ws = test_workspace("/workspace/project");

    assert!(check_bash_cds(&ws, "cd /tmp && cd src").is_ok());
  }

  #[test]
  fn check_bash_cds_tracks_workspace_relative_cd_chain() {
    let ws = test_workspace("/workspace/project");

    assert!(check_bash_cds(&ws, "cd src && cd ..").is_ok());
    assert!(check_bash_cds(&ws, "cd src && cd ../..").is_err());
  }

  #[test]
  fn check_bash_cds_ignores_heredoc_body_examples() {
    let ws = test_workspace("/workspace/project");
    let command = "cat <<'EOF'\ncd /tmp && cd ..\nEOF";

    assert!(check_bash_cds(&ws, command).is_ok());
  }

  #[test]
  fn configured_worker_tools_includes_expected() {
    let tools = configured_worker_tools();
    let names: Vec<_> = tools.iter().map(|t| t.function.name.as_str()).collect();
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"write_file"));
    assert!(names.contains(&"read_hash_anchors"));
    assert!(names.contains(&"code_map"));
    assert!(names.contains(&"edit_hash_anchors"));
  }


  #[tokio::test]
  async fn execute_tool_unknown_returns_error() {
    let result = execute_tool(
      ToolContext {
        workspace: crate::workspace::Workspace::from_current_dir(),
      },
      "nonexistent_tool",
      "{}",
    )
    .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("unknown tool"));
  }
}
