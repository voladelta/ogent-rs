use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use crate::tools::{ToolContext, parse_args};

const MAX_GIT_CONTEXT: u32 = 20;
const MAX_GIT_LOG_ENTRIES: u32 = 100;

#[derive(Deserialize)]
struct GitStatusArgs {
  staged: Option<bool>,
  #[serde(default)]
  paths: Vec<String>,
  #[serde(default = "default_true")]
  untracked: bool,
}

#[derive(Deserialize)]
struct GitDiffArgs {
  staged: Option<bool>,
  base: Option<String>,
  #[serde(default)]
  paths: Vec<String>,
  #[serde(default = "default_three")]
  context: u32,
  #[serde(default)]
  stat_only: bool,
}

#[derive(Deserialize)]
struct GitChangesArgs {
  base: Option<String>,
  #[serde(default)]
  paths: Vec<String>,
  #[serde(default = "default_three")]
  context: u32,
  #[serde(default)]
  stat_only: bool,
}

#[derive(Deserialize)]
struct GitShowArgs {
  path: String,
  #[serde(default = "default_head", alias = "ref")]
  git_ref: String,
}

#[derive(Deserialize)]
struct GitLogArgs {
  #[serde(default)]
  paths: Vec<String>,
  #[serde(default = "default_ten")]
  n: u32,
}

fn default_true() -> bool {
  true
}
fn default_three() -> u32 {
  3
}
fn default_head() -> String {
  "HEAD".to_string()
}
fn default_ten() -> u32 {
  10
}

fn bounded_context(context: u32) -> u32 {
  context.min(MAX_GIT_CONTEXT)
}

fn bounded_log_entries(n: u32) -> u32 {
  n.min(MAX_GIT_LOG_ENTRIES)
}

fn validate_git_paths(workspace: &crate::workspace::Workspace, paths: &[String]) -> Result<()> {
  for path in paths {
    validate_git_path(workspace, path)?;
  }
  Ok(())
}

fn validate_git_path(workspace: &crate::workspace::Workspace, path: &str) -> Result<()> {
  if path.trim().is_empty() {
    bail!("git path entries must be non-empty");
  }
  if Path::new(path).is_absolute() {
    bail!("git paths must be relative to the workspace root: {path}");
  }
  workspace
    .workspace_path(path)
    .with_context(|| format!("git path is outside workspace: {path}"))?;
  Ok(())
}

fn validate_git_ref_arg(name: &str, value: &str) -> Result<()> {
  if value.trim().is_empty() {
    bail!("git {name} must be non-empty");
  }
  if value.starts_with('-') {
    bail!("git {name} must be a ref or revision, not an option: {value}");
  }
  if value.contains('\0') || value.contains('\n') || value.contains('\r') {
    bail!("git {name} contains an invalid control character");
  }
  Ok(())
}

#[derive(Serialize)]
struct GitStatusEntry {
  path: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  old_path: Option<String>,
  status: String,
  staged: bool,
  worktree: bool,
  index_char: String,
  worktree_char: String,
  display: String,
  state_description: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  diff: Option<GitDiffDelta>,
  #[serde(skip_serializing_if = "Option::is_none")]
  staged_diff: Option<GitDiffDelta>,
}

#[derive(Serialize, Clone)]
struct GitDiffDelta {
  path: String,
  old_path: String,
  change_type: String,
  is_binary: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  old_mode: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  new_mode: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  similarity: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  insertions: Option<usize>,
  #[serde(skip_serializing_if = "Option::is_none")]
  deletions: Option<usize>,
  #[serde(skip_serializing_if = "Option::is_none")]
  hunks: Option<Vec<GitDiffHunk>>,
}

#[derive(Serialize, Clone)]
struct GitDiffHunk {
  old_start: u32,
  old_lines: u32,
  new_start: u32,
  new_lines: u32,
  header: String,
  lines: Vec<GitDiffLine>,
}

#[derive(Serialize, Clone)]
struct GitDiffLine {
  r#type: String,
  text: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  old_line: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  new_line: Option<u32>,
}

async fn run_git(workspace: &std::path::Path, args: &[&str]) -> Result<std::process::Output> {
  let mut cmd = Command::new("git");
  cmd.arg("-C").arg(workspace);
  for arg in args {
    cmd.arg(arg);
  }
  cmd.stdout(std::process::Stdio::piped());
  cmd.stderr(std::process::Stdio::piped());

  let output = timeout(Duration::from_secs(30), cmd.output()).await;
  match output {
    Err(_) => bail!("git command timed out after 30s"),
    Ok(Err(e)) => bail!("failed to run git: {e}"),
    Ok(Ok(out)) => {
      if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("git failed: {stderr}");
      }
      Ok(out)
    }
  }
}

