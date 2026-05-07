use anyhow::Result;
use crossterm::event::{
  self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
  MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
  DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
  enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
  Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use std::io;
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::mpsc;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SteerEvent {
  Message(String),
  Auto,
  Stop,
  Cancel,
  Complete,
  Exit,
}

#[derive(Clone, Default)]
pub struct UiLog {
  lines: Arc<Mutex<Vec<String>>>,
}

impl UiLog {
  pub fn push(&self, line: impl Into<String>) {
    self
      .lines
      .lock()
      .expect("ui log poisoned")
      .push(line.into());
  }

  pub fn push_assistant_markdown(&self, content: &str) {
    for line in content.lines() {
      self.push(format!("assistant: {line}"));
    }
    if content.ends_with('\n') {
      self.push("assistant:");
    }
  }

  fn snapshot(&self) -> Vec<String> {
    self.lines.lock().expect("ui log poisoned").clone()
  }
}

#[derive(Clone)]
pub struct UiStatus {
  inner: Arc<Mutex<StatusInner>>,
}

#[derive(Clone)]
struct StatusInner {
  profile: String,
  model: String,
  turn: i32,
  tokens: i32,
  auto: bool,
  input: String,
}

impl UiStatus {
  pub fn new(profile: String, model: String, auto: bool) -> Self {
    Self {
      inner: Arc::new(Mutex::new(StatusInner {
        profile,
        model,
        turn: 0,
        tokens: 0,
        auto,
        input: String::new(),
      })),
    }
  }

  pub fn set_turn_tokens(&self, turn: i32, tokens: i32) {
    let mut s = self.inner.lock().expect("ui status poisoned");
    s.turn = turn;
    s.tokens = tokens;
  }

  pub fn set_auto(&self, auto: bool) {
    self.inner.lock().expect("ui status poisoned").auto = auto;
  }

  fn set_input(&self, input: String) {
    self.inner.lock().expect("ui status poisoned").input = input;
  }

  fn snapshot(&self) -> StatusInner {
    self.inner.lock().expect("ui status poisoned").clone()
  }
}

pub struct TuiHandle {
  pub rx: mpsc::UnboundedReceiver<SteerEvent>,
  pub log: UiLog,
  pub status: UiStatus,
  stop: Arc<AtomicBool>,
  thread: Option<JoinHandle<()>>,
}

pub fn start(profile: String, model: String, auto: bool) -> Result<TuiHandle> {
  let (tx, rx) = mpsc::unbounded_channel();
  let log = UiLog::default();
  let status = UiStatus::new(profile, model, auto);
  let ui_log = log.clone();
  let ui_status = status.clone();
  let stop = Arc::new(AtomicBool::new(false));
  let ui_stop = stop.clone();
  let thread = std::thread::spawn(move || {
    if let Err(err) = run_ui(tx, ui_log.clone(), ui_status, ui_stop) {
      ui_log.push(format!("[tui] {err}"));
    }
  });
  Ok(TuiHandle {
    rx,
    log,
    status,
    stop,
    thread: Some(thread),
  })
}

impl Drop for TuiHandle {
  fn drop(&mut self) {
    self.close();
  }
}

impl TuiHandle {
  pub fn close(&mut self) {
    self.stop.store(true, Ordering::Relaxed);
    if let Some(thread) = self.thread.take() {
      let _ = thread.join();
    }
  }
}

fn run_ui(
  tx: mpsc::UnboundedSender<SteerEvent>,
  log: UiLog,
  status: UiStatus,
  stop: Arc<AtomicBool>,
) -> Result<()> {
  enable_raw_mode()?;
  let mut stdout = io::stdout();
  execute!(
    stdout,
    EnterAlternateScreen,
    EnableMouseCapture,
    DisableLineWrap
  )?;
  let backend = CrosstermBackend::new(stdout);
  let mut terminal = Terminal::new(backend)?;
  let result = run_ui_loop(&mut terminal, tx, log, status, stop);

  let _ = execute!(
    terminal.backend_mut(),
    DisableMouseCapture,
    EnableLineWrap,
    LeaveAlternateScreen
  )
  .and_then(|_| terminal.show_cursor());
  let _ = disable_raw_mode();

  result
}

fn run_ui_loop(
  terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
  tx: mpsc::UnboundedSender<SteerEvent>,
  log: UiLog,
  status: UiStatus,
  stop: Arc<AtomicBool>,
) -> Result<()> {
  let mut input = String::new();
  let mut scroll_y: usize = 0;
  let mut follow_bottom = true;

  while !stop.load(Ordering::Relaxed) {
    let (log_width, log_height) = draw(terminal, &log, &status, &mut scroll_y, follow_bottom)?;
    if event::poll(Duration::from_millis(100))? {
      match event::read()? {
        Event::Key(key) => {
          if key.kind != KeyEventKind::Press {
            continue;
          }
          match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
              let _ = tx.send(SteerEvent::Exit);
              break;
            }
            KeyCode::Char(c) => {
              input.push(c);
              follow_bottom = true;
            }
            KeyCode::Backspace => {
              input.pop();
            }
            KeyCode::Enter => {
              let line = input.trim().to_string();
              input.clear();
              status.set_input(String::new());
              follow_bottom = true;
              let event = parse_steer_event(&line);
              let exit = matches!(event, SteerEvent::Exit);
              let _ = tx.send(event);
              if exit {
                break;
              }
            }
            KeyCode::Esc => {
              let _ = tx.send(SteerEvent::Exit);
              break;
            }
            KeyCode::Up => {
              follow_bottom = false;
              scroll_y = scroll_y.saturating_sub(1);
            }
            KeyCode::Down => {
              scroll_y = scroll_y.saturating_add(1);
              if scroll_y >= max_scroll(&log, log_width, log_height) {
                follow_bottom = true;
              }
            }
            KeyCode::PageUp => {
              follow_bottom = false;
              scroll_y = scroll_y.saturating_sub(log_height as usize);
            }
            KeyCode::PageDown => {
              scroll_y = scroll_y.saturating_add(log_height as usize);
              if scroll_y >= max_scroll(&log, log_width, log_height) {
                follow_bottom = true;
              }
            }
            KeyCode::Home => {
              follow_bottom = false;
              scroll_y = 0;
            }
            KeyCode::End => {
              follow_bottom = true;
            }
            _ => {}
          }
          status.set_input(input.clone());
        }
        Event::Mouse(mouse) => match mouse.kind {
          MouseEventKind::ScrollUp => {
            follow_bottom = false;
            scroll_y = scroll_y.saturating_sub(3);
          }
          MouseEventKind::ScrollDown => {
            scroll_y = scroll_y.saturating_add(3);
            if scroll_y >= max_scroll(&log, log_width, log_height) {
              follow_bottom = true;
            }
          }
          _ => {}
        },
        _ => {}
      }
    }
  }

  Ok(())
}

