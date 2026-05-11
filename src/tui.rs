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
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
  Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui_textarea::{CursorMove, TextArea, WrapMode};
use std::io;
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
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
  New,
  Fork,
  Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
  Idle,
  Reasoning,
  Replying,
  Working,
}

#[derive(Clone)]
pub struct UiLog {
  lines: Arc<Mutex<Vec<String>>>,
  len: Arc<AtomicUsize>,
  generation: Arc<AtomicU64>,
  streaming: Arc<AtomicBool>,
}

impl Default for UiLog {
  fn default() -> Self {
    Self {
      lines: Arc::new(Mutex::new(Vec::new())),
      len: Arc::new(AtomicUsize::new(0)),
      generation: Arc::new(AtomicU64::new(0)),
      streaming: Arc::new(AtomicBool::new(false)),
    }
  }
}

impl UiLog {
  pub fn push(&self, line: impl Into<String>) {
    self
      .lines
      .lock()
      .expect("ui log poisoned")
      .push(line.into());
    self.len.fetch_add(1, Ordering::Relaxed);
    self.generation.fetch_add(1, Ordering::Relaxed);
  }

  pub fn push_assistant_markdown(&self, content: &str) {
    for line in content.lines() {
      self.push(format!("ogent: {line}"));
    }
    if content.ends_with('\n') {
      self.push("ogent:");
    }
  }

  pub fn clear(&self) {
    self.lines.lock().expect("ui log poisoned").clear();
    self.len.store(0, Ordering::Relaxed);
    self.generation.fetch_add(1, Ordering::Relaxed);
  }

  pub fn start_stream(&self) {
    let mut lines = self.lines.lock().expect("ui log poisoned");
    lines.push("ogent: ".to_string());
    self.len.fetch_add(1, Ordering::Relaxed);
    drop(lines);
    self.streaming.store(true, Ordering::Relaxed);
  }

  fn append_chunk_prefixed(&self, chunk: &str, prefix: &str) {
    if !self.streaming.load(Ordering::Relaxed) {
      return;
    }
    let mut lines = self.lines.lock().expect("ui log poisoned");
    let last = lines.pop();
    let mut current = match last {
      Some(l) if l.starts_with(prefix) => l,
      Some(other) => {
        lines.push(other);
        prefix.to_string()
      }
      None => prefix.to_string(),
    };
    for (i, part) in chunk.split('\n').enumerate() {
      if i > 0 {
        lines.push(current);
        current = prefix.to_string();
      }
      current.push_str(part);
    }
    lines.push(current);
    self.len.store(lines.len(), Ordering::Relaxed);
    drop(lines);
    self.generation.fetch_add(1, Ordering::Relaxed);
  }

  pub fn append_stream_chunk(&self, chunk: &str) {
    self.append_chunk_prefixed(chunk, "ogent: ");
  }

  pub fn append_reasoning_chunk(&self, chunk: &str) {
    self.append_chunk_prefixed(chunk, "reasoning: ");
  }

  pub fn end_stream(&self) {
    self.streaming.store(false, Ordering::Relaxed);
  }

  fn snapshot(&self) -> Vec<String> {
    self.lines.lock().expect("ui log poisoned").clone()
  }

