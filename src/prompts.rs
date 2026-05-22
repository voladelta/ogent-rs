use anyhow::{Result, bail};
use std::fs;
use std::path::PathBuf;

use crate::types::{Message, MessageOrigin};

pub const SYSTEM_PROMPT: &str = include_str!("../SYSTEM_PROMPT.md");

pub fn compose_system_prompt(context: &str) -> String {
  let context_section = format!("## Context\n\n{}", context.trim());
  compose_prompt_sections(SYSTEM_PROMPT, Some(&context_section))
}

fn compose_prompt_sections(base_prompt: &str, extra_section: Option<&str>) -> String {
  let mut sections = vec![base_prompt.trim().to_string()];
  if let Some(extra) = extra_section
    && !extra.trim().is_empty()
  {
    sections.push(extra.trim().to_string());
  }
  sections.join("\n\n")
}

pub(crate) fn build_initial_messages(
  system_prompt: &str,
  task_prompt: &str,
  session_id: &str,
) -> Vec<Message> {
  let mut messages = vec![Message::system(system_prompt.to_string())];
  enrich_initial_messages(&mut messages);
  messages.push(Message::user(
    format!("[session: {session_id}]\n\n{}", task_prompt.trim()),
    MessageOrigin::Human,
  ));
  messages
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
  let mut messages = vec![Message::system(SYSTEM_PROMPT.to_string())];
  if !prompt.is_empty() {
    messages.push(Message::user(prompt.to_string(), MessageOrigin::Human));
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
  messages.push(Message::user(content, MessageOrigin::Internal));
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::types::Role;

  #[test]
  fn compose_system_prompt_includes_context() {
    let sys = compose_system_prompt("## Write Scope\n- src/lib.rs");
    assert!(sys.contains("Core Contract"));
    assert!(sys.contains("## Context"));
    assert!(sys.contains("src/lib.rs"));
    assert!(sys.contains("# Status"));
  }

  #[test]
  fn build_initial_messages_keeps_human_task_last() {
    let messages = build_initial_messages("system", "do the task", "session-1");
    let last = messages.last().unwrap();
    assert_eq!(last.origin, MessageOrigin::Human);
    assert_eq!(last.content, "[session: session-1]\n\ndo the task");
  }

  #[test]
  fn push_internal_user_message_preserves_human_message() {
    let mut messages = build_messages("do the task");

    push_internal_user_message(&mut messages, "internal context".into());

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1].role, Role::User);
    assert_eq!(messages[1].origin, MessageOrigin::Human);
    assert_eq!(messages[1].content, "do the task");
    assert_eq!(messages[2].role, Role::User);
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
}
