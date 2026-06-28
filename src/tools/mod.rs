use anyhow::{Context, Result};

pub mod agent_tool;
pub mod artifacts;
pub mod fs;
pub mod git;
pub mod lua;
pub mod repo;
pub mod search;
pub mod shell;
pub mod skills;
pub mod web;

#[derive(Clone)]
pub struct ToolContext {
  pub workspace: crate::workspace::Workspace,
  pub skill_store: std::sync::Arc<crate::skills::SkillStore>,
  pub lua_session: std::sync::Arc<parking_lot::Mutex<Option<mlua::Lua>>>,
  pub client: crate::client::Client,
  pub output_sink: Option<std::sync::Arc<dyn crate::agent::AgentOutputSink>>,
  pub verbose: bool,
  pub actor_id: String,
  /// Nesting depth of agent spawns: 0 for the root agent, +1 for each `agent{}` call.
  /// Enforced in lua.rs to prevent unbounded subagent recursion.
  pub agent_depth: u32,
}

pub fn parse_args<T: serde::de::DeserializeOwned>(args: &str) -> Result<T> {
  serde_json::from_str(args).context("bad args")
}

pub fn require_nonempty(value: &str, name: &str) -> Result<()> {
  if value.trim().is_empty() {
    anyhow::bail!("{name} is required");
  }
  Ok(())
}

pub use agent_tool::{AgentTool, run_agent_tool};
pub use lua::agent_tools;
pub use lua::eval;
pub use lua::exec;
pub use web::ensure_exa_api_key_set;

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn agent_tools_includes_expected() {
    let tools = agent_tools();
    let names: Vec<_> = tools.iter().map(|t| t.function.name.as_str()).collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"exec"));
    assert!(names.contains(&"eval"));
  }
}
