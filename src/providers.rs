use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::env;

use crate::client::{Client, ClientConfig};
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
struct MiniMaxRequest<'a> {
  model: &'a str,
  messages: Vec<ProviderMessage<'a>>,
  #[serde(skip_serializing_if = "<[Tool]>::is_empty")]
  tools: &'a [Tool],
  stream: bool,
  max_tokens: i32,
  /// Splits reasoning tokens into a separate `reasoning_content` field
  /// instead of embedding them in the response content.
  reasoning_split: bool,
}

#[derive(Serialize)]
struct ZThinking {
  #[serde(rename = "type")]
  kind: &'static str,
  clear_thinking: bool,
}

#[derive(Debug, Clone)]
struct ProviderMessageContent<'a> {
  text: &'a str,
  image_url: Option<&'a str>,
}

impl<'a> serde::Serialize for ProviderMessageContent<'a> {
  fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
  where
    S: serde::Serializer,
  {
    if let Some(img_url) = self.image_url {
      #[derive(Serialize)]
      struct TextPart<'b> {
        #[serde(rename = "type")]
        kind: &'static str,
        text: &'b str,
      }
      #[derive(Serialize)]
      struct ImageUrlPart<'b> {
        url: &'b str,
      }
      #[derive(Serialize)]
      struct ImagePart<'b> {
        #[serde(rename = "type")]
        kind: &'static str,
        image_url: ImageUrlPart<'b>,
      }
      #[derive(Serialize)]
      #[serde(untagged)]
      enum Part<'b> {
        Text(TextPart<'b>),
        Image(ImagePart<'b>),
      }

      let parts = vec![
        Part::Text(TextPart {
          kind: "text",
          text: self.text,
        }),
        Part::Image(ImagePart {
          kind: "image_url",
          image_url: ImageUrlPart { url: img_url },
        }),
      ];
      parts.serialize(serializer)
    } else {
      self.text.serialize(serializer)
    }
  }
}

