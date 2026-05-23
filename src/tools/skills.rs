use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

use crate::tools::{Capability, Handler, ToolContext, ToolDef, parse_args, require_nonempty};

pub fn tools() -> Vec<ToolDef> {
  vec![ToolDef {
    name: "load_skill",
    description: "Load a skill from .ogent/skills/ or ~/.ogent/skills/.",
    parameters: json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}),
    handler: Handler::Sync(load_skill),
    capability: Capability::ReadOnly,
  }]
}

#[derive(Deserialize)]
struct LoadSkillArgs {
  name: String,
}

fn load_skill(ctx: ToolContext, args: &str) -> Result<String> {
  let args: LoadSkillArgs = parse_args(args)?;
  require_nonempty(&args.name, "name")?;
  let skill = ctx.skill_store.load_skill(&args.name)?;
  Ok(format!(
    "<skill name=\"{}\" root=\"{}\">\n{}\n</skill>",
    skill.name,
    skill.root.display(),
    skill.body
  ))
}