  pub(crate) fn generation(&self) -> u64 {
    self.generation.load(Ordering::Relaxed)
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
  state: AgentState,
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
        state: AgentState::Idle,
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

  pub fn set_state(&self, state: AgentState) {
    self.inner.lock().expect("ui status poisoned").state = state;
  }

  pub fn state(&self) -> AgentState {
    self.inner.lock().expect("ui status poisoned").state
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

  #[cfg(test)]
  pub fn test_handle() -> Self {
    let (_tx, rx) = mpsc::unbounded_channel();
    Self {
      rx,
      log: UiLog::default(),
      status: UiStatus::new("test".into(), "test".into(), false),
      stop: Arc::new(AtomicBool::new(false)),
      thread: None,
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
  terminal.hide_cursor()?;
  let result = run_ui_loop(&mut terminal, tx, log, status, stop);

  let restore = execute!(
    terminal.backend_mut(),
    DisableMouseCapture,
    EnableLineWrap,
    LeaveAlternateScreen
  )
  .and_then(|()| terminal.show_cursor())
  .and_then(|()| disable_raw_mode());

  result.or(restore.map_err(Into::into))
}

fn run_ui_loop(
  terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
  tx: mpsc::UnboundedSender<SteerEvent>,
  log: UiLog,
  status: UiStatus,
  stop: Arc<AtomicBool>,
) -> Result<()> {
  let mut textarea = TextArea::default();
  textarea.set_block(
    Block::default()
      .borders(Borders::ALL)
      .title("message or command"),
  );
  textarea.set_placeholder_text("waiting for input...");
  textarea.set_placeholder_style(Style::default().fg(Color::DarkGray));
  textarea.set_wrap_mode(WrapMode::Word);
  textarea.set_cursor_line_style(Style::default());
  let mut scroll_y: usize = 0;
  let mut follow_bottom = true;
  let mut file_selector: Option<FileSelector> = None;
  let mut selector_start: Option<ratatui_textarea::DataCursor> = None;
  let mut all_files: Option<Vec<String>> = None;
  let mut cursor_visible = false;

  let mut prev_generation = log.generation().wrapping_sub(1);
  let mut log_height = 0u16;
  let mut max_scroll_y = 0usize;
  let mut prev_state = status.state();
  while !stop.load(Ordering::Relaxed) {
    let has_selector = file_selector.is_some();
    if has_selector != cursor_visible {
      if has_selector {
        terminal.show_cursor()?;
      } else {
        terminal.hide_cursor()?;
      }
      cursor_visible = has_selector;
    }

    let current_generation = log.generation();
    let log_changed = current_generation != prev_generation;
    if log_changed {
      prev_generation = current_generation;
    }
    let has_event = event::poll(Duration::from_millis(100))?;
    if has_event {
      match event::read()? {
        Event::Key(key) => {
          if key.kind != KeyEventKind::Press {
            continue;
          }

          if let Some(ref mut selector) = file_selector {
            match key.code {
              KeyCode::Char(c) => {
                selector.update_query(c);
                let input = key_event_to_input(&key);
                textarea.input_without_shortcuts(input);
              }
              KeyCode::Backspace => {
                selector.backspace();
                let input = key_event_to_input(&key);
                textarea.input_without_shortcuts(input);
                if let Some(start) = selector_start {
                  let cursor = textarea.cursor();
                  if cursor.0 < start.0 || (cursor.0 == start.0 && cursor.1 <= start.1) {
                    file_selector = None;
                    selector_start = None;
                  }
                }
              }
              KeyCode::Enter => {
                let filtered = selector.filtered();
                if let Some(selected) = filtered.get(selector.selected)
                  && let Some(start) = selector_start
                {
                  let end = textarea.cursor();
                  textarea.move_cursor(CursorMove::Jump(start.0 as u16, start.1 as u16));
                  textarea.start_selection();
                  textarea.move_cursor(CursorMove::Jump(end.0 as u16, end.1 as u16));
                  textarea.cut();
                  textarea.insert_str(selected);
                }
                file_selector = None;
                selector_start = None;
              }
              KeyCode::Esc => {
                file_selector = None;
                selector_start = None;
              }
              KeyCode::Up => {
                selector.selected = selector.selected.saturating_sub(1);
              }
              KeyCode::Down => {
                let filtered_count = selector.filtered().len();
                if selector.selected + 1 < filtered_count {
                  selector.selected += 1;
                }
              }
              _ => {}
            }
          } else {
            match key.code {
              KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                tx.send(SteerEvent::Exit).ok();
                break;
              }
              KeyCode::Esc => {
                tx.send(SteerEvent::Exit).ok();
                break;
              }
              KeyCode::Char('@') => {
                if all_files.is_none() {
                  all_files = Some(collect_workspace_files());
                }
                selector_start = Some(textarea.cursor());
                let input = key_event_to_input(&key);
                textarea.input_without_shortcuts(input);
                file_selector = Some(FileSelector::new(all_files.as_ref().unwrap().clone()));
              }
              KeyCode::Enter => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                  textarea.insert_newline();
                } else {
                  let text = textarea.lines().join("\n").trim().to_string();
                  if !text.is_empty() {
                    textarea.clear();
                    follow_bottom = true;
                    let event = parse_steer_event(&text);
                    let exit = matches!(event, SteerEvent::Exit);
                    if tx.send(event).is_err() || exit {
                      break;
                    }
                  }
                }
              }
              KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                textarea.insert_newline();
              }
              KeyCode::Up => {
                let prev = textarea.cursor();
                textarea.move_cursor(CursorMove::Up);
                if textarea.cursor() == prev {
                  follow_bottom = false;
                  scroll_y = scroll_y.saturating_sub(1);
                }
              }
              KeyCode::Down => {
                let prev = textarea.cursor();
                textarea.move_cursor(CursorMove::Down);
                if textarea.cursor() == prev {
                  scroll_y = scroll_y.saturating_add(1);
                  if scroll_y >= max_scroll_y {
                    follow_bottom = true;
                  }
                }
              }
              KeyCode::PageUp => {
                follow_bottom = false;
                scroll_y = scroll_y.saturating_sub(log_height as usize);
              }
              KeyCode::PageDown => {
                scroll_y = scroll_y.saturating_add(log_height as usize);
                if scroll_y >= max_scroll_y {
                  follow_bottom = true;
                }
              }
              _ => {
                let input = key_event_to_input(&key);
                textarea.input(input);
              }
            }
          }
        }
        Event::Mouse(mouse) => match mouse.kind {
          MouseEventKind::ScrollUp => {
            if let Some(ref mut selector) = file_selector {
              selector.selected = selector.selected.saturating_sub(3);
            } else {
              follow_bottom = false;
              scroll_y = scroll_y.saturating_sub(3);
            }
          }
          MouseEventKind::ScrollDown => {
            if let Some(ref mut selector) = file_selector {
              let filtered_count = selector.filtered().len();
              selector.selected = (selector.selected + 3).min(filtered_count.saturating_sub(1));
            } else {
              scroll_y = scroll_y.saturating_add(3);
              if scroll_y >= max_scroll_y {
                follow_bottom = true;
              }
            }
          }
          _ => {}
        },
        _ => {}
      }
    }
    if log_changed || has_event {
      let state = status.state();
      if state != prev_state {
        prev_state = state;
        let title = match state {
          AgentState::Reasoning => "reasoning...",
          AgentState::Replying => "replying...",
          AgentState::Working => "working...",
          AgentState::Idle => "message or command",
        };
        textarea.set_block(Block::default().borders(Borders::ALL).title(title));
      }
      (log_height, max_scroll_y) = draw(
        terminal,
        &log,
        &status,
        &textarea,
        &mut scroll_y,
        follow_bottom,
        file_selector.as_ref(),
      )?;
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
    "/new" => SteerEvent::New,
    "/fork" => SteerEvent::Fork,
    "/q" | "/quit" | "quit" | "exit" => SteerEvent::Exit,
    other => SteerEvent::Message(other.to_string()),
  }
}

struct FileSelector {
  files: Vec<String>,
  query: String,
  selected: usize,
  filtered_cache: Vec<String>,
}

impl FileSelector {
  fn new(files: Vec<String>) -> Self {
    Self {
      files,
      query: String::new(),
      selected: 0,
      filtered_cache: Vec::new(),
    }
  }

