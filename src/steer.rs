use std::future::Future;
use std::pin::Pin;
use tokio::sync::mpsc::error::TryRecvError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SteerEvent {
  Message(String),
  Cancel,
  New,
  Exit(Option<String>),
  Profile(String),
  Compact(Option<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
  Idle,
  Reasoning,
  Replying,
  Working,
}

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

pub struct NoopSteer;

impl SteerChannel for NoopSteer {
  fn try_recv_event(&mut self) -> Result<SteerEvent, TryRecvError> {
    Err(TryRecvError::Empty)
  }

  fn recv_event(&mut self) -> Pin<Box<dyn Future<Output = Option<SteerEvent>> + Send + '_>> {
    Box::pin(std::future::pending())
  }

  fn set_state(&self, _state: AgentState) {}

  fn set_tokens(&self, _tokens: u64) {}

  fn set_profile(&self, _profile: String, _model: String) {}

  fn log_push(&self, _line: String) {}

  fn log_push_assistant_markdown(&self, _content: &str) {}

  fn log_clear(&self) {}

  fn log_start_stream(&self) {}

  fn log_append_stream_chunk(&self, _chunk: &str) {}

  fn log_append_reasoning_chunk(&self, _chunk: &str) {}

  fn log_end_stream(&self) {}
}

#[cfg(test)]
pub struct TestSteerHandle {
  rx: tokio::sync::mpsc::UnboundedReceiver<SteerEvent>,
}

#[cfg(test)]
impl TestSteerHandle {
  pub fn new() -> Self {
    let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
    Self { rx }
  }
}

#[cfg(test)]
impl SteerChannel for TestSteerHandle {
  fn try_recv_event(&mut self) -> Result<SteerEvent, TryRecvError> {
    self.rx.try_recv()
  }

  fn recv_event(&mut self) -> Pin<Box<dyn Future<Output = Option<SteerEvent>> + Send + '_>> {
    Box::pin(self.rx.recv())
  }

  fn set_state(&self, _state: AgentState) {}

  fn set_tokens(&self, _tokens: u64) {}

  fn set_profile(&self, _profile: String, _model: String) {}

  fn log_push(&self, _line: String) {}

  fn log_push_assistant_markdown(&self, _content: &str) {}

  fn log_clear(&self) {}

  fn log_start_stream(&self) {}

  fn log_append_stream_chunk(&self, _chunk: &str) {}

  fn log_append_reasoning_chunk(&self, _chunk: &str) {}

  fn log_end_stream(&self) {}
}
