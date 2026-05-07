use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Message {
  pub role: String,
  pub content: String,
  #[serde(default, skip_serializing_if = "String::is_empty")]
  pub reasoning_content: String,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub tool_calls: Vec<ToolCall>,
  #[serde(default, skip_serializing_if = "String::is_empty")]
  pub tool_call_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ToolCall {
  #[serde(default)]
  pub id: String,
  #[serde(rename = "type", default)]
  pub kind: String,
  pub function: FunctionCall,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolFunction {
  pub name: String,
  pub description: String,
  pub parameters: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Usage {
  #[serde(default)]
  pub prompt_tokens: i32,
  #[serde(default)]
  pub completion_tokens: i32,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatAbortedError {
  pub resp: ChatResponse,
}

impl std::fmt::Display for ChatAbortedError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "chat aborted by context cancellation")
  }
}

impl std::error::Error for ChatAbortedError {}
