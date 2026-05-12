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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn workspace_path_rejects_empty() {
    assert!(workspace_path("").is_err());
  }

  #[test]
  fn workspace_path_accepts_relative_in_workspace() {
    assert!(workspace_path("src/main.rs").is_ok());
    assert!(workspace_path("./src/main.rs").is_ok());
    assert!(workspace_path(".").is_ok());
  }

  #[test]
  fn workspace_path_rejects_outside_workspace() {
    assert!(workspace_path("/etc/passwd").is_err());
    assert!(workspace_path("/tmp/foo").is_err());
  }

  #[test]
  fn workspace_path_rejects_ogent_config() {
    assert!(workspace_path("~/.ogent/skills/test.md").is_err());
  }

  #[test]
  fn readable_path_rejects_empty() {
    assert!(readable_path("").is_err());
  }

  #[test]
  fn readable_path_accepts_relative_in_workspace() {
    assert!(readable_path("src/main.rs").is_ok());
  }

  #[test]
  fn readable_path_rejects_outside_workspace_and_ogent() {
    assert!(readable_path("/etc/passwd").is_err());
    assert!(readable_path("/tmp/foo").is_err());
  }

  #[test]
  fn readable_path_accepts_ogent_config() {
    if std::env::var_os("HOME").is_none() {
      return;
    }
    assert!(readable_path("~/.ogent/skills/test.md").is_ok());
  }

  #[test]
  fn normalize_collapses_dot_and_dotdot() {
    let p = Path::new("/a/b/./c/../d");
    let n = normalize(p);
    assert_eq!(n, Path::new("/a/b/d"));
  }

  #[test]
  fn normalize_leading_parent_dir_pops_nothing() {
    let p = Path::new("../a/b");
    let n = normalize(p);
    assert_eq!(n, Path::new("a/b"));
  }
}
