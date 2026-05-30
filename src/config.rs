use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
  pub default_profile: String,
  pub profiles: HashMap<String, Profile>,
  pub providers: HashMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
  pub backend: String,
  pub model: String,
  pub effort: String,
  #[allow(dead_code)]
  pub context_limit: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
  pub base_url: String,
  pub key_env: String,
}

impl Config {
  pub fn get_profile(&self, name: &str) -> Option<&Profile> {
    self.profiles.get(name)
  }

  pub fn provider_for(&self, profile: &Profile) -> Option<&ProviderConfig> {
    self.providers.get(&profile.backend)
  }
}

pub fn load_config(workspace_root: &Path) -> Result<Config> {
  let repo_config = workspace_root.join(".ogent/config.yaml");
  if repo_config.exists() {
    let content = std::fs::read_to_string(&repo_config)
      .with_context(|| format!("failed to read {}", repo_config.display()))?;
    return serde_yaml::from_str(&content)
      .with_context(|| format!("failed to parse {}", repo_config.display()));
  }

  let home_config = home_ogent_config()?;
  if home_config.exists() {
    let content = std::fs::read_to_string(&home_config)
      .with_context(|| format!("failed to read {}", home_config.display()))?;
    return serde_yaml::from_str(&content)
      .with_context(|| format!("failed to parse {}", home_config.display()));
  }

  bail!(
    "config.yaml not found. Create one from dotogent/config.yaml at either:\n  {}\n  {}",
    repo_config.display(),
    home_config.display()
  )
}

fn home_ogent_config() -> Result<PathBuf> {
  let home = std::env::var_os("HOME").context("HOME not set")?;
  Ok(PathBuf::from(home).join(".ogent/config.yaml"))
}

pub fn load_or_exit(workspace_root: &Path) -> Config {
  match load_config(workspace_root) {
    Ok(cfg) => cfg,
    Err(err) => {
      eprintln!("Error: {err}");
      std::process::exit(1);
    }
  }
}
