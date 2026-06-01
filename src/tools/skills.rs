use anyhow::Result;
use serde::Deserialize;
use std::fmt::Write;

use crate::tools::{ToolContext, parse_args, require_nonempty};

#[derive(Deserialize)]
struct LoadSkillArgs {
  name: String,
}

#[derive(Deserialize)]
struct LoadSkillAssetArgs {
  root: String,
  path: String,
}

pub fn load_skill(ctx: ToolContext, args: &str) -> Result<String> {
  let args: LoadSkillArgs = parse_args(args)?;
  require_nonempty(&args.name, "name")?;
  let skill = ctx.skill_store.load_skill(&args.name)?;
  Ok(crate::skills::format_loaded_skill(&skill))
}

pub fn list_skills(ctx: ToolContext, _args: &str) -> Result<String> {
  let infos = ctx.skill_store.discover_skills();
  if infos.is_empty() {
    return Ok("# Available Skills\nNo skills found.".to_string());
  }

  let mut out = String::new();
  writeln!(out, "# Available Skills")?;
  writeln!(out, "Use `load_skill(name)` to load a skill.\n")?;

  for info in &infos {
    let root_path = match ctx.skill_store.load_skill(&info.name) {
      Ok(skill) => skill.root.to_string_lossy().to_string(),
      Err(_) => "unknown".to_string(),
    };

    writeln!(out, "## {}", info.name)?;
    writeln!(out, "- **Root**: `{}`", root_path)?;
    writeln!(out, "- **Description**: {}", info.description)?;
    out.push('\n');
  }

  Ok(out.trim_end().to_string())
}

pub fn load_skill_asset(ctx: ToolContext, args: &str) -> Result<String> {
  use anyhow::{Context, bail};
  use std::path::PathBuf;

  let args: LoadSkillAssetArgs = parse_args(args)?;
  require_nonempty(&args.root, "root")?;
  require_nonempty(&args.path, "path")?;

  let root_abs = if let Some(rest) = args.root.strip_prefix("~/") {
    let home = std::env::var("HOME").context("HOME env var not set")?;
    PathBuf::from(home).join(rest)
  } else {
    let p = PathBuf::from(&args.root);
    if p.is_absolute() {
      p
    } else {
      ctx.workspace.root().join(p)
    }
  };
  let root_abs = crate::workspace::normalize(&root_abs);
  let root_real = std::fs::canonicalize(&root_abs)
    .with_context(|| format!("canonicalize skill root at {}", root_abs.display()))?;

  // Check the resolved path so symlinks cannot turn an allowed-looking root into an escape.
  let whitelisted = ctx.skill_store.skill_roots().any(|skill_root| {
    std::fs::canonicalize(skill_root)
      .ok()
      .is_some_and(|skill_root_real| {
        root_real.starts_with(&skill_root_real) && root_real != skill_root_real
      })
  });

  if !whitelisted {
    bail!(
      "root path {} is not inside a whitelisted skills directory",
      args.root
    );
  }

  let asset_path = root_real.join(&args.path);
  let asset_path_norm = crate::workspace::normalize(&asset_path);
  if !asset_path_norm.starts_with(&root_real) {
    bail!("asset path is outside the skill root directory");
  }
  let asset_path_real = std::fs::canonicalize(&asset_path_norm)
    .with_context(|| format!("canonicalize skill asset at {}", asset_path_norm.display()))?;

  // Ensure the resolved asset path is inside the resolved skill root.
  if !asset_path_real.starts_with(&root_real) {
    bail!("asset path is outside the skill root directory");
  }

  let meta = std::fs::metadata(&asset_path_real)
    .with_context(|| format!("stat skill asset at {}", asset_path_real.display()))?;
  if !meta.is_file() {
    bail!("skill asset path is not a file");
  }
  if meta.len() > (1 << 20) {
    bail!(
      "skill asset exceeds size limit ({} > {} bytes)",
      meta.len(),
      1 << 20
    );
  }

  let content = std::fs::read_to_string(&asset_path_real).with_context(|| {
    format!(
      "failed to read skill asset at {}",
      asset_path_real.display()
    )
  })?;
  Ok(content)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::workspace::Workspace;
  use std::sync::Arc;

  fn test_context(root: &std::path::Path) -> ToolContext {
    let workspace = Workspace::from_root(root.to_path_buf());
    let skill_store = Arc::new(crate::skills::SkillStore::new(workspace.root()));
    let client = crate::client::Client::new(
      crate::client::ClientConfig {
        url: "http://localhost".to_string(),
        api_key: "dummy".into(),
        request_timeout_secs: 30,
        require_sse_done: true,
      },
      |_, _| Ok(serde_json::Value::Null),
    )
    .unwrap();
    ToolContext {
      workspace,
      skill_store,
      lua_session: Arc::new(parking_lot::Mutex::new(None)),
      client,
      output_sink: None,
      verbose: false,
      actor_id: "director".to_string(),
      agent_depth: 0,
    }
  }

  #[test]
  fn load_skill_asset_reads_regular_asset() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let skill_dir = temp.path().join(".ogent/skills/demo");
    let refs_dir = skill_dir.join("references");
    std::fs::create_dir_all(&refs_dir)?;
    std::fs::write(skill_dir.join("SKILL.md"), "---\nname: demo\n---\nBody")?;
    std::fs::write(refs_dir.join("MANUAL.md"), "manual")?;

    let args = serde_json::json!({
      "root": skill_dir.to_string_lossy(),
      "path": "references/MANUAL.md"
    })
    .to_string();

    let content = load_skill_asset(test_context(temp.path()), &args)?;

    assert_eq!(content, "manual");
    Ok(())
  }

  #[test]
  #[cfg(unix)]
  fn load_skill_asset_rejects_symlink_escape() -> Result<()> {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir()?;
    let skill_dir = temp.path().join(".ogent/skills/demo");
    let refs_dir = skill_dir.join("references");
    let outside_dir = temp.path().join("outside");
    std::fs::create_dir_all(&refs_dir)?;
    std::fs::create_dir_all(&outside_dir)?;
    std::fs::write(skill_dir.join("SKILL.md"), "---\nname: demo\n---\nBody")?;

    let secret = outside_dir.join("secret.txt");
    std::fs::write(&secret, "secret")?;
    symlink(&secret, refs_dir.join("secret-link"))?;

    let args = serde_json::json!({
      "root": skill_dir.to_string_lossy(),
      "path": "references/secret-link"
    })
    .to_string();

    let err = load_skill_asset(test_context(temp.path()), &args).unwrap_err();

    assert!(err.to_string().contains("outside the skill root"));
    Ok(())
  }
}
