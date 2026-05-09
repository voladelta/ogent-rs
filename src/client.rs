use serde_json::Value;
use std::sync::Arc;
use tokio::time::{Duration, sleep};

use crate::sse::parse_sse_response;
use crate::types::{ChatResponse, Message, Tool};

pub type BuildReq = Arc<dyn Fn(&[Message], &[Tool]) -> Value + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
  #[error("chat aborted by context cancellation")]
  Aborted { resp: ChatResponse },
  #[error("rate limited (429): {body}")]
  RateLimited { body: String },
  #[error("api error {status}: {body}")]
  ApiError { status: u16, body: String },
  #[error("http request failed")]
  Http(#[source] reqwest::Error),
  #[error("sse error")]
  Sse(#[from] crate::sse::SseError),
}

impl ClientError {
  pub fn is_retryable(&self) -> bool {
    match self {
      ClientError::ApiError { status, .. } => matches!(status, 429 | 500 | 502 | 503 | 504),
      ClientError::Http(e) => e.is_connect() || e.is_timeout(),
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
  max_retries: usize,
}

impl Client {
  pub fn new<F>(
    url: &str,
    api_key: String,
    max_retries: usize,
    build_req: F,
    request_timeout_secs: u64,
  ) -> Result<Self, ClientError>
  where
    F: Fn(&[Message], &[Tool]) -> Value + Send + Sync + 'static,
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
      max_retries,
    })
  }

  pub async fn chat(
    &self,
    messages: &[Message],
    tools: &[Tool],
    cancel: Option<&tokio_util::sync::CancellationToken>,
  ) -> Result<ChatResponse, ClientError> {
    let req_body = (self.build_req)(messages, tools);
    let mut last_err = None;
    for attempt in 0..=self.max_retries {
      if attempt > 0 {
        let delay_secs = 2u64.saturating_pow((attempt - 1) as u32).min(60);
        sleep(Duration::from_secs(delay_secs)).await;
      }
      match self.chat_once(&req_body, cancel).await {
        Ok(resp) => return Ok(resp),
        Err(err) if !err.is_retryable() => return Err(err),
        Err(err) => last_err = Some(err),
      }
    }
    Err(last_err.expect("at least one attempt"))
  }

  async fn chat_once(
    &self,
    req_body: &Value,
    cancel: Option<&tokio_util::sync::CancellationToken>,
  ) -> Result<ChatResponse, ClientError> {
    if cancel.is_some_and(tokio_util::sync::CancellationToken::is_cancelled) {
      return Err(ClientError::Aborted {
        resp: ChatResponse::default(),
      });
    }
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
    parse_sse_response(resp, cancel).await.map_err(Into::into)
  }
}
