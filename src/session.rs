use crate::types::Message;
use crate::workspace::Workspace;
use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static SESSION_NONCE: AtomicU64 = AtomicU64::new(0);

pub fn generate_session_id() -> String {
  let now = elapsed_since_epoch();
  let nonce = SESSION_NONCE.fetch_add(1, Ordering::Relaxed);
  format!(
    "{}-{:04x}-{:x}-{:x}",
    now.as_secs(),
    std::process::id(),
    now.subsec_nanos(),
    nonce
  )
}

pub fn session_file_in(workspace: &Workspace, session_id: &str) -> PathBuf {
  workspace
    .root()
    .join(".ogent/sessions")
    .join(format!("{}.jsonl", session_id))
}

pub fn persist_session_in(
  workspace: &Workspace,
  messages: &[Message],
  session_id: &str,
) -> Result<()> {
  let path = session_file_in(workspace, session_id);
  persist_messages(messages, &path)
}

fn persist_messages(messages: &[Message], path: &Path) -> Result<()> {
  if messages.is_empty() {
    return Ok(());
  }
  let dir = path
    .parent()
    .context("messages path must have a parent directory")?;
  fs::create_dir_all(dir)?;
  let mut file = fs::File::create(path)?;
  for message in messages {
    serde_json::to_writer(&mut file, message)?;
    file.write_all(b"\n")?;
  }
  Ok(())
}

fn elapsed_since_epoch() -> std::time::Duration {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
}

#[cfg(test)]
pub fn timestamp_ms() -> u64 {
  elapsed_since_epoch().as_millis() as u64
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn timestamp_ms_increases() {
    let a = timestamp_ms();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let b = timestamp_ms();
    assert!(b > a);
  }

  #[test]
  fn generate_session_id_is_unique_in_process() {
    let a = generate_session_id();
    let b = generate_session_id();
    assert_ne!(a, b);
  }

  #[test]
  fn workspace_scoped_paths_use_workspace_root() {
    let ws = Workspace::from_root(PathBuf::from("/tmp/ogent-session-test"));
    assert_eq!(
      session_file_in(&ws, "abc"),
      PathBuf::from("/tmp/ogent-session-test/.ogent/sessions/abc.jsonl")
    );
  }
}
