use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::env;

use crate::client::Client;
use crate::profiles::Profile;
use crate::types::{Message, Tool, ToolCall};

const DEEPSEEK_URL: &str = "https://api.deepseek.com/chat/completions";
const KIMI_URL: &str = "https://inference.baseten.co/v1/chat/completions";
const Z_URL: &str = "https://api.z.ai/api/coding/paas/v4/chat/completions";

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
struct ZThinking {
  #[serde(rename = "type")]
  kind: &'static str,
  clear_thinking: bool,
}

#[derive(Serialize)]
struct ProviderMessage<'a> {
  role: &'a str,
  content: &'a str,
  #[serde(skip_serializing_if = "str::is_empty")]
  reasoning_content: &'a str,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  tool_calls: &'a Vec<ToolCall>,
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

pub fn new_client(profile: &Profile) -> Result<Client> {
  match profile.backend {
    "kimi" => {
      let model = profile.model;
      make_client(
        KIMI_URL,
        "BASETEN_API_KEY",
        move |messages, tools| {
          let provider_messages: Vec<_> = messages.iter().map(ProviderMessage::from).collect();
          serde_json::to_value(KimiRequest {
            model,
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
      )
    }
    "z" => {
      let model = profile.model;
      make_client(
        Z_URL,
        "Z_API_KEY",
        move |messages, tools| {
          let provider_messages: Vec<_> = messages.iter().map(ProviderMessage::from).collect();
          serde_json::to_value(ZRequest {
            model,
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
      )
    }
    "deepseek" => {
      let model = profile.model;
      let effort = profile.effort;
      make_client(
        DEEPSEEK_URL,
        "DEEPSEEK_API_KEY",
        move |messages, tools| {
          let provider_messages: Vec<_> = messages.iter().map(ProviderMessage::from).collect();
          serde_json::to_value(DeepSeekRequest {
            model,
            messages: provider_messages,
            tools,
            stream: true,
            max_tokens: 393_216,
            thinking: DeepSeekThinking { kind: "enabled" },
            reasoning_effort: effort,
          })
        },
        600,
      )
    }
    other => bail!("unknown backend: {other}"),
  }
}

fn make_client<F>(url: &str, key_env: &str, build: F, timeout_secs: u64) -> Result<Client>
where
  F:
    Fn(&[Message], &[Tool]) -> Result<serde_json::Value, serde_json::Error> + Send + Sync + 'static,
{
  Ok(Client::new(url, env_key(key_env)?, build, timeout_secs)?)
}

fn env_key(name: &str) -> Result<String> {
  env::var(name).with_context(|| format!("{name} is not set"))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::types::MessageOrigin;

  #[test]
  fn provider_message_does_not_serialize_origin() {
    let message = Message {
      role: "user".into(),
      content: "hello".into(),
      origin: MessageOrigin::Internal,
      ..Default::default()
    };
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
      role: "user",
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
