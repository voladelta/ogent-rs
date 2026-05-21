use anyhow::Result;

const WORKER_PROGRESS_PROMPT_SUFFIX: &str = r#"## Integrity and Failure Reporting

Honest progress beats fake success.

Valid outcomes are:
- `completed`: the contract is satisfied and supported by evidence.
- `partial`: useful progress was made, but a specific remaining gap exists.
- `blocked`: no clean path is available under the current constraints.
- `question`: the task cannot continue without one specific answer.

Do not convert uncertainty into completion. If the task cannot be completed cleanly, stop, state the blocker, show the evidence you have, and say what would be needed next.

Invalid success paths:
- claiming a command passed unless you ran it and saw the result
- editing tests, fixtures, prompts, or expected outputs to hide broken behavior unless explicitly asked
- hardcoding known examples instead of solving the intended case
- suppressing errors, hiding logs, or omitting relevant failures
- weakening acceptance criteria or silently changing the contract
- using a workaround instead of a root-cause fix while reporting completion

Verification is evidence, not decoration. Report commands, checks, source files, artifacts, or reasoning actually used. If verification was not run, say so and explain why.

## Progress Reporting

When your task requires more than one tool call, write concise current progress before each tool call using the `state` tool:
- `action`: `write`
- `path`: `progress/current`
- `content`: short factual status

Update this value when the phase changes. Keep it brief and factual. Examples: "reading parser", "defining trait", "refactoring call sites", "running tests". Skip this for trivial one-shot answers.

## Result Reporting

Your final response must use these Markdown sections exactly:

```md
# Status

completed | partial | blocked | question

# Summary

# Changed Files

# Verification

# Evidence

# Risks

# Question

# Next Action
```

Leave `# Question` empty unless status is `question`.

Do not add other top-level Markdown headings in the final response. Put role-specific content under the required sections."#;

pub async fn resolve_worker_prompts(
  role: &str,
  task: &str,
  context: &str,
) -> Result<(String, String)> {
  let requested_role = normalize_role(role);
  let builtin = crate::prompts::get_builtin_worker_prompt(requested_role)
    .ok_or_else(|| anyhow::anyhow!("unknown worker role: {requested_role}"))?;
  let context_section = format!("## Context\n\n{}", context.trim());
  let system_prompt = compose_worker_system_prompt(builtin, Some(&context_section));
  Ok((system_prompt, task.trim().to_string()))
}

fn compose_worker_system_prompt(base_prompt: &str, extra_section: Option<&str>) -> String {
  let mut sections = vec![base_prompt.trim().to_string()];
  if let Some(extra) = extra_section
    && !extra.trim().is_empty()
  {
    sections.push(extra.trim().to_string());
  }
  sections.push(WORKER_PROGRESS_PROMPT_SUFFIX.to_string());
  sections.join("\n\n")
}

fn normalize_role(role: &str) -> &str {
  let role = role.trim();
  if role.is_empty() { "ogent" } else { role }
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

  #[tokio::test]
  async fn resolve_worker_prompts_uses_implementer_builtin() {
    let (sys, task) = resolve_worker_prompts(
      "implementer",
      "edit src/lib.rs",
      "## Write Scope\n- src/lib.rs",
    )
    .await
    .unwrap();
    assert!(sys.contains("produce the requested artifact or code change"));
    assert!(sys.contains("## Context"));
    assert!(sys.contains("src/lib.rs"));
    assert!(sys.contains("## Progress Reporting"));
    assert!(sys.contains("# Status"));
    assert_eq!(task, "edit src/lib.rs");
  }

  #[tokio::test]
  async fn empty_role_uses_ogent_builtin() {
    let (sys, task) = resolve_worker_prompts("", "fix the bug", "").await.unwrap();
    assert!(sys.contains("Core Contract"));
    assert_eq!(task, "fix the bug");
  }

  #[tokio::test]
  async fn resolve_worker_prompts_uses_reviewer_builtin() {
    let (sys, task) =
      resolve_worker_prompts("reviewer", "review src/lib.rs", "## Files\n- src/lib.rs")
        .await
        .unwrap();
    assert!(sys.contains("judge whether work satisfies the contract"));
    assert!(sys.contains("## Context"));
    assert!(sys.contains("src/lib.rs"));
    assert_eq!(task, "review src/lib.rs");
  }

  #[test]
  fn build_worker_messages_keeps_human_task_last() {
    let messages = build_worker_messages("system", "do the task", "session-1");
    let last = messages.last().unwrap();
    assert_eq!(last.origin, crate::types::MessageOrigin::Human);
    assert_eq!(last.content, "[session: session-1]\n\ndo the task");
  }

  #[tokio::test]
  async fn resolve_worker_prompts_errors_on_unknown_role() {
    let err = resolve_worker_prompts("unknown_role_xyz", "do something", "")
      .await
      .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("unknown worker role"), "error was: {msg}");
    assert!(msg.contains("unknown_role_xyz"), "error was: {msg}");
  }
}
