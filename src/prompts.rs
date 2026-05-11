use anyhow::{Result, bail};
use std::fmt::Write;
use std::fs;
use std::path::PathBuf;

use crate::types::Message;

pub const TENX_CODER_SYSTEM_PROMPT: &str = include_str!("../prompts/SYSTEM_PROMPT.md");

pub const WORKER_SUMMARY_PROMPT: &str = "\n\n## Worker Report Protocol\n\nWhen done, call `worker_complete` with a concise Markdown summary:\n\n```json\n{\"summary\":\"...\"}\n```\n\nInclude in the summary:\n- What you accomplished\n- Files inspected, commands run, results\n- Decisions made\n- Files modified (list)\n- Blockers (omit if none)\n\nRules:\n- Concise fragments are preferred.\n- Never fabricate or embellish results. Report only what you actually observed or did.\n- Do not write intermediate analysis, planning, or decision documents to the repo.";

pub const WORKER_TEMPLATE_GENERIC: &str = include_str!("../prompts/templates/generic.md");
pub const WORKER_TEMPLATE_TESTER: &str = include_str!("../prompts/templates/tester.md");
pub const WORKER_TEMPLATE_REVIEWER: &str = include_str!("../prompts/templates/reviewer.md");
pub const WORKER_TEMPLATE_VALIDATOR: &str = include_str!("../prompts/templates/validator.md");

pub fn get_worker_template(name: &str) -> Option<&'static str> {
  match name {
    "generic" => Some(WORKER_TEMPLATE_GENERIC),
    "tester" => Some(WORKER_TEMPLATE_TESTER),
    "reviewer" => Some(WORKER_TEMPLATE_REVIEWER),
    "validator" => Some(WORKER_TEMPLATE_VALIDATOR),
    _ => None,
  }
}

pub fn skill_roots() -> Vec<PathBuf> {
  let mut dirs = vec![PathBuf::from(".ogent/skills"), PathBuf::from(".skills")];
  if let Some(home) = std::env::var_os("HOME") {
    dirs.push(PathBuf::from(home).join(".ogent/skills"));
  }
  dirs
}

pub fn load_skill_content(
  skill_name: &str,
) -> Result<(String, String, String, Option<crate::workflow::Workflow>)> {
  for dir in skill_roots() {
    let root = dir.join(skill_name);
    let path = root.join("SKILL.md");
    let Ok(content) = fs::read_to_string(&path) else {
      continue;
    };
    let (name, _, workflow) = parse_skill_frontmatter(&content);
    return Ok((
      if name.is_empty() {
        skill_name.to_string()
      } else {
        name
      },
      root.display().to_string(),
      strip_frontmatter(&content),
      workflow,
    ));
  }
  bail!("skill {skill_name} not found in local .ogent/skills, .skills, or ~/.ogent/skills")
}

pub fn discover_skills_message() -> String {
  let mut seen = std::collections::HashSet::new();
  let mut out = String::from("<skills>\n");
  for root in skill_roots() {
    let Ok(entries) = fs::read_dir(root) else {
      continue;
    };
    for entry in entries.flatten() {
      if !entry.file_type().is_ok_and(|t| t.is_dir()) {
        continue;
      }
      let Ok(content) = fs::read_to_string(entry.path().join("SKILL.md")) else {
        continue;
      };
      let (name, desc, _) = parse_skill_frontmatter(&content);
      if name.is_empty() || !seen.insert(name.clone()) {
        continue;
      }
      let _ = writeln!(
        out,
        "  <skill name=\"{}\" description=\"{}\" />",
        xml_escape(&name),
        xml_escape(&desc)
      );
    }
  }
  if seen.is_empty() {
    String::new()
  } else {
    out.push_str("</skills>");
    out
  }
}

fn parse_frontmatter(content: &str) -> Option<&str> {
  content
    .strip_prefix("---")
    .and_then(|rest| rest.find("---").map(|end| &rest[..end]))
}

fn strip_frontmatter(content: &str) -> String {
  parse_frontmatter(content).map_or_else(
    || content.trim().to_string(),
    |_| {
      let end = content[3..].find("---").unwrap() + 6;
      content[end..].trim().to_string()
    },
  )
}

fn parse_skill_frontmatter(content: &str) -> (String, String, Option<crate::workflow::Workflow>) {
  let Some(fm) = parse_frontmatter(content) else {
    return (String::new(), String::new(), None);
  };
  let mut name = String::new();
  let mut description = String::new();
  let mut workflow = None;

  if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(fm) {
    if let Some(w) = value.get("workflow") {
      workflow = serde_yaml::from_value(w.clone()).ok();
    }
    if let Some(n) = value.get("name").and_then(|v| v.as_str()) {
      name = n.to_string();
    }
    if let Some(d) = value.get("description").and_then(|v| v.as_str()) {
      description = d.to_string();
    }
  } else {
    for line in fm.lines().map(str::trim) {
      if let Some(rest) = line.strip_prefix("name:") {
        name = rest.trim().to_string();
      } else if let Some(rest) = line.strip_prefix("description:") {
        description = rest.trim().to_string();
      }
    }
  }
  (name, description, workflow)
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

pub fn build_messages(prompt: &str) -> Vec<Message> {
  vec![
    Message {
      role: "system".into(),
      content: TENX_CODER_SYSTEM_PROMPT.to_string(),
      ..Default::default()
    },
    Message {
      role: "user".into(),
      content: prompt.to_string(),
      ..Default::default()
    },
  ]
}

pub fn enrich_initial_messages(messages: &mut [Message]) -> Option<crate::workflow::WorkflowState> {
  let mut workflow_state = None;
  append_to_last_user_message(messages, &discover_skills_message());
  if let Ok((name, root, body, workflow)) = load_skill_content("colgrep") {
    append_to_last_user_message(
      messages,
      &format!("<skill name=\"{name}\" root=\"{root}\">\n{body}\n</skill>"),
    );
    if let Some(wf) = workflow {
      workflow_state = Some(crate::workflow::WorkflowState::new(wf));
    }
  }
  if let Ok((name, root, body, workflow)) = load_skill_content("codectx") {
    append_to_last_user_message(
      messages,
      &format!("<skill name=\"{name}\" root=\"{root}\">\n{body}\n</skill>"),
    );
    if let Some(wf) = workflow {
      workflow_state = Some(crate::workflow::WorkflowState::new(wf));
    }
  }

  workflow_state
}

fn append_to_last_user_message(messages: &mut [Message], content: &str) {
  if content.is_empty() {
    return;
  }
  if let Some(message) = messages.iter_mut().rev().find(|m| m.role == "user") {
    if message.content.is_empty() {
      message.content = content.to_string();
    } else {
      message.content.push_str("\n\n");
      message.content.push_str(content);
    }
  }
}