  fn update_query(&mut self, c: char) {
    self.query.push(c);
    self.selected = 0;
    self.recompute();
  }

  fn backspace(&mut self) {
    self.query.pop();
    self.selected = 0;
    self.recompute();
  }

  fn recompute(&mut self) {
    if self.query.is_empty() {
      self.filtered_cache.clear();
    } else {
      let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
      let pattern = Pattern::parse(&self.query, CaseMatching::Ignore, Normalization::Smart);
      let matches = pattern.match_list(&self.files, &mut matcher);
      self.filtered_cache.clear();
      self
        .filtered_cache
        .extend(matches.into_iter().map(|(s, _)| s.clone()));
    }
  }

  fn filtered(&self) -> &[String] {
    if self.query.is_empty() {
      &self.files
    } else {
      &self.filtered_cache
    }
  }
}

fn collect_workspace_files() -> Vec<String> {
  let mut files = Vec::new();
  let root = crate::workspace::workspace_root();
  collect_files_recursive(root, root, &mut files);
  files.sort();
  files
}

fn collect_files_recursive(root: &std::path::Path, dir: &std::path::Path, files: &mut Vec<String>) {
  if let Ok(entries) = std::fs::read_dir(dir) {
    for entry in entries.flatten() {
      let path = entry.path();
      let name = entry.file_name();
      let name_str = name.to_string_lossy();
      if name_str.starts_with('.') {
        continue;
      }
      if path.is_dir() {
        match name_str.as_ref() {
          "target" | "node_modules" | "__pycache__" | "build" | "dist" | "out" => {}
          _ => collect_files_recursive(root, &path, files),
        }
      } else if let Ok(rel) = path.strip_prefix(root) {
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if !rel_str.is_empty() {
          files.push(rel_str);
        }
      }
    }
  }
}

fn draw(
  terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
  log: &UiLog,
  status: &UiStatus,
  textarea: &TextArea,
  scroll_y: &mut usize,
  follow_bottom: bool,
  file_selector: Option<&FileSelector>,
) -> Result<(u16, usize)> {
  let status_snapshot = status.snapshot();
  let log_lines = log.snapshot();
  let area = terminal.size()?.into();
  let chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
      Constraint::Length(1),
      Constraint::Min(1),
      Constraint::Length(5),
    ])
    .split(area);

  let log_width = chunks[1].width.saturating_sub(2);
  let log_height = chunks[1].height.saturating_sub(2);

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

  terminal.draw(|frame| {
    frame.render_widget(Clear, frame.area());

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

    frame.render_widget(textarea, chunks[2]);

    if let Some(selector) = file_selector {
      let filtered = selector.filtered();
      let popup_width = (area.width * 3 / 5).clamp(40, 80);
      let content_height = filtered.len().min(15) as u16;
      let popup_height = content_height + 3;
      let popup_x = (area.width.saturating_sub(popup_width)) / 2;
      let popup_y = (area.height.saturating_sub(popup_height)) / 2;
      let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

      let mut lines = vec![Line::from(vec![
        Span::raw("> "),
        Span::raw(&selector.query),
      ])];
      let max_display = content_height as usize;
      for (i, file) in filtered.iter().enumerate().take(max_display) {
        let style = if i == selector.selected {
          Style::default().bg(Color::Blue).fg(Color::White)
        } else {
          Style::default()
        };
        let display = tail_cells(file, popup_width.saturating_sub(2) as usize);
        lines.push(Line::from(Span::styled(display, style)));
      }
      if filtered.is_empty() {
        lines.push(Line::from(Span::styled(
          "no matches",
          Style::default().fg(Color::DarkGray),
        )));
      }

      frame.render_widget(Clear, popup_area);
      frame.render_widget(
        Paragraph::new(lines).block(
          Block::default()
            .borders(Borders::ALL)
            .title("file selector"),
        ),
        popup_area,
      );

      let cursor_x = popup_area.x + 2 + UnicodeWidthStr::width(selector.query.as_str()) as u16;
      let cursor_y = popup_area.y + 1;
      frame.set_cursor_position((cursor_x.min(popup_area.x + popup_area.width - 1), cursor_y));
    }
  })?;
  Ok((log_height, max_scroll))
}

