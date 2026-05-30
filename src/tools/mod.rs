use anyhow::{Context, Result};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

use crate::types::{Tool, ToolFunction};

pub mod fs;
pub mod lua;
pub mod repo;
pub mod shell;
pub mod skills;
pub mod web;

#[derive(Clone)]
pub struct ToolContext {
  pub workspace: crate::workspace::Workspace,
  pub skill_store: std::sync::Arc<crate::skills::SkillStore>,
  pub lua_session: std::sync::Arc<std::sync::Mutex<Option<mlua::Lua>>>,
  pub client: crate::client::Client,
  pub output_sink: Option<std::sync::Arc<dyn crate::agent::AgentOutputSink>>,
  pub verbose: bool,
  pub actor_id: String,
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

impl Handler {
  pub fn async_fn<F, Fut>(f: F) -> Self
  where
    F: Fn(ToolContext, String) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<String>> + Send + 'static,
  {
    Self::Async(Box::new(move |ctx, args| Box::pin(f(ctx, args.to_owned()))))
  }
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

pub fn all_tools() -> &'static [ToolDef] {
  ALL_TOOLS.get_or_init(|| {
    let mut tools = Vec::new();
    tools.extend(fs::tools());
    tools.extend(lua::tools());
    tools.extend(shell::tools());
    tools.extend(repo::tools());
    tools.extend(web::tools());
    tools.extend(skills::tools());
    tools
  })
}

static AGENT_TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();

pub fn configured_agent_tools() -> Vec<Tool> {
  AGENT_TOOLS
    .get_or_init(|| {
      all_tools()
        .iter()
        .filter(|t| t.name == "exec" || t.name == "eval")
        .map(|t| t.schema())
        .collect()
    })
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
  fn configured_agent_tools_includes_expected() {
    let tools = configured_agent_tools();
    let names: Vec<_> = tools.iter().map(|t| t.function.name.as_str()).collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"exec"));
    assert!(names.contains(&"eval"));
  }

  #[tokio::test]
  async fn execute_tool_unknown_returns_error() {
    let workspace = crate::workspace::Workspace::from_current_dir();
    let skill_store =
      std::sync::Arc::new(crate::skills::SkillStore::new(workspace.root(), Vec::new()));
    let client = crate::client::Client::new(
      "http://localhost",
      "dummy".into(),
      |_, _| Ok(serde_json::Value::Null),
      30,
    )
    .unwrap();
    let result = execute_tool(
      ToolContext {
        workspace,
        skill_store,
        lua_session: std::sync::Arc::new(std::sync::Mutex::new(None)),
        client,
        output_sink: None,
        verbose: false,
        actor_id: "director".to_string(),
      },
      "nonexistent_tool",
      "{}",
    )
    .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("unknown tool"));
  }
}
