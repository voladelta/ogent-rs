use crate::types::Message;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
  pub session_id: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub parent_session: Option<String>,
  pub profile: String,
  pub mode: String,
  pub flags: SessionFlags,
  pub usage: SessionUsage,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub prompt: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub start_ts: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub end_ts: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFlags {
  pub steer: bool,
  pub worker: bool,
  pub autocompact: i32,
  pub handoff: bool,
  #[serde(rename = "continue")]
  pub continue_flag: bool,
  pub resume: bool,
  pub temp: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionUsage {
  pub total_tokens: i32,
}

pub fn generate_session_id() -> String {
  format!("{}-{:04x}", timestamp(), std::process::id())
}

pub fn session_dir(session_id: &str) -> PathBuf {
  PathBuf::from(format!(".ogent/sessions/{session_id}"))
}

pub fn write_meta(meta: &SessionMeta) -> Result<()> {
  let dir = session_dir(&meta.session_id);
  fs::create_dir_all(&dir)?;
  let data = serde_json::to_string_pretty(meta)?;
  fs::write(dir.join("meta.json"), data)?;
  Ok(())
}

pub fn read_meta(session_id: &str) -> Result<SessionMeta> {
  let path = session_dir(session_id).join("meta.json");
  let data =
    fs::read_to_string(&path).with_context(|| format!("no meta.json in session {session_id}"))?;
  serde_json::from_str(&data).context("invalid meta.json")
}

pub fn persist_session(messages: &[Message], session_id: &str) -> Result<()> {
  if messages.is_empty() {
    return Ok(());
  }
  let dir = session_dir(session_id);
  fs::create_dir_all(&dir)?;
  let path = dir.join("messages.jsonl");
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
  let latest_dir = find_latest_session_dir(dir);
  if latest_dir.is_some() {
    return latest_dir;
  }
  find_latest_file(dir, "jsonl", |name| !name.contains("-worker-"))
}

fn find_latest_session_dir(dir: &str) -> Option<String> {
  let mut entries: Vec<_> = fs::read_dir(dir)
    .ok()?
    .flatten()
    .filter(|e| e.path().is_dir())
    .filter(|e| e.path().join("meta.json").exists())
    .collect();
  entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
  entries
    .last()
    .and_then(|e| e.path().file_name()?.to_str().map(String::from))
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

pub fn load_session(path_or_id: &str) -> Result<Vec<Message>> {
  let dir = session_dir(path_or_id);
  if dir.join("messages.jsonl").exists() {
    return load_jsonl_file(&dir.join("messages.jsonl"));
  }
  let p = path_or_id;
  let p = p.strip_suffix("/messages.jsonl").unwrap_or(p);
  let p = p.strip_suffix(".jsonl").unwrap_or(p);
  if let Some(id) = p.strip_prefix(".ogent/sessions/") {
    let id = id.strip_suffix('/').unwrap_or(id);
    let dir = session_dir(id);
    if dir.join("messages.jsonl").exists() {
      return load_jsonl_file(&dir.join("messages.jsonl"));
    }
  }
  let jsonl_path = PathBuf::from(path_or_id);
  if jsonl_path.exists() {
    return load_jsonl_file(&jsonl_path);
  }
  let jsonl_path = PathBuf::from(format!("{path_or_id}.jsonl"));
  if jsonl_path.exists() {
    return load_jsonl_file(&jsonl_path);
  }
  anyhow::bail!("session not found: {path_or_id}")
}

fn load_jsonl_file(path: &PathBuf) -> Result<Vec<Message>> {
  let data = fs::read_to_string(path)?;
  data
    .lines()
    .filter(|l| !l.trim().is_empty())
    .map(|line| serde_json::from_str(line).context("parse error in session file"))
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

pub fn timestamp_ms() -> u64 {
  use std::time::{SystemTime, UNIX_EPOCH};
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_millis() as u64
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn meta_serializes_new_fields() {
    let meta = SessionMeta {
      session_id: "abc".into(),
      parent_session: None,
      profile: "ds-pro".into(),
      mode: "steer".into(),
      flags: SessionFlags {
        steer: true,
        worker: false,
        autocompact: -1,
        handoff: false,
        continue_flag: false,
        resume: false,
        temp: false,
      },
      usage: SessionUsage { total_tokens: 150 },
      prompt: Some("fix bug".into()),
      start_ts: Some(1_234_567_890),
      end_ts: Some(1_234_567_999),
    };
    let json = serde_json::to_string_pretty(&meta).unwrap();
    assert!(json.contains("\"prompt\""));
    assert!(json.contains("\"fix bug\""));
    assert!(json.contains("\"start_ts\""));
    assert!(json.contains("1234567890"));
    assert!(json.contains("\"end_ts\""));
    assert!(json.contains("1234567999"));
  }

  #[test]
  fn meta_omits_none_fields() {
    let meta = SessionMeta {
      session_id: "abc".into(),
      parent_session: None,
      profile: "ds-pro".into(),
      mode: "steer".into(),
      flags: SessionFlags {
        steer: true,
        worker: false,
        autocompact: -1,
        handoff: false,
        continue_flag: false,
        resume: false,
        temp: false,
      },
      usage: SessionUsage { total_tokens: 0 },
      prompt: None,
      start_ts: None,
      end_ts: None,
    };
    let json = serde_json::to_string_pretty(&meta).unwrap();
    assert!(!json.contains("\"prompt\""));
    assert!(!json.contains("\"start_ts\""));
    assert!(!json.contains("\"end_ts\""));
  }

  #[test]
  fn timestamp_ms_increases() {
    let a = timestamp_ms();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let b = timestamp_ms();
    assert!(b > a);
  }
}
