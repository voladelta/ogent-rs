use anyhow::{Result, bail};
use std::fs;
use std::path::PathBuf;

pub const TENX_CODER_SYSTEM_PROMPT: &str = include_str!("../prompts/10x-coder.md");

pub const WORKER_SUMMARY_PROMPT: &str = r#"

## Worker Report Protocol

Before returning, write a concise task summary to the artifact path. This is read by the parent agent.

Include:
- Summary: what you accomplished
- Evidence: files inspected, commands run, results
- Decisions: choices made
- Blockers: anything needing parent input; omit if none
- Files modified: list of files changed

Rules:
- Concise fragments are preferred.
- If blocked, use worker_question tool instead of stopping silently.
- Write your final report to the artifact path specified by the parent."#;

pub fn skill_roots() -> Vec<PathBuf> {
  let mut dirs = vec![PathBuf::from(".ogent/skills"), PathBuf::from(".skills")];
  if let Some(home) = std::env::var_os("HOME") {
    dirs.push(PathBuf::from(home).join(".ogent/skills"));
  }
  dirs
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
      let (name, desc) = parse_skill_frontmatter(&content);
      if name.is_empty() || !seen.insert(name.clone()) {
        continue;
      }
      out.push_str(&format!(
        "  <skill name=\"{}\" description=\"{}\" />\n",
        xml_escape(&name),
        xml_escape(&desc)
      ));
    }
  }
  if seen.is_empty() {
    String::new()
  } else {
    out.push_str("</skills>");
    out
  }
}

fn strip_frontmatter(content: &str) -> String {
  if !content.starts_with("---") {
    return content.trim().to_string();
  }
  let Some(end) = content[3..].find("---") else {
    return content.trim().to_string();
  };
  content[3 + end + 3..].trim().to_string()
}

fn parse_skill_frontmatter(content: &str) -> (String, String) {
  if !content.starts_with("---") {
    return (String::new(), String::new());
  }
  let Some(end) = content[3..].find("---") else {
    return (String::new(), String::new());
  };
  let fm = &content[3..3 + end];
  let mut name = String::new();
  let mut description = String::new();
  for line in fm.lines().map(str::trim) {
    if let Some(rest) = line.strip_prefix("name:") {
      name = rest.trim().to_string();
    }
    if let Some(rest) = line.strip_prefix("description:") {
      description = rest.trim().to_string();
    }
  }
  (name, description)
}

fn xml_escape(s: &str) -> String {
  s.replace('&', "&amp;")
    .replace('"', "&quot;")
    .replace('<', "&lt;")
}
