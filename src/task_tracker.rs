use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

const HANDOFF_STATE_START: &str = "<ogent_task_tracker_state>";
const HANDOFF_STATE_END: &str = "</ogent_task_tracker_state>";
const STALE_NUDGE_TURNS: usize = 3;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
  Pending,
  InProgress,
  Completed,
  Blocked,
  Skipped,
}

impl Status {
  fn is_open(self) -> bool {
    !matches!(self, Self::Completed | Self::Skipped)
  }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Complexity {
  Simple,
  Medium,
  Complex,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalState {
  pub title: String,
  pub status: Status,
  pub complexity: Complexity,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub success_criteria: Vec<String>,
  #[serde(default, skip_serializing_if = "String::is_empty")]
  pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalRevision {
  pub previous_goal: GoalState,
  pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoState {
  pub id: String,
  pub title: String,
  pub status: Status,
  pub complexity: Complexity,
  #[serde(default, skip_serializing_if = "String::is_empty")]
  pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidationContract {
  pub id: String,
  pub assertion: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PhaseState {
  pub id: String,
  pub title: String,
  pub status: Status,
  pub complexity: Complexity,
  #[serde(default, skip_serializing_if = "String::is_empty")]
  pub notes: String,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub contracts: Vec<ValidationContract>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub todos: Vec<TodoState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PhaseUpdate {
  pub id: String,
  pub title: String,
  pub status: Status,
  pub complexity: Complexity,
  #[serde(default)]
  pub notes: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub contracts: Option<Vec<ValidationContract>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoUpdate {
  pub phase_id: String,
  pub id: String,
  pub title: String,
  pub status: Status,
  pub complexity: Complexity,
  #[serde(default)]
  pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskTracker {
  #[serde(default = "default_version")]
  pub version: u8,
  pub goal: GoalState,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub revisions: Vec<GoalRevision>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub phases: Vec<PhaseState>,
  #[serde(default)]
  stale_turns: usize,
  #[serde(default)]
  stale_nudge_emitted: bool,
  #[serde(default)]
  pending_reminder: bool,
}

fn default_version() -> u8 {
  1
}

pub fn is_tracking_tool_name(name: &str) -> bool {
  matches!(
    name,
    "set_goal" | "revise_goal" | "update_phase" | "update_todo"
  )
}

impl TaskTracker {
  pub fn new(goal: GoalState) -> Self {
    Self {
      version: default_version(),
      goal,
      revisions: Vec::new(),
      phases: Vec::new(),
      stale_turns: 0,
      stale_nudge_emitted: false,
      pending_reminder: true,
    }
  }

  pub fn revise_goal(&mut self, next: GoalState, reason: String) {
    self.revisions.push(GoalRevision {
      previous_goal: self.goal.clone(),
      reason,
    });
    self.goal = next;
    self.note_tracking_update();
  }

  pub fn update_phase(&mut self, update: PhaseUpdate) {
    if update.status == Status::InProgress {
      for phase in &mut self.phases {
        if phase.status == Status::InProgress && phase.id != update.id {
          phase.status = Status::Pending;
        }
      }
    }
    if let Some(phase) = self.phases.iter_mut().find(|phase| phase.id == update.id) {
      phase.title = update.title;
      phase.status = update.status;
      phase.complexity = update.complexity;
      phase.notes = update.notes;
      if let Some(contracts) = update.contracts {
        phase.contracts = contracts;
      }
    } else {
      self.phases.push(PhaseState {
        id: update.id,
        title: update.title,
        status: update.status,
        complexity: update.complexity,
        notes: update.notes,
        contracts: update.contracts.unwrap_or_default(),
        todos: Vec::new(),
      });
    }
    self.note_tracking_update();
  }

  pub fn update_todo(&mut self, update: TodoUpdate) -> Result<()> {
    let Some(phase) = self
      .phases
      .iter_mut()
      .find(|phase| phase.id == update.phase_id)
    else {
      bail!(
        "phase_id {} does not exist; call update_phase first",
        update.phase_id
      );
    };
    if let Some(todo) = phase.todos.iter_mut().find(|todo| todo.id == update.id) {
      todo.title = update.title;
      todo.status = update.status;
      todo.complexity = update.complexity;
      todo.notes = update.notes;
    } else {
      phase.todos.push(TodoState {
        id: update.id,
        title: update.title,
        status: update.status,
        complexity: update.complexity,
        notes: update.notes,
      });
    }
    self.note_tracking_update();
    Ok(())
  }

  fn any_open_phase_or_todo(&self) -> bool {
    self
      .phases
      .iter()
      .any(|phase| phase.status.is_open() || phase.todos.iter().any(|todo| todo.status.is_open()))
  }

  pub fn open_work_exists(&self) -> bool {
    self.goal.status.is_open() || self.any_open_phase_or_todo()
  }

  pub fn open_phase_or_todo_exists(&self) -> bool {
    self.any_open_phase_or_todo()
  }

  pub fn render_tool_snapshot(&self) -> String {
    format!("Task tracking state:\n{}", self.render_summary_lines())
  }

  pub fn note_tool_turn(&mut self, saw_tracking_update: bool, saw_meaningful_non_tracking: bool) {
    if saw_tracking_update {
      self.note_tracking_update();
      return;
    }
    if saw_meaningful_non_tracking {
      self.stale_turns = self.stale_turns.saturating_add(1);
    }
  }

  pub fn mark_restored(&mut self) {
    self.pending_reminder = true;
  }

  pub fn take_reminder(&mut self) -> Option<String> {
    let stale_nudge =
      self.open_work_exists() && !self.stale_nudge_emitted && self.stale_turns >= STALE_NUDGE_TURNS;
    if !self.pending_reminder && !stale_nudge {
      return None;
    }
    if stale_nudge {
      self.stale_nudge_emitted = true;
    }
    self.pending_reminder = false;
    Some(self.render_compact_reminder(stale_nudge))
  }

  pub fn render_handoff_appendix(&self) -> String {
    let mut out = String::new();
    out.push_str("## Runtime Task Tracking\n\n");
    out.push_str(&self.render_summary_lines());
    out.push('\n');
    out.push_str(HANDOFF_STATE_START);
    out.push('\n');
    out.push_str(
      &serde_json::to_string_pretty(self).unwrap_or_else(|_| "{\"error\":\"encode\"}".to_string()),
    );
    out.push('\n');
    out.push_str(HANDOFF_STATE_END);
    out
  }

  pub fn from_handoff_text(text: &str) -> Option<Self> {
    let start = text.find(HANDOFF_STATE_START)?;
    let after_start = start + HANDOFF_STATE_START.len();
    let end_rel = text[after_start..].find(HANDOFF_STATE_END)?;
    let end = after_start + end_rel;
    serde_json::from_str(text[after_start..end].trim()).ok()
  }

  pub fn strip_handoff_state_block(text: &str) -> String {
    let Some(start) = text.find(HANDOFF_STATE_START) else {
      return text.to_string();
    };
    let after_start = start + HANDOFF_STATE_START.len();
    let Some(end_rel) = text[after_start..].find(HANDOFF_STATE_END) else {
      return text.to_string();
    };
    let end = after_start + end_rel + HANDOFF_STATE_END.len();
    let mut out = String::new();
    out.push_str(text[..start].trim_end());
    let trailing = text[end..].trim_start();
    if !out.is_empty() && !trailing.is_empty() {
      out.push_str("\n\n");
    }
    out.push_str(trailing);
    out
  }

  fn note_tracking_update(&mut self) {
    self.pending_reminder = true;
    self.stale_turns = 0;
    self.stale_nudge_emitted = false;
  }

  fn render_compact_reminder(&self, stale_nudge: bool) -> String {
    use std::fmt::Write;
    let mut body = String::from("<system_reminder kind=\"task_tracking\">\n");
    body.push_str("Task tracker already exists. Do not call `set_goal` again.\n");
    body.push_str(&self.render_summary_lines());
    if stale_nudge {
      let _ = writeln!(
        body,
        "- Stale: non-tracking work progressed for {} turns without tracker updates.",
        self.stale_turns
      );
    }
    body.push_str("- Keep Goal -> Phases -> Todos current with update_phase/update_todo. Use revise_goal only if the objective changed.\n");
    body.push_str("</system_reminder>");
    body
  }

  fn render_summary_lines(&self) -> String {
    use std::fmt::Write;
    let mut lines = String::new();
    let _ = writeln!(
      lines,
      "- Goal: [{}|{}] {}",
      format_status(self.goal.status),
      format_complexity(self.goal.complexity),
      self.goal.title
    );
    if !self.goal.notes.is_empty() {
      let _ = writeln!(lines, "  notes: {}", self.goal.notes);
    }
    for criterion in self.goal.success_criteria.iter().take(4) {
      let _ = writeln!(lines, "  success: {criterion}");
    }
    if !self.revisions.is_empty() {
      let _ = writeln!(lines, "- Goal revisions: {}", self.revisions.len());
    }
    let mut counts = [0usize; 5];
    for phase in &self.phases {
      counts[status_index(phase.status)] += 1;
    }
    let _ = writeln!(
      lines,
      "- Phases: pending={} in_progress={} blocked={} completed={} skipped={}",
      counts[0], counts[1], counts[3], counts[2], counts[4]
    );
    let mut emitted = 0usize;
    for phase in &self.phases {
      if phase.status.is_open() && emitted < 4 {
        let _ = writeln!(
          lines,
          "- phase({}) [{}|{}] {}",
          phase.id,
          format_status(phase.status),
          format_complexity(phase.complexity),
          phase.title
        );
        if !phase.contracts.is_empty() {
          for c in phase.contracts.iter().take(6) {
            let _ = writeln!(lines, "    contract {}: {}", c.id, c.assertion);
          }
        }
        emitted += 1;
      }
      for todo in &phase.todos {
        if todo.status.is_open() && emitted < 8 {
          let _ = writeln!(
            lines,
            "  - todo({}/{}) [{}|{}] {}",
            phase.id,
            todo.id,
            format_status(todo.status),
            format_complexity(todo.complexity),
            todo.title
          );
          emitted += 1;
        }
      }
    }
    lines
  }
}

fn status_index(status: Status) -> usize {
  match status {
    Status::Pending => 0,
    Status::InProgress => 1,
    Status::Completed => 2,
    Status::Blocked => 3,
    Status::Skipped => 4,
  }
}

fn format_status(status: Status) -> &'static str {
  match status {
    Status::Pending => "pending",
    Status::InProgress => "in_progress",
    Status::Completed => "completed",
    Status::Blocked => "blocked",
    Status::Skipped => "skipped",
  }
}

fn format_complexity(complexity: Complexity) -> &'static str {
  match complexity {
    Complexity::Simple => "simple",
    Complexity::Medium => "medium",
    Complexity::Complex => "complex",
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn seed() -> TaskTracker {
    TaskTracker::new(GoalState {
      title: "Ship runtime tracker".into(),
      status: Status::InProgress,
      complexity: Complexity::Complex,
      success_criteria: Vec::new(),
      notes: String::new(),
    })
  }

  #[test]
  fn goal_revisions_capture_previous_goal() {
    let mut tracker = seed();
    tracker.revise_goal(
      GoalState {
        title: "Ship tracker v2".into(),
        status: Status::InProgress,
        complexity: Complexity::Complex,
        success_criteria: vec!["runtime reminders work".into()],
        notes: "scope expanded".into(),
      },
      "user changed scope".into(),
    );
    assert_eq!(tracker.revisions.len(), 1);
    assert_eq!(
      tracker.revisions[0].previous_goal.title,
      "Ship runtime tracker"
    );
    assert_eq!(tracker.goal.title, "Ship tracker v2");
  }

  #[test]
  fn todo_requires_existing_phase() {
    let mut tracker = seed();
    let err = tracker
      .update_todo(TodoUpdate {
        phase_id: "p1".into(),
        id: "t1".into(),
        title: "x".into(),
        status: Status::Pending,
        complexity: Complexity::Simple,
        notes: String::new(),
      })
      .expect_err("missing phase must fail");
    assert!(err.to_string().contains("phase_id p1 does not exist"));
  }

  #[test]
  fn open_work_and_stale_nudge_behave() {
    let mut tracker = seed();
    tracker.update_phase(PhaseUpdate {
      id: "phase-1".into(),
      title: "plumbing".into(),
      status: Status::InProgress,
      complexity: Complexity::Medium,
      notes: String::new(),
      contracts: None,
    });
    assert!(tracker.open_work_exists());
    tracker.take_reminder();
    tracker.note_tool_turn(false, true);
    tracker.note_tool_turn(false, true);
    assert!(tracker.take_reminder().is_none());
    tracker.note_tool_turn(false, true);
    let reminder = tracker.take_reminder().expect("stale nudge expected");
    assert!(reminder.contains("Stale"));
    assert!(reminder.contains("Do not call `set_goal` again"));
    assert!(tracker.take_reminder().is_none());
  }

  #[test]
  fn restored_tracker_reminder_warns_against_set_goal() {
    let mut tracker = seed();
    tracker.mark_restored();
    let reminder = tracker
      .take_reminder()
      .expect("restored tracker should remind");
    assert!(reminder.contains("Task tracker already exists"));
    assert!(reminder.contains("Use revise_goal only if the objective changed"));
  }

  #[test]
  fn starting_new_phase_demotes_prior_in_progress_phase() {
    let mut tracker = seed();
    tracker.update_phase(PhaseUpdate {
      id: "phase-1".into(),
      title: "orient".into(),
      status: Status::InProgress,
      complexity: Complexity::Medium,
      notes: String::new(),
      contracts: None,
    });
    tracker.update_phase(PhaseUpdate {
      id: "phase-2".into(),
      title: "implement".into(),
      status: Status::InProgress,
      complexity: Complexity::Medium,
      notes: String::new(),
      contracts: None,
    });
    assert_eq!(tracker.phases[0].status, Status::Pending);
    assert_eq!(tracker.phases[1].status, Status::InProgress);
  }

  #[test]
  fn open_phase_or_todo_excludes_goal_status() {
    let mut tracker = seed();
    tracker.update_phase(PhaseUpdate {
      id: "phase-1".into(),
      title: "orient".into(),
      status: Status::Completed,
      complexity: Complexity::Medium,
      notes: String::new(),
      contracts: None,
    });
    assert!(tracker.open_work_exists());
    assert!(!tracker.open_phase_or_todo_exists());
  }

  #[test]
  fn handoff_round_trip_and_strip() {
    let mut tracker = seed();
    tracker.update_phase(PhaseUpdate {
      id: "phase-1".into(),
      title: "plumbing".into(),
      status: Status::InProgress,
      complexity: Complexity::Medium,
      notes: String::new(),
      contracts: None,
    });
    let mut handoff = String::from("User brief");
    handoff.push_str("\n\n");
    handoff.push_str(&tracker.render_handoff_appendix());
    let restored = TaskTracker::from_handoff_text(&handoff).expect("tracker should restore");
    assert_eq!(restored.goal.title, tracker.goal.title);
    let stripped = TaskTracker::strip_handoff_state_block(&handoff);
    assert!(!stripped.contains(HANDOFF_STATE_START));
    assert!(stripped.contains("Runtime Task Tracking"));
  }

  #[test]
  fn update_phase_preserves_contracts_when_not_provided() {
    let mut tracker = seed();
    tracker.update_phase(PhaseUpdate {
      id: "p1".into(),
      title: "P1".into(),
      status: Status::Pending,
      complexity: Complexity::Simple,
      notes: String::new(),
      contracts: Some(vec![ValidationContract {
        id: "c1".into(),
        assertion: "a1".into(),
        command: None,
      }]),
    });
    tracker.update_phase(PhaseUpdate {
      id: "p1".into(),
      title: "P1".into(),
      status: Status::InProgress,
      complexity: Complexity::Simple,
      notes: String::new(),
      contracts: None,
    });
    assert_eq!(tracker.phases[0].contracts.len(), 1);
    assert_eq!(tracker.phases[0].contracts[0].id, "c1");
  }

  #[test]
  fn update_phase_overwrites_contracts_when_provided() {
    let mut tracker = seed();
    tracker.update_phase(PhaseUpdate {
      id: "p1".into(),
      title: "P1".into(),
      status: Status::Pending,
      complexity: Complexity::Simple,
      notes: String::new(),
      contracts: Some(vec![ValidationContract {
        id: "c1".into(),
        assertion: "a1".into(),
        command: None,
      }]),
    });
    tracker.update_phase(PhaseUpdate {
      id: "p1".into(),
      title: "P1".into(),
      status: Status::Pending,
      complexity: Complexity::Simple,
      notes: String::new(),
      contracts: Some(vec![ValidationContract {
        id: "c2".into(),
        assertion: "a2".into(),
        command: None,
      }]),
    });
    assert_eq!(tracker.phases[0].contracts.len(), 1);
    assert_eq!(tracker.phases[0].contracts[0].id, "c2");
  }

  #[test]
  fn update_todo_creates_and_updates_todo() {
    let mut tracker = seed();
    tracker.update_phase(PhaseUpdate {
      id: "p1".into(),
      title: "P1".into(),
      status: Status::Pending,
      complexity: Complexity::Simple,
      notes: String::new(),
      contracts: None,
    });
    tracker
      .update_todo(TodoUpdate {
        phase_id: "p1".into(),
        id: "t1".into(),
        title: "T1".into(),
        status: Status::Pending,
        complexity: Complexity::Simple,
        notes: String::new(),
      })
      .unwrap();
    assert_eq!(tracker.phases[0].todos.len(), 1);
    assert_eq!(tracker.phases[0].todos[0].title, "T1");
    tracker
      .update_todo(TodoUpdate {
        phase_id: "p1".into(),
        id: "t1".into(),
        title: "T1 Updated".into(),
        status: Status::Completed,
        complexity: Complexity::Medium,
        notes: "done".into(),
      })
      .unwrap();
    assert_eq!(tracker.phases[0].todos[0].title, "T1 Updated");
    assert_eq!(tracker.phases[0].todos[0].status, Status::Completed);
  }

  #[test]
  fn open_work_exists_with_open_phase() {
    let mut tracker = seed();
    tracker.goal.status = Status::Completed;
    tracker.update_phase(PhaseUpdate {
      id: "p1".into(),
      title: "P1".into(),
      status: Status::InProgress,
      complexity: Complexity::Simple,
      notes: String::new(),
      contracts: None,
    });
    assert!(tracker.open_work_exists());
    assert!(tracker.open_phase_or_todo_exists());
  }

  #[test]
  fn open_work_exists_with_open_todo() {
    let mut tracker = seed();
    tracker.goal.status = Status::Completed;
    tracker.update_phase(PhaseUpdate {
      id: "p1".into(),
      title: "P1".into(),
      status: Status::Completed,
      complexity: Complexity::Simple,
      notes: String::new(),
      contracts: None,
    });
    tracker
      .update_todo(TodoUpdate {
        phase_id: "p1".into(),
        id: "t1".into(),
        title: "T1".into(),
        status: Status::Pending,
        complexity: Complexity::Simple,
        notes: String::new(),
      })
      .unwrap();
    assert!(tracker.open_work_exists());
    assert!(tracker.open_phase_or_todo_exists());
  }

  #[test]
  fn open_work_exists_false_when_goal_skipped() {
    let mut tracker = seed();
    tracker.goal.status = Status::Skipped;
    assert!(!tracker.open_work_exists());
  }

  #[test]
  fn note_tool_turn_resets_stale_on_tracking_update() {
    let mut tracker = seed();
    tracker.note_tool_turn(false, true);
    tracker.note_tool_turn(false, true);
    tracker.note_tool_turn(true, false);
    tracker.note_tool_turn(false, true);
    let reminder = tracker.take_reminder().unwrap();
    assert!(!reminder.contains("Stale"));
  }

  #[test]
  fn take_reminder_returns_none_when_no_pending_and_not_stale() {
    let mut tracker = seed();
    tracker.take_reminder(); // consume initial
    assert!(tracker.take_reminder().is_none());
  }

  #[test]
  fn render_tool_snapshot_includes_goal_and_phases() {
    let mut tracker = seed();
    tracker.update_phase(PhaseUpdate {
      id: "p1".into(),
      title: "P1".into(),
      status: Status::Pending,
      complexity: Complexity::Simple,
      notes: String::new(),
      contracts: None,
    });
    let snapshot = tracker.render_tool_snapshot();
    assert!(snapshot.contains("Ship runtime tracker"));
    assert!(snapshot.contains("P1"));
  }

  #[test]
  fn from_handoff_text_returns_none_when_no_state_block() {
    assert!(TaskTracker::from_handoff_text("no state here").is_none());
  }

  #[test]
  fn strip_handoff_state_block_returns_original_when_no_block() {
    let text = "no state here";
    assert_eq!(TaskTracker::strip_handoff_state_block(text), text);
  }

  #[test]
  fn is_tracking_tool_name_matches() {
    assert!(is_tracking_tool_name("set_goal"));
    assert!(is_tracking_tool_name("revise_goal"));
    assert!(is_tracking_tool_name("update_phase"));
    assert!(is_tracking_tool_name("update_todo"));
    assert!(!is_tracking_tool_name("read_file"));
  }
}
