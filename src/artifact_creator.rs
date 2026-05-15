use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

use crate::client::Client;
use crate::types::{Message, MessageOrigin};

const SKILL_CREATOR_PROMPT: &str = include_str!("../prompts/SKILL_CREATOR_PROMPT.md");
const WORKFLOW_CREATOR_PROMPT: &str = include_str!("../prompts/WORKFLOW_CREATOR_PROMPT.md");

const MAX_ATTEMPTS: usize = 2;

pub struct ArtifactResult {
  pub path: PathBuf,
  pub action: ArtifactAction,
}

#[derive(Clone, Copy)]
pub enum ArtifactAction {
  Created,
  Updated,
}

pub async fn create_skill(
  client: &Client,
  raw_name: &str,
  objective: &str,
) -> Result<ArtifactResult> {
  let name = normalize_artifact_name(raw_name)?;
  require_objective(objective)?;
  let path = PathBuf::from(".ogent")
    .join("skills")
    .join(&name)
    .join("SKILL.md");
  let existing = read_existing_artifact(&path)?;
  let action = action_for_existing(existing.as_deref());

  let prompt = skill_user_prompt(&name, objective, existing.as_deref());
  let content = generate_validated(client, SKILL_CREATOR_PROMPT, &prompt, |content| {
    validate_skill(&name, content)
  })
  .await?;
  write_artifact(&path, &content)?;
  Ok(ArtifactResult { path, action })
}

pub async fn create_workflow(
  client: &Client,
  raw_name: &str,
  objective: &str,
) -> Result<ArtifactResult> {
  let name = normalize_artifact_name(raw_name)?;
  require_objective(objective)?;
  let path = PathBuf::from(".ogent")
    .join("workflows")
    .join(format!("{name}.yaml"));
  let existing = read_existing_artifact(&path)?;
  let action = action_for_existing(existing.as_deref());

  let prompt = workflow_user_prompt(&name, objective, existing.as_deref());
  let content = generate_validated(client, WORKFLOW_CREATOR_PROMPT, &prompt, |content| {
    validate_workflow(&name, content)
  })
  .await?;
  write_artifact(&path, &content)?;
  Ok(ArtifactResult { path, action })
}

async fn generate_validated<F>(
  client: &Client,
  system_prompt: &str,
  user_prompt: &str,
  validate: F,
) -> Result<String>
where
  F: Fn(&str) -> Result<String>,
{
  let mut prompt = user_prompt.to_string();
  let mut last_error = None;

  for attempt in 1..=MAX_ATTEMPTS {
    let messages = vec![
      Message {
        role: "system".into(),
        content: system_prompt.trim().to_string(),
        origin: MessageOrigin::Internal,
        ..Default::default()
      },
      Message {
        role: "user".into(),
        content: prompt.clone(),
        origin: MessageOrigin::Human,
        ..Default::default()
      },
    ];
    let resp = client.chat_json(&messages, &[]).await?;
    match validate(&resp.content) {
      Ok(content) => return Ok(content),
      Err(err) if attempt < MAX_ATTEMPTS => {
        let err_text = err.to_string();
        prompt = format!(
          "{user_prompt}\n\nThe previous output failed validation:\n{err_text}\n\nReturn a corrected artifact only."
        );
        last_error = Some(err);
      }
      Err(err) => last_error = Some(err),
    }
  }

  Err(last_error.expect("at least one validation attempt"))
}

fn skill_user_prompt(name: &str, objective: &str, existing: Option<&str>) -> String {
  let mut prompt = format!(
    r#"## Requested Artifact

name: {name}
objective: {objective}

## Skill Authoring Notes

An ogent skill is a directory containing `SKILL.md`. The runtime discovers local skills by reading `SKILL.md` frontmatter and shows the name and description to the agent. The full body is loaded later when the agent needs the capability.

Good skills are narrow, reusable, and procedural. Put the activation criteria in `description`. Put execution steps in the body. Keep reference material compact. Prefer checklists and concrete commands over broad advice.

## Minimal Shape

---
name: {name}
description: Use when ...
---

# {name}

## When To Use

...

## Procedure

...
"#
  );
  append_existing_artifact(&mut prompt, existing, "SKILL.md");
  prompt
}