fn key_event_to_input(key: &event::KeyEvent) -> ratatui_textarea::Input {
  use ratatui_textarea::Key;
  let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
  let alt = key.modifiers.contains(KeyModifiers::ALT);
  let shift = key.modifiers.contains(KeyModifiers::SHIFT);

  if key.code == KeyCode::BackTab {
    return ratatui_textarea::Input {
      key: Key::Tab,
      shift: true,
      ctrl,
      alt,
    };
  }

  let key_val = match key.code {
    KeyCode::Char(c) => Key::Char(c),
    KeyCode::Backspace => Key::Backspace,
    KeyCode::Enter => Key::Enter,
    KeyCode::Left => Key::Left,
    KeyCode::Right => Key::Right,
    KeyCode::Up => Key::Up,
    KeyCode::Down => Key::Down,
    KeyCode::Tab => Key::Tab,
    KeyCode::Delete => Key::Delete,
    KeyCode::Home => Key::Home,
    KeyCode::End => Key::End,
    KeyCode::PageUp => Key::PageUp,
    KeyCode::PageDown => Key::PageDown,
    KeyCode::Esc => Key::Esc,
    KeyCode::F(x) => Key::F(x),
    _ => Key::Null,
  };

  ratatui_textarea::Input {
    key: key_val,
    ctrl,
    alt,
    shift,
  }
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
  if let Some(markdown) = value.strip_prefix("ogent: ") {
    let mut spans = vec![Span::styled(
      "ogent: ",
      Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD),
    )];
    spans.extend(markdown_spans(markdown));
    return Line::from(spans);
  }
  if value == "ogent:" {
    return Line::from(vec![Span::styled(
      "ogent:",
      Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD),
    )]);
  }
  if let Some(reasoning) = value.strip_prefix("reasoning: ") {
    return Line::from(vec![
      Span::styled(
        "reasoning: ",
        Style::default()
          .fg(Color::Magenta)
          .add_modifier(Modifier::ITALIC),
      ),
      Span::raw(reasoning.to_string()),
    ]);
  }
  if value == "reasoning:" {
    return Line::from(vec![Span::styled(
      "reasoning:",
      Style::default()
        .fg(Color::Magenta)
        .add_modifier(Modifier::ITALIC),
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
      if let Some(after_tick) = rest.strip_prefix('`')
        && let Some(end) = after_tick.find('`')
      {
        spans.push(Span::styled(
          after_tick[..end].to_string(),
          Style::default().fg(Color::Green),
        ));
        rest = &after_tick[end + 1..];
        continue;
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
    assert_eq!(parse_steer_event("/new"), SteerEvent::New);
    assert_eq!(parse_steer_event("/fork"), SteerEvent::Fork);
    assert_eq!(parse_steer_event("/q"), SteerEvent::Exit);
    assert_eq!(parse_steer_event("/quit"), SteerEvent::Exit);
    assert_eq!(parse_steer_event("quit"), SteerEvent::Exit);
    assert_eq!(parse_steer_event("exit"), SteerEvent::Exit);
    assert_eq!(parse_steer_event("hi"), SteerEvent::Message("hi".into()));
  }
}
