use anyhow::Result;
use crossterm::event::{
  self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
  Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind,
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

type SkillEntries = Arc<[(String, String)]>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SteerEvent {
  Message(String),
  Cancel,
  Complete,
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
  tokens: u64,
  state: AgentState,
  compact_threshold: i32,
  context_limit: usize,
}

impl UiStatus {
  pub fn new(profile: String, model: String) -> Self {
    Self {
      inner: Arc::new(Mutex::new(StatusInner {
        profile,
        model,
        tokens: 0,
        state: AgentState::Idle,
        compact_threshold: -1,
        context_limit: 0,
      })),
    }
  }

  pub fn set_tokens(&self, tokens: u64) {
    let mut s = self.inner.lock().expect("ui status poisoned");
    s.tokens = tokens;
  }

  pub fn set_profile(&self, profile: String, model: String) {
    let mut s = self.inner.lock().expect("ui status poisoned");
    s.profile = profile;
    s.model = model;
  }

  pub fn set_state(&self, state: AgentState) {
    self.inner.lock().expect("ui status poisoned").state = state;
  }

  pub fn set_compact_threshold(&self, threshold: i32) {
    self
      .inner
      .lock()
      .expect("ui status poisoned")
      .compact_threshold = threshold;
  }

  pub fn set_context_limit(&self, limit: usize) {
    self.inner.lock().expect("ui status poisoned").context_limit = limit;
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

pub fn start(profile: String, model: String, skills: Vec<(String, String)>) -> Result<TuiHandle> {
  let (tx, rx) = mpsc::unbounded_channel();
  let log = UiLog::default();
  let status = UiStatus::new(profile, model);
  let ui_log = log.clone();
  let ui_status = status.clone();
  let stop = Arc::new(AtomicBool::new(false));
  let ui_stop = stop.clone();
  let skills = SkillEntries::from(skills);
  let thread = std::thread::spawn(move || {
    if let Err(err) = run_ui(tx, ui_log.clone(), ui_status, ui_stop, skills) {
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
      status: UiStatus::new("test".into(), "test".into()),
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
  skills: SkillEntries,
) -> Result<()> {
  enable_raw_mode()?;
  let mut stdout = io::stdout();
  execute!(
    stdout,
    EnterAlternateScreen,
    EnableBracketedPaste,
    EnableMouseCapture,
    DisableLineWrap
  )?;
  let backend = CrosstermBackend::new(stdout);
  let mut terminal = Terminal::new(backend)?;
  terminal.hide_cursor()?;
  let result = run_ui_loop(&mut terminal, tx, log, status, stop, skills);

  let restore = execute!(
    terminal.backend_mut(),
    DisableBracketedPaste,
    DisableMouseCapture,
    EnableLineWrap,
    LeaveAlternateScreen
  )
  .and_then(|()| terminal.show_cursor())
  .and_then(|()| disable_raw_mode());

  let _ = restore;
  result
}

fn run_ui_loop(
  terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
  tx: mpsc::UnboundedSender<SteerEvent>,
  log: UiLog,
  status: UiStatus,
  stop: Arc<AtomicBool>,
  skills: SkillEntries,
) -> Result<()> {
  let mut textarea = TextArea::default();
  textarea.set_block(
    Block::default()
      .borders(Borders::ALL)
      .border_style(Style::default().fg(Color::Rgb(122, 115, 104)))
      .title("message or command")
      .title_style(Style::default().fg(Color::Rgb(98, 93, 85))),
  );
  textarea.set_placeholder_text("waiting for input...");
  textarea.set_placeholder_style(Style::default().fg(Color::Rgb(122, 115, 104)));
  textarea.set_wrap_mode(WrapMode::Word);
  textarea.set_cursor_line_style(Style::default());
  let mut scroll_y: usize = 0;
  let mut follow_bottom = true;
  let mut active_selector: Option<ActiveSelector> = None;
  let mut selector_start: Option<ratatui_textarea::DataCursor> = None;
  let mut cursor_visible = false;

  let mut prev_generation = log.generation().wrapping_sub(1);
  let mut log_height = 0u16;
  let mut max_scroll_y = 0usize;
  let mut prev_state = status.state();
  while !stop.load(Ordering::Relaxed) {
    let has_selector = active_selector.is_some();
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

          if let Some(ref mut selector) = active_selector {
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
                    active_selector = None;
                    selector_start = None;
                  }
                }
              }
              KeyCode::Enter => {
                if let Some(completion) = selector.completion() {
                  if let Some(start) = selector_start {
                    let end = textarea.cursor();
                    textarea.move_cursor(CursorMove::Jump(start.0 as u16, start.1 as u16));
                    textarea.start_selection();
                    textarea.move_cursor(CursorMove::Jump(end.0 as u16, end.1 as u16));
                    textarea.cut();
                  }
                  textarea.insert_str(completion);
                }
                active_selector = None;
                selector_start = None;
              }
              KeyCode::Esc => {
                active_selector = None;
                selector_start = None;
              }
              KeyCode::Up => {
                selector.move_up(1);
              }
              KeyCode::Down => {
                selector.move_down(1);
              }
              _ => {}
            }
          } else {
            match key.code {
              KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let text = textarea.lines().join("\n").trim().to_string();
                let draft = (!text.is_empty()).then_some(text);
                tx.send(SteerEvent::Exit(draft)).ok();
                break;
              }
              KeyCode::Esc => {
                let text = textarea.lines().join("\n").trim().to_string();
                let draft = (!text.is_empty()).then_some(text);
                tx.send(SteerEvent::Exit(draft)).ok();
                break;
              }
              KeyCode::Char('@') => {
                let all_files = collect_workspace_files();
                selector_start = Some(textarea.cursor());
                let input = key_event_to_input(&key);
                textarea.input_without_shortcuts(input);
                active_selector = Some(ActiveSelector::File(FileSelector::new(all_files)));
              }
              KeyCode::Char('$') => {
                if !skills.is_empty() {
                  selector_start = Some(textarea.cursor());
                  textarea.input_without_shortcuts(key_event_to_input(&key));
                  active_selector = Some(ActiveSelector::Skill(SkillSelector::new(skills.clone())));
                } else {
                  textarea.input_without_shortcuts(key_event_to_input(&key));
                }
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
                    let exit = matches!(event, SteerEvent::Exit(_));
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
            if let Some(ref mut selector) = active_selector {
              selector.move_up(3);
            } else {
              follow_bottom = false;
              scroll_y = scroll_y.saturating_sub(3);
            }
          }
          MouseEventKind::ScrollDown => {
            if let Some(ref mut selector) = active_selector {
              selector.move_down(3);
            } else {
              scroll_y = scroll_y.saturating_add(3);
              if scroll_y >= max_scroll_y {
                follow_bottom = true;
              }
            }
          }
          _ => {}
        },
        Event::Paste(text) => {
          if active_selector.is_none() {
            textarea.insert_str(&text);
          }
        }
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
        textarea.set_block(
          Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(122, 115, 104)))
            .title(title)
            .title_style(Style::default().fg(Color::Rgb(98, 93, 85))),
        );
      }
      (log_height, max_scroll_y) = draw(
        terminal,
        &log,
        &status,
        &textarea,
        &mut scroll_y,
        follow_bottom,
        active_selector.as_ref(),
      )?;
    }
  }

  Ok(())
}

