use anyhow::{Result, bail};
use std::fs;
use std::path::PathBuf;

pub const TENX_CODER_SYSTEM_PROMPT: &str = include_str!("../prompts/10x-coder.md");

pub const WORKER_SUMMARY_PROMPT: &str = "\n\n## Worker Report Protocol\n\nWhen done, call `worker_complete` with a concise Markdown summary:\n\n```json\n{\"summary\":\"...\"}\n```\n\nInclude in the summary:\n- What you accomplished\n- Files inspected, commands run, results\n- Decisions made\n- Files modified (list)\n- Blockers (omit if none)\n\nRules:\n- Concise fragments are preferred.\n- Never fabricate or embellish results. Report only what you actually observed or did.\n- If blocked or missing information, use `worker_question` instead of stopping silently.\n- Do not write intermediate analysis, planning, or decision documents to the repo.";

pub const WORKER_TEMPLATE_GENERIC: &str = include_str!("../prompts/templates/generic.md");
pub const WORKER_TEMPLATE_TESTER: &str = include_str!("../prompts/templates/tester.md");
pub const WORKER_TEMPLATE_REVIEWER: &str = include_str!("../prompts/templates/reviewer.md");

pub fn get_worker_template(name: &str) -> Option<&'static str> {
  match name {
    "generic" => Some(WORKER_TEMPLATE_GENERIC),
    "tester" => Some(WORKER_TEMPLATE_TESTER),
    "reviewer" => Some(WORKER_TEMPLATE_REVIEWER),
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
