use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
  #[default]
  System,
  User,
  Assistant,
  Tool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageOrigin {
  Internal,
  #[default]
  Human,
  Model,
  Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Message {
  pub role: Role,
  pub content: String,
  #[serde(default)]
  pub origin: MessageOrigin,
  #[serde(default, skip_serializing_if = "String::is_empty")]
  pub reasoning_content: String,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub tool_calls: Vec<ToolCall>,
  #[serde(default, skip_serializing_if = "String::is_empty")]
  pub tool_call_id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub image_url: Option<String>,
}

impl Message {
  pub fn system(content: impl Into<String>) -> Self {
    Self {
      role: Role::System,
      content: content.into(),
      origin: MessageOrigin::Internal,
      ..Default::default()
    }
  }

  pub fn user(content: impl Into<String>, origin: MessageOrigin) -> Self {
    Self {
      role: Role::User,
      content: content.into(),
      origin,
      ..Default::default()
    }
  }

  pub fn assistant(resp: ChatResponse) -> Self {
    Self {
      role: Role::Assistant,
      content: resp.content,
      origin: MessageOrigin::Model,
      reasoning_content: resp.reasoning_content,
      tool_calls: resp.tool_calls,
      ..Default::default()
    }
  }

  pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
    Self {
      role: Role::Tool,
      content: content.into(),
      origin: MessageOrigin::Tool,
      tool_call_id: tool_call_id.into(),
      ..Default::default()
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ToolCall {
  #[serde(default)]
  pub id: String,
  #[serde(rename = "type", default)]
  pub kind: String,
  pub function: FunctionCall,
}

impl ToolCall {
  pub fn function(
    id: impl Into<String>,
    name: impl Into<String>,
    arguments: impl Into<String>,
  ) -> Self {
    Self {
      id: id.into(),
      kind: "function".to_string(),
      function: FunctionCall {
        name: name.into(),
        arguments: arguments.into(),
      },
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct FunctionCall {
  #[serde(default)]
  pub name: String,
  #[serde(default)]
  pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tool {
  #[serde(rename = "type")]
  pub kind: String,
  pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolFunction {
  pub name: String,
  pub description: String,
  pub parameters: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Usage {
  #[serde(default)]
  pub total_tokens: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatResponse {
  pub content: String,
  pub reasoning_content: String,
  pub tool_calls: Vec<ToolCall>,
  pub usage: Usage,
}