pub fn parse_steer_event(line: &str) -> SteerEvent {
  match line.trim() {
    "/cancel" => SteerEvent::Cancel,
    "/complete" => SteerEvent::Complete,
    "/compact" => SteerEvent::Compact(None),
    s if s.starts_with("/compact ") => SteerEvent::Compact(Some(
      s.strip_prefix("/compact ").unwrap().trim().to_string(),
    )),
    "/new" => SteerEvent::New,
    "/q" | "/quit" | "quit" | "exit" => SteerEvent::Exit(None),
    s if s.starts_with("/profile ") => {
      SteerEvent::Profile(s.strip_prefix("/profile ").unwrap().trim().to_string())
    }
    other => SteerEvent::Message(other.to_string()),
  }
}

enum ActiveSelector {
  File(FileSelector),
  Skill(SkillSelector),
}

impl ActiveSelector {
  fn update_query(&mut self, c: char) {
    match self {
      Self::File(selector) => selector.update_query(c),
      Self::Skill(selector) => selector.update_query(c),
    }
  }

  fn backspace(&mut self) {
    match self {
      Self::File(selector) => selector.backspace(),
      Self::Skill(selector) => selector.backspace(),
    }
  }

  fn filtered_len(&self) -> usize {
    match self {
      Self::File(selector) => selector.filtered().len(),
      Self::Skill(selector) => selector.filtered_len(),
    }
  }

  fn selected(&self) -> usize {
    match self {
      Self::File(selector) => selector.selected,
      Self::Skill(selector) => selector.selected,
    }
  }

  fn selected_mut(&mut self) -> &mut usize {
    match self {
      Self::File(selector) => &mut selector.selected,
      Self::Skill(selector) => &mut selector.selected,
    }
  }

  fn move_up(&mut self, amount: usize) {
    *self.selected_mut() = self.selected().saturating_sub(amount);
  }

  fn move_down(&mut self, amount: usize) {
    let max_selected = self.filtered_len().saturating_sub(1);
    *self.selected_mut() = self.selected().saturating_add(amount).min(max_selected);
  }

