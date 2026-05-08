use anyhow::{Result, bail};
use serde::Serialize;
use std::env;

use crate::client::Client;
use crate::profiles::Profile;
use crate::types::{Message, Tool};

const DEEPSEEK_URL: &str = "https://api.deepseek.com/chat/completions";
const KIMI_URL: &str = "https://inference.baseten.co/v1/chat/completions";
const Z_URL: &str = "https://api.z.ai/api/coding/paas/v4/chat/completions";

#[derive(Serialize)]
struct DeepSeekRequest<'a> {
  model: &'a str,
  messages: &'a [Message],
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
  messages: &'a [Message],
  #[serde(skip_serializing_if = "<[Tool]>::is_empty")]
  tools: &'a [Tool],
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
  messages: &'a [Message],
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

pub fn new_client(profile: &Profile, max_retries: usize) -> Result<Client> {
  match profile.backend {
    "kimi" => {
      let key = env_key("BASETEN_API_KEY")?;
      let model = profile.model;
      Ok(Client::new(
        KIMI_URL,
        key,
        max_retries,
        move |messages, tools| {
          serde_json::to_value(KimiRequest {
            model,
            messages,
            tools,
            stream: true,
            max_tokens: 262_144,
            chat_template_args: KimiThinking {
              enable_thinking: true,
            },
          })
          .expect("serialize request")
        },
      ))
    }
    "z" => {
      let key = env_key("Z_API_KEY")?;
      let model = profile.model;
      Ok(Client::new(
        Z_URL,
        key,
        max_retries,
        move |messages, tools| {
          serde_json::to_value(ZRequest {
            model,
            messages,
            tools,
            stream: true,
            max_tokens: 131_072,
            thinking: ZThinking {
              kind: "enabled",
              clear_thinking: false,
            },
          })
          .expect("serialize request")
        },
      ))
    }
    "deepseek" => {
      let key = env_key("DEEPSEEK_API_KEY")?;
      let model = profile.model;
      let effort = profile.effort;
      Ok(Client::new(
        DEEPSEEK_URL,
        key,
        max_retries,
        move |messages, tools| {
          serde_json::to_value(DeepSeekRequest {
            model,
            messages,
            tools,
            stream: true,
            max_tokens: 393_216,
            thinking: DeepSeekThinking { kind: "enabled" },
            reasoning_effort: effort,
          })
          .expect("serialize request")
        },
      ))
    }
    other => bail!("unknown backend: {other}"),
  }
}

fn env_key(name: &str) -> Result<String> {
  let value = env::var(name).unwrap_or_default();
  if value.is_empty() {
    bail!("{name} is not set");
  }
  Ok(value)
}
