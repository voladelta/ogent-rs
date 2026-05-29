use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

use crate::tools::{Handler, ToolContext, ToolDef, parse_args};

pub fn tools() -> Vec<ToolDef> {
  vec![ToolDef {
    name: "repo_map",
    description: "Display a tree map of the repository directory structure. path defaults to the workspace root; levels defaults to 3.",
    parameters: json!({"type":"object","properties":{"path":{"type":"string","description":"Directory path relative to workspace root. Default: \".\""},"levels":{"type":"integer","description":"Max depth to descend. Default: 3 if 0 or omitted."}},"additionalProperties":false}),
    handler: Handler::Sync(repo_map),
  }]
}

#[derive(Deserialize)]
struct RepoMapArgs {
  #[serde(default)]
  path: String,
  #[serde(default)]
  levels: usize,
}

fn repo_map(ctx: ToolContext, args: &str) -> Result<String> {
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
      out.push('\n');
    }
  }

  Ok(out)
}
fn path_or_root(path: &str) -> &str {
  if path.is_empty() { "." } else { path }
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
    let skill_store = Arc::new(SkillStore::new(workspace.root(), Vec::new()));
    let ctx = ToolContext {
      workspace,
      skill_store,
      lua_session: Arc::new(std::sync::Mutex::new(None)),
    };

    let args = r#"{"path":"","levels":3}"#;
    let res = repo_map(ctx, args)?;

    assert!(!res.contains("ignored_dir"), "should not contain ignored_dir: \n{}", res);
    assert!(!res.contains("test.tmp"), "should not contain test.tmp: \n{}", res);
    assert!(res.contains("allowed_dir"), "should contain allowed_dir: \n{}", res);
    assert!(res.contains("file.txt"), "should contain file.txt: \n{}", res);
    assert!(res.contains("test.txt"), "should contain test.txt: \n{}", res);

    Ok(())
  }
}


