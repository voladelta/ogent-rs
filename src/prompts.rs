use crate::types::{Message, MessageOrigin};

pub const PROMPT_SYSTEM: &str = include_str!("../PROMPT_SYSTEM.md");
pub const PROMPT_TOOLSET: &str = include_str!("../PROMPT_TOOLSET.md");
pub const PROMPT_COLGREP: &str = include_str!("../PROMPT_COLGREP.md");

pub(crate) fn build_initial_messages(
  task_prompt: &str,
  _discovered_skills: &[crate::skills::SkillInfo],
  _loaded_skills: &[crate::skills::Skill],
) -> Vec<Message> {
  let mut messages = vec![
    Message::system(PROMPT_SYSTEM.trim().to_string()),
    Message::user(PROMPT_TOOLSET.trim().to_string(), MessageOrigin::Internal),
    Message::user(PROMPT_COLGREP.trim().to_string(), MessageOrigin::Internal),
  ];
  messages.push(Message::user(task_prompt.trim(), MessageOrigin::Human));
  messages
}

// Skill formatting functions removed as they are no longer used

fn push_internal_user_message(messages: &mut Vec<Message>, content: String) {
  if content.is_empty() {
    return;
  }
  messages.push(Message::user(content, MessageOrigin::Internal));
}

#[cfg(test)]
pub fn build_messages(prompt: &str) -> Vec<Message> {
  let mut messages = vec![
    Message::system(PROMPT_SYSTEM.to_string()),
    Message::user(PROMPT_TOOLSET.trim().to_string(), MessageOrigin::Internal),
    Message::user(PROMPT_COLGREP.trim().to_string(), MessageOrigin::Internal),
  ];
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
    let sys = PROMPT_SYSTEM;
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

    assert_eq!(messages.len(), 5);
    assert_eq!(messages[3].role, Role::User);
    assert_eq!(messages[3].origin, MessageOrigin::Human);
    assert_eq!(messages[3].content, "do the task");
    assert_eq!(messages[4].role, Role::User);
    assert_eq!(messages[4].origin, MessageOrigin::Internal);
    assert_eq!(messages[4].content, "internal context");
  }

  #[test]
  fn push_internal_user_message_does_not_merge_internal_messages() {
    let mut messages = build_messages("do the task");

    push_internal_user_message(&mut messages, "first".into());
    push_internal_user_message(&mut messages, "second".into());

    assert_eq!(messages.len(), 6);
    assert_eq!(messages[4].content, "first");
    assert_eq!(messages[4].origin, MessageOrigin::Internal);
    assert_eq!(messages[5].content, "second");
    assert_eq!(messages[5].origin, MessageOrigin::Internal);
  }

  #[test]
  fn push_internal_user_message_skips_empty_content() {
    let mut messages = build_messages("do the task");

    push_internal_user_message(&mut messages, String::new());

    assert_eq!(messages.len(), 4);
  }
}