fn workflow_user_prompt(name: &str, objective: &str, existing: Option<&str>) -> String {
  let mut prompt = format!(
    r#"## Requested Artifact

id: {name}
objective: {objective}

## Workflow Schema

Workflow:
- id: string
- name: string
- version: integer, default 1
- start: step id
- instructions: optional string
- steps: map of step id to step

WorkflowStep:
- title: optional string
- instructions: optional string
- next: list of step ids
- terminal: boolean
- gate: boolean
- max_visits: optional positive integer
- checks: list of checks

WorkflowCheck:
- id: string
- type: manual or command
- required: boolean
- command: optional string, required for command checks

Validation requires a non-empty id, name, start, at least one step, at least one terminal step, all next references to exist, all non-terminal steps to have next, unique check ids per step, and all steps reachable from start.

## Example

id: example-loop
name: Example Loop
version: 1
start: frame
instructions: |
  Use this workflow when the task needs a small evidence-backed loop.
steps:
  frame:
    title: Frame
    instructions: |
      Define the objective, constraints, and verification path.
    next: [execute]
    checks:
      - id: objective
        type: manual
        required: true
  execute:
    title: Execute
    instructions: |
      Do one narrow unit of work.
    next: [verify]
    checks:
      - id: work_done
        type: manual
        required: true
  verify:
    title: Verify
    instructions: |
      Run or record the relevant verification.
    next: [execute, done]
    gate: true
    max_visits: 5
    checks:
      - id: evidence
        type: manual
        required: true
  done:
    title: Done
    terminal: true
"#
  );
  append_existing_artifact(&mut prompt, existing, "workflow YAML");
  prompt
}

fn validate_skill(expected_name: &str, raw: &str) -> Result<String> {
  let content = strip_optional_code_fence(raw, "markdown");
  let trimmed = content.trim();
  let frontmatter = parse_frontmatter(trimmed)?;
  let yaml: serde_yaml::Value =
    serde_yaml::from_str(frontmatter).context("invalid skill frontmatter YAML")?;

  let name = yaml
    .get("name")
    .and_then(serde_yaml::Value::as_str)
    .unwrap_or_default();
  if name != expected_name {
    bail!("skill frontmatter name must be '{expected_name}', got '{name}'");
  }
  let description = yaml
    .get("description")
    .and_then(serde_yaml::Value::as_str)
    .unwrap_or_default()
    .trim();
  if description.is_empty() {
    bail!("skill frontmatter description is required");
  }
  if body_after_frontmatter(trimmed)?.is_empty() {
    bail!("skill body is required");
  }
  Ok(ensure_trailing_newline(trimmed))
}

fn validate_workflow(expected_name: &str, raw: &str) -> Result<String> {
  let content = strip_optional_code_fence(raw, "yaml");
  let trimmed = content.trim();
  let workflow: crate::workflow::Workflow =
    serde_yaml::from_str(trimmed).context("invalid workflow YAML")?;
  workflow.validate().context("invalid workflow definition")?;
  if workflow.id != expected_name {
    bail!(
      "workflow id must be '{expected_name}', got '{}'",
      workflow.id
    );
  }
  Ok(ensure_trailing_newline(trimmed))
}

fn parse_frontmatter(content: &str) -> Result<&str> {
  let Some(rest) = content.strip_prefix("---") else {
    bail!("skill must start with YAML frontmatter");
  };
  let Some(end) = rest.find("---") else {
    bail!("skill frontmatter is not closed");
  };
  Ok(&rest[..end])
}

fn body_after_frontmatter(content: &str) -> Result<&str> {
  let Some(rest) = content.strip_prefix("---") else {
    bail!("skill must start with YAML frontmatter");
  };
  let Some(end) = rest.find("---") else {
    bail!("skill frontmatter is not closed");
  };
  Ok(rest[end + 3..].trim())
}