pub async fn git_status(ctx: ToolContext, args: &str) -> Result<String> {
  let args: GitStatusArgs = parse_args(args)?;
  validate_git_paths(&ctx.workspace, &args.paths)?;

  let mut git_args: Vec<String> = vec![
    "status".to_string(),
    "--porcelain=1".to_string(),
    "-z".to_string(),
  ];
  if !args.untracked {
    git_args.push("--untracked-files=no".to_string());
  }
  if !args.paths.is_empty() {
    git_args.push("--".to_string());
    for p in &args.paths {
      git_args.push(p.clone());
    }
  }

  let output = run_git(
    ctx.workspace.root(),
    &git_args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
  )
  .await?;
  let entries = parse_porcelain_v1_z(&output.stdout)?;

  let filtered: Vec<_> = entries
    .into_iter()
    .filter(|e| match args.staged {
      Some(true) => e.staged,
      Some(false) => e.worktree || e.status == "untracked",
      None => true,
    })
    .collect();

  Ok(serde_json::to_string(&filtered)?)
}

fn parse_porcelain_v1_z(stdout: &[u8]) -> Result<Vec<GitStatusEntry>> {
  let mut entries = Vec::new();

  let fields: Vec<&[u8]> = stdout.split(|&b| b == 0).collect();

  let mut i = 0;
  while i < fields.len() {
    let field = fields[i];
    i += 1;
    if field.is_empty() {
      continue;
    }
    if field.len() < 2 {
      continue;
    }

    let display = std::str::from_utf8(&field[0..2])
      .unwrap_or("  ")
      .to_string();

    match &field[..2] {
      b"??" => {
        let path = std::str::from_utf8(&field[3..]).unwrap_or("").to_string();
        entries.push(GitStatusEntry {
          path,
          old_path: None,
          status: "untracked".to_string(),
          staged: false,
          worktree: true,
          index_char: "?".to_string(),
          worktree_char: "?".to_string(),
          display,
          state_description: "Untracked".to_string(),
          diff: None,
          staged_diff: None,
        });
      }
      b"!!" => {
        let path = std::str::from_utf8(&field[3..]).unwrap_or("").to_string();
        entries.push(GitStatusEntry {
          path,
          old_path: None,
          status: "ignored".to_string(),
          staged: false,
          worktree: false,
          index_char: "!".to_string(),
          worktree_char: "!".to_string(),
          display,
          state_description: "Ignored".to_string(),
          diff: None,
          staged_diff: None,
        });
      }
      _ => {
        let x = field[0] as char;
        let y = field[1] as char;

        let path = std::str::from_utf8(&field[3..]).unwrap_or("").to_string();

        let status = resolve_status(x, y);
        let staged = x != ' ' && x != '?' && x != '!';
        let worktree = y != ' ' && y != '?' && y != '!';

        let (entry_path, old_p) = if x == 'R' || x == 'C' || y == 'R' || y == 'C' {
          let orig = if i < fields.len() {
            let f = fields[i];
            i += 1;
            std::str::from_utf8(f).unwrap_or("").to_string()
          } else {
            String::new()
          };
          (path, if orig.is_empty() { None } else { Some(orig) })
        } else {
          (path, None)
        };

        entries.push(GitStatusEntry {
          path: entry_path,
          old_path: old_p,
          status,
          staged,
          worktree,
          index_char: x.to_string(),
          worktree_char: y.to_string(),
          display,
          state_description: state_description(x, y),
          diff: None,
          staged_diff: None,
        });
      }
    }
  }

  Ok(entries)
}

/// Resolve a single status string from the two porcelain characters.
///
/// Precedence (highest first):
/// 1. Unmerged (U on either side) → "unmerged"
/// 2. Rename (R on either side) → "renamed"
/// 3. Copy (C on either side) → "copied"
/// 4. Type change (T on either side) → "type_changed"
/// 5. Fallback to the active side: worktree (y) takes priority over index (x)
///    because worktree is the most recent state.
fn resolve_status(x: char, y: char) -> String {
  if x == 'U' || y == 'U' {
    return "unmerged".to_string();
  }
  if x == 'R' || y == 'R' {
    return "renamed".to_string();
  }
  if x == 'C' || y == 'C' {
    return "copied".to_string();
  }
  if x == 'T' || y == 'T' {
    return "type_changed".to_string();
  }
  let c = if y != ' ' { y } else { x };
  match c {
    'A' => "added",
    'D' => "deleted",
    'M' => "modified",
    '?' => "untracked",
    '!' => "ignored",
    _ => "modified",
  }
  .to_string()
}