pub fn parse_steer_event(line: &str) -> SteerEvent {
  match line.trim() {
    "/auto" => SteerEvent::Auto,
    "/stop" => SteerEvent::Stop,
    "/cancel" => SteerEvent::Cancel,
    "/complete" => SteerEvent::Complete,
    "/q" | "/quit" | "quit" | "exit" => SteerEvent::Exit,
    other => SteerEvent::Message(other.to_string()),
  }
}

fn draw(
  terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
  log: &UiLog,
  status: &UiStatus,
  scroll_y: &mut usize,
  follow_bottom: bool,
) -> Result<(u16, u16)> {
  let status_snapshot = status.snapshot();
  let log_lines = log.snapshot();
  let mut dims: Option<(u16, u16)> = None;
  terminal.draw(|frame| {
    frame.render_widget(Clear, frame.area());
    let chunks = Layout::default()
      .direction(Direction::Vertical)
      .constraints([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(3),
      ])
      .split(frame.area());

    let auto = if status_snapshot.auto {
      "auto on"
    } else {
      "auto off"
    };
    let mut bar = format!(
      "{} | {} | turn {} | tokens {} | {}",
      status_snapshot.profile,
      status_snapshot.model,
      status_snapshot.turn,
      status_snapshot.tokens,
      auto
    );
    truncate_to_width(&mut bar, chunks[0].width.saturating_sub(1) as usize);
    frame.render_widget(Paragraph::new(bar), chunks[0]);

    let log_width = chunks[1].width.saturating_sub(2);
    let log_height = chunks[1].height.saturating_sub(2);
    dims = Some((log_width, log_height));

    let lines: Vec<Line> = log_lines.iter().map(|line| render_log_line(line)).collect();
    let paragraph = Paragraph::new(lines)
      .wrap(Wrap { trim: true })
      .block(Block::default().borders(Borders::ALL).title("log"));
    let total_wrapped = paragraph.line_count(log_width.max(1));
    let max_scroll = total_wrapped.saturating_sub(log_height as usize);
    let y = if follow_bottom {
      max_scroll
    } else {
      (*scroll_y).min(max_scroll)
    };
    *scroll_y = y;
    frame.render_widget(paragraph.scroll((y as u16, 0)), chunks[1]);

    let mut scrollbar_state = ScrollbarState::new(total_wrapped).position(y);
    frame.render_stateful_widget(
      Scrollbar::new(ScrollbarOrientation::VerticalRight),
      chunks[1].inner(Margin {
        vertical: 1,
        horizontal: 0,
      }),
      &mut scrollbar_state,
    );

    let input_width = chunks[2].width.saturating_sub(3) as usize;
    let input = tail_cells(&status_snapshot.input, input_width);
    frame.render_widget(
      Paragraph::new(input).block(
        Block::default()
          .borders(Borders::ALL)
          .title("message or command"),
      ),
      chunks[2],
    );
  })?;
  Ok(dims.unwrap_or((0, 0)))
}

