use anyhow::{Result, bail};
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Skill {
  pub name: String,
  pub root: PathBuf,
  pub body: String,
}

#[derive(Debug, Clone)]
pub struct SkillInfo {
  pub name: String,
  pub description: String,
}

pub struct SkillStore {
  repo_roots: Vec<PathBuf>,
  home_roots: Vec<PathBuf>,
  startup_skills: Vec<String>,
}

impl SkillStore {
  pub fn new(workspace_root: &Path, startup_skills: Vec<String>) -> Self {
    let repo_roots = vec![workspace_root.join(".ogent/skills")];
    let mut home_roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
      home_roots.push(PathBuf::from(home).join(".ogent/skills"));
    }
    Self {
      repo_roots,
      home_roots,
      startup_skills,
    }
  }

  pub fn skill_roots(&self) -> impl Iterator<Item = &PathBuf> {
    self.repo_roots.iter().chain(self.home_roots.iter())
  }

  pub fn discover_skills(&self) -> Vec<SkillInfo> {
    let mut seen = HashSet::new();
    let mut skills = Vec::new();
    for root in self.skill_roots() {
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
        if seen.insert(key.clone()) {
          skills.push(SkillInfo {
            name: key,
            description: desc,
          });
        }
      }
    }
    skills
  }

  pub fn load_skill(&self, skill_name: &str) -> Result<Skill> {
    for dir in self.skill_roots() {
      let root = dir.join(skill_name);
      let path = root.join("SKILL.md");
      if let Ok(content) = fs::read_to_string(&path) {
        let (name, _) = parse_skill_frontmatter(&content);
        let resolved_name = if name.is_empty() {
          skill_name.to_string()
        } else {
          name
        };
        return Ok(Skill {
          name: resolved_name,
          root,
          body: strip_frontmatter(&content),
        });
      }
    }
    bail!("skill {skill_name} not found in skill roots")
  }

  pub fn startup_skills(&self) -> &[String] {
    &self.startup_skills
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

#[derive(Deserialize, Default)]
struct SkillFrontmatter {
  #[serde(default)]
  name: String,
  #[serde(default)]
  description: String,
}

fn parse_skill_frontmatter(content: &str) -> (String, String) {
  let fm = parse_frontmatter(content).unwrap_or("");
  let parsed = serde_yaml::from_str::<SkillFrontmatter>(fm).unwrap_or_default();
  (parsed.name, parsed.description)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_parse_frontmatter_valid() {
    let content = "---\nname: test-skill\ndescription: A test skill\n---\nBody here";
    let (name, desc) = parse_skill_frontmatter(content);
    assert_eq!(name, "test-skill");
    assert_eq!(desc, "A test skill");
    assert_eq!(strip_frontmatter(content), "Body here");
  }

  #[test]
  fn test_parse_frontmatter_empty() {
    let content = "Body here without frontmatter";
    let (name, desc) = parse_skill_frontmatter(content);
    assert_eq!(name, "");
    assert_eq!(desc, "");
    assert_eq!(strip_frontmatter(content), "Body here without frontmatter");
  }
}
