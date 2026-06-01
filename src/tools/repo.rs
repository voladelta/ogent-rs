use anyhow::Result;
use serde::Deserialize;

use crate::tools::{ToolContext, parse_args, require_nonempty};

#[derive(Deserialize)]
struct RepoMapArgs {
  #[serde(default)]
  path: String,
  #[serde(default)]
  levels: usize,
}

pub fn repo_map(ctx: ToolContext, args: &str) -> Result<String> {
  let args: RepoMapArgs = parse_args(args)?;
  let rel = path_or_root(&args.path);
  let path = ctx.workspace.readable_path(rel)?;
  let levels = if args.levels == 0 { 3 } else { args.levels };
  let mut out = String::new();

  let walker = ignore::WalkBuilder::new(&path)
    .max_depth(Some(levels))
    .sort_by_file_name(|a, b| a.cmp(b))
    .build();

  for entry in walker {
    let entry = entry?;
    let depth = entry.depth();
    if depth == 0 {
      out.push_str(".\n");
    } else {
      let name = entry.file_name().to_string_lossy();
      out.push_str(&"  ".repeat(depth));
      out.push_str(&name);
      // Append human-readable size for files only
      if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false)
        && let Ok(meta) = entry.metadata()
      {
        out.push_str(&format!("  {}", human_size(meta.len())));
      }
      out.push('\n');
    }
  }

  Ok(out)
}

fn human_size(bytes: u64) -> String {
  const KB: f64 = 1024.0;
  const MB: f64 = KB * 1024.0;
  const GB: f64 = MB * 1024.0;

  if bytes < 1024 {
    format!("{} B", bytes)
  } else if bytes < 1024 * 1024 {
    format!("{:.1} KB", bytes as f64 / KB)
  } else if bytes < 1024 * 1024 * 1024 {
    format!("{:.1} MB", bytes as f64 / MB)
  } else {
    format!("{:.1} GB", bytes as f64 / GB)
  }
}
fn path_or_root(path: &str) -> &str {
  if path.is_empty() { "." } else { path }
}

#[derive(Deserialize)]
struct GlobArgs {
  pattern: String,
}

pub fn glob(ctx: ToolContext, args: &str) -> Result<String> {
  let args: GlobArgs = parse_args(args)?;
  require_nonempty(&args.pattern, "pattern")?;

  let glob = globset::Glob::new(&args.pattern)?;
  let matcher = glob.compile_matcher();

  let mut matches = Vec::new();
  let root = ctx.workspace.root();
  let walker = ignore::WalkBuilder::new(root)
    .sort_by_file_name(|a, b| a.cmp(b))
    .build();

  for entry in walker {
    let entry = entry?;
    if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false)
      && let Ok(rel_path) = entry.path().strip_prefix(root)
      && matcher.is_match(rel_path)
    {
      matches.push(rel_path.to_string_lossy().into_owned());
    }
  }

  Ok(serde_json::to_string(&matches)?)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::skills::SkillStore;
  use crate::workspace::Workspace;
  use std::sync::Arc;

  #[test]
  fn test_repo_map_respects_gitignore() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();

    std::fs::write(root.join(".gitignore"), "ignored_dir/\n*.tmp\n")?;
    std::fs::create_dir(root.join(".git"))?;

    std::fs::create_dir(root.join("ignored_dir"))?;
    std::fs::write(root.join("ignored_dir/file.txt"), "hello")?;

    std::fs::create_dir(root.join("allowed_dir"))?;
    std::fs::write(root.join("allowed_dir/file.txt"), "hello")?;
    std::fs::write(root.join("test.tmp"), "ignored")?;
    std::fs::write(root.join("test.txt"), "allowed")?;

    let workspace = Workspace::from_root(root.to_path_buf());
    let skill_store = Arc::new(SkillStore::new(workspace.root()));
    let client = crate::client::Client::new(
      crate::client::ClientConfig {
        url: "http://localhost".to_string(),
        api_key: "dummy".into(),
        request_timeout_secs: 30,
        require_sse_done: true,
      },
      |_, _| Ok(serde_json::Value::Null),
    )
    .unwrap();
    let ctx = ToolContext {
      workspace,
      skill_store,
      lua_session: Arc::new(parking_lot::Mutex::new(None)),
      client,
      output_sink: None,
      verbose: false,
      actor_id: "director".to_string(),
      agent_depth: 0,
    };

    let args = r#"{"path":"","levels":3}"#;
    let res = repo_map(ctx, args)?;

    assert!(
      !res.contains("ignored_dir"),
      "should not contain ignored_dir: \n{}",
      res
    );
    assert!(
      !res.contains("test.tmp"),
      "should not contain test.tmp: \n{}",
      res
    );
    assert!(
      res.contains("allowed_dir"),
      "should contain allowed_dir: \n{}",
      res
    );
    assert!(
      res.contains("file.txt"),
      "should contain file.txt: \n{}",
      res
    );
    assert!(
      res.contains("test.txt"),
      "should contain test.txt: \n{}",
      res
    );

    Ok(())
  }

  #[test]
  fn test_glob_respects_gitignore() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();

    std::fs::write(root.join(".gitignore"), "ignored_dir/\n*.tmp\n")?;
    std::fs::create_dir(root.join(".git"))?;

    std::fs::create_dir(root.join("ignored_dir"))?;
    std::fs::write(root.join("ignored_dir/file.rs"), "hello")?;

    std::fs::create_dir(root.join("allowed_dir"))?;
    std::fs::write(root.join("allowed_dir/file.rs"), "hello")?;
    std::fs::write(root.join("test.tmp"), "ignored")?;
    std::fs::write(root.join("test.rs"), "allowed")?;

    let workspace = Workspace::from_root(root.to_path_buf());
    let skill_store = Arc::new(SkillStore::new(workspace.root()));
    let client = crate::client::Client::new(
      crate::client::ClientConfig {
        url: "http://localhost".to_string(),
        api_key: "dummy".into(),
        request_timeout_secs: 30,
        require_sse_done: true,
      },
      |_, _| Ok(serde_json::Value::Null),
    )
    .unwrap();
    let ctx = ToolContext {
      workspace,
      skill_store,
      lua_session: Arc::new(parking_lot::Mutex::new(None)),
      client,
      output_sink: None,
      verbose: false,
      actor_id: "director".to_string(),
      agent_depth: 0,
    };

    let args = r#"{"pattern":"**/*.rs"}"#;
    let res = glob(ctx, args)?;
    let files: Vec<String> = serde_json::from_str(&res)?;

    assert_eq!(files.len(), 2);
    assert!(files.contains(&"test.rs".to_string()));
    assert!(files.contains(&"allowed_dir/file.rs".to_string()));
    assert!(!files.contains(&"ignored_dir/file.rs".to_string()));

    Ok(())
  }
}
