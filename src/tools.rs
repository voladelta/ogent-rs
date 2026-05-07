use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::agent::Agent;
use crate::types::{Tool, ToolFunction};

pub struct ToolContext<'a> {
  pub agent: Option<&'a mut Agent>,
}

pub async fn execute_tool(ctx: ToolContext<'_>, name: &str, args: &str) -> Result<String> {
  crate::toolimpl::execute_tool(ctx, name, args).await
}

pub fn configured_coder_tools(_steer: bool) -> Vec<Tool> {
  vec![
    schema(
      "read_file",
      "Read a file from the local filesystem.",
      json!({"type":"object","properties":{"path":{"type":"string"},"start":{"type":"integer"},"end":{"type":"integer"}},"required":["path"],"additionalProperties":false}),
    ),
    schema(
      "write_file",
      "Write content to a new file. For existing files, prefer edit_hash_anchors.",
      json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},"overwrite_existing":{"type":"boolean"}},"required":["path","content"],"additionalProperties":false}),
    ),
    schema(
      "bash",
      "Execute a shell command in the workspace and return stdout and stderr combined.",
      json!({"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer"}},"required":["command"],"additionalProperties":false}),
    ),
    schema(
      "repo_map",
      "Display a tree map of the repository directory structure.",
      json!({"type":"object","properties":{"path":{"type":"string"},"levels":{"type":"integer"}},"additionalProperties":false}),
    ),
    schema(
      "read_hash_anchors",
      "Read a file returning each line prefixed as line:hash|content.",
      json!({"type":"object","properties":{"path":{"type":"string"},"start":{"type":"integer"},"end":{"type":"integer"}},"required":["path"],"additionalProperties":false}),
    ),
    schema(
      "edit_hash_anchors",
      "Edit a file using hashline anchors from read_hash_anchors.",
      json!({"type":"object","properties":{"path":{"type":"string"},"ops":{"type":"array","items":{"type":"object","properties":{"anchor":{"type":"string"},"end_anchor":{"type":"string"},"action":{"type":"string","enum":["replace","before","after"]},"new_string":{"type":"string"}},"required":["anchor","action","new_string"]}}},"required":["path","ops"],"additionalProperties":false}),
    ),
    schema(
      "web_search",
      "Search the web using Exa.",
      json!({"type":"object","properties":{"query":{"type":"string"},"num_results":{"type":"integer"},"type":{"type":"string","enum":["auto","deep-reasoning"]}},"required":["query"],"additionalProperties":false}),
    ),
    schema(
      "web_read",
      "Read the content of one or more URLs using Exa.",
      json!({"type":"object","properties":{"urls":{"type":"array","items":{"type":"string"}},"mode":{"type":"string","enum":["text","highlights"]}},"required":["urls"],"additionalProperties":false}),
    ),
    schema(
      "code_web_context",
      "Search for code examples and practical implementation context.",
      json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}),
    ),
    schema(
      "dispatch_worker",
      "Hire a specialist coworker subprocess and return its report.",
      json!({"type":"object","properties":{"system_prompt":{"type":"string"},"task":{"type":"string"},"artifact_path":{"type":"string"},"max_turns":{"type":"integer"}},"required":["system_prompt","task","artifact_path"],"additionalProperties":false}),
    ),
    schema(
      "start_workers",
      "Start a batch of specialist coworkers asynchronously.",
      json!({"type":"object","properties":{"coworkers":{"type":"array","minItems":1,"items":{"type":"object","properties":{"name":{"type":"string"},"system_prompt":{"type":"string"},"task_prompt":{"type":"string"},"artifact_path":{"type":"string"},"max_turns":{"type":"integer"}},"required":["system_prompt","task_prompt"],"additionalProperties":false}}},"required":["coworkers"],"additionalProperties":false}),
    ),
    schema(
      "check_workers",
      "Wait for all active async coworkers and return reports.",
      json!({"type":"object","properties":{},"additionalProperties":false}),
    ),
    schema(
      "handoff",
      "Write a session handoff brief to disk.",
      json!({"type":"object","properties":{"brief":{"type":"string"}},"required":["brief"],"additionalProperties":false}),
    ),
    schema(
      "load_skill",
      "Load a skill from .ogent/skills/ or .skills/.",
      json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}),
    ),
    schema(
      "question",
      "Ask the user a question. Only available on the first turn.",
      json!({"type":"object","properties":{"question":{"type":"string"}},"required":["question"],"additionalProperties":false}),
    ),
  ]
}

pub fn configured_worker_tools() -> Vec<Tool> {
  let mut tools = configured_coder_tools(false);
  tools.retain(|t| {
    !matches!(
      t.function.name.as_str(),
      "dispatch_worker" | "start_workers" | "check_workers" | "handoff" | "question"
    )
  });
  tools.push(schema("worker_question", "Ask the parent coder agent a question when blocked.", json!({"type":"object","properties":{"question":{"type":"string"}},"required":["question"],"additionalProperties":false})));
  tools
}

pub fn remove_question(tools: &mut Vec<Tool>) {
  if tools.last().is_some_and(|t| t.function.name == "question") {
    tools.pop();
  }
}

pub fn is_read_only_tool(name: &str) -> bool {
  matches!(
    name,
    "read_file"
      | "read_hash_anchors"
      | "repo_map"
      | "web_search"
      | "web_read"
      | "code_web_context"
      | "load_skill"
  )
}

pub fn parse_args<T: serde::de::DeserializeOwned>(args: &str) -> Result<T> {
  serde_json::from_str(args).map_err(|e| anyhow::anyhow!("bad args: {e}"))
}

pub fn require_nonempty(value: &str, name: &str) -> Result<()> {
  if value.trim().is_empty() {
    bail!("{name} is required");
  }
  Ok(())
}

fn schema(name: &str, description: &str, parameters: Value) -> Tool {
  Tool {
    kind: "function".to_string(),
    function: ToolFunction {
      name: name.to_string(),
      description: description.to_string(),
      parameters,
    },
  }
}
