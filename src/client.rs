use serde_json::Value;
use std::sync::Arc;
use tokio::time::{Duration, sleep};

use crate::sse::parse_sse_response;
use crate::types::{ChatResponse, Message, Tool};

const MAX_RETRIES: usize = 5;

type BuildReq = Arc<dyn Fn(&[Message], &[Tool]) -> Result<Value, serde_json::Error> + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
  #[error("rate limited (429): {body}")]
  RateLimited { body: String },
  #[error("api error {status}: {body}")]
  ApiError { status: u16, body: String },
  #[error("http request failed")]
  Http(#[source] reqwest::Error),
  #[error("failed to build request body")]
  BuildRequest(#[source] serde_json::Error),
  #[error("sse error")]
  Sse(#[from] crate::sse::SseError),
}

impl ClientError {
  pub fn is_retryable(&self) -> bool {
    match self {
      Self::ApiError { status, .. } => matches!(status, 500 | 502 | 503 | 504),
      Self::Http(e) => e.is_connect() || e.is_timeout(),
      Self::Sse(e) => e.is_retryable(),
      _ => false,
    }
  }
}

#[derive(Clone)]
pub struct Client {
  http: reqwest::Client,
  url: String,
  api_key: String,
  build_req: BuildReq,
}

impl Client {
  pub fn new<F>(
    url: &str,
    api_key: String,
    build_req: F,
    request_timeout_secs: u64,
  ) -> Result<Self, ClientError>
  where
    F: Fn(&[Message], &[Tool]) -> Result<Value, serde_json::Error> + Send + Sync + 'static,
  {
    Ok(Self {
      http: reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(request_timeout_secs))
        .pool_max_idle_per_host(128)
        .pool_idle_timeout(Duration::from_secs(30))
        .build()
        .map_err(ClientError::Http)?,
      url: url.to_string(),
      api_key,
      build_req: Arc::new(build_req),
    })
  }

  pub async fn chat(
    &self,
    messages: &[Message],
    tools: &[Tool],
    stream_tx: Option<tokio::sync::mpsc::Sender<crate::sse::StreamEvent>>,
  ) -> Result<ChatResponse, ClientError> {
    let req_body = (self.build_req)(messages, tools).map_err(ClientError::BuildRequest)?;
    let mut last_err = None;
    for attempt in 0..=MAX_RETRIES {
      if attempt > 0 {
        let delay_secs = 2u64.saturating_pow((attempt - 1) as u32).min(60);
        sleep(Duration::from_secs(delay_secs)).await;
      }
      match self.chat_once(&req_body, stream_tx.clone()).await {
        Ok(resp) => return Ok(resp),
        Err(err) if !err.is_retryable() => return Err(err),
        Err(err) => last_err = Some(err),
      }
    }
    Err(last_err.unwrap_or_else(|| ClientError::ApiError {
      status: 0,
      body: "retry loop exhausted without an error".to_string(),
    }))
  }

  async fn chat_once(
    &self,
    req_body: &Value,
    stream_tx: Option<tokio::sync::mpsc::Sender<crate::sse::StreamEvent>>,
  ) -> Result<ChatResponse, ClientError> {
    let resp = self
      .http
      .post(&self.url)
      .bearer_auth(&self.api_key)
      .json(req_body)
      .send()
      .await
      .map_err(ClientError::Http)?;
    if !resp.status().is_success() {
      let status = resp.status();
      let body = resp.text().await.unwrap_or_default();
      if status.as_u16() == 429 {
        return Err(ClientError::RateLimited {
          body: body.trim().to_string(),
        });
      }
      return Err(ClientError::ApiError {
        status: status.as_u16(),
        body: body.trim().to_string(),
      });
    }
    parse_sse_response(resp, stream_tx)
      .await
      .map_err(Into::into)
  }
}