fn state_description(x: char, y: char) -> String {
  fn side_word(c: char) -> &'static str {
    match c {
      'M' => "modified",
      'A' => "added",
      'D' => "deleted",
      'R' => "renamed",
      'C' => "copied",
      'U' => "unmerged",
      'T' => "type changed",
      '?' => "untracked",
      '!' => "ignored",
      _ => "modified",
    }
  }
  if x == '?' && y == '?' {
    return "Untracked".to_string();
  }
  if x == '!' && y == '!' {
    return "Ignored".to_string();
  }
  let index = side_word(x);
  let worktree = side_word(y);
  if x == ' ' {
    format!("{} in worktree", capitalize(worktree))
  } else if y == ' ' {
    format!("{} in index", capitalize(index))
  } else {
    format!("{} in index, {} in worktree", capitalize(index), worktree)
  }
}

fn capitalize(s: &str) -> String {
  let mut c = s.chars();
  match c.next() {
    None => String::new(),
    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
  }
}

pub async fn git_diff(ctx: ToolContext, args: &str) -> Result<String> {
  let args: GitDiffArgs = parse_args(args)?;
  validate_git_paths(&ctx.workspace, &args.paths)?;
  if let Some(base) = &args.base {
    validate_git_ref_arg("base", base)?;
  }
  let context = bounded_context(args.context);

  let mut git_args: Vec<String> = vec!["diff".to_string(), "--no-ext-diff".to_string()];

  if let Some(true) = args.staged {
    git_args.push("--cached".to_string());
  } else if let Some(ref base) = args.base {
    git_args.push(base.clone());
  }

  git_args.push(format!("-U{}", context));

  if !args.paths.is_empty() {
    git_args.push("--".to_string());
    for p in &args.paths {
      git_args.push(p.clone());
    }
  }

  let output = run_git(
    ctx.workspace.root(),
    &git_args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
  )
  .await?;

  let text = String::from_utf8_lossy(&output.stdout);
  let deltas = parse_unified_diff(&text, args.stat_only)?;
  Ok(serde_json::to_string(&deltas)?)
}

