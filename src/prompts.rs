use anyhow::{Result, bail};
use std::fs;
use std::path::PathBuf;

pub const TENX_CODER_SYSTEM_PROMPT: &str = include_str!("../prompts/10x-coder.md");

pub const WORKER_SUMMARY_PROMPT: &str = "\n\n## Worker Report Protocol\n\nBefore returning, call `worker_complete` with JSON arguments:\n\n```json\n{\"summary\":\"concise Markdown summary for the parent coder\"}\n```\n\nInclude:\n- Summary: what you accomplished\n- Evidence: files inspected, commands run, results\n- Decisions: choices made\n- Blockers: anything needing parent input; omit if none\n- Files modified: list of files changed\n\nRules:\n- Concise fragments are preferred.\n- If blocked, use worker_question tool instead of stopping silently.\n- Return the report through `worker_complete({\"summary\":\"...\"})`.";

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

fn parse_frontmatter(content: &str) -> Option<&str> {
  content.strip_prefix("---").and_then(|rest| rest.find("---").map(|end| &rest[..end]))
}

fn strip_frontmatter(content: &str) -> String {
  parse_frontmatter(content)
    .map(|_| {
      let end = content[3..].find("---").unwrap() + 6;
      content[end..].trim().to_string()
    })
    .unwrap_or_else(|| content.trim().to_string())
}

fn parse_skill_frontmatter(content: &str) -> (String, String) {
  let Some(fm) = parse_frontmatter(content) else {
    return (String::new(), String::new());
  };
  let mut name = String::new();
  let mut description = String::new();
  for line in fm.lines().map(str::trim) {
    if let Some(rest) = line.strip_prefix("name:") {
      name = rest.trim().to_string();
    } else if let Some(rest) = line.strip_prefix("description:") {
      description = rest.trim().to_string();
    }
  }
  (name, description)
}

fn xml_escape(s: &str) -> String {
  s.replace('&', "&amp;")
    .replace('"', "&quot;")
    .replace('<', "&lt;")
    .replace('>', "&gt;")
}
