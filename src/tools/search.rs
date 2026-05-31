use anyhow::{Context, Result, bail};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use crate::tools::{ToolContext, parse_args, require_nonempty};

const MAX_CONTEXT: usize = 5;
const DEFAULT_MAX_MATCHES: usize = 100;
const MAX_MATCHES: usize = 500;
const MAX_SEARCH_FILE_BYTES: u64 = 1 << 20;

#[derive(Deserialize)]
struct SearchTextArgs {
  pattern: String,
  paths: Option<Vec<String>>,
  #[serde(default)]
  regex: bool,
  #[serde(default = "default_true")]
  case_sensitive: bool,
  #[serde(default)]
  context: usize,
  #[serde(default)]
  max_matches: usize,
}

#[derive(Deserialize, Serialize)]
struct TextMatch {
  path: String,
  line: usize,
  column: usize,
  text: String,
  before: Vec<String>,
  after: Vec<String>,
}

pub fn search_text(ctx: ToolContext, args: &str) -> Result<String> {
  let args: SearchTextArgs = parse_args(args)?;
  require_nonempty(&args.pattern, "pattern")?;
  let context = args.context.min(MAX_CONTEXT);
  let max_matches = if args.max_matches == 0 {
    DEFAULT_MAX_MATCHES
  } else {
    args.max_matches.min(MAX_MATCHES)
  };
  let paths = args.paths.unwrap_or_else(|| vec![".".to_string()]);
  let matcher = build_matcher(&args.pattern, args.regex, args.case_sensitive)?;
  let mut out = Vec::new();

  for requested in paths {
    require_nonempty(&requested, "paths entries")?;
    let start = ctx.workspace.workspace_path(&requested)?;
    if !start.is_file() && !start.is_dir() {
      bail!("path {requested} is not a file or directory");
    }

    let walk_root = if start.is_file() {
      ctx.workspace.root()
    } else {
      &start
    };
    let walker = ignore::WalkBuilder::new(walk_root)
      .sort_by_file_name(|a, b| a.cmp(b))
      .build();
    for entry in walker {
      let entry = entry?;
      if out.len() >= max_matches {
        break;
      }
      if start.is_file() && entry.path() != start {
        continue;
      }
      if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
        search_file(
          ctx.workspace.root(),
          entry.path(),
          &matcher,
          context,
          max_matches,
          &mut out,
        )?;
      }
    }
    if out.len() >= max_matches {
      break;
    }
  }

  Ok(serde_json::to_string(&out)?)
}

#[derive(Deserialize)]
struct OutlineArgs {
  path: String,
}

#[derive(Deserialize, Serialize)]
struct OutlineEntry {
  name: String,
  kind: String,
  start_line: usize,
  #[serde(skip_serializing_if = "Option::is_none")]
  end_line: Option<usize>,
  signature: String,
}

pub fn outline(ctx: ToolContext, args: &str) -> Result<String> {
  let args: OutlineArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  if Path::new(&args.path).extension().and_then(|e| e.to_str()) != Some("rs") {
    bail!("outline currently supports Rust .rs files only");
  }

  let path = ctx.workspace.workspace_path(&args.path)?;
  let meta = fs::metadata(&path).with_context(|| format!("stat {}", args.path))?;
  if meta.len() > MAX_SEARCH_FILE_BYTES {
    bail!(
      "file {} exceeds size limit ({} > {} bytes)",
      args.path,
      meta.len(),
      MAX_SEARCH_FILE_BYTES
    );
  }
  let source = fs::read_to_string(&path).with_context(|| format!("read {}", args.path))?;
  let lines: Vec<&str> = source.lines().collect();
  let mut entries = Vec::new();

  for (idx, line) in lines.iter().enumerate() {
    let Some((kind, name)) = rust_item(line) else {
      continue;
    };
    entries.push(OutlineEntry {
      name,
      kind,
      start_line: idx + 1,
      end_line: item_end_line(&lines, idx),
      signature: compact_signature(line),
    });
  }

  Ok(serde_json::to_string(&entries)?)
}

fn build_matcher(pattern: &str, regex: bool, case_sensitive: bool) -> Result<Regex> {
  let pattern = if regex {
    pattern.to_string()
  } else {
    regex::escape(pattern)
  };
  RegexBuilder::new(&pattern)
    .case_insensitive(!case_sensitive)
    .build()
    .with_context(|| format!("invalid regex pattern {pattern:?}"))
}

fn search_file(
  root: &Path,
  path: &Path,
  matcher: &Regex,
  context: usize,
  max_matches: usize,
  out: &mut Vec<TextMatch>,
) -> Result<()> {
  if out.len() >= max_matches {
    return Ok(());
  }
  let meta = match fs::metadata(path) {
    Ok(meta) => meta,
    Err(_) => return Ok(()),
  };
  if meta.len() > MAX_SEARCH_FILE_BYTES {
    return Ok(());
  }
  let source = match fs::read_to_string(path) {
    Ok(source) => source,
    Err(_) => return Ok(()),
  };
  let lines: Vec<&str> = source.lines().collect();
  let rel_path = path.strip_prefix(root).unwrap_or(path).to_string_lossy();

  for (idx, line) in lines.iter().enumerate() {
    if let Some(found) = matcher.find(line) {
      let before_start = idx.saturating_sub(context);
      let after_end = (idx + 1 + context).min(lines.len());
      out.push(TextMatch {
        path: rel_path.to_string(),
        line: idx + 1,
        column: found.start() + 1,
        text: (*line).to_string(),
        before: lines[before_start..idx]
          .iter()
          .map(|line| (*line).to_string())
          .collect(),
        after: lines[(idx + 1)..after_end]
          .iter()
          .map(|line| (*line).to_string())
          .collect(),
      });
      if out.len() >= max_matches {
        break;
      }
    }
  }
  Ok(())
}

