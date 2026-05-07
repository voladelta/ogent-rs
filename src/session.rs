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
