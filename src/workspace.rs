use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Workspace {
  root: PathBuf,
  allowed_roots: Vec<PathBuf>,
}

impl Workspace {
  pub fn from_current_dir() -> Self {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    Self {
      root: normalize(&cwd),
      allowed_roots: Vec::new(),
    }
  }

  #[cfg(test)]
  pub fn from_root(root: PathBuf) -> Self {
    Self {
      root: normalize(&root),
      allowed_roots: Vec::new(),
    }
  }

  pub fn root(&self) -> &Path {
    &self.root
  }

  pub fn add_allowed_root(&mut self, path: PathBuf) {
    self.allowed_roots.push(normalize(&path));
  }

  pub fn workspace_path(&self, path: &str) -> Result<PathBuf> {
    self.resolve_scoped_path(path, false)
  }

  pub fn readable_path(&self, path: &str) -> Result<PathBuf> {
    self.resolve_scoped_path(path, true)
  }

  fn resolve_scoped_path(&self, path: &str, allow_extra_roots: bool) -> Result<PathBuf> {
    if path.is_empty() {
      bail!("path is required");
    }
    let abs = self.absolute_tool_path(path);
    if self.path_in_workspace(&abs) {
      return Ok(abs);
    }
    if allow_extra_roots {
      for root in &self.allowed_roots {
        if abs == *root || abs.starts_with(root) {
          return Ok(abs);
        }
      }
    }
    bail!("path {path} is outside workspace {}", self.root.display())
  }

  fn absolute_tool_path(&self, path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
      && let Some(home) = std::env::var_os("HOME")
    {
      return PathBuf::from(home).join(rest);
    }
    let p = PathBuf::from(path);
    if p.is_absolute() {
      normalize(&p)
    } else {
      normalize(&self.root.join(p))
    }
  }

  fn path_in_workspace(&self, path: &Path) -> bool {
    let path = normalize(path);
    path == self.root || path.starts_with(&self.root)
  }
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
    let ws = Workspace::from_current_dir();
    assert!(ws.workspace_path("").is_err());
  }

  #[test]
  fn workspace_path_accepts_relative_in_workspace() {
    let ws = Workspace::from_current_dir();
    assert!(ws.workspace_path("src/main.rs").is_ok());
    assert!(ws.workspace_path("./src/main.rs").is_ok());
    assert!(ws.workspace_path(".").is_ok());
  }

  #[test]
  fn workspace_path_rejects_outside_workspace() {
    let ws = Workspace::from_current_dir();
    assert!(ws.workspace_path("/etc/passwd").is_err());
    assert!(ws.workspace_path("/tmp/foo").is_err());
  }

  #[test]
  fn workspace_path_rejects_ogent_config() {
    let ws = Workspace::from_current_dir();
    assert!(ws.workspace_path("~/.ogent/skills/test.md").is_err());
  }

  #[test]
  fn readable_path_rejects_empty() {
    let ws = Workspace::from_current_dir();
    assert!(ws.readable_path("").is_err());
  }

  #[test]
  fn readable_path_accepts_relative_in_workspace() {
    let ws = Workspace::from_current_dir();
    assert!(ws.readable_path("src/main.rs").is_ok());
  }

  #[test]
  fn readable_path_rejects_outside_workspace_and_ogent() {
    let ws = Workspace::from_current_dir();
    assert!(ws.readable_path("/etc/passwd").is_err());
    assert!(ws.readable_path("/tmp/foo").is_err());
  }

  #[test]
  fn readable_path_accepts_ogent_config() {
    if std::env::var_os("HOME").is_none() {
      return;
    }
    let mut ws = Workspace::from_current_dir();
    let home = std::env::var_os("HOME").unwrap();
    ws.add_allowed_root(PathBuf::from(home).join(".ogent"));
    assert!(ws.readable_path("~/.ogent/skills/test.md").is_ok());
  }

  #[test]
  fn workspace_from_root_resolves_relative_paths_against_given_root() {
    let ws = Workspace::from_root(PathBuf::from("/tmp/example"));
    let p = ws.workspace_path("dir/file.txt").unwrap();
    assert_eq!(p, PathBuf::from("/tmp/example/dir/file.txt"));
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
