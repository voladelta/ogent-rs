use anyhow::{Context, Result, bail};

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

Leave `# Question` empty unless status is `question`."#;

pub async fn resolve_worker_prompts(
  role: &str,
  task: &str,
  context: &str,
) -> Result<(String, String)> {
  let requested_role = normalize_role(role);
  if let Some(builtin) = crate::prompts::get_builtin_worker_prompt(requested_role) {
    let context_section = format!("## Context\n\n{}", context.trim());
    let system_prompt = compose_worker_system_prompt(builtin, Some(&context_section));
    return Ok((system_prompt, task.trim().to_string()));
  }

  let client = get_architect_client()?;
  let user_content = format!(
    "## Desired Role\n\n{requested_role}\n\n## Hiring Request\n\n{}\n\n## Context\n\n{}",
    task.trim(),
    context.trim()
  );
  let messages = vec![
    crate::types::Message {
      role: "system".into(),
      content: crate::prompts::CONTRACTOR_FACTORY.to_string(),
      origin: crate::types::MessageOrigin::Internal,
      ..Default::default()
    },
    crate::types::Message {
      role: "user".into(),
      content: user_content,
      origin: crate::types::MessageOrigin::Human,
      ..Default::default()
    },
  ];
  let resp = client
    .chat_json(&messages, &[])
    .await
    .context("architect LLM call failed")?;
  let (system_prompt, task_prompt) = parse_architect_output(&resp.content)?;
  Ok((
    compose_worker_system_prompt(&system_prompt, None),
    task_prompt,
  ))
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

fn parse_architect_output(text: &str) -> Result<(String, String)> {
  let sys =
    extract_tag(text, "system_prompt").context("architect output missing <system_prompt> block")?;
  let task =
    extract_tag(text, "task_prompt").context("architect output missing <task_prompt> block")?;
  if sys.is_empty() {
    bail!("architect produced empty system_prompt");
  }
  if task.is_empty() {
    bail!("architect produced empty task_prompt");
  }
  Ok((sys, task))
}

fn extract_tag(text: &str, tag: &str) -> Option<String> {
  let start_tag = format!("<{tag}>");
  let end_tag = format!("</{tag}>");
  let start = text.find(&start_tag)? + start_tag.len();
  let end = text[start..].find(&end_tag)? + start;
  Some(text[start..end].trim().to_string())
}

pub(crate) fn build_worker_messages(
  system_prompt: &str,
  prompt: &str,
  session_id: &str,
) -> Vec<crate::types::Message> {
  let mut messages = vec![
    crate::types::Message {
      role: "system".into(),
      content: system_prompt.to_string(),
      origin: crate::types::MessageOrigin::Internal,
      ..Default::default()
    },
    crate::types::Message {
      role: "user".into(),
      content: format!("[session: {session_id}]\n\n{prompt}"),
      origin: crate::types::MessageOrigin::Human,
      ..Default::default()
    },
  ];
  crate::prompts::enrich_initial_messages(&mut messages);
  messages
}

static ARCHITECT_CLIENT: std::sync::OnceLock<Result<crate::client::Client, String>> =
  std::sync::OnceLock::new();

fn get_architect_client() -> Result<&'static crate::client::Client> {
  let result = ARCHITECT_CLIENT.get_or_init(|| {
    let config = crate::config::Config::default();
    let profile = config
      .get_profile("ds-flash")
      .ok_or_else(|| "architect profile 'ds-flash' not found".to_string())?;
    let provider = config
      .provider_for(profile)
      .ok_or_else(|| "missing provider config for architect profile 'ds-flash'".to_string())?;
    crate::providers::new_client(profile, provider).map_err(|e| e.to_string())
  });
  match result {
    Ok(client) => Ok(client),
    Err(e) => bail!("architect client init: {e}"),
  }
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
}
