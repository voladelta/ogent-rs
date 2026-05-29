use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::env;

use crate::client::Client;
use crate::config::{Profile, ProviderConfig};
use crate::types::{Message, Role, Tool, ToolCall};

#[derive(Serialize)]
struct DeepSeekRequest<'a> {
  model: &'a str,
  messages: Vec<ProviderMessage<'a>>,
  #[serde(skip_serializing_if = "<[Tool]>::is_empty")]
  tools: &'a [Tool],
  stream: bool,
  max_tokens: i32,
  thinking: DeepSeekThinking,
  reasoning_effort: &'a str,
}

#[derive(Serialize)]
struct DeepSeekThinking {
  #[serde(rename = "type")]
  kind: &'static str,
}

#[derive(Serialize)]
struct KimiRequest<'a> {
  model: &'a str,
  messages: Vec<ProviderMessage<'a>>,
  #[serde(skip_serializing_if = "<[Tool]>::is_empty")]
  tools: &'a [Tool],
  #[serde(skip_serializing_if = "Option::is_none")]
  tool_choice: Option<&'static str>,
  stream: bool,
  max_tokens: i32,
  chat_template_args: KimiThinking,
}

#[derive(Serialize)]
struct KimiThinking {
  enable_thinking: bool,
}

#[derive(Serialize)]
struct ZRequest<'a> {
  model: &'a str,
  messages: Vec<ProviderMessage<'a>>,
  #[serde(skip_serializing_if = "<[Tool]>::is_empty")]
  tools: &'a [Tool],
  stream: bool,
  max_tokens: i32,
  thinking: ZThinking,
}

#[derive(Serialize)]
struct Xaomi<'a> {
  model: &'a str,
  messages: Vec<ProviderMessage<'a>>,
  #[serde(skip_serializing_if = "<[Tool]>::is_empty")]
  tools: &'a [Tool],
  stream: bool,
  max_tokens: i32,
}

#[derive(Serialize)]
struct ZThinking {
  #[serde(rename = "type")]
  kind: &'static str,
  clear_thinking: bool,
}

#[derive(Serialize)]
struct ProviderMessage<'a> {
  role: &'a Role,
  content: &'a str,
  #[serde(skip_serializing_if = "str::is_empty")]
  reasoning_content: &'a str,
  #[serde(skip_serializing_if = "<[ToolCall]>::is_empty")]
  tool_calls: &'a [ToolCall],
  #[serde(skip_serializing_if = "str::is_empty")]
  tool_call_id: &'a str,
}

impl<'a> From<&'a Message> for ProviderMessage<'a> {
  fn from(value: &'a Message) -> Self {
    Self {
      role: &value.role,
      content: &value.content,
      reasoning_content: &value.reasoning_content,
      tool_calls: &value.tool_calls,
      tool_call_id: &value.tool_call_id,
    }
  }
}

pub fn new_client(profile: &Profile, provider: &ProviderConfig) -> Result<Client> {
  let url = provider.base_url.as_str();
  let key_env = provider.key_env.as_str();
  let api_key = env::var(key_env).with_context(|| format!("{key_env} is not set"))?;
  match profile.backend.as_str() {
    "kimi" => {
      let model = profile.model.clone();
      Ok(Client::new(
        url,
        api_key,
        move |messages, tools| {
          let provider_messages: Vec<_> = messages.iter().map(ProviderMessage::from).collect();
          serde_json::to_value(KimiRequest {
            model: &model,
            messages: provider_messages,
            tools,
            tool_choice: if tools.is_empty() { None } else { Some("auto") },
            stream: true,
            max_tokens: 262_144,
            chat_template_args: KimiThinking {
              enable_thinking: true,
            },
          })
        },
        600,
      )?)
    }
    "z" => {
      let model = profile.model.clone();
      Ok(Client::new(
        url,
        api_key,
        move |messages, tools| {
          let provider_messages: Vec<_> = messages.iter().map(ProviderMessage::from).collect();
          serde_json::to_value(ZRequest {
            model: &model,
            messages: provider_messages,
            tools,
            stream: true,
            max_tokens: 131_072,
            thinking: ZThinking {
              kind: "enabled",
              clear_thinking: false,
            },
          })
        },
        600,
      )?)
    }
    "xaomi" => {
      let model = profile.model.clone();
      Ok(Client::new(
        url,
        api_key,
        move |messages, tools| {
          let provider_messages: Vec<_> = messages.iter().map(ProviderMessage::from).collect();
          serde_json::to_value(Xaomi {
            model: &model,
            messages: provider_messages,
            tools,
            stream: true,
            max_tokens: 131_072,
          })
        },
        600,
      )?)
    }
    "deepseek" => {
      let model = profile.model.clone();
      let effort = profile.effort.clone();
      Ok(Client::new(
        url,
        api_key,
        move |messages, tools| {
          let provider_messages: Vec<_> = messages.iter().map(ProviderMessage::from).collect();
          serde_json::to_value(DeepSeekRequest {
            model: &model,
            messages: provider_messages,
            tools,
            stream: true,
            max_tokens: 393_216,
            thinking: DeepSeekThinking { kind: "enabled" },
            reasoning_effort: &effort,
          })
        },
        600,
      )?)
    }
    other => bail!("unknown backend: {other}"),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::types::{MessageOrigin, Role};

  #[test]
  fn provider_message_does_not_serialize_origin() {
    let message = Message::user("hello", MessageOrigin::Internal);
    let value = serde_json::to_value(ProviderMessage::from(&message)).unwrap();
    assert!(value.get("origin").is_none());
    assert_eq!(
      value.get("role").and_then(serde_json::Value::as_str),
      Some("user")
    );
    assert_eq!(
      value.get("content").and_then(serde_json::Value::as_str),
      Some("hello")
    );
  }

  #[test]
  fn kimi_request_omits_tool_choice_without_tools() {
    let tool_calls = Vec::new();
    let messages = vec![ProviderMessage {
      role: &Role::User,
      content: "hello",
      reasoning_content: "",
      tool_calls: &tool_calls,
      tool_call_id: "",
    }];
    let value = serde_json::to_value(KimiRequest {
      model: "test",
      messages,
      tools: &[],
      tool_choice: None,
      stream: false,
      max_tokens: 1,
      chat_template_args: KimiThinking {
        enable_thinking: true,
      },
    })
    .unwrap();
    assert!(value.get("tool_choice").is_none());
    assert!(value.get("tools").is_none());
  }
}
