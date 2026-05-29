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
