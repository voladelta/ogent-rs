use anyhow::{Context, Result, bail};
use std::sync::OnceLock;
use serde_json::{Value, json};

use crate::agent::Agent;
use crate::types::{Tool, ToolFunction};

pub struct ToolContext<'a> {
  pub agent: Option<&'a mut Agent>,
}

pub async fn execute_tool(ctx: ToolContext<'_>, name: &str, args: &str) -> Result<String> {
  crate::toolimpl::execute_tool(ctx, name, args).await
}

static CODER_TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();
static WORKER_TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();

pub fn configured_coder_tools(_steer: bool) -> Vec<Tool> {
  CODER_TOOLS.get_or_init(build_coder_tools).clone()
}

pub fn configured_worker_tools() -> Vec<Tool> {
  WORKER_TOOLS.get_or_init(build_worker_tools).clone()
}

const WORKER_EXCLUDED: &[&str] = &[
  "dispatch_worker",
  "start_workers",
  "check_workers",
  "handoff",
  "complete",
  "question",
  "set_goal",
  "revise_goal",
  "update_phase",
  "update_todo",
  "load_worker_template",
];

fn build_coder_tools() -> Vec<Tool> {
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
      "Hire a specialist coworker. system_prompt shapes worker behavior/scope; task states the concrete assignment. The worker runs as a separate process and returns a Markdown summary.",
      json!({"type":"object","properties":{"system_prompt":{"type":"string","description":"Complete behavior-shaping system prompt for the worker: role, permissions, read/write scope, constraints, commands, and summary format"},"task":{"type":"string","description":"Concrete task-shaping user prompt for the worker: exact assignment, expected output, success criteria, and immediate next step"},"max_turns":{"type":"integer","description":"Optional max turns for the worker (-1=unlimited). If omitted, worker has no turn limit."}},"required":["system_prompt","task"],"additionalProperties":false}),
    ),
    schema(
      "start_workers",
      "Start a batch of specialist coworkers asynchronously and return immediately with worker IDs.",
      json!({"type":"object","properties":{"coworkers":{"type":"array","minItems":1,"items":{"type":"object","properties":{"name":{"type":"string","description":"Optional short unique label for status"},"system_prompt":{"type":"string","description":"Behavior-shaping system prompt: role, permissions, read/write scope, constraints, commands, and summary format"},"task_prompt":{"type":"string","description":"Concrete task prompt: assignment, expected output, success criteria, and immediate next step"},"max_turns":{"type":"integer","description":"Optional max turns for this worker. If omitted or <=0, worker has no turn limit."}},"required":["system_prompt","task_prompt"],"additionalProperties":false}}},"required":["coworkers"],"additionalProperties":false}),
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
      "set_goal",
      "Initialize runtime task tracking with one Goal.",
      json!({"type":"object","properties":{"goal":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed","blocked","skipped"]},"complexity":{"type":"string","enum":["simple","medium","complex"]},"success_criteria":{"type":"array","items":{"type":"string"}},"notes":{"type":"string"}},"required":["goal","status","complexity"],"additionalProperties":false}),
    ),
    schema(
      "revise_goal",
      "Rarely revise the Goal and record the prior Goal plus reason.",
      json!({"type":"object","properties":{"goal":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed","blocked","skipped"]},"complexity":{"type":"string","enum":["simple","medium","complex"]},"success_criteria":{"type":"array","items":{"type":"string"}},"reason":{"type":"string"},"notes":{"type":"string"}},"required":["goal","status","complexity","reason"],"additionalProperties":false}),
    ),
    schema(
      "update_phase",
      "Upsert one Phase under the current Goal.",
      json!({"type":"object","properties":{"phase_id":{"type":"string"},"title":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed","blocked","skipped"]},"complexity":{"type":"string","enum":["simple","medium","complex"]},"notes":{"type":"string"}},"required":["phase_id","title","status","complexity"],"additionalProperties":false}),
    ),
    schema(
      "update_todo",
      "Upsert one Todo under an existing Phase.",
      json!({"type":"object","properties":{"phase_id":{"type":"string"},"todo_id":{"type":"string"},"title":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed","blocked","skipped"]},"complexity":{"type":"string","enum":["simple","medium","complex"]},"notes":{"type":"string"}},"required":["phase_id","todo_id","title","status","complexity"],"additionalProperties":false}),
    ),
    schema(
      "load_skill",
      "Load a skill from .ogent/skills/ or .skills/.",
      json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}),
    ),
    schema(
      "load_worker_template",
      "Load a built-in worker template (generic, tester, reviewer). Returns the template content with placeholders. Fill placeholders before using as system_prompt.",
      json!({"type":"object","properties":{"name":{"type":"string","enum":["generic","tester","reviewer"],"description":"Built-in worker template name"}},"required":["name"],"additionalProperties":false}),
    ),
    schema(
      "complete",
      "Mark the current task complete and provide a retrospective structured Markdown session summary.",
      json!({"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"],"additionalProperties":false}),
    ),
    schema(
      "question",
      "Ask the user a question. Only available on the first turn.",
      json!({"type":"object","properties":{"question":{"type":"string"}},"required":["question"],"additionalProperties":false}),
    ),
  ]
}

fn build_worker_tools() -> Vec<Tool> {
  let mut tools: Vec<Tool> = build_coder_tools()
    .into_iter()
    .filter(|t| !WORKER_EXCLUDED.contains(&t.function.name.as_str()))
    .collect();
  tools.push(schema("worker_question", "Ask the parent coder agent a question when blocked.", json!({"type":"object","properties":{"question":{"type":"string"}},"required":["question"],"additionalProperties":false})));
  tools.push(schema("worker_complete", "Finish this worker subprocess and return a concise Markdown summary to the parent coder.", json!({"type":"object","properties":{"summary":{"type":"string","description":"Concise Markdown summary for the parent coder"}},"required":["summary"],"additionalProperties":false})));
  tools
}

pub fn remove_question(tools: &mut Vec<Tool>) {
  tools.retain(|t| t.function.name != "question");
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
      | "load_worker_template"
  )
}

pub fn parse_args<T: serde::de::DeserializeOwned>(args: &str) -> Result<T> {
  serde_json::from_str(args).context("bad args")
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
