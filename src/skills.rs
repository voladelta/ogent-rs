use anyhow::{Result, bail};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
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

#[derive(Debug, Clone)]
struct SkillManifest {
  skills: HashMap<String, Skill>,
  ordered_info: Vec<SkillInfo>,
}

impl SkillManifest {
  fn build(roots: &[PathBuf]) -> Self {
    let mut skills = HashMap::new();
    let mut ordered_info = Vec::new();
    let mut seen = HashSet::new();

    for root in roots {
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
          let skill = Skill {
            name: key.clone(),
            root: entry.path(),
            body: strip_frontmatter(&content),
          };
          skills.insert(key.clone(), skill);
          ordered_info.push(SkillInfo {
            name: key,
            description: desc,
          });
        }
      }
    }

    Self {
      skills,
      ordered_info,
    }
  }

  fn get(&self, name: &str) -> Option<&Skill> {
    self.skills.get(name)
  }

  fn infos(&self) -> &[SkillInfo] {
    &self.ordered_info
  }
}

pub struct SkillStore {
  repo_roots: Vec<PathBuf>,
  home_roots: Vec<PathBuf>,
  startup_skills: Vec<String>,
  manifest: SkillManifest,
}

impl SkillStore {
  pub fn new(workspace_root: &Path, startup_skills: Vec<String>) -> Self {
    let repo_roots = vec![workspace_root.join(".ogent/skills")];
    let mut home_roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
      home_roots.push(PathBuf::from(home).join(".ogent/skills"));
    }
    let all_roots: Vec<PathBuf> = repo_roots
      .iter()
      .chain(home_roots.iter())
      .cloned()
      .collect();
    let manifest = SkillManifest::build(&all_roots);
    Self {
      repo_roots,
      home_roots,
      startup_skills,
      manifest,
    }
  }

  pub fn skill_roots(&self) -> impl Iterator<Item = &PathBuf> {
    self.repo_roots.iter().chain(self.home_roots.iter())
  }

  pub fn discover_skills(&self) -> Vec<SkillInfo> {
    self.manifest.infos().to_vec()
  }

  pub fn load_skill(&self, skill_name: &str) -> Result<Skill> {
    if let Some(skill) = self.manifest.get(skill_name) {
      Ok(skill.clone())
    } else {
      bail!("skill {skill_name} not found in skill roots")
    }
  }

  pub fn startup_skills(&self) -> &[String] {
    &self.startup_skills
  }
}

pub fn format_loaded_skill(skill: &Skill) -> String {
  format!(
    "<skill name=\"{}\" root=\"{}\">\n{}\n</skill>",
    crate::util::xml_escape(&skill.name),
    crate::util::xml_escape(&skill.root.to_string_lossy()),
    skill.body
  )
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

  #[test]
  fn test_skill_manifest_build_and_precedence() {
    let temp = std::env::temp_dir().join(format!(
      "ogent-skills-test-{}",
      crate::session::timestamp_ms()
    ));
    let repo_dir = temp.join("repo-skills");
    let home_dir = temp.join("home-skills");

    let skill_a_path = repo_dir.join("dir_a");
    std::fs::create_dir_all(&skill_a_path).unwrap();
    std::fs::write(
      skill_a_path.join("SKILL.md"),
      "---\nname: my-first-skill\ndescription: Repo version\n---\nHello from repo!",
    )
    .unwrap();

    // Home skill that shares the same exposed name but different directory
    let skill_b_path = home_dir.join("dir_b");
    std::fs::create_dir_all(&skill_b_path).unwrap();
    std::fs::write(
      skill_b_path.join("SKILL.md"),
      "---\nname: my-first-skill\ndescription: Home version\n---\nHello from home!",
    )
    .unwrap();

    // A skill that uses directory name because it lacks frontmatter name
    let skill_c_path = repo_dir.join("dir_c");
    std::fs::create_dir_all(&skill_c_path).unwrap();
    std::fs::write(
      skill_c_path.join("SKILL.md"),
      "No frontmatter at all, just body content",
    )
    .unwrap();

    let roots = vec![repo_dir, home_dir];
    let manifest = SkillManifest::build(&roots);

    // Verify key mappings
    assert_eq!(manifest.infos().len(), 2);

    // "my-first-skill" should be resolved to the repo version (first root in list takes precedence)
    let skill_1 = manifest.get("my-first-skill").unwrap();
    assert_eq!(skill_1.body, "Hello from repo!");
    assert_eq!(skill_1.name, "my-first-skill");
    assert_eq!(skill_1.root, skill_a_path);

    // "dir_c" should be the key for the skill without a frontmatter name
    let skill_2 = manifest.get("dir_c").unwrap();
    assert_eq!(skill_2.body, "No frontmatter at all, just body content");
    assert_eq!(skill_2.name, "dir_c");
    assert_eq!(skill_2.root, skill_c_path);

    // Let's clean up
    let _ = std::fs::remove_dir_all(temp);
  }

  #[test]
  fn test_format_loaded_skill() {
    let skill = Skill {
      name: "hello-world & \"test\"".to_string(),
      root: PathBuf::from("/path/to/my & skill"),
      body: "My Body\nContent".to_string(),
    };
    let formatted = format_loaded_skill(&skill);
    assert!(formatted.contains("name=\"hello-world &amp; &quot;test&quot;\""));
    assert!(formatted.contains("root=\"/path/to/my &amp; skill\""));
    assert!(formatted.contains("My Body\nContent"));
  }
}