fn strip_optional_code_fence(raw: &str, expected_lang: &str) -> String {
  let trimmed = raw.trim();
  if !trimmed.starts_with("```") || !trimmed.ends_with("```") {
    return trimmed.to_string();
  }

  let mut lines = trimmed.lines();
  let first = lines.next().unwrap_or_default().trim();
  let lang = first.trim_start_matches("```").trim();
  if !lang.is_empty() && lang != expected_lang && lang != "md" && lang != "yml" {
    return trimmed.to_string();
  }

  let mut body: Vec<&str> = lines.collect();
  if body.last().is_some_and(|line| line.trim() == "```") {
    body.pop();
    body.join("\n")
  } else {
    trimmed.to_string()
  }
}

fn normalize_artifact_name(raw: &str) -> Result<String> {
  let mut out = String::new();
  let mut previous_dash = false;
  for c in raw.trim().chars() {
    if c.is_ascii_alphanumeric() {
      out.push(c.to_ascii_lowercase());
      previous_dash = false;
    } else if matches!(c, '-' | '_' | ' ' | '.') && !previous_dash && !out.is_empty() {
      out.push('-');
      previous_dash = true;
    }
  }
  while out.ends_with('-') {
    out.pop();
  }
  if out.is_empty() {
    bail!("artifact name must contain at least one ASCII letter or digit");
  }
  Ok(out)
}

fn require_objective(objective: &str) -> Result<()> {
  if objective.trim().is_empty() {
    bail!("description/objective is required");
  }
  Ok(())
}

fn read_existing_artifact(path: &Path) -> Result<Option<String>> {
  if !path.exists() {
    return Ok(None);
  }
  fs::read_to_string(path)
    .map(Some)
    .with_context(|| format!("read existing artifact {}", path.display()))
}

fn action_for_existing(existing: Option<&str>) -> ArtifactAction {
  if existing.is_some() {
    ArtifactAction::Updated
  } else {
    ArtifactAction::Created
  }
}

fn write_artifact(path: &Path, content: &str) -> Result<()> {
  let parent = path.parent().context("artifact path has no parent")?;
  fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
  fs::write(path, content).with_context(|| format!("write {}", path.display()))
}

fn append_existing_artifact(prompt: &mut String, existing: Option<&str>, label: &str) {
  let Some(existing) = existing else {
    return;
  };
  prompt.push_str("\n## Existing Artifact To Improve\n\n");
  prompt.push_str("Improve this existing ");
  prompt.push_str(label);
  prompt.push_str(". Treat it as an editable draft, not authoritative truth. Preserve the same artifact name/id, keep useful existing intent, remove content that conflicts with the objective, remove obsolete or weak guidance, and return the complete improved artifact.\n\n```");
  prompt.push_str(if label.contains("YAML") {
    "yaml"
  } else {
    "markdown"
  });
  prompt.push('\n');
  prompt.push_str(existing.trim());
  prompt.push_str("\n```\n");
}

fn ensure_trailing_newline(content: &str) -> String {
  if content.ends_with('\n') {
    content.to_string()
  } else {
    format!("{content}\n")
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn normalizes_artifact_names() {
    assert_eq!(
      normalize_artifact_name("My Skill.v1").unwrap(),
      "my-skill-v1"
    );
    assert!(normalize_artifact_name("../").is_err());
  }

  #[test]
  fn validates_skill_frontmatter_and_body() {
    let content = r#"---
name: repo-audit
description: Use when auditing a repository.
---

# Repo Audit

Review the repository.
"#;
    assert!(validate_skill("repo-audit", content).is_ok());
    assert!(validate_skill("other", content).is_err());
  }

  #[test]
  fn validates_workflow_yaml() {
    let content = r#"id: tiny-flow
name: Tiny Flow
version: 1
start: start
steps:
  start:
    next: [done]
  done:
    terminal: true
"#;
    assert!(validate_workflow("tiny-flow", content).is_ok());
    assert!(validate_workflow("other", content).is_err());
  }

  #[test]
  fn prompts_include_existing_artifact_when_present() {
    let skill = skill_user_prompt("repo-audit", "improve it", Some("old skill"));
    assert!(skill.contains("Existing Artifact To Improve"));
    assert!(skill.contains("old skill"));

    let workflow = workflow_user_prompt("release-gate", "improve it", Some("old workflow"));
    assert!(workflow.contains("Existing Artifact To Improve"));
    assert!(workflow.contains("```yaml"));
    assert!(workflow.contains("old workflow"));
  }
}