fn parse_unified_diff(text: &str, stat_only: bool) -> Result<Vec<GitDiffDelta>> {
  let mut deltas: Vec<GitDiffDelta> = Vec::new();
  let mut current: Option<GitDiffDelta> = None;
  let mut current_hunk: Option<GitDiffHunk> = None;
  let mut old_line: u32 = 0;
  let mut new_line: u32 = 0;

  fn maybe_push_delta(deltas: &mut Vec<GitDiffDelta>, delta: &mut Option<GitDiffDelta>) {
    if let Some(mut d) = delta.take() {
      if d.change_type == "modified" && d.old_mode.is_some() && d.new_mode.is_some() {
        d.change_type = "type_changed".to_string();
      }
      deltas.push(d);
    }
  }

  for line in text.lines() {
    if let Some(after) = line.strip_prefix("diff --git ") {
      if let Some(h) = current_hunk.take()
        && let Some(d) = current.as_mut()
        && let Some(hunks) = &mut d.hunks
      {
        hunks.push(h);
      }
      maybe_push_delta(&mut deltas, &mut current);

      let parts: Vec<&str> = after.split_whitespace().collect();
      let old_raw = strip_quotes(parts.first().unwrap_or(&""));
      let new_raw = strip_quotes(parts.get(1).unwrap_or(&""));
      let old_path = old_raw.strip_prefix("a/").unwrap_or(old_raw).to_string();
      let new_path = new_raw.strip_prefix("b/").unwrap_or(new_raw).to_string();

      current = Some(GitDiffDelta {
        path: new_path,
        old_path,
        change_type: "modified".to_string(),
        is_binary: false,
        old_mode: None,
        new_mode: None,
        similarity: None,
        insertions: Some(0),
        deletions: Some(0),
        hunks: if stat_only { None } else { Some(Vec::new()) },
      });
      continue;
    }

    if current.is_none() {
      continue;
    }

    // Helper closure to push a hunk to current delta
    let push_hunk = |cur: &mut Option<GitDiffDelta>, hunk: &mut Option<GitDiffHunk>| {
      if let Some(h) = hunk.take()
        && let Some(d) = cur
        && let Some(hunks) = &mut d.hunks
      {
        hunks.push(h);
      }
    };

    if let Some(stripped) = line.strip_prefix("old mode ") {
      if let Some(cur) = current.as_mut() {
        cur.old_mode = Some(stripped.to_string());
      }
    } else if let Some(stripped) = line.strip_prefix("new mode ") {
      if let Some(cur) = current.as_mut() {
        cur.new_mode = Some(stripped.to_string());
      }
    } else if let Some(stripped) = line.strip_prefix("deleted file mode ") {
      if let Some(cur) = current.as_mut() {
        cur.change_type = "deleted".to_string();
        cur.old_mode = Some(stripped.to_string());
      }
    } else if let Some(stripped) = line.strip_prefix("new file mode ") {
      if let Some(cur) = current.as_mut() {
        cur.change_type = "added".to_string();
        cur.new_mode = Some(stripped.to_string());
      }
    } else if let Some(stripped) = line.strip_prefix("similarity index ") {
      if let Some(cur) = current.as_mut() {
        let pct = stripped.trim_end_matches('%');
        cur.similarity = pct.parse().ok();
      }
    } else if let Some(stripped) = line.strip_prefix("rename from ") {
      if let Some(cur) = current.as_mut() {
        cur.change_type = "renamed".to_string();
        cur.old_path = strip_quotes(stripped).to_string();
      }
    } else if let Some(stripped) = line.strip_prefix("rename to ") {
      if let Some(cur) = current.as_mut() {
        cur.path = strip_quotes(stripped).to_string();
      }
    } else if let Some(stripped) = line.strip_prefix("copy from ") {
      if let Some(cur) = current.as_mut() {
        cur.change_type = "copied".to_string();
        cur.old_path = strip_quotes(stripped).to_string();
      }
    } else if let Some(stripped) = line.strip_prefix("copy to ") {
      if let Some(cur) = current.as_mut() {
        cur.path = strip_quotes(stripped).to_string();
      }
    } else if line.starts_with("index ") {
      // skip
    } else if line.starts_with("Binary files ") {
      if let Some(cur) = current.as_mut() {
        cur.is_binary = true;
        cur.hunks = None;
        current_hunk = None;
      }
    } else if line.starts_with("--- ") || line.starts_with("+++ ") {
      // skip
    } else if line.starts_with("@@") {
      push_hunk(&mut current, &mut current_hunk);
      if let Some(h) = parse_hunk_header(line) {
        old_line = h.old_start;
        new_line = h.new_start;
        if let Some(cur) = current.as_mut()
          && cur.hunks.is_some()
        {
          current_hunk = Some(h);
        }
      }
    } else if let Some(stripped) = line.strip_prefix(" ") {
      if let Some(h) = current_hunk.as_mut() {
        h.lines.push(GitDiffLine {
          r#type: "context".to_string(),
          text: stripped.to_string(),
          old_line: Some(old_line),
          new_line: Some(new_line),
        });
        old_line += 1;
        new_line += 1;
      }
    } else if let Some(stripped) = line.strip_prefix("-") {
      if let Some(h) = current_hunk.as_mut() {
        h.lines.push(GitDiffLine {
          r#type: "deletion".to_string(),
          text: stripped.to_string(),
          old_line: Some(old_line),
          new_line: None,
        });
        old_line += 1;
      }
      if let Some(cur) = current.as_mut()
        && let Some(del) = &mut cur.deletions
      {
        *del += 1;
      }
    } else if let Some(stripped) = line.strip_prefix("+") {
      if let Some(h) = current_hunk.as_mut() {
        h.lines.push(GitDiffLine {
          r#type: "addition".to_string(),
          text: stripped.to_string(),
          old_line: None,
          new_line: Some(new_line),
        });
        new_line += 1;
      }
      if let Some(cur) = current.as_mut()
        && let Some(ins) = &mut cur.insertions
      {
        *ins += 1;
      }
    } else if line == "\\ No newline at end of file" {
      // skip
    }
  }

  if let Some(h) = current_hunk.take()
    && let Some(d) = current.as_mut()
    && let Some(hunks) = &mut d.hunks
  {
    hunks.push(h);
  }
  maybe_push_delta(&mut deltas, &mut current);

  Ok(deltas)
}

