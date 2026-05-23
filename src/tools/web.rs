use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::Duration;

use crate::tools::{Handler, ToolContext, ToolDef, parse_args, require_nonempty};

pub fn tools() -> Vec<ToolDef> {
  vec![
    ToolDef {
      name: "web_search",
      description: "Search the web for relevant excerpts. Use type=auto for quick facts and deep-reasoning for complex or niche topics.",
      parameters: json!({"type":"object","properties":{"query":{"type":"string"},"num_results":{"type":"integer"},"type":{"type":"string","enum":["auto","deep-reasoning"]}},"required":["query"],"additionalProperties":false}),
      handler: Handler::Async(Box::new(|ctx, args| {
        let args = args.to_owned();
        Box::pin(async move { web_search(ctx, &args).await })
      })),
    },
    ToolDef {
      name: "web_read",
      description: "Read key excerpts from one or more URLs. Set mode=text for full text or highlights for key excerpts.",
      parameters: json!({"type":"object","properties":{"urls":{"type":"array","items":{"type":"string"}},"mode":{"type":"string","enum":["text","highlights"],"description":"text for full page text, highlights for key excerpts. Default: highlights."}},"required":["urls"],"additionalProperties":false}),
      handler: Handler::Async(Box::new(|ctx, args| {
        let args = args.to_owned();
        Box::pin(async move { web_read(ctx, &args).await })
      })),
    },
    ToolDef {
      name: "web_code_context",
      description: "Search real code for syntax, APIs, and patterns to avoid hallucinating implementation details. Not for general web search or URL reading.",
      parameters: json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}),
      handler: Handler::Async(Box::new(|ctx, args| {
        let args = args.to_owned();
        Box::pin(async move { web_code_context(ctx, &args).await })
      })),
    },
  ]
}

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

#[derive(Deserialize)]
struct WebSearchArgs {
  query: String,
  #[serde(default)]
  num_results: usize,
  #[serde(default, rename = "type")]
  search_type: SearchType,
}

async fn web_search(_ctx: ToolContext, args: &str) -> Result<String> {
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
    out.push_str(&format!(
      "{}. {}\n",
      i + 1,
      r["title"].as_str().unwrap_or("")
    ));
    out.push_str(&format!("   {}\n", r["url"].as_str().unwrap_or("")));
    if let Some(highlights) = r["highlights"].as_array() {
      for h in highlights {
        out.push_str(&format!("   > {}\n", h.as_str().unwrap_or("")));
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

async fn web_read(_ctx: ToolContext, args: &str) -> Result<String> {
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
    out.push_str(&format!("--- {} ---\n", r["title"].as_str().unwrap_or("")));
    out.push_str(&format!("{}\n", r["url"].as_str().unwrap_or("")));
    out.push('\n');
    match mode {
      WebReadMode::Text => {
        out.push_str(r["text"].as_str().unwrap_or(""));
        out.push_str("\n\n");
      }
      WebReadMode::Highlights => {
        if let Some(highlights) = r["highlights"].as_array() {
          for h in highlights {
            out.push_str(&format!("> {}\n", h.as_str().unwrap_or("")));
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

async fn web_code_context(_ctx: ToolContext, args: &str) -> Result<String> {
  let args: CodeWebContextArgs = parse_args(args)?;
  require_nonempty(&args.query, "query")?;
  let v = exa_post(
    "https://api.exa.ai/context",
    json!({"query": args.query, "tokensNum": "dynamic"}),
  )
  .await?;
  Ok(v["response"].as_str().unwrap_or("").to_string())
}

fn exa_client() -> Result<&'static reqwest::Client> {
  static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
  if let Some(client) = CLIENT.get() {
    return Ok(client);
  }
  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(60))
    .build()
    .context("build exa client")?;
  Ok(CLIENT.get_or_init(|| client))
}

fn exa_api_key() -> String {
  std::env::var("EXA_API_KEY").unwrap_or_default()
}

pub fn ensure_exa_api_key_set() -> Result<()> {
  let key = std::env::var("EXA_API_KEY").unwrap_or_default();
  if key.trim().is_empty() {
    bail!("EXA_API_KEY is not set. Set EXA_API_KEY before running ogent.");
  }
  Ok(())
}

async fn exa_post(url: &str, body: Value) -> Result<Value> {
  let key = exa_api_key();
  let resp = exa_client()?
    .post(url)
    .header("x-api-key", key)
    .json(&body)
    .send()
    .await?;
  let status = resp.status();
  let text = resp.text().await?;
  if !status.is_success() {
    eprintln!("exa request failed: {} {}", status.as_u16(), text.trim());
    bail!("exa {}: {}", status.as_u16(), text.trim());
  }
  let v: Value = serde_json::from_str(&text).context("unmarshal exa response")?;
  if let Some(err) = v["error"].as_str().filter(|s| !s.is_empty()) {
    eprintln!("exa returned error: {err}");
    bail!("exa error: {err}");
  }
  Ok(v)
}
