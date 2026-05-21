use anyhow::{Result, bail};
use std::fs;
use std::path::PathBuf;

use crate::types::{Message, MessageOrigin};

pub const WORKER_PROMPT_OGENT: &str = include_str!("../workers/ogent.md");
pub const WORKER_PROMPT_IMPLEMENTER: &str = include_str!("../workers/implementer.md");
pub const WORKER_PROMPT_VERIFIER: &str = include_str!("../workers/verifier.md");
pub const WORKER_PROMPT_DEBUGGER: &str = include_str!("../workers/debugger.md");
pub const WORKER_PROMPT_RESEARCHER: &str = include_str!("../workers/researcher.md");
pub const WORKER_PROMPT_WRITER: &str = include_str!("../workers/writer.md");
pub const WORKER_PROMPT_CRITIC: &str = include_str!("../workers/critic.md");
pub const WORKER_PROMPT_VISUAL_DESIGNER: &str = include_str!("../workers/visual_designer.md");
pub const WORKER_PROMPT_DATABASE_ARCHITECT: &str = include_str!("../workers/database_architect.md");
pub const WORKER_PROMPT_SYSTEM_ARCHITECT: &str = include_str!("../workers/system_architect.md");
pub const WORKER_PROMPT_SUMMARIZER: &str = include_str!("../workers/summarizer.md");
pub const WORKER_PROMPT_REVIEWER: &str = include_str!("../workers/reviewer.md");
pub const WORKER_PROMPT_QA_WRITER: &str = include_str!("../workers/qa_writer.md");

pub fn get_builtin_worker_prompt(name: &str) -> Option<&'static str> {
  match name {
    "ogent" => Some(WORKER_PROMPT_OGENT),
    "implementer" => Some(WORKER_PROMPT_IMPLEMENTER),
    "verifier" => Some(WORKER_PROMPT_VERIFIER),
    "debugger" => Some(WORKER_PROMPT_DEBUGGER),
    "researcher" => Some(WORKER_PROMPT_RESEARCHER),
    "writer" => Some(WORKER_PROMPT_WRITER),
    "critic" => Some(WORKER_PROMPT_CRITIC),
    "visual_designer" => Some(WORKER_PROMPT_VISUAL_DESIGNER),
    "database_architect" => Some(WORKER_PROMPT_DATABASE_ARCHITECT),
    "system_architect" => Some(WORKER_PROMPT_SYSTEM_ARCHITECT),
    "summarizer" => Some(WORKER_PROMPT_SUMMARIZER),
    "reviewer" => Some(WORKER_PROMPT_REVIEWER),
    "qa_writer" => Some(WORKER_PROMPT_QA_WRITER),
    _ => None,
  }
}

pub fn skill_roots() -> Vec<PathBuf> {
  let mut dirs = vec![PathBuf::from(".ogent/skills")];
  if let Some(home) = std::env::var_os("HOME") {
    dirs.push(PathBuf::from(home).join(".ogent/skills"));
  }
  dirs
}

pub fn load_skill_content(skill_name: &str) -> Result<(String, String, String)> {
  for dir in skill_roots() {
    let root = dir.join(skill_name);
    let path = root.join("SKILL.md");
    let Ok(content) = fs::read_to_string(&path) else {
      continue;
    };
    let (name, _) = parse_skill_frontmatter(&content);
    return Ok((
      if name.is_empty() {
        skill_name.to_string()
      } else {
        name
      },
      root.display().to_string(),
      strip_frontmatter(&content),
    ));
  }
  bail!("skill {skill_name} not found in local .ogent/skills or ~/.ogent/skills")
}

pub fn discover_skills_message() -> String {
  let mut seen = std::collections::HashSet::new();
  let mut out = String::from("<skills>\n");
  for root in skill_roots() {
    let Ok(entries) = fs::read_dir(root) else {
      continue;
    };
    for entry in entries.flatten() {
      if !entry.file_type().is_ok_and(|t| t.is_dir()) {
        continue;
      }
      let Ok(content) = fs::read_to_string(entry.path().join("SKILL.md")) else {
        continue;
      };
      let (name, desc) = parse_skill_frontmatter(&content);
      let dir_name = entry.file_name().to_string_lossy().to_string();
      let key = if name.is_empty() { dir_name } else { name };
      if !seen.insert(key.clone()) {
        continue;
      }
      out.push_str("  <skill name=\"");
      out.push_str(&xml_escape(&key));
      out.push_str("\" description=\"");
      out.push_str(&xml_escape(&desc));
      out.push_str("\" />\n");
    }
  }
  if seen.is_empty() {
    String::new()
  } else {
    out.push_str("</skills>");
    out
  }
}

