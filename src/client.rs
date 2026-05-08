use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::sync::Arc;
use tokio::time::{Duration, sleep};

use crate::sse::parse_sse_response;
use crate::types::{ChatResponse, Message, Tool};

pub type BuildReq = Arc<dyn Fn(&[Message], &[Tool]) -> Value + Send + Sync>;

#[derive(Clone)]
pub struct Client {
  http: reqwest::Client,
  url: String,
  api_key: String,
  build_req: BuildReq,
  max_retries: usize,
}

impl Client {
  pub fn new<F>(url: &str, api_key: String, max_retries: usize, build_req: F) -> Self
  where
    F: Fn(&[Message], &[Tool]) -> Value + Send + Sync + 'static,
  {
    Self {
      http: reqwest::Client::new(),
      url: url.to_string(),
      api_key,
      build_req: Arc::new(build_req),
      max_retries,
    }
  }

  pub async fn chat(
    &self,
    messages: &[Message],
    tools: &[Tool],
    cancel: Option<&tokio_util::sync::CancellationToken>,
  ) -> Result<ChatResponse> {
    let req_body = (self.build_req)(messages, tools);
    let mut last_err = None;
    for attempt in 0..=self.max_retries {
      if attempt > 0 {
        sleep(Duration::from_secs(attempt as u64)).await;
      }
      match self.chat_once(&req_body, cancel).await {
        Ok(resp) => return Ok(resp),
        Err(err) if err.to_string().starts_with("rate limited (429)") => return Err(err),
        Err(err) => last_err = Some(err),
      }
    }
    Err(last_err.expect("at least one attempt"))
  }

  async fn chat_once(
    &self,
    req_body: &Value,
    cancel: Option<&tokio_util::sync::CancellationToken>,
  ) -> Result<ChatResponse> {
    if cancel.map(|c| c.is_cancelled()).unwrap_or(false) {
      return Err(
        crate::types::ChatAbortedError {
          resp: ChatResponse::default(),
        }
        .into(),
      );
    }
    let resp = self
      .http
      .post(&self.url)
      .bearer_auth(&self.api_key)
      .json(req_body)
      .send()
      .await
      .context("http")?;
    if !resp.status().is_success() {
      let status = resp.status();
      let body = resp.text().await.unwrap_or_default();
      if status.as_u16() == 429 {
        bail!("rate limited (429): {}", body.trim());
      }
      bail!("api {}: {}", status.as_u16(), body.trim());
    }
    parse_sse_response(resp, cancel).await
  }
}
