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
  discovered_skills: &[crate::skills::SkillInfo],
  loaded_skills: &[crate::skills::Skill],
) -> Vec<Message> {
  let mut messages = vec![Message::system(system_prompt.to_string())];
  enrich_initial_messages(&mut messages, discovered_skills, loaded_skills);
  messages.push(Message::user(
    format!("[session: {session_id}]\n\n{}", task_prompt.trim()),
    MessageOrigin::Human,
  ));
  messages
}

pub fn format_discover_skills(skills: &[crate::skills::SkillInfo]) -> String {
  if skills.is_empty() {
    return String::new();
  }
  let mut out = String::from("<skills>\n");
  for skill in skills {
    out.push_str("  <skill name=\"");
    out.push_str(&xml_escape(&skill.name));
    out.push_str("\" description=\"");
    out.push_str(&xml_escape(&skill.description));
    out.push_str("\" />\n");
  }
  out.push_str("</skills>");
  out
}

pub fn format_loaded_skill(skill: &crate::skills::Skill) -> String {
  format!(
    "<skill name=\"{}\" root=\"{}\">\n{}\n</skill>",
    skill.name,
    skill.root.display(),
    skill.body
  )
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

pub fn enrich_initial_messages(
  messages: &mut Vec<Message>,
  discovered_skills: &[crate::skills::SkillInfo],
  loaded_skills: &[crate::skills::Skill],
) {
  push_internal_user_message(messages, format_discover_skills(discovered_skills));
  for skill in loaded_skills {
    push_internal_user_message(messages, format_loaded_skill(skill));
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
    let messages = build_initial_messages("system", "do the task", "session-1", &[], &[]);
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