fn max_scroll(log: &UiLog, log_width: u16, log_height: u16) -> usize {
  let lines: Vec<Line> = log
    .snapshot()
    .iter()
    .map(|line| render_log_line(line))
    .collect();
  let paragraph = Paragraph::new(lines).wrap(Wrap { trim: true });
  let total_wrapped = paragraph.line_count(log_width.max(1));
  total_wrapped.saturating_sub(log_height as usize)
}

fn truncate_to_width(value: &mut String, width: usize) {
  if UnicodeWidthStr::width(value.as_str()) > width {
    *value = head_cells(value, width);
  }
}

fn tail_cells(value: &str, width: usize) -> String {
  let mut out = Vec::new();
  let mut used = 0usize;
  for c in value.chars().rev() {
    let c_width = c.width().unwrap_or(0);
    if used + c_width > width {
      break;
    }
    out.push(c);
    used += c_width;
  }
  out.into_iter().rev().collect()
}

fn head_cells(value: &str, width: usize) -> String {
  let mut out = String::new();
  let mut used = 0usize;
  for c in value.chars() {
    let c_width = c.width().unwrap_or(0);
    if used + c_width > width {
      break;
    }
    out.push(c);
    used += c_width;
  }
  out
}

fn render_log_line(value: &str) -> Line<'static> {
  if let Some(markdown) = value.strip_prefix("assistant: ") {
    let mut spans = vec![Span::styled(
      "assistant: ",
      Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD),
    )];
    spans.extend(markdown_spans(markdown));
    return Line::from(spans);
  }
  if value == "assistant:" {
    return Line::from(vec![Span::styled(
      "assistant:",
      Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD),
    )]);
  }
  Line::from(Span::raw(value.to_string()))
}

fn markdown_spans(value: &str) -> Vec<Span<'static>> {
  let trimmed = value.trim_start();
  if trimmed.starts_with("### ") || trimmed.starts_with("## ") || trimmed.starts_with("# ") {
    return vec![Span::styled(
      trimmed.trim_start_matches('#').trim_start().to_string(),
      Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD),
    )];
  }
  if let Some(rest) = trimmed
    .strip_prefix("- ")
    .or_else(|| trimmed.strip_prefix("* "))
  {
    let indent = value.len() - trimmed.len();
    let mut spans = vec![
      Span::raw(" ".repeat(indent)),
      Span::styled("• ", Style::default().fg(Color::Yellow)),
    ];
    spans.extend(inline_markdown_spans(rest));
    return spans;
  }
  inline_markdown_spans(value)
}

fn inline_markdown_spans(value: &str) -> Vec<Span<'static>> {
  let mut spans = Vec::new();
  let mut rest = value;
  let mut style = Style::default();
  while !rest.is_empty() {
    if let Some(next) = rest.find(['`', '*']) {
      if next > 0 {
        spans.push(Span::styled(rest[..next].to_string(), style));
        rest = &rest[next..];
        continue;
      }
      if let Some(after_tick) = rest.strip_prefix('`') {
        if let Some(end) = after_tick.find('`') {
          spans.push(Span::styled(
            after_tick[..end].to_string(),
            Style::default().fg(Color::Green),
          ));
          rest = &after_tick[end + 1..];
          continue;
        }
      }
      if let Some(after_bold) = rest.strip_prefix("**") {
        style = if style.add_modifier.contains(Modifier::BOLD) {
          style.remove_modifier(Modifier::BOLD)
        } else {
          style.add_modifier(Modifier::BOLD)
        };
        rest = after_bold;
        continue;
      }
      if let Some(after_italic) = rest.strip_prefix('*') {
        style = if style.add_modifier.contains(Modifier::ITALIC) {
          style.remove_modifier(Modifier::ITALIC)
        } else {
          style.add_modifier(Modifier::ITALIC)
        };
        rest = after_italic;
        continue;
      }
      spans.push(Span::styled(rest[..1].to_string(), style));
      rest = &rest[1..];
    } else {
      spans.push(Span::styled(rest.to_string(), style));
      break;
    }
  }
  spans
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_steer_commands() {
    assert_eq!(parse_steer_event("/auto"), SteerEvent::Auto);
    assert_eq!(parse_steer_event("/stop"), SteerEvent::Stop);
    assert_eq!(parse_steer_event("/cancel"), SteerEvent::Cancel);
    assert_eq!(parse_steer_event("/complete"), SteerEvent::Complete);
    assert_eq!(parse_steer_event("/q"), SteerEvent::Exit);
    assert_eq!(parse_steer_event("/quit"), SteerEvent::Exit);
    assert_eq!(parse_steer_event("quit"), SteerEvent::Exit);
    assert_eq!(parse_steer_event("exit"), SteerEvent::Exit);
    assert_eq!(parse_steer_event("hi"), SteerEvent::Message("hi".into()));
  }
}
