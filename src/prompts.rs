use crate::types::{Message, MessageOrigin};

pub const PROMPT_SYSTEM: &str = include_str!("../PROMPT_SYSTEM.md");
pub const PROMPT_TOOLSET: &str = include_str!("../PROMPT_TOOLSET.md");
pub const PROMPT_COLGREP: &str = include_str!("../PROMPT_COLGREP.md");
pub const PROMPT_ROLE_GENERIC: &str = include_str!("../PROMPT_ROLE_GENERIC.md");

pub(crate) fn build_initial_messages(task_prompt: &str) -> Vec<Message> {
  let mut messages = vec![
    Message::system(PROMPT_SYSTEM.trim().to_string()),
    Message::user(PROMPT_TOOLSET.trim().to_string(), MessageOrigin::Internal),
    Message::user(PROMPT_COLGREP.trim().to_string(), MessageOrigin::Internal),
  ];
  messages.push(Message::user(task_prompt.trim(), MessageOrigin::Human));
  messages
}

pub(crate) fn load_subagent_role(workspace: &crate::workspace::Workspace, role: &str) -> String {
  let role_upper = role.to_uppercase().replace(" ", "_");
  let filename = format!("PROMPT_ROLE_{role_upper}.md");
  let ogent_path = workspace.workspace_path(&format!(".ogent/{filename}"));
  let root_path = workspace.workspace_path(&filename);
  let global_path = workspace.readable_path(&format!("~/.ogent/{filename}"));

  ogent_path
    .ok()
    .and_then(|p| std::fs::read_to_string(p).ok())
    .or_else(|| root_path.ok().and_then(|p| std::fs::read_to_string(p).ok()))
    .or_else(|| {
      global_path
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
    })
    .unwrap_or_else(|| PROMPT_ROLE_GENERIC.to_string())
}

pub(crate) fn build_subagent_messages(
  workspace: &crate::workspace::Workspace,
  role: &str,
  task: String,
) -> Vec<Message> {
  let role_prompt = load_subagent_role(workspace, role);
  vec![
    Message::system(PROMPT_SYSTEM.trim().to_string()),
    Message::user(role_prompt.trim().to_string(), MessageOrigin::Internal),
    Message::user(PROMPT_TOOLSET.trim().to_string(), MessageOrigin::Internal),
    Message::user(PROMPT_COLGREP.trim().to_string(), MessageOrigin::Internal),
    Message::user(task, MessageOrigin::Human),
  ]
}