fn rust_item(line: &str) -> Option<(String, String)> {
  let trimmed = line.trim_start();
  if trimmed.starts_with("//") || trimmed.starts_with("#[") {
    return None;
  }
  let without_vis = trimmed
    .strip_prefix("pub(crate) ")
    .or_else(|| trimmed.strip_prefix("pub(super) "))
    .or_else(|| trimmed.strip_prefix("pub "))
    .unwrap_or(trimmed);
  let without_async = without_vis.strip_prefix("async ").unwrap_or(without_vis);

  for (keyword, kind) in [
    ("fn ", "function"),
    ("struct ", "struct"),
    ("enum ", "enum"),
    ("trait ", "trait"),
    ("mod ", "mod"),
  ] {
    if let Some(rest) = without_async.strip_prefix(keyword) {
      return Some((kind.to_string(), leading_ident(rest)?));
    }
  }
  without_vis
    .strip_prefix("impl")
    .map(|rest| ("impl".to_string(), impl_name(rest)))
}

fn leading_ident(rest: &str) -> Option<String> {
  let name: String = rest
    .chars()
    .skip_while(|ch| ch.is_whitespace())
    .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
    .collect();
  if name.is_empty() { None } else { Some(name) }
}

fn impl_name(rest: &str) -> String {
  let head = rest
    .split('{')
    .next()
    .unwrap_or(rest)
    .split(" where ")
    .next()
    .unwrap_or(rest)
    .trim();
  if head.is_empty() {
    "impl".to_string()
  } else {
    head.to_string()
  }
}

fn compact_signature(line: &str) -> String {
  line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn item_end_line(lines: &[&str], start_idx: usize) -> Option<usize> {
  let mut depth = 0usize;
  let mut saw_open = false;
  for (idx, line) in lines.iter().enumerate().skip(start_idx) {
    for ch in line.chars() {
      match ch {
        '{' => {
          saw_open = true;
          depth += 1;
        }
        '}' if depth > 0 => {
          depth -= 1;
          if saw_open && depth == 0 {
            return Some(idx + 1);
          }
        }
        _ => {}
      }
    }
    if !saw_open && line.trim_end().ends_with(';') {
      return Some(idx + 1);
    }
  }
  None
}

fn default_true() -> bool {
  true
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::skills::SkillStore;
  use crate::workspace::Workspace;
  use std::path::PathBuf;
  use std::sync::Arc;

  fn test_context(root: PathBuf) -> ToolContext {
    let workspace = Workspace::from_root(root);
    let skill_store = Arc::new(SkillStore::new(workspace.root()));
    let client = crate::client::Client::new(
      "http://localhost",
      "dummy".into(),
      |_, _| Ok(serde_json::Value::Null),
      30,
    )
    .unwrap();
    ToolContext {
      workspace,
      skill_store,
      lua_session: Arc::new(parking_lot::Mutex::new(None)),
      client,
      output_sink: None,
      verbose: false,
      actor_id: "director".to_string(),
      agent_depth: 0,
    }
  }

  #[test]
  fn search_text_respects_gitignore_and_bounds_results() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::write(root.join(".gitignore"), "ignored/\n")?;
    fs::create_dir(root.join(".git"))?;
    fs::create_dir(root.join("ignored"))?;
    fs::write(root.join("ignored/file.rs"), "needle\n")?;
    fs::write(root.join("a.rs"), "one\nNeedle\nneedle\n")?;

    let res = search_text(
      test_context(root.to_path_buf()),
      r#"{"pattern":"needle","case_sensitive":false,"context":1,"max_matches":1}"#,
    )?;
    let matches: Vec<TextMatch> = serde_json::from_str(&res)?;
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].path, "a.rs");
    assert_eq!(matches[0].line, 2);
    assert_eq!(matches[0].column, 1);
    assert_eq!(matches[0].before, vec!["one"]);

    let res = search_text(
      test_context(root.to_path_buf()),
      r#"{"pattern":"needle","paths":["ignored/file.rs"]}"#,
    )?;
    let matches: Vec<TextMatch> = serde_json::from_str(&res)?;
    assert!(matches.is_empty());

    Ok(())
  }

  #[test]
  fn outline_returns_lightweight_rust_items() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::write(
      root.join("lib.rs"),
      "pub mod api;\n\npub struct Thing {\n  value: i32,\n}\n\nimpl Thing {\n  pub fn new() -> Self {\n    Self { value: 1 }\n  }\n}\n",
    )?;

    let res = outline(test_context(root.to_path_buf()), r#"{"path":"lib.rs"}"#)?;
    let entries: Vec<OutlineEntry> = serde_json::from_str(&res)?;
    assert!(entries.iter().any(|e| e.kind == "mod" && e.name == "api"));
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "struct" && e.name == "Thing" && e.end_line == Some(5))
    );
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "impl" && e.name == "Thing")
    );
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "function" && e.name == "new")
    );

    Ok(())
  }
}
