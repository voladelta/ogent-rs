use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::types::Message;

static COUNTER: AtomicU32 = AtomicU32::new(0);

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
  let mut file = fs::File::create(&path)?;
  for message in messages {
    serde_json::to_writer(&mut file, message)?;
    file.write_all(b"\n")?;
  }
  Ok(())
}

pub fn append_journal(session_id: &str, summary: &str) -> Result<()> {
  if summary.trim().is_empty() {
    return Ok(());
  }
  fs::create_dir_all(".ogent")?;
  let ts = timestamp();
  let mut file = OpenOptions::new()
    .create(true)
    .append(true)
    .open(".ogent/journal.md")?;
  writeln!(file, "## Session {ts}")?;
  writeln!(file)?;
  writeln!(file, "- Timestamp: {ts}")?;
  writeln!(file, "- Session: {session_id}")?;
  writeln!(file)?;
  writeln!(file, "{}", summary.trim())?;
  writeln!(file)?;
  writeln!(file, "---")?;
  writeln!(file)?;
  Ok(())
}

pub fn find_latest_handoff(dir: &str) -> Option<String> {
  find_latest_file(dir, "md", |_| true)
}

pub fn find_latest_session(dir: &str) -> Option<String> {
  find_latest_file(dir, "jsonl", |name| !name.contains("-worker-"))
}

fn find_latest_file(dir: &str, ext: &str, name_filter: fn(&str) -> bool) -> Option<String> {
  let mut entries: Vec<_> = fs::read_dir(dir)
    .ok()?
    .flatten()
    .filter(|e| {
      let path = e.path();
      path.extension().is_some_and(|e| e == ext)
        && path
          .file_name()
          .and_then(|n| n.to_str())
          .is_some_and(name_filter)
    })
    .collect();
  entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
  entries.last().map(|e| e.path().display().to_string())
}

pub fn load_session(path: &str) -> Result<Vec<Message>> {
  let data = fs::read_to_string(path)?;
  data.lines()
    .filter(|l| !l.trim().is_empty())
    .map(|line| {
      serde_json::from_str(line)
        .context("parse error in session file")
    })
    .collect()
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
  format!("{:04x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}
