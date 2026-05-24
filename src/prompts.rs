use crate::types::{Message, MessageOrigin};

pub const SYSTEM_PROMPT: &str = include_str!("../SYSTEM_PROMPT.md");

pub(crate) fn build_initial_messages(
  task_prompt: &str,
  discovered_skills: &[crate::skills::SkillInfo],
  loaded_skills: &[crate::skills::Skill],
) -> Vec<Message> {
  let mut messages = vec![Message::system(SYSTEM_PROMPT.trim().to_string())];
  enrich_initial_messages(&mut messages, discovered_skills, loaded_skills);
  messages.push(Message::user(task_prompt.trim(), MessageOrigin::Human));
  messages
}

pub fn format_discover_skills(skills: &[crate::skills::SkillInfo]) -> String {
  if skills.is_empty() {
    return String::new();
  }
  let mut out = String::from("<skills>\n");
  for skill in skills {
    out.push_str("  <skill name=\"");
    out.push_str(&crate::util::xml_escape(&skill.name));
    out.push_str("\" description=\"");
    out.push_str(&crate::util::xml_escape(&skill.description));
    out.push_str("\" />\n");
  }
  out.push_str("</skills>");
  out
}

pub fn enrich_initial_messages(
  messages: &mut Vec<Message>,
  discovered_skills: &[crate::skills::SkillInfo],
  loaded_skills: &[crate::skills::Skill],
) {
  push_internal_user_message(messages, format_discover_skills(discovered_skills));
  for skill in loaded_skills {
    push_internal_user_message(messages, crate::skills::format_loaded_skill(skill));
  }
}

fn push_internal_user_message(messages: &mut Vec<Message>, content: String) {
  if content.is_empty() {
    return;
  }
  messages.push(Message::user(content, MessageOrigin::Internal));
}

#[cfg(test)]
pub fn build_messages(prompt: &str) -> Vec<Message> {
  let mut messages = vec![Message::system(SYSTEM_PROMPT.to_string())];
  if !prompt.is_empty() {
    messages.push(Message::user(prompt.to_string(), MessageOrigin::Human));
  }
  messages
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::types::Role;

  #[test]
  fn test_compose_system_prompt() {
    let sys = SYSTEM_PROMPT;
    assert!(sys.contains("Core Contract"));
  }

  #[test]
  fn build_initial_messages_keeps_human_task_last() {
    let messages = build_initial_messages("do the task", &[], &[]);
    let last = messages.last().unwrap();
    assert_eq!(last.origin, MessageOrigin::Human);
    assert_eq!(last.content, "do the task");
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