  fn completion(&self) -> Option<String> {
    match self {
      Self::File(selector) => selector.filtered().get(selector.selected).cloned(),
      Self::Skill(selector) => selector
        .filtered_skill(selector.selected)
        .map(|(name, _)| format!("{name} skill")),
    }
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

struct SkillSelector {
  skills: SkillEntries,
  query: String,
  selected: usize,
  filtered_cache: Vec<usize>,
}

impl SkillSelector {
  fn new(skills: SkillEntries) -> Self {
    Self {
      skills,
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
      let mut matcher = Matcher::new(Config::DEFAULT);
      let pattern = Pattern::parse(&self.query, CaseMatching::Ignore, Normalization::Smart);
      let matches = pattern.match_list(
        self
          .skills
          .iter()
          .enumerate()
          .map(|(index, (name, _))| SkillCandidate {
            index,
            name: name.as_str(),
          }),
        &mut matcher,
      );
      self.filtered_cache.clear();
      self
        .filtered_cache
        .extend(matches.into_iter().map(|(candidate, _)| candidate.index));
    }
  }

  fn filtered_len(&self) -> usize {
    if self.query.is_empty() {
      self.skills.len()
    } else {
      self.filtered_cache.len()
    }
  }

  fn filtered_skill(&self, index: usize) -> Option<&(String, String)> {
    let skill_index = if self.query.is_empty() {
      index
    } else {
      *self.filtered_cache.get(index)?
    };
    self.skills.get(skill_index)
  }
}

struct SkillCandidate<'a> {
  index: usize,
  name: &'a str,
}

impl AsRef<str> for SkillCandidate<'_> {
  fn as_ref(&self) -> &str {
    self.name
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
          _ => {
            if let Ok(rel) = path.strip_prefix(root) {
              let rel_str = rel.to_string_lossy().replace('\\', "/") + "/";
              if rel_str.len() > 1 {
                files.push(rel_str);
              }
            }
            collect_files_recursive(root, &path, files);
          }
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
  selector: Option<&ActiveSelector>,
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
  let paragraph = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
    Block::default()
      .borders(Borders::ALL)
      .border_style(Style::default().fg(Color::Rgb(122, 115, 104)))
      .title("log")
      .title_style(Style::default().fg(Color::Rgb(98, 93, 85))),
  );
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

    let mut bar = format!(
      "{} | {} | tokens {}",
      status_snapshot.profile, status_snapshot.model, status_snapshot.tokens,
    );
    if status_snapshot.compact_threshold >= 0 && status_snapshot.context_limit > 0 {
      let pct = status_snapshot.tokens as usize * 100 / status_snapshot.context_limit;
      bar.push_str(&format!(
        " | compact@{}% [{}% used]",
        status_snapshot.compact_threshold, pct
      ));
    }
    truncate_to_width(&mut bar, chunks[0].width.saturating_sub(1) as usize);
    frame.render_widget(
      Paragraph::new(bar).style(Style::default().fg(Color::Rgb(122, 115, 104))),
      chunks[0],
    );

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

    if let Some(ActiveSelector::File(selector)) = selector {
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
          Style::default()
            .bg(Color::Rgb(221, 214, 204))
            .fg(Color::Rgb(74, 70, 64))
        } else {
          Style::default().fg(Color::Rgb(98, 93, 85))
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
            .border_style(Style::default().fg(Color::Rgb(122, 115, 104)))
            .title("file selector")
            .title_style(Style::default().fg(Color::Rgb(98, 93, 85))),
        ),
        popup_area,
      );

      let cursor_x = popup_area.x + 2 + UnicodeWidthStr::width(selector.query.as_str()) as u16;
      let cursor_y = popup_area.y + 1;
      frame.set_cursor_position((cursor_x.min(popup_area.x + popup_area.width - 1), cursor_y));
    }

    if let Some(ActiveSelector::Skill(selector)) = selector {
      let popup_width = (area.width * 3 / 5).clamp(40, 80);
      let max_skills = 8usize;
      let desc_indent = 2;
      let max_desc_lines = 3usize;
      let inner_width = popup_width.saturating_sub(2) as usize;
      let desc_wrap_width = inner_width.saturating_sub(desc_indent);

      let filtered_count = selector.filtered_len();
      let visible_count = filtered_count.min(max_skills);
      let first_visible = selector
        .selected
        .saturating_add(1)
        .saturating_sub(visible_count)
        .min(filtered_count.saturating_sub(visible_count));
      let mut lines: Vec<Line<'static>> = vec![Line::from(vec![
        Span::raw("> "),
        Span::raw(selector.query.clone()),
      ])];
      let desc_prefix = " ".repeat(desc_indent);
      for i in first_visible..first_visible + visible_count {
        let Some((name, desc)) = selector.filtered_skill(i) else {
          continue;
        };
        let name_style = if i == selector.selected {
          Style::default()
            .bg(Color::Rgb(221, 214, 204))
            .fg(Color::Rgb(74, 70, 64))
        } else {
          Style::default().fg(Color::Rgb(98, 93, 85))
        };
        let desc_style = if i == selector.selected {
          Style::default()
            .bg(Color::Rgb(221, 214, 204))
            .fg(Color::Rgb(122, 115, 104))
        } else {
          Style::default().fg(Color::Rgb(122, 115, 104))
        };
        lines.push(Line::from(Span::styled(
          head_cells(name, inner_width),
          name_style,
        )));
        if i == selector.selected && !desc.is_empty() {
          for wrap_line in wrap_text(desc, desc_wrap_width)
            .into_iter()
            .take(max_desc_lines)
          {
            lines.push(Line::from(Span::styled(
              format!("{desc_prefix}{wrap_line}"),
              desc_style,
            )));
          }
        }
      }
      if filtered_count == 0 {
        lines.push(Line::from(Span::styled(
          "no skills found",
          Style::default().fg(Color::DarkGray),
        )));
      }
      let popup_height = lines.len() as u16 + 2;
      let popup_x = (area.width.saturating_sub(popup_width)) / 2;
      let popup_y = (area.height.saturating_sub(popup_height)) / 2;
      let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

      frame.render_widget(Clear, popup_area);
      frame.render_widget(
        Paragraph::new(lines).block(
          Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(122, 115, 104)))
            .title("skill selector")
            .title_style(Style::default().fg(Color::Rgb(98, 93, 85))),
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

