use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

pub fn workspace_root() -> PathBuf {
  std::env::current_dir()
    .unwrap_or_else(|_| PathBuf::from("."))
    .canonicalize()
    .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn workspace_path(path: &str) -> Result<PathBuf> {
  if path.is_empty() {
    bail!("path is required");
  }
  let abs = absolute_tool_path(path);
  if path_in_workspace(&abs) {
    return Ok(abs);
  }
  bail!(
    "path {path} is outside workspace {}",
    workspace_root().display()
  )
}

pub fn readable_path(path: &str) -> Result<PathBuf> {
  if path.is_empty() {
    bail!("path is required");
  }
  let abs = absolute_tool_path(path);
  if path_in_workspace(&abs) || path_in_allowed_root(&abs) {
    return Ok(abs);
  }
  bail!(
    "path {path} is outside workspace {}",
    workspace_root().display()
  )
}

fn absolute_tool_path(path: &str) -> PathBuf {
  if let Some(rest) = path.strip_prefix("~/") {
    if let Some(home) = std::env::var_os("HOME") {
      return PathBuf::from(home).join(rest);
    }
  }
  let p = PathBuf::from(path);
  if p.is_absolute() {
    clean(&p)
  } else {
    clean(&workspace_root().join(p))
  }
}

fn path_in_workspace(path: &Path) -> bool {
  path_in_root(path, &workspace_root())
}

fn path_in_allowed_root(path: &Path) -> bool {
  std::env::var_os("HOME")
    .map(|h| path_in_root(path, &PathBuf::from(h).join(".ogent")))
    .unwrap_or(false)
}

fn path_in_root(path: &Path, root: &Path) -> bool {
  let path = clean(path);
  let root = clean(root);
  path == root || path.starts_with(root)
}

fn clean(path: &Path) -> PathBuf {
  let mut out = PathBuf::new();
  for c in path.components() {
    match c {
      std::path::Component::CurDir => {}
      std::path::Component::ParentDir => {
        out.pop();
      }
      other => out.push(other.as_os_str()),
    }
  }
  out
}
