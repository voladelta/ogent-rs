use crate::types::Message;
use crate::workspace::Workspace;
use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
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

pub fn session_dir_in(workspace: &Workspace, session_id: &str) -> PathBuf {
  workspace.root().join(".ogent/sessions").join(session_id)
}

pub fn state_path_in(workspace: &Workspace, session_id: &str) -> PathBuf {
  session_dir_in(workspace, session_id).join("states.json")
}

#[cfg(test)]
pub struct SessionLock {
  path: PathBuf,
}

#[cfg(test)]
impl Drop for SessionLock {
  fn drop(&mut self) {
    let _ = fs::remove_file(&self.path);
  }
}

#[cfg(test)]
pub fn try_acquire_session_lock_in(workspace: &Workspace, session_id: &str) -> Result<SessionLock> {
  let dir = session_dir_in(workspace, session_id);
  fs::create_dir_all(&dir)?;
  let lock_path = dir.join("active.lock");
  for _ in 0..2 {
    match std::fs::OpenOptions::new()
      .write(true)
      .create_new(true)
      .open(&lock_path)
    {
      Ok(mut file) => {
        writeln!(file, "pid={}", std::process::id())?;
        writeln!(file, "ts_ms={}", timestamp_ms())?;
        return Ok(SessionLock {
          path: lock_path.clone(),
        });
      }
      Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
        if maybe_remove_stale_session_lock(&lock_path)? {
          continue;
        }
      }
      Err(err) => return Err(err).with_context(|| "failed to acquire session lock"),
    }
    break;
  }
  anyhow::bail!("session {session_id} is already active")
}

#[cfg(test)]
fn maybe_remove_stale_session_lock(lock_path: &std::path::Path) -> Result<bool> {
  let Ok(data) = fs::read_to_string(lock_path) else {
    return Ok(false);
  };
  let mut pid = None;
  for line in data.lines() {
    if let Some(value) = line.trim().strip_prefix("pid=")
      && let Ok(parsed) = value.trim().parse::<u32>()
    {
      pid = Some(parsed);
      break;
    }
  }
  if let Some(pid) = pid
    && process_is_alive(pid)
  {
    return Ok(false);
  }
  match fs::remove_file(lock_path) {
    Ok(()) => Ok(true),
    Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(true),
    Err(err) => Err(err).with_context(|| "failed to remove stale lock"),
  }
}

#[cfg(unix)]
#[cfg(test)]
fn process_is_alive(pid: u32) -> bool {
  matches!(
    std::process::Command::new("kill")
      .arg("-0")
      .arg(pid.to_string())
      .stdout(std::process::Stdio::null())
      .stderr(std::process::Stdio::null())
      .status(),
    Ok(status) if status.success()
  )
}

pub fn persist_session_in(
  workspace: &Workspace,
  messages: &[Message],
  session_id: &str,
) -> Result<()> {
  let path = session_dir_in(workspace, session_id).join("messages.jsonl");
  persist_messages(messages, &path)
}

fn persist_messages(messages: &[Message], path: &PathBuf) -> Result<()> {
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

#[allow(dead_code)]
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
  fn session_lock_is_exclusive() {
    let ws = Workspace::from_current_dir();
    let session_id = format!("lock-test-{}", timestamp_ms());
    let lock1 = try_acquire_session_lock_in(&ws, &session_id).unwrap();
    assert!(try_acquire_session_lock_in(&ws, &session_id).is_err());
    drop(lock1);
    let lock2 = try_acquire_session_lock_in(&ws, &session_id).unwrap();
    drop(lock2);
    let _ = std::fs::remove_dir_all(session_dir_in(&ws, &session_id));
  }

  #[test]
  fn stale_session_lock_is_reclaimed() {
    let ws = Workspace::from_current_dir();
    let session_id = format!("stale-lock-test-{}", timestamp_ms());
    let lock_path = session_dir_in(&ws, &session_id).join("active.lock");
    std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    std::fs::write(&lock_path, "pid=4294967295\nts_ms=0\n").unwrap();
    let lock = try_acquire_session_lock_in(&ws, &session_id).unwrap();
    drop(lock);
    let _ = std::fs::remove_dir_all(session_dir_in(&ws, &session_id));
  }

  #[test]
  fn workspace_scoped_paths_use_workspace_root() {
    let ws = Workspace::from_root(PathBuf::from("/tmp/ogent-session-test"));
    assert_eq!(
      session_dir_in(&ws, "abc"),
      PathBuf::from("/tmp/ogent-session-test/.ogent/sessions/abc")
    );
  }
}
