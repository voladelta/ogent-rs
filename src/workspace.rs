use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub fn workspace_root() -> &'static Path {
  static ROOT: OnceLock<PathBuf> = OnceLock::new();
  ROOT.get_or_init(|| {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    normalize(&cwd)
  })
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
  if let Some(rest) = path.strip_prefix("~/")
    && let Some(home) = std::env::var_os("HOME")
  {
    return PathBuf::from(home).join(rest);
  }
  let p = PathBuf::from(path);
  if p.is_absolute() {
    normalize(&p)
  } else {
    normalize(&workspace_root().join(p))
  }
}

fn path_in_workspace(path: &Path) -> bool {
  let path = normalize(path);
  let root = workspace_root();
  path == *root || path.starts_with(root)
}

fn path_in_allowed_root(path: &Path) -> bool {
  let Some(home) = std::env::var_os("HOME") else {
    return false;
  };
  let path = normalize(path);
  let root = normalize(&PathBuf::from(home).join(".ogent"));
  path == root || path.starts_with(&root)
}

pub fn normalize(path: &Path) -> PathBuf {
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