fn parse_frontmatter(content: &str) -> Option<&str> {
  content
    .strip_prefix("---")
    .and_then(|rest| rest.find("---").map(|end| &rest[..end]))
}

fn strip_frontmatter(content: &str) -> String {
  let Some(fm) = parse_frontmatter(content) else {
    return content.trim().to_string();
  };
  let start = 3 + fm.len() + 3;
  content[start..].trim().to_string()
}

fn parse_skill_frontmatter(content: &str) -> (String, String) {
  let Some(fm) = parse_frontmatter(content) else {
    return (String::new(), String::new());
  };
  let mut name = String::new();
  let mut description = String::new();

  if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(fm) {
    if let Some(n) = value.get("name").and_then(|v| v.as_str()) {
      name = n.to_string();
    }
    if let Some(d) = value.get("description").and_then(|v| v.as_str()) {
      description = d.to_string();
    }
  } else {
    for line in fm.lines().map(str::trim) {
      if let Some(rest) = line.strip_prefix("name:") {
        name = rest.trim().to_string();
      } else if let Some(rest) = line.strip_prefix("description:") {
        description = rest.trim().to_string();
      }
    }
  }
  (name, description)
}

fn xml_escape(s: &str) -> String {
  let mut out = String::with_capacity(s.len());
  for c in s.chars() {
    match c {
      '&' => out.push_str("&amp;"),
      '"' => out.push_str("&quot;"),
      '<' => out.push_str("&lt;"),
      '>' => out.push_str("&gt;"),
      _ => out.push(c),
    }
  }
  out
}

#[cfg(test)]
pub fn build_messages(prompt: &str) -> Vec<Message> {
  let mut messages = vec![Message {
    role: "system".into(),
    content: WORKER_PROMPT_OGENT.to_string(),
    origin: MessageOrigin::Internal,
    ..Default::default()
  }];
  if !prompt.is_empty() {
    messages.push(Message {
      role: "user".into(),
      content: prompt.to_string(),
      origin: MessageOrigin::Human,
      ..Default::default()
    });
  }
  messages
}

pub fn enrich_initial_messages(messages: &mut Vec<Message>) {
  push_internal_user_message(messages, discover_skills_message());
  if let Ok((name, root, body)) = load_skill_content("colgrep") {
    push_internal_user_message(
      messages,
      format!("<skill name=\"{name}\" root=\"{root}\">\n{body}\n</skill>"),
    );
  }
}

fn push_internal_user_message(messages: &mut Vec<Message>, content: String) {
  if content.is_empty() {
    return;
  }
  messages.push(Message {
    role: "user".into(),
    content,
    origin: MessageOrigin::Internal,
    ..Default::default()
  });
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn push_internal_user_message_preserves_human_message() {
    let mut messages = build_messages("do the task");

    push_internal_user_message(&mut messages, "internal context".into());

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[1].origin, MessageOrigin::Human);
    assert_eq!(messages[1].content, "do the task");
    assert_eq!(messages[2].role, "user");
    assert_eq!(messages[2].origin, MessageOrigin::Internal);
    assert_eq!(messages[2].content, "internal context");
  }

  #[test]
  fn push_internal_user_message_does_not_merge_internal_messages() {
    let mut messages = build_messages("do the task");

    push_internal_user_message(&mut messages, "first".into());
    push_internal_user_message(&mut messages, "second".into());

    assert_eq!(messages.len(), 4);
    assert_eq!(messages[2].content, "first");
    assert_eq!(messages[2].origin, MessageOrigin::Internal);
    assert_eq!(messages[3].content, "second");
    assert_eq!(messages[3].origin, MessageOrigin::Internal);
  }

  #[test]
  fn push_internal_user_message_skips_empty_content() {
    let mut messages = build_messages("do the task");

    push_internal_user_message(&mut messages, String::new());

    assert_eq!(messages.len(), 2);
  }

  #[test]
  fn qa_writer_is_builtin_worker_prompt() {
    let prompt = get_builtin_worker_prompt("qa_writer").unwrap();
    assert!(prompt.contains("You are QA Writer."));
    assert!(prompt.contains("Accepted Answer"));
  }

  #[test]
  fn ogent_is_builtin_worker_prompt() {
    let prompt = get_builtin_worker_prompt("ogent").unwrap();
    assert!(prompt.contains("Core Contract"));
    assert!(prompt.contains("Final Reporting"));
  }
}
