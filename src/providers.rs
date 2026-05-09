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
  tool_choice: &'static str,
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
      let model = profile.model;
      make_client(
        KIMI_URL,
        "BASETEN_API_KEY",
        max_retries,
        move |messages, tools| {
          serde_json::to_value(KimiRequest {
            model,
            messages,
            tools,
            tool_choice: "auto",
            stream: true,
            max_tokens: 262_144,
            chat_template_args: KimiThinking {
              enable_thinking: true,
            },
          })
          .expect("serialize request")
        },
      )
    }
    "z" => {
      let model = profile.model;
      make_client(Z_URL, "Z_API_KEY", max_retries, move |messages, tools| {
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
      })
    }
    "deepseek" => {
      let model = profile.model;
      let effort = profile.effort;
      make_client(
        DEEPSEEK_URL,
        "DEEPSEEK_API_KEY",
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
      )
    }
    other => bail!("unknown backend: {other}"),
  }
}

fn make_client<F>(url: &str, key_env: &str, max_retries: usize, build: F) -> Result<Client>
where
  F: Fn(&[Message], &[Tool]) -> serde_json::Value + Send + Sync + 'static,
{
  Ok(Client::new(url, env_key(key_env)?, max_retries, build)?)
}

fn env_key(name: &str) -> Result<String> {
  let value = env::var(name).unwrap_or_default();
  if value.is_empty() {
    bail!("{name} is not set");
  }
  Ok(value)
}
