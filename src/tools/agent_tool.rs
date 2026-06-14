use serde::Deserialize;
use std::result::Result;

use crate::tools::{ToolContext, eval, exec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTool {
  Exec,
  Eval,
}

impl AgentTool {
  pub fn from_name(name: &str) -> Result<Self, AgentToolNameError> {
    match name {
      "exec" => Ok(Self::Exec),
      "eval" => Ok(Self::Eval),
      other => Err(AgentToolNameError::Unknown {
        name: other.to_string(),
      }),
    }
  }

  pub const fn name(self) -> &'static str {
    match self {
      Self::Exec => "exec",
      Self::Eval => "eval",
    }
  }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentToolNameError {
  #[error("unknown agent tool: {name}")]
  Unknown { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaToolRequest {
  pub tool: AgentTool,
  pub code: String,
}

#[derive(Debug, Deserialize)]
struct LuaToolArguments {
  code: String,
  #[allow(dead_code)]
  reason: Option<String>,
}

impl LuaToolRequest {
  pub fn from_tool_call(tool_call: &crate::types::ToolCall) -> Result<Self, AgentToolRequestError> {
    let tool = AgentTool::from_name(&tool_call.function.name)?;
    Self::from_arguments(tool, &tool_call.function.arguments)
  }

  pub fn from_arguments(tool: AgentTool, arguments: &str) -> Result<Self, AgentToolRequestError> {
    let parsed: LuaToolArguments = serde_json::from_str(arguments)
      .map_err(|source| AgentToolRequestError::InvalidArguments { tool, source })?;
    if parsed.code.trim().is_empty() {
      return Err(AgentToolRequestError::MissingCode { tool });
    }
    Ok(Self {
      tool,
      code: parsed.code,
    })
  }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentToolRequestError {
  #[error(transparent)]
  Name(#[from] AgentToolNameError),
  #[error("invalid arguments for `{}`: {source}", .tool.name())]
  InvalidArguments {
    tool: AgentTool,
    #[source]
    source: serde_json::Error,
  },
  #[error("missing code for `{}`", .tool.name())]
  MissingCode { tool: AgentTool },
}

#[derive(Debug, thiserror::Error)]
pub enum AgentToolExecutionError {
  #[error(transparent)]
  Request(#[from] AgentToolRequestError),
  #[error("`{}` failed: {source}", .tool.name())]
  Runtime {
    tool: AgentTool,
    #[source]
    source: anyhow::Error,
  },
}

pub async fn run_agent_tool(
  ctx: ToolContext,
  tool_call: &crate::types::ToolCall,
) -> Result<String, AgentToolExecutionError> {
  let request = LuaToolRequest::from_tool_call(tool_call)?;
  match request.tool {
    AgentTool::Exec => {
      exec(ctx, &request.code)
        .await
        .map_err(|source| AgentToolExecutionError::Runtime {
          tool: request.tool,
          source,
        })
    }
    AgentTool::Eval => {
      eval(ctx, &request.code)
        .await
        .map_err(|source| AgentToolExecutionError::Runtime {
          tool: request.tool,
          source,
        })
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn agent_tool_names_are_closed_domain() {
    assert_eq!(AgentTool::from_name("exec").unwrap(), AgentTool::Exec);
    assert_eq!(AgentTool::from_name("eval").unwrap(), AgentTool::Eval);
    assert!(matches!(
      AgentTool::from_name("shell"),
      Err(AgentToolNameError::Unknown { name }) if name == "shell"
    ));
  }

  #[test]
  fn lua_tool_request_decodes_schema_arguments() {
    let request =
      LuaToolRequest::from_arguments(AgentTool::Exec, r#"{"code":"return 1","reason":"test"}"#)
        .unwrap();

    assert_eq!(
      request,
      LuaToolRequest {
        tool: AgentTool::Exec,
        code: "return 1".to_string(),
      }
    );
  }

  #[test]
  fn lua_tool_request_rejects_missing_code() {
    let err = LuaToolRequest::from_arguments(AgentTool::Eval, r#"{"code":"   "}"#).unwrap_err();

    assert!(matches!(
      err,
      AgentToolRequestError::MissingCode {
        tool: AgentTool::Eval
      }
    ));
    assert_eq!(err.to_string(), "missing code for `eval`");
  }

  #[test]
  fn invalid_arguments_error_displays_parse_source() {
    let err = LuaToolRequest::from_arguments(AgentTool::Exec, "not json").unwrap_err();

    assert!(err.to_string().contains("invalid arguments for `exec`:"));
    assert!(err.to_string().contains("expected ident"));
  }
}
