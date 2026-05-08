use anyhow::Result;
use std::fs::{self, OpenOptions};
use std::io::Write;

use crate::types::Message;

pub fn persist_session(messages: &[Message], worker: bool, session_id: &str) -> Result<()> {
  if messages.is_empty() {
    return Ok(());
  }
  fs::create_dir_all(".ogent/sessions")?;
  let mut name = format!("{}-{}", timestamp(), rand_suffix());
  if worker {
    name = format!("{name}-worker-{session_id}");
  }
  let path = format!(".ogent/sessions/{name}.jsonl");
  let mut out = String::new();
  for message in messages {
    out.push_str(&serde_json::to_string(message)?);
    out.push('\n');
  }
  fs::write(path, out)?;
  Ok(())
}

pub fn append_journal(session_id: &str, summary: &str) -> Result<()> {
  if summary.trim().is_empty() {
    return Ok(());
  }
  fs::create_dir_all(".ogent")?;
  let timestamp = timestamp();
  let mut file = OpenOptions::new()
    .create(true)
    .append(true)
    .open(".ogent/journal.md")?;
  writeln!(file, "## Session {timestamp}")?;
  writeln!(file)?;
  writeln!(file, "- Timestamp: {timestamp}")?;
  writeln!(file, "- Session: {session_id}")?;
  writeln!(file)?;
  writeln!(file, "{}", summary.trim())?;
  writeln!(file)?;
  writeln!(file, "---")?;
  writeln!(file)?;
  Ok(())
}

pub fn find_latest_handoff(dir: &str) -> Option<String> {
  let mut entries: Vec<_> = fs::read_dir(dir)
    .ok()?
    .flatten()
    .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
    .collect();
  entries.sort_by_key(|e| e.file_name());
  entries.last().map(|e| e.path().display().to_string())
}

pub fn find_latest_session(dir: &str) -> Option<String> {
  let mut entries: Vec<_> = fs::read_dir(dir)
    .ok()?
    .flatten()
    .filter(|e| {
      let path = e.path();
      let ext_ok = path.extension().is_some_and(|ext| ext == "jsonl");
      let name_ok = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|name| !name.contains("-worker-"))
        .unwrap_or(false);
      ext_ok && name_ok
    })
    .collect();
  entries.sort_by_key(|e| e.file_name());
  entries.last().map(|e| e.path().display().to_string())
}

pub fn load_session(path: &str) -> Result<Vec<Message>> {
  let data = fs::read_to_string(path)?;
  let mut messages = Vec::new();
  for line in data.lines() {
    if line.trim().is_empty() {
      continue;
    }
    let msg: Message = serde_json::from_str(line)
      .map_err(|e| anyhow::anyhow!("parse error in session file: {e}"))?;
    messages.push(msg);
  }
  Ok(messages)
}

pub fn timestamp() -> String {
  use std::time::{SystemTime, UNIX_EPOCH};
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
    .to_string()
}

fn rand_suffix() -> String {
  use std::time::{SystemTime, UNIX_EPOCH};
  let nanos = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos();
  format!("{:04x}", nanos & 0xFFFF)
}
