use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;
use tokio::time::Duration;

use crate::tools::{ToolContext, parse_args, require_nonempty};

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum SearchType {
  #[default]
  Auto,
  DeepReasoning,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum WebReadMode {
  #[default]
  Highlights,
  Text,
}

fn default_num_results() -> usize {
  10
}

#[derive(Deserialize)]
struct WebSearchArgs {
  query: String,
  #[serde(default = "default_num_results")]
  num_results: usize,
  #[serde(default, rename = "type")]
  search_type: SearchType,
}

pub async fn web_search(_ctx: ToolContext, args: &str) -> Result<String> {
  let args: WebSearchArgs = parse_args(args)?;
  require_nonempty(&args.query, "query")?;
  let n = args.num_results.clamp(1, 100);
  let search_type = match args.search_type {
    SearchType::Auto => "auto",
    SearchType::DeepReasoning => "deep-reasoning",
  };
  let body = json!({"query": args.query, "type": search_type, "numResults": n, "contents": {"highlights": true}});
  let v = exa_post("https://api.exa.ai/search", body).await?;
  let mut out = String::new();
  for (i, r) in v["results"].as_array().into_iter().flatten().enumerate() {
    writeln!(out, "{}. {}", i + 1, r["title"].as_str().unwrap_or(""))?;
    writeln!(out, "   {}", r["url"].as_str().unwrap_or(""))?;
    if let Some(highlights) = r["highlights"].as_array() {
      for h in highlights {
        writeln!(out, "   > {}", h.as_str().unwrap_or(""))?;
      }
    }
    out.push('\n');
  }
  Ok(out)
}

#[derive(Deserialize)]
struct WebReadArgs {
  urls: Vec<String>,
  #[serde(default)]
  mode: WebReadMode,
}

pub async fn web_read(_ctx: ToolContext, args: &str) -> Result<String> {
  let args: WebReadArgs = parse_args(args)?;
  if args.urls.is_empty() {
    bail!("urls is required");
  }
  let mode = args.mode;
  let body = match mode {
    WebReadMode::Text => json!({"urls": args.urls, "text": true}),
    WebReadMode::Highlights => json!({"urls": args.urls, "highlights": true}),
  };
  let v = exa_post("https://api.exa.ai/contents", body).await?;
  let mut out = String::new();
  for r in v["results"].as_array().into_iter().flatten() {
    writeln!(out, "--- {} ---", r["title"].as_str().unwrap_or(""))?;
    writeln!(out, "{}", r["url"].as_str().unwrap_or(""))?;
    out.push('\n');
    match mode {
      WebReadMode::Text => {
        out.push_str(r["text"].as_str().unwrap_or(""));
        out.push_str("\n\n");
      }
      WebReadMode::Highlights => {
        if let Some(highlights) = r["highlights"].as_array() {
          for h in highlights {
            writeln!(out, "> {}", h.as_str().unwrap_or(""))?;
          }
          out.push('\n');
        }
      }
    }
  }
  Ok(out)
}

#[derive(Deserialize)]
struct CodeWebContextArgs {
  query: String,
}

pub async fn web_code_context(_ctx: ToolContext, args: &str) -> Result<String> {
  let args: CodeWebContextArgs = parse_args(args)?;
  require_nonempty(&args.query, "query")?;
  let v = exa_post(
    "https://api.exa.ai/context",
    json!({"query": args.query, "tokensNum": "dynamic"}),
  )
  .await?;
  Ok(v["response"].as_str().unwrap_or("").to_string())
}

fn exa_client() -> &'static reqwest::Client {
  static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
  CLIENT.get_or_init(|| {
    reqwest::Client::builder()
      .timeout(Duration::from_secs(60))
      .build()
      .expect("build exa client")
  })
}

pub fn ensure_exa_api_key_set() -> Result<()> {
  let key = std::env::var("EXA_API_KEY").unwrap_or_default();
  if key.trim().is_empty() {
    bail!("EXA_API_KEY is not set. Set EXA_API_KEY before running ogent.");
  }
  Ok(())
}

async fn exa_post(url: &str, body: Value) -> Result<Value> {
  let key = std::env::var("EXA_API_KEY").unwrap_or_default();
  let resp = exa_client()
    .post(url)
    .header("x-api-key", key)
    .json(&body)
    .send()
    .await?;
  let status = resp.status();
  let text = resp.text().await?;
  if !status.is_success() {
    bail!("exa {}: {}", status.as_u16(), text.trim());
  }
  let v: Value = serde_json::from_str(&text).context("unmarshal exa response")?;
  if let Some(err) = v["error"].as_str().filter(|s| !s.is_empty()) {
    bail!("exa error: {err}");
  }
  Ok(v)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_web_search_args_default_num_results() {
    let args: WebSearchArgs = serde_json::from_str(r#"{"query": "hello"}"#).unwrap();
    assert_eq!(args.num_results, 10);
  }
}