fn strip_quotes(s: &str) -> &str {
  let s = s.trim();
  if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
    &s[1..s.len() - 1]
  } else {
    s
  }
}

fn parse_hunk_header(line: &str) -> Option<GitDiffHunk> {
  let inner = line
    .trim_start_matches('@')
    .trim_start_matches(' ')
    .trim_end_matches('@')
    .trim_end_matches(' ')
    .trim();

  let mut parts = inner.split_whitespace();
  let old_part = parts.next()?;
  let new_part = parts.next()?;

  let (old_start, old_lines) = parse_range(old_part);
  let (new_start, new_lines) = parse_range(new_part);

  Some(GitDiffHunk {
    old_start,
    old_lines,
    new_start,
    new_lines,
    header: line.to_string(),
    lines: Vec::new(),
  })
}

fn parse_range(s: &str) -> (u32, u32) {
  let s = s.trim_start_matches(['+', '-']);
  if let Some((start, count)) = s.split_once(',') {
    let start = start.parse().unwrap_or(0);
    let count = count.parse().unwrap_or(0);
    (start, count)
  } else {
    (s.parse().unwrap_or(0), 1)
  }
}

async fn run_diff(
  workspace: &std::path::Path,
  paths: &[String],
  context: u32,
  staged: bool,
  stat_only: bool,
  base: Option<&str>,
) -> Result<HashMap<String, GitDiffDelta>> {
  let context = bounded_context(context);
  let mut git_args: Vec<String> = vec![
    "diff".to_string(),
    "--no-ext-diff".to_string(),
    format!("-U{}", context),
  ];
  if staged {
    git_args.push("--cached".to_string());
  }
  if let Some(base) = base {
    git_args.push(base.to_string());
  }
  git_args.push("--".to_string());
  for p in paths {
    git_args.push(p.clone());
  }

  let output = run_git(
    workspace,
    &git_args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
  )
  .await?;
  let text = String::from_utf8_lossy(&output.stdout);
  let deltas = parse_unified_diff(&text, stat_only)?;
  Ok(deltas.into_iter().map(|d| (d.path.clone(), d)).collect())
}

pub async fn git_changes(ctx: ToolContext, args: &str) -> Result<String> {
  let args: GitChangesArgs = parse_args(args)?;
  validate_git_paths(&ctx.workspace, &args.paths)?;
  if let Some(base) = &args.base {
    validate_git_ref_arg("base", base)?;
  }
  let context = bounded_context(args.context);

  // 1. Get status
  let mut git_args: Vec<String> = vec![
    "status".to_string(),
    "--porcelain=1".to_string(),
    "-z".to_string(),
  ];
  if !args.paths.is_empty() {
    git_args.push("--".to_string());
    for p in &args.paths {
      git_args.push(p.clone());
    }
  }

  let output = run_git(
    ctx.workspace.root(),
    &git_args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
  )
  .await?;
  let mut entries = parse_porcelain_v1_z(&output.stdout)?;

  // 2. Categorize paths by what diff sources they need
  let mut worktree_paths: Vec<String> = Vec::new();
  let mut staged_paths: Vec<String> = Vec::new();

  for e in &entries {
    if e.status == "untracked" || e.status == "ignored" {
      continue;
    }
    if e.worktree {
      worktree_paths.push(e.path.clone());
    }
    if e.staged {
      staged_paths.push(e.path.clone());
    }
  }

  // 3. Run diffs
  let base = args.base.as_deref();
  let worktree_deltas = if !worktree_paths.is_empty() {
    run_diff(
      ctx.workspace.root(),
      &worktree_paths,
      context,
      false,
      args.stat_only,
      base,
    )
    .await?
  } else {
    HashMap::new()
  };

  let staged_deltas = if !staged_paths.is_empty() {
    run_diff(
      ctx.workspace.root(),
      &staged_paths,
      context,
      true,
      args.stat_only,
      base,
    )
    .await?
  } else {
    HashMap::new()
  };

  // 4. Attach diffs
  for entry in &mut entries {
    if let Some(delta) = worktree_deltas.get(&entry.path) {
      entry.diff = Some(delta.clone());
    }
    if let Some(delta) = staged_deltas.get(&entry.path) {
      entry.staged_diff = Some(delta.clone());
    }
  }

  Ok(serde_json::to_string(&entries)?)
}