fn wrap_text(value: &str, width: usize) -> Vec<String> {
  if width == 0 {
    return vec![value.to_string()];
  }
  let mut lines = Vec::new();
  let mut current = String::new();
  let mut used = 0usize;
  for word in value.split_whitespace() {
    let word_width = UnicodeWidthStr::width(word);
    if used > 0 && used + 1 + word_width > width {
      lines.push(current.clone());
      current.clear();
      used = 0;
    }
    if used > 0 {
      current.push(' ');
      used += 1;
    }
    if used + word_width <= width {
      current.push_str(word);
      used += word_width;
    } else {
      let mut chars_used = 0usize;
      for c in word.chars() {
        let cw = c.width().unwrap_or(0);
        if chars_used + cw > width {
          break;
        }
        current.push(c);
        chars_used += cw;
      }
      used = chars_used;
    }
  }
  if !current.is_empty() {
    lines.push(current);
  }
  if lines.is_empty() {
    lines.push(String::new());
  }
  lines
}

fn render_log_line(value: &str) -> Line<'static> {
  if let Some(markdown) = value.strip_prefix("ogent: ") {
    let spans: Vec<Span> = markdown_spans(markdown)
      .into_iter()
      .map(|s| s.patch_style(Style::default().bg(Color::Rgb(221, 214, 204))))
      .collect();
    return Line::from(spans);
  }
  if value == "ogent:" {
    return Line::from(Span::styled(
      " ",
      Style::default().bg(Color::Rgb(221, 214, 204)),
    ));
  }
  if let Some(reasoning) = value.strip_prefix("reasoning: ") {
    return Line::from(Span::styled(
      reasoning.to_string(),
      Style::default().fg(Color::Rgb(98, 93, 85)),
    ));
  }
  if value == "reasoning:" {
    return Line::from(Span::styled(
      " ",
      Style::default().fg(Color::Rgb(98, 93, 85)),
    ));
  }
  if let Some(user_msg) = value.strip_prefix("[user] ") {
    return Line::from(Span::styled(
      user_msg.to_string(),
      Style::default().bg(Color::Rgb(242, 238, 235)),
    ));
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
    assert_eq!(parse_steer_event("/cancel"), SteerEvent::Cancel);
    assert_eq!(parse_steer_event("/complete"), SteerEvent::Complete);
    assert_eq!(parse_steer_event("/compact"), SteerEvent::Compact(None));
    assert_eq!(
      parse_steer_event("/compact fix auth"),
      SteerEvent::Compact(Some("fix auth".into()))
    );
    assert_eq!(parse_steer_event("/new"), SteerEvent::New);
    assert_eq!(parse_steer_event("/q"), SteerEvent::Exit(None));
    assert_eq!(parse_steer_event("/quit"), SteerEvent::Exit(None));
    assert_eq!(parse_steer_event("quit"), SteerEvent::Exit(None));
    assert_eq!(parse_steer_event("exit"), SteerEvent::Exit(None));
    assert_eq!(parse_steer_event("hi"), SteerEvent::Message("hi".into()));
  }
}