#[derive(Serialize)]
struct ProviderMessage<'a> {
  role: &'a Role,
  content: ProviderMessageContent<'a>,
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
      content: ProviderMessageContent {
        text: &value.content,
        image_url: value.image_url.as_deref(),
      },
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
      let max_tokens = profile.max_tokens;
      Ok(Client::new(
        ClientConfig {
          url: url.to_string(),
          api_key,
          request_timeout_secs: 600,
          require_sse_done: true,
        },
        move |messages, tools| {
          let provider_messages: Vec<_> = messages.iter().map(ProviderMessage::from).collect();
          serde_json::to_value(KimiRequest {
            model: &model,
            messages: provider_messages,
            tools,
            tool_choice: if tools.is_empty() { None } else { Some("auto") },
            stream: true,
            max_tokens,
            chat_template_args: KimiThinking {
              enable_thinking: true,
            },
          })
        },
      )?)
    }
    "z" => {
      let model = profile.model.clone();
      let max_tokens = profile.max_tokens;
      Ok(Client::new(
        ClientConfig {
          url: url.to_string(),
          api_key,
          request_timeout_secs: 600,
          require_sse_done: true,
        },
        move |messages, tools| {
          let provider_messages: Vec<_> = messages.iter().map(ProviderMessage::from).collect();
          serde_json::to_value(ZRequest {
            model: &model,
            messages: provider_messages,
            tools,
            stream: true,
            max_tokens,
            thinking: ZThinking {
              kind: "enabled",
              clear_thinking: false,
            },
          })
        },
      )?)
    }
    "xaomi" => {
      let model = profile.model.clone();
      let max_tokens = profile.max_tokens;
      Ok(Client::new(
        ClientConfig {
          url: url.to_string(),
          api_key,
          request_timeout_secs: 600,
          require_sse_done: true,
        },
        move |messages, tools| {
          let provider_messages: Vec<_> = messages.iter().map(ProviderMessage::from).collect();
          serde_json::to_value(Xaomi {
            model: &model,
            messages: provider_messages,
            tools,
            stream: true,
            max_tokens,
          })
        },
      )?)
    }
    "minimax" => {
      let model = profile.model.clone();
      let max_tokens = profile.max_tokens;
      Ok(Client::new(
        ClientConfig {
          url: url.to_string(),
          api_key,
          request_timeout_secs: 600,
          // MiniMax SSE streams end cleanly without a data: [DONE] sentinel.
          require_sse_done: false,
        },
        move |messages, tools| {
          let provider_messages: Vec<_> = messages.iter().map(ProviderMessage::from).collect();
          serde_json::to_value(MiniMaxRequest {
            model: &model,
            messages: provider_messages,
            tools,
            stream: true,
            max_tokens,
            reasoning_split: true,
          })
        },
      )?)
    }
    "deepseek" => {
      let model = profile.model.clone();
      let effort = profile.effort.clone();
      let max_tokens = profile.max_tokens;
      Ok(Client::new(
        ClientConfig {
          url: url.to_string(),
          api_key,
          request_timeout_secs: 600,
          require_sse_done: true,
        },
        move |messages, tools| {
          let provider_messages: Vec<_> = messages.iter().map(ProviderMessage::from).collect();
          serde_json::to_value(DeepSeekRequest {
            model: &model,
            messages: provider_messages,
            tools,
            stream: true,
            max_tokens,
            thinking: DeepSeekThinking { kind: "enabled" },
            reasoning_effort: &effort,
          })
        },
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
      content: ProviderMessageContent { text: "hello", image_url: None },
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

  #[test]
  fn minimax_request_includes_reasoning_split() {
    let tool_calls = Vec::new();
    let messages = vec![ProviderMessage {
      role: &Role::User,
      content: ProviderMessageContent { text: "hello", image_url: None },
      reasoning_content: "",
      tool_calls: &tool_calls,
      tool_call_id: "",
    }];
    let value = serde_json::to_value(MiniMaxRequest {
      model: "MiniMax-M3",
      messages,
      tools: &[],
      stream: false,
      max_tokens: 1,
      reasoning_split: true,
    })
    .unwrap();
    assert_eq!(
      value
        .get("reasoning_split")
        .and_then(serde_json::Value::as_bool),
      Some(true)
    );
  }

  #[test]
  fn minimax_request_omits_tools_when_empty() {
    let tool_calls = Vec::new();
    let messages = vec![ProviderMessage {
      role: &Role::User,
      content: ProviderMessageContent { text: "hello", image_url: None },
      reasoning_content: "",
      tool_calls: &tool_calls,
      tool_call_id: "",
    }];
    let value = serde_json::to_value(MiniMaxRequest {
      model: "MiniMax-M3",
      messages,
      tools: &[],
      stream: false,
      max_tokens: 1,
      reasoning_split: true,
    })
    .unwrap();
    assert!(value.get("tools").is_none());
  }

  #[test]
  fn provider_message_serializes_with_image() {
    let mut message = Message::user("describe this image", MessageOrigin::Human);
    message.image_url = Some("https://example.com/image.png".to_string());
    let value = serde_json::to_value(ProviderMessage::from(&message)).unwrap();

    let content = value.get("content").unwrap();
    assert!(content.is_array());
    let arr = content.as_array().unwrap();
    assert_eq!(arr.len(), 2);

    assert_eq!(arr[0].get("type").and_then(|t| t.as_str()), Some("text"));
    assert_eq!(arr[0].get("text").and_then(|t| t.as_str()), Some("describe this image"));

    assert_eq!(arr[1].get("type").and_then(|t| t.as_str()), Some("image_url"));
    let img_url_obj = arr[1].get("image_url").unwrap();
    assert_eq!(img_url_obj.get("url").and_then(|u| u.as_str()), Some("https://example.com/image.png"));
  }
}