pub async fn git_show(ctx: ToolContext, args: &str) -> Result<String> {
  let args: GitShowArgs = parse_args(args)?;
  validate_git_path(&ctx.workspace, &args.path)?;
  if args.git_ref != "staged" {
    validate_git_ref_arg("ref", &args.git_ref)?;
  }

  let git_ref = if args.git_ref == "staged" {
    ":0".to_string()
  } else {
    args.git_ref
  };

  let spec = format!("{}:{}", git_ref, args.path);
  let output = run_git(ctx.workspace.root(), &["show", &spec]).await;
  match output {
    Ok(out) => {
      let content = String::from_utf8_lossy(&out.stdout);
      Ok(content.into_owned())
    }
    Err(e) => {
      let err_str = e.to_string();
      if err_str.contains("exists on disk, but not in")
        || err_str.contains("does not exist")
        || err_str.contains("Not a valid object name")
        || err_str.contains("Path")
      {
        bail!("file '{}' not found at ref '{}'", args.path, git_ref)
      } else {
        Err(e)
      }
    }
  }
}

#[derive(Serialize)]
struct GitLogEntry {
  sha: String,
  subject: String,
  author: String,
  date: String,
}

fn parse_git_log(text: &str) -> Result<Vec<GitLogEntry>> {
  let mut entries = Vec::new();
  for line in text.lines() {
    if line.is_empty() {
      continue;
    }
    let parts: Vec<&str> = line.split('\x1E').collect();
    if parts.len() >= 4 {
      entries.push(GitLogEntry {
        sha: parts[0].to_string(),
        subject: parts[1].to_string(),
        author: parts[2].to_string(),
        date: parts[3].to_string(),
      });
    }
  }
  Ok(entries)
}

