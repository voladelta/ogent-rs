use anyhow::{Result, bail};
use std::fmt::Write;
use std::fs;
use std::path::PathBuf;

use crate::types::{Message, MessageOrigin};

pub const SYSTEM_PROMPT: &str = include_str!("../prompts/SYSTEM_PROMPT.md");

pub const WORKER_SUMMARY_PROMPT: &str = "\n\n## Worker Report Protocol\n\nWhen done, call `worker_complete` with a concise Markdown summary:\n\n```json\n{\"summary\":\"...\"}\n```\n\nInclude in the summary:\n- What you accomplished\n- Files inspected, commands run, results\n- Decisions made\n- Files modified (list)\n- Blockers (omit if none)\n\nRules:\n- Concise fragments are preferred.\n- Never fabricate or embellish results. Report only what you actually observed or did.\n- Do not write intermediate analysis, planning, or decision documents to the repo.";

pub const WORKER_TEMPLATE_GENERIC: &str = include_str!("../prompts/workers/generic.md");
pub const WORKER_PROMPT_CODER: &str = include_str!("../prompts/workers/coder.md");
pub const WORKER_PROMPT_REVIEWER: &str = include_str!("../prompts/workers/reviewer.md");
pub const WORKER_PROMPT_TESTER: &str = include_str!("../prompts/workers/tester.md");
pub const WORKER_PROMPT_VALIDATOR: &str = include_str!("../prompts/workers/validator.md");
pub const ARCHITECT_PROMPT: &str = include_str!("../prompts/ARCHITECT_PROMPT.md");

pub fn get_worker_template(_name: &str) -> &'static str {
  WORKER_TEMPLATE_GENERIC
}

pub fn get_builtin_worker_prompt(name: &str) -> Option<&'static str> {
  match name {
    "coder" => Some(WORKER_PROMPT_CODER),
    "reviewer" => Some(WORKER_PROMPT_REVIEWER),
    "tester" => Some(WORKER_PROMPT_TESTER),
    "validator" => Some(WORKER_PROMPT_VALIDATOR),
    _ => None,
  }
}

pub fn skill_roots() -> Vec<PathBuf> {
  let mut dirs = vec![PathBuf::from(".ogent/skills")];
  if let Some(home) = std::env::var_os("HOME") {
    dirs.push(PathBuf::from(home).join(".ogent/skills"));
  }
  dirs
}

fn system_prompt_paths() -> Vec<PathBuf> {
  let mut paths = vec![PathBuf::from(".ogent/SYSTEM_PROMPT.md")];
  if let Some(home) = std::env::var_os("HOME") {
    paths.push(PathBuf::from(home).join(".ogent/SYSTEM_PROMPT.md"));
  }
  paths
}

fn load_system_prompt() -> String {
  for path in system_prompt_paths() {
    if let Ok(content) = fs::read_to_string(path) {
      let trimmed = content.trim();
      if !trimmed.is_empty() {
        return trimmed.to_string();
      }
    }
  }
  SYSTEM_PROMPT.trim().to_string()
}

pub fn load_skill_content(skill_name: &str) -> Result<(String, String, String)> {
  for dir in skill_roots() {
    let root = dir.join(skill_name);
    let path = root.join("SKILL.md");
    let Ok(content) = fs::read_to_string(&path) else {
      continue;
    };
    let (name, _) = parse_skill_frontmatter(&content);
    return Ok((
      if name.is_empty() {
        skill_name.to_string()
      } else {
        name
      },
      root.display().to_string(),
      strip_frontmatter(&content),
    ));
  }
  bail!("skill {skill_name} not found in local .ogent/skills or ~/.ogent/skills")
}

pub fn discover_skill_names() -> Vec<(String, String)> {
  let mut seen = std::collections::HashSet::new();
  let mut skills = Vec::new();
  for root in skill_roots() {
    let Ok(entries) = fs::read_dir(&root) else {
      continue;
    };
    for entry in entries.flatten() {
      let dir_name = entry.file_name().to_string_lossy().to_string();
      if !entry.file_type().is_ok_and(|t| t.is_dir()) {
        continue;
      }
      let Ok(content) = fs::read_to_string(entry.path().join("SKILL.md")) else {
        continue;
      };
      let (name, desc) = parse_skill_frontmatter(&content);
      let key = if name.is_empty() {
        dir_name.clone()
      } else {
        name.clone()
      };
      if !seen.insert(key.clone()) {
        continue;
      }
      skills.push((key, desc));
    }
  }
  skills.sort_by(|a, b| a.0.cmp(&b.0));
  skills
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
      let (name, desc) = parse_skill_frontmatter(&content);
      let dir_name = entry.file_name().to_string_lossy().to_string();
      let key = if name.is_empty() { dir_name } else { name };
      if !seen.insert(key.clone()) {
        continue;
      }
      writeln!(
        out,
        "  <skill name=\"{}\" description=\"{}\" />",
        xml_escape(&key),
        xml_escape(&desc)
      )
      .unwrap();
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
  let Some(fm) = parse_frontmatter(content) else {
    return content.trim().to_string();
  };
  let start = 3 + fm.len() + 3;
  content[start..].trim().to_string()
}

fn parse_skill_frontmatter(content: &str) -> (String, String) {
  let Some(fm) = parse_frontmatter(content) else {
    return (String::new(), String::new());
  };
  let mut name = String::new();
  let mut description = String::new();

  if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(fm) {
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
  (name, description)
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
      content: load_system_prompt(),
      origin: MessageOrigin::Internal,
      ..Default::default()
    },
    Message {
      role: "user".into(),
      content: prompt.to_string(),
      origin: MessageOrigin::Human,
      ..Default::default()
    },
  ]
}

pub fn enrich_initial_messages(messages: &mut [Message]) {
  append_to_system_message(messages, &discover_skills_message());
  if let Ok((name, root, body)) = load_skill_content("colgrep") {
    append_to_system_message(
      messages,
      &format!("<skill name=\"{name}\" root=\"{root}\">\n{body}\n</skill>"),
    );
  }
}

fn append_to_system_message(messages: &mut [Message], content: &str) {
  if content.is_empty() {
    return;
  }
  if let Some(message) = messages.iter_mut().find(|m| m.role == "system") {
    if message.content.is_empty() {
      message.content = content.to_string();
    } else {
      message.content.push_str("\n\n");
      message.content.push_str(content);
    }
  }
}
