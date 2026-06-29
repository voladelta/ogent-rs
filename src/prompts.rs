use crate::types::{Message, MessageOrigin};

pub const PROMPT_SYSTEM: &str = include_str!("../PROMPT_SYSTEM.md");
pub const PROMPT_TOOLSET_CORE: &str = include_str!("../PROMPT_TOOLSET_CORE.md");
pub const PROMPT_TOOLSET_WRITE: &str = include_str!("../PROMPT_TOOLSET_WRITE.md");
pub const PROMPT_TOOLSET_GIT: &str = include_str!("../PROMPT_TOOLSET_GIT.md");
pub const PROMPT_TOOLSET_SUBAGENT: &str = include_str!("../PROMPT_TOOLSET_SUBAGENT.md");
pub const PROMPT_COLGREP: &str = include_str!("../PROMPT_COLGREP.md");
pub const PROMPT_ROLE_GENERIC: &str = include_str!("../PROMPT_ROLE_GENERIC.md");

fn toolset_messages() -> Vec<Message> {
  [PROMPT_TOOLSET_CORE]
    .into_iter()
    .map(|prompt| Message::user(prompt.trim().to_string(), MessageOrigin::Internal))
    .collect()
}

pub(crate) fn build_initial_messages(task_prompt: &str) -> Vec<Message> {
  let mut messages = vec![Message::system(PROMPT_SYSTEM.trim().to_string())];
  messages.extend(toolset_messages());
  messages.push(Message::user(
    PROMPT_COLGREP.trim().to_string(),
    MessageOrigin::Internal,
  ));
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
  let mut messages = vec![
    Message::system(PROMPT_SYSTEM.trim().to_string()),
    Message::user(role_prompt.trim().to_string(), MessageOrigin::Internal),
  ];
  messages.extend(toolset_messages());
  messages.push(Message::user(
    PROMPT_COLGREP.trim().to_string(),
    MessageOrigin::Internal,
  ));
  messages.push(Message::user(task, MessageOrigin::Human));
  messages
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn initial_messages_use_core_toolset_not_full_reference() {
    let messages = build_initial_messages("do the task");
    let contents: Vec<_> = messages
      .iter()
      .map(|message| message.content.as_str())
      .collect();

    assert!(
      contents
        .iter()
        .any(|content| content.contains("# Lua Toolset Core"))
    );
    assert!(
      !contents
        .iter()
        .any(|content| content.contains("# Lua Toolset Git"))
    );
    assert!(
      !contents
        .iter()
        .any(|content| content.contains("# Lua Toolset Write"))
    );
    assert!(
      !contents
        .iter()
        .any(|content| content.contains("# Lua Toolset Subagent"))
    );
    assert!(
      !contents
        .iter()
        .any(|content| content.contains("# Lua Toolset Guide"))
    );
  }

  #[test]
  fn subagent_messages_include_role_then_core_toolset() {
    let workspace = crate::workspace::Workspace::from_root(std::env::temp_dir());
    let messages = build_subagent_messages(&workspace, "subagent", "do the task".to_string());

    assert!(
      messages[1]
        .content
        .contains("You are a developer acting as")
    );
    assert!(messages[2].content.contains("# Lua Toolset Core"));
    assert!(messages[3].content.contains("Default search policy"));
  }
}