pub async fn git_log(ctx: ToolContext, args: &str) -> Result<String> {
  let args: GitLogArgs = parse_args(args)?;
  validate_git_paths(&ctx.workspace, &args.paths)?;
  let n = bounded_log_entries(args.n);

  let mut git_args: Vec<String> = vec![
    "log".to_string(),
    "--format=%H%x1E%s%x1E%an%x1E%ad".to_string(),
    "--no-decorate".to_string(),
    format!("-n{}", n),
  ];
  if !args.paths.is_empty() {
    git_args.push("--".to_string());
    for p in &args.paths {
      git_args.push(p.clone());
    }
  }

  let output = run_git(
    ctx.workspace.root(),
    &git_args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
  )
  .await?;
  let text = String::from_utf8_lossy(&output.stdout);
  let entries = parse_git_log(&text)?;
  Ok(serde_json::to_string(&entries)?)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::workspace::Workspace;

  #[test]
  fn test_parse_hunk_header() {
    let h = parse_hunk_header("@@ -10,5 +10,7 @@").unwrap();
    assert_eq!(h.old_start, 10);
    assert_eq!(h.old_lines, 5);
    assert_eq!(h.new_start, 10);
    assert_eq!(h.new_lines, 7);

    let h = parse_hunk_header("@@ -0,0 +1 @@").unwrap();
    assert_eq!(h.old_start, 0);
    assert_eq!(h.old_lines, 0);
    assert_eq!(h.new_start, 1);
    assert_eq!(h.new_lines, 1);

    let h = parse_hunk_header("@@ -1,3 +0,0 @@").unwrap();
    assert_eq!(h.old_start, 1);
    assert_eq!(h.old_lines, 3);
    assert_eq!(h.new_start, 0);
    assert_eq!(h.new_lines, 0);
  }

  #[test]
  fn bounds_agent_controlled_git_sizes() {
    assert_eq!(bounded_context(0), 0);
    assert_eq!(bounded_context(3), 3);
    assert_eq!(bounded_context(MAX_GIT_CONTEXT + 1), MAX_GIT_CONTEXT);
    assert_eq!(bounded_log_entries(10), 10);
    assert_eq!(
      bounded_log_entries(MAX_GIT_LOG_ENTRIES + 1),
      MAX_GIT_LOG_ENTRIES
    );
  }

  #[test]
  fn validate_git_path_requires_workspace_relative_paths() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let ws = Workspace::from_root(temp.path().to_path_buf());

    assert!(validate_git_path(&ws, "src/lib.rs").is_ok());
    assert!(validate_git_path(&ws, "*.rs").is_ok());
    assert!(validate_git_path(&ws, "").is_err());
    assert!(validate_git_path(&ws, "/tmp/outside.rs").is_err());
    assert!(validate_git_path(&ws, "../outside.rs").is_err());

    Ok(())
  }

  #[test]
  fn validate_git_ref_rejects_option_shaped_values() {
    assert!(validate_git_ref_arg("base", "HEAD~1").is_ok());
    assert!(validate_git_ref_arg("base", "abc123").is_ok());
    assert!(validate_git_ref_arg("base", "--help").is_err());
    assert!(validate_git_ref_arg("base", "").is_err());
    assert!(validate_git_ref_arg("base", "HEAD\nnext").is_err());
  }

  #[test]
  fn test_parse_unified_diff_basic() {
    let text = r#"diff --git a/src/main.rs b/src/main.rs
index abc..def 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,5 +10,7 @@
     let x = 1;
-    let y = 2;
+    let z = 3;
     context
"#;
    let deltas = parse_unified_diff(text, false).unwrap();
    assert_eq!(deltas.len(), 1);
    let d = &deltas[0];
    assert_eq!(d.path, "src/main.rs");
    assert_eq!(d.old_path, "src/main.rs");
    assert_eq!(d.change_type, "modified");
    assert!(!d.is_binary);
    assert_eq!(d.hunks.as_ref().unwrap().len(), 1);
    let h = &d.hunks.as_ref().unwrap()[0];
    assert_eq!(h.old_start, 10);
    assert_eq!(h.lines.len(), 4);
    assert_eq!(h.lines[0].r#type, "context");
    assert_eq!(h.lines[0].old_line, Some(10));
    assert_eq!(h.lines[0].new_line, Some(10));
    assert_eq!(h.lines[1].r#type, "deletion");
    assert_eq!(h.lines[1].old_line, Some(11));
    assert_eq!(h.lines[1].new_line, None);
    assert_eq!(h.lines[2].r#type, "addition");
    assert_eq!(h.lines[2].old_line, None);
    assert_eq!(h.lines[2].new_line, Some(11));
    assert_eq!(h.lines[3].r#type, "context");
    assert_eq!(h.lines[3].old_line, Some(12));
    assert_eq!(h.lines[3].new_line, Some(12));
    assert_eq!(d.insertions, Some(1));
    assert_eq!(d.deletions, Some(1));
  }

  #[test]
  fn test_parse_unified_diff_rename() {
    let text = r#"diff --git a/old.rs b/new.rs
similarity index 100%
rename from old.rs
rename to new.rs
index abc..def 100644
--- a/old.rs
+++ b/new.rs
"#;
    let deltas = parse_unified_diff(text, false).unwrap();
    assert_eq!(deltas.len(), 1);
    let d = &deltas[0];
    assert_eq!(d.path, "new.rs");
    assert_eq!(d.old_path, "old.rs");
    assert_eq!(d.change_type, "renamed");
    assert_eq!(d.similarity, Some(100));
  }

  #[test]
  fn test_parse_unified_diff_new_file() {
    let text = r#"diff --git a/new.rs b/new.rs
new file mode 100644
index 0000000..abc
--- /dev/null
+++ b/new.rs
@@ -0,0 +1,3 @@
+line1
+line2
+line3
"#;
    let deltas = parse_unified_diff(text, false).unwrap();
    assert_eq!(deltas.len(), 1);
    let d = &deltas[0];
    assert_eq!(d.path, "new.rs");
    assert_eq!(d.change_type, "added");
    assert_eq!(d.new_mode, Some("100644".to_string()));
    assert_eq!(d.insertions, Some(3));
  }

  #[test]
  fn test_parse_unified_diff_deleted_file() {
    let text = r#"diff --git a/del.rs b/del.rs
deleted file mode 100644
index abc..0000000
--- a/del.rs
+++ /dev/null
@@ -1,3 +0,0 @@
-line1
-line2
-line3
"#;
    let deltas = parse_unified_diff(text, false).unwrap();
    assert_eq!(deltas.len(), 1);
    let d = &deltas[0];
    assert_eq!(d.path, "del.rs");
    assert_eq!(d.change_type, "deleted");
    assert_eq!(d.old_mode, Some("100644".to_string()));
    assert_eq!(d.deletions, Some(3));
  }

  #[test]
  fn test_parse_unified_diff_stat_only() {
    let text = r#"diff --git a/src/main.rs b/src/main.rs
index abc..def 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,5 +10,7 @@
     let x = 1;
-    let y = 2;
+    let z = 3;
     context
"#;
    let deltas = parse_unified_diff(text, true).unwrap();
    assert_eq!(deltas.len(), 1);
    let d = &deltas[0];
    assert!(d.hunks.is_none());
    assert_eq!(d.insertions, Some(1));
    assert_eq!(d.deletions, Some(1));
  }

  #[test]
  fn test_parse_porcelain_v1_z_simple() {
    let data = b" M README.md\0?? untracked.txt\0!! ignored.txt\0";
    let entries = parse_porcelain_v1_z(data).unwrap();
    assert_eq!(entries.len(), 3);

    assert_eq!(entries[0].path, "README.md");
    assert_eq!(entries[0].status, "modified");
    assert!(!entries[0].staged);
    assert!(entries[0].worktree);
    assert_eq!(entries[0].index_char, " ");
    assert_eq!(entries[0].worktree_char, "M");
    assert_eq!(entries[0].display, " M");
    assert_eq!(entries[0].state_description, "Modified in worktree");

    assert_eq!(entries[1].path, "untracked.txt");
    assert_eq!(entries[1].status, "untracked");
    assert_eq!(entries[1].index_char, "?");
    assert_eq!(entries[1].worktree_char, "?");
    assert_eq!(entries[1].state_description, "Untracked");

    assert_eq!(entries[2].path, "ignored.txt");
    assert_eq!(entries[2].status, "ignored");
    assert_eq!(entries[2].index_char, "!");
    assert_eq!(entries[2].worktree_char, "!");
    assert_eq!(entries[2].state_description, "Ignored");
  }

  #[test]
  fn test_parse_porcelain_v1_z_rename() {
    let data = b"R  new.rs\0old.rs\0";
    let entries = parse_porcelain_v1_z(data).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "new.rs");
    assert_eq!(entries[0].old_path, Some("old.rs".to_string()));
    assert_eq!(entries[0].status, "renamed");
    assert!(entries[0].staged);
    assert!(!entries[0].worktree);
    assert_eq!(entries[0].index_char, "R");
    assert_eq!(entries[0].worktree_char, " ");
    assert_eq!(entries[0].state_description, "Renamed in index");
  }

  #[test]
  fn test_parse_porcelain_v1_z_staged_and_worktree() {
    let data = b"AM file.rs\0";
    let entries = parse_porcelain_v1_z(data).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "file.rs");
    assert_eq!(entries[0].status, "modified");
    assert!(entries[0].staged);
    assert!(entries[0].worktree);
    assert_eq!(
      entries[0].state_description,
      "Added in index, modified in worktree"
    );
  }

  #[test]
  fn test_parse_porcelain_v1_z_unmerged() {
    // UU = both modified, AU = added by us, UA = added by them
    let data = b"UU conflict.rs\0AU added_by_us.rs\0UA added_by_them.rs\0";
    let entries = parse_porcelain_v1_z(data).unwrap();
    assert_eq!(entries.len(), 3);

    assert_eq!(entries[0].path, "conflict.rs");
    assert_eq!(entries[0].status, "unmerged");
    assert_eq!(entries[0].index_char, "U");
    assert_eq!(entries[0].worktree_char, "U");
    assert!(entries[0].staged);
    assert!(entries[0].worktree);
    assert_eq!(
      entries[0].state_description,
      "Unmerged in index, unmerged in worktree"
    );

    assert_eq!(entries[1].path, "added_by_us.rs");
    assert_eq!(entries[1].status, "unmerged");
    assert_eq!(entries[1].index_char, "A");
    assert_eq!(entries[1].worktree_char, "U");
    assert_eq!(
      entries[1].state_description,
      "Added in index, unmerged in worktree"
    );

    assert_eq!(entries[2].path, "added_by_them.rs");
    assert_eq!(entries[2].status, "unmerged");
    assert_eq!(entries[2].index_char, "U");
    assert_eq!(entries[2].worktree_char, "A");
    assert_eq!(
      entries[2].state_description,
      "Unmerged in index, added in worktree"
    );
  }

  #[test]
  fn test_parse_porcelain_v1_z_type_changed() {
    let data = b"T  file.rs\0";
    let entries = parse_porcelain_v1_z(data).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "file.rs");
    assert_eq!(entries[0].status, "type_changed");
    assert_eq!(entries[0].index_char, "T");
    assert_eq!(entries[0].worktree_char, " ");
    assert!(entries[0].staged);
    assert!(!entries[0].worktree);
    assert_eq!(entries[0].state_description, "Type changed in index");
  }
}
