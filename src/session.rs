use anyhow::Result;
use std::fs;
use std::path::Path;

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

pub fn load_mementos() -> String {
  let dir = Path::new(".ogent/mementos");
  let Ok(entries) = fs::read_dir(dir) else {
    return String::new();
  };
  let mut parts = Vec::new();
  for entry in entries.flatten() {
    let path = entry.path();
    if path.extension().is_some_and(|e| e == "md") {
      if let Ok(data) = fs::read_to_string(path) {
        parts.push(data);
      }
    }
  }
  if parts.is_empty() {
    String::new()
  } else {
    format!("## Previous Session Mementos\n\n{}", parts.join("\n"))
  }
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

pub fn save_memento_to_file(memento: &str) {
  if memento.is_empty() {
    return;
  }
  let _ = fs::create_dir_all(".ogent/mementos");
  let path = format!(".ogent/mementos/{}.md", timestamp());
  let data = format!("# Memento {}\n\n{}\n\n---\n\n", timestamp(), memento);
  let _ = fs::write(path, data);
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
