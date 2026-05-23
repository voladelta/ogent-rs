use anyhow::{Context, Result};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

use crate::types::{Tool, ToolFunction};

pub mod fs;
pub mod repo;
pub mod shell;
pub mod skills;
pub mod web;

pub struct ToolContext {
  pub workspace: crate::workspace::Workspace,
  pub skill_store: std::sync::Arc<crate::skills::SkillStore>,
}

pub type AsyncHandler = Box<
  dyn Fn(ToolContext, &str) -> Pin<Box<dyn Future<Output = Result<String>> + Send>> + Send + Sync,
>;

pub struct ToolDef {
  pub name: &'static str,
  pub description: &'static str,
  pub parameters: Value,
  pub handler: Handler,
}

pub enum Handler {
  Sync(fn(ToolContext, &str) -> Result<String>),
  Async(AsyncHandler),
}

impl ToolDef {
  pub fn schema(&self) -> Tool {
    Tool {
      kind: "function".to_string(),
      function: ToolFunction {
        name: self.name.to_string(),
        description: self.description.to_string(),
        parameters: self.parameters.clone(),
      },
    }
  }
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

static ALL_TOOLS: OnceLock<Vec<ToolDef>> = OnceLock::new();

fn build_all_tools() -> Vec<ToolDef> {
  let mut tools = Vec::new();
  tools.extend(fs::tools());
  tools.extend(shell::tools());
  tools.extend(repo::tools());
  tools.extend(web::tools());
  tools.extend(skills::tools());
  tools
}

pub fn all_tools() -> &'static [ToolDef] {
  ALL_TOOLS.get_or_init(build_all_tools)
}

static WORKER_TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();

pub fn configured_worker_tools() -> Vec<Tool> {
  WORKER_TOOLS
    .get_or_init(|| all_tools().iter().map(|t| t.schema()).collect())
    .clone()
}

pub async fn execute_tool(ctx: ToolContext, name: &str, args: &str) -> Result<String> {
  let tool = all_tools()
    .iter()
    .find(|t| t.name == name)
    .with_context(|| format!("unknown tool: {name}"))?;
  match &tool.handler {
    Handler::Sync(f) => f(ctx, args),
    Handler::Async(f) => f(ctx, args).await,
  }
}

pub use web::ensure_exa_api_key_set;

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn configured_worker_tools_includes_expected() {
    let tools = configured_worker_tools();
    let names: Vec<_> = tools.iter().map(|t| t.function.name.as_str()).collect();
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"write_file"));
    assert!(names.contains(&"read_hash_anchors"));
    assert!(names.contains(&"code_map"));
    assert!(names.contains(&"edit_hash_anchors"));
  }

  #[tokio::test]
  async fn execute_tool_unknown_returns_error() {
    let workspace = crate::workspace::Workspace::from_current_dir();
    let skill_store = std::sync::Arc::new(crate::skills::SkillStore::new(workspace.root(), Vec::new()));
    let result = execute_tool(
      ToolContext {
        workspace,
        skill_store,
      },
      "nonexistent_tool",
      "{}",
    )
    .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("unknown tool"));
  }
}
