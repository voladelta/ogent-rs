use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

use crate::tools::{Handler, ToolContext, ToolDef, parse_args, require_nonempty};

pub fn tools() -> Vec<ToolDef> {
  vec![
    ToolDef {
      name: "load_skill",
      description: "Load a skill from .ogent/skills/ or ~/.ogent/skills/.",
      parameters: json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}),
      handler: Handler::Sync(load_skill),
    },
    ToolDef {
      name: "list_skills",
      description: "List all available skills from workspace and home skill directories.",
      parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
      handler: Handler::Sync(list_skills),
    },
    ToolDef {
      name: "load_skill_asset",
      description: "Load an asset file from a skill root directory (e.g. references/MANUAL.md or scripts/analyze.py).",
      parameters: json!({
        "type": "object",
        "properties": {
          "root": {"type": "string", "description": "Absolute or workspace-relative root directory of the skill"},
          "path": {"type": "string", "description": "Asset file relative path inside the skill root"}
        },
        "required": ["root", "path"],
        "additionalProperties": false
      }),
      handler: Handler::Sync(load_skill_asset),
    },
  ]
}

#[derive(Deserialize)]
struct LoadSkillArgs {
  name: String,
}

#[derive(Deserialize)]
struct LoadSkillAssetArgs {
  root: String,
  path: String,
}

fn load_skill(ctx: ToolContext, args: &str) -> Result<String> {
  let args: LoadSkillArgs = parse_args(args)?;
  require_nonempty(&args.name, "name")?;
  let skill = ctx.skill_store.load_skill(&args.name)?;
  Ok(crate::skills::format_loaded_skill(&skill))
}

fn list_skills(ctx: ToolContext, _args: &str) -> Result<String> {
  let infos = ctx.skill_store.discover_skills();
  if infos.is_empty() {
    return Ok("# Available Skills\nNo skills found.".to_string());
  }

  let mut out = String::new();
  out.push_str("# Available Skills\n");
  out.push_str("Use `load_skill(name)` to load a skill.\n\n");

  for info in &infos {
    let root_path = match ctx.skill_store.load_skill(&info.name) {
      Ok(skill) => skill.root.to_string_lossy().to_string(),
      Err(_) => "unknown".to_string(),
    };

    out.push_str(&format!("## {}\n", info.name));
    out.push_str(&format!("- **Root**: `{}`\n", root_path));
    out.push_str(&format!("- **Description**: {}\n\n", info.description));
  }

  Ok(out.trim_end().to_string())
}

fn load_skill_asset(ctx: ToolContext, args: &str) -> Result<String> {
  use anyhow::{Context, bail};
  use std::path::PathBuf;

  let args: LoadSkillAssetArgs = parse_args(args)?;
  require_nonempty(&args.root, "root")?;
  require_nonempty(&args.path, "path")?;

  let root_abs = if args.root.starts_with("~/") {
    let home = std::env::var("HOME").context("HOME env var not set")?;
    let path_str = args.root.replacen("~/", "", 1);
    PathBuf::from(home).join(path_str)
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
  let mut whitelisted = false;
  for skill_root in ctx.skill_store.skill_roots() {
    let skill_root_abs = crate::workspace::normalize(skill_root);
    if root_abs.starts_with(&skill_root_abs) && root_abs != skill_root_abs {
      whitelisted = true;
      break;
    }
  }

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
