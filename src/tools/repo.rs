use anyhow::Result;
use serde::Deserialize;
use serde_json::json;
use std::fs;
use std::path::Path;

use crate::tools::{Handler, ToolContext, ToolDef, parse_args};

pub fn tools() -> Vec<ToolDef> {
  vec![
    ToolDef {
      name: "repo_map",
      description: "Display a tree map of the repository directory structure. path defaults to the workspace root; levels defaults to 3.",
      parameters: json!({"type":"object","properties":{"path":{"type":"string","description":"Directory path relative to workspace root. Default: \".\""},"levels":{"type":"integer","description":"Max depth to descend. Default: 3 if 0 or omitted."}},"additionalProperties":false}),
      handler: Handler::Sync(repo_map),
    },
    ToolDef {
      name: "code_map",
      description: "Display a symbol map of source files (Rust, Go, TypeScript, JavaScript, Python, C++, and C#), showing structs, enums, traits, impls, functions, interfaces, types, classes, variables, namespaces, and modules with line ranges. Use to understand the shape and contents of source files before deciding which files or line ranges to read. For a single file, pass its path; for a directory, pass the directory path to map all supported source files inside. Use before read_file to target exact line ranges.",
      parameters: json!({"type":"object","properties":{"path":{"type":"string","description":"File or directory path relative to workspace root. Default: \".\""}},"additionalProperties":false}),
      handler: Handler::Sync(code_map),
    },
  ]
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

fn code_map(ctx: ToolContext, args: &str) -> Result<String> {
  let args: CodeMapArgs = parse_args(args)?;
  let rel = path_or_root(&args.path);
  let path = ctx.workspace.readable_path(rel)?;
  crate::symbol_tree::format_path(&path)
}

fn path_or_root(path: &str) -> &str {
  if path.is_empty() { "." } else { path }
}
