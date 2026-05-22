pub fn resolve_worker_prompt(task: &str, context: &str) -> (String, String) {
  let context_section = format!("## Context\n\n{}", context.trim());
  let system_prompt =
    compose_worker_system_prompt(crate::prompts::SYSTEM_PROMPT, Some(&context_section));
  (system_prompt, task.trim().to_string())
}

fn compose_worker_system_prompt(base_prompt: &str, extra_section: Option<&str>) -> String {
  let mut sections = vec![base_prompt.trim().to_string()];
  if let Some(extra) = extra_section
    && !extra.trim().is_empty()
  {
    sections.push(extra.trim().to_string());
  }
  sections.join("\n\n")
}

pub(crate) fn build_worker_messages(
  system_prompt: &str,
  prompt: &str,
  session_id: &str,
) -> Vec<crate::types::Message> {
  let mut messages = vec![crate::types::Message {
    role: "system".into(),
    content: system_prompt.to_string(),
    origin: crate::types::MessageOrigin::Internal,
    ..Default::default()
  }];
  crate::prompts::enrich_initial_messages(&mut messages);
  messages.push(crate::types::Message {
    role: "user".into(),
    content: format!("[session: {session_id}]\n\n{prompt}"),
    origin: crate::types::MessageOrigin::Human,
    ..Default::default()
  });
  messages
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn resolve_worker_prompt_includes_context() {
    let (sys, task) = resolve_worker_prompt("edit src/lib.rs", "## Write Scope\n- src/lib.rs");
    assert!(sys.contains("Core Contract"));
    assert!(sys.contains("## Context"));
    assert!(sys.contains("src/lib.rs"));
    assert!(sys.contains("## Progress Reporting"));
    assert!(sys.contains("# Status"));
    assert_eq!(task, "edit src/lib.rs");
  }

  #[test]
  fn resolve_worker_prompt_uses_system_prompt() {
    let (sys, task) = resolve_worker_prompt("fix the bug", "");
    assert!(sys.contains("Core Contract"));
    assert_eq!(task, "fix the bug");
  }

  #[test]
  fn build_worker_messages_keeps_human_task_last() {
    let messages = build_worker_messages("system", "do the task", "session-1");
    let last = messages.last().unwrap();
    assert_eq!(last.origin, crate::types::MessageOrigin::Human);
    assert_eq!(last.content, "[session: session-1]\n\ndo the task");
  }
}
