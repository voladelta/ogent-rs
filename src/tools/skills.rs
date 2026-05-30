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

  // Check if root_abs is inside one of the skill_roots
  let whitelisted = ctx.skill_store.skill_roots().any(|skill_root| {
    let skill_root_abs = crate::workspace::normalize(skill_root);
    root_abs.starts_with(&skill_root_abs) && root_abs != skill_root_abs
  });

  if !whitelisted {
    bail!(
      "root path {} is not inside a whitelisted skills directory",
      args.root
    );
  }

  let asset_path = root_abs.join(&args.path);
  let asset_path_norm = crate::workspace::normalize(&asset_path);

  // Ensure asset_path_norm is inside root_abs to prevent directory traversal
  if !asset_path_norm.starts_with(&root_abs) {
    bail!("asset path is outside the skill root directory");
  }

  let meta = std::fs::metadata(&asset_path_norm)
    .with_context(|| format!("stat skill asset at {}", asset_path_norm.display()))?;
  if meta.len() > (1 << 20) {
    bail!(
      "skill asset exceeds size limit ({} > {} bytes)",
      meta.len(),
      1 << 20
    );
  }

  let content = std::fs::read_to_string(&asset_path_norm).with_context(|| {
    format!(
      "failed to read skill asset at {}",
      asset_path_norm.display()
    )
  })?;
  Ok(content)
}
