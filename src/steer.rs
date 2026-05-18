use crate::tui::{AgentState, SteerEvent, TuiHandle};
use std::future::Future;
use std::pin::Pin;
use tokio::sync::mpsc::error::TryRecvError;

pub trait SteerChannel {
  fn try_recv_event(&mut self) -> Result<SteerEvent, TryRecvError>;
  fn recv_event(&mut self) -> Pin<Box<dyn Future<Output = Option<SteerEvent>> + Send + '_>>;
  fn set_state(&self, state: AgentState);
  fn set_tokens(&self, tokens: u64);
  fn set_profile(&self, profile: String, model: String);
  fn log_push(&self, line: String);
  fn log_push_assistant_markdown(&self, content: &str);
  fn log_clear(&self);
  fn log_start_stream(&self);
  fn log_append_stream_chunk(&self, chunk: &str);
  fn log_append_reasoning_chunk(&self, chunk: &str);
  fn log_end_stream(&self);
}

impl SteerChannel for TuiHandle {
  fn try_recv_event(&mut self) -> Result<SteerEvent, TryRecvError> {
    self.rx.try_recv()
  }

  fn recv_event(&mut self) -> Pin<Box<dyn Future<Output = Option<SteerEvent>> + Send + '_>> {
    Box::pin(self.rx.recv())
  }

  fn set_state(&self, state: AgentState) {
    self.status.set_state(state);
  }

  fn set_tokens(&self, tokens: u64) {
    self.status.set_tokens(tokens);
  }

  fn set_profile(&self, profile: String, model: String) {
    self.status.set_profile(profile, model);
  }

  fn log_push(&self, line: String) {
    self.log.push(line);
  }

  fn log_push_assistant_markdown(&self, content: &str) {
    self.log.push_assistant_markdown(content);
  }

  fn log_clear(&self) {
    self.log.clear();
  }

  fn log_start_stream(&self) {
    self.log.start_stream();
  }

  fn log_append_stream_chunk(&self, chunk: &str) {
    self.log.append_stream_chunk(chunk);
  }

  fn log_append_reasoning_chunk(&self, chunk: &str) {
    self.log.append_reasoning_chunk(chunk);
  }

  fn log_end_stream(&self) {
    self.log.end_stream();
  }
}
