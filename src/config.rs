use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
  pub default_profile: String,
  pub autocompact: i32,
  pub profiles: HashMap<String, Profile>,
  pub providers: HashMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
  pub backend: String,
  pub model: String,
  pub effort: String,
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
    "config.yaml not found. Create one from config.yaml.sample at either:\n  {}\n  {}",
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

impl Default for Config {
  fn default() -> Self {
    let mut profiles = HashMap::new();
    profiles.insert(
      "ds-flash".to_string(),
      Profile {
        backend: "deepseek".to_string(),
        model: "deepseek-v4-flash".to_string(),
        effort: "high".to_string(),
        context_limit: 1_000_000,
      },
    );
    profiles.insert(
      "ds-flash-max".to_string(),
      Profile {
        backend: "deepseek".to_string(),
        model: "deepseek-v4-flash".to_string(),
        effort: "max".to_string(),
        context_limit: 1_000_000,
      },
    );
    profiles.insert(
      "ds-pro".to_string(),
      Profile {
        backend: "deepseek".to_string(),
        model: "deepseek-v4-pro".to_string(),
        effort: "high".to_string(),
        context_limit: 1_000_000,
      },
    );
    profiles.insert(
      "ds-pro-max".to_string(),
      Profile {
        backend: "deepseek".to_string(),
        model: "deepseek-v4-pro".to_string(),
        effort: "max".to_string(),
        context_limit: 1_000_000,
      },
    );
    profiles.insert(
      "kimi".to_string(),
      Profile {
        backend: "kimi".to_string(),
        model: "moonshotai/Kimi-K2.6".to_string(),
        effort: "".to_string(),
        context_limit: 256_000,
      },
    );
    profiles.insert(
      "glm".to_string(),
      Profile {
        backend: "z".to_string(),
        model: "glm-5.1".to_string(),
        effort: "".to_string(),
        context_limit: 200_000,
      },
    );

    let mut providers = HashMap::new();
    providers.insert(
      "deepseek".to_string(),
      ProviderConfig {
        base_url: "https://api.deepseek.com/chat/completions".to_string(),
        key_env: "DEEPSEEK_API_KEY".to_string(),
      },
    );
    providers.insert(
      "kimi".to_string(),
      ProviderConfig {
        base_url: "https://inference.baseten.co/v1/chat/completions".to_string(),
        key_env: "BASETEN_API_KEY".to_string(),
      },
    );
    providers.insert(
      "z".to_string(),
      ProviderConfig {
        base_url: "https://api.z.ai/api/coding/paas/v4/chat/completions".to_string(),
        key_env: "Z_API_KEY".to_string(),
      },
    );

    Self {
      default_profile: "ds-flash".to_string(),
      autocompact: 80,
      profiles,
      providers,
    }
  }
}
