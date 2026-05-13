use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
  pub id: String,
  pub name: String,
  #[serde(default = "default_version")]
  pub version: u32,
  pub start: String,
  #[serde(default)]
  pub instructions: String,
  #[serde(default)]
  pub steps: HashMap<String, WorkflowStep>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowStep {
  #[serde(default)]
  pub title: String,
  #[serde(default)]
  pub instructions: String,
  #[serde(default)]
  pub next: Vec<String>,
  #[serde(default)]
  pub terminal: bool,
  #[serde(default)]
  pub gate: bool,
  #[serde(default)]
  pub max_visits: Option<u32>,
  #[serde(default)]
  pub checks: Vec<WorkflowCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowCheck {
  pub id: String,
  #[serde(rename = "type")]
  pub kind: CheckKind,
  #[serde(default)]
  pub required: bool,
  #[serde(default)]
  pub command: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
  Manual,
  Command,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
  Passed,
  Failed,
  Waived,
}

impl CheckStatus {
  fn satisfies_required(self) -> bool {
    matches!(self, Self::Passed | Self::Waived)
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckResult {
  pub step_id: String,
  pub check_id: String,
  pub status: CheckStatus,
  #[serde(default)]
  pub evidence: String,
  #[serde(default)]
  pub waiver_reason: String,
  #[serde(default)]
  pub waiver_risk: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub command: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub exit_code: Option<i32>,
  #[serde(default)]
  pub timestamp_ms: u64,
}

pub struct ManualCheckInput<'a> {
  pub step_id: &'a str,
  pub check_id: &'a str,
  pub status: CheckStatus,
  pub evidence: &'a str,
  pub waiver_reason: &'a str,
  pub waiver_risk: &'a str,
  pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransitionRecord {
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub from: Option<String>,
  pub to: String,
  #[serde(default)]
  pub reason: String,
  #[serde(default)]
  pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowState {
  pub definition: Workflow,
  pub current_step: Option<String>,
  #[serde(default)]
  pub visits: HashMap<String, u32>,
  #[serde(default)]
  pub check_results: HashMap<String, HashMap<String, CheckResult>>,
  #[serde(default)]
  pub transition_log: Vec<TransitionRecord>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum WorkflowError {
  #[error("workflow id is required")]
  MissingId,
  #[error("workflow name is required")]
  MissingName,
  #[error("workflow start step is required")]
  MissingStart,
  #[error("workflow start step '{0}' does not exist")]
  UnknownStart(String),
  #[error("workflow must define at least one step")]
  EmptySteps,
  #[error("workflow must define at least one terminal step")]
  NoTerminalStep,
  #[error("step '{step}' points to unknown next step '{next}'")]
  UnknownNext { step: String, next: String },
  #[error("step '{0}' is non-terminal but has no next steps")]
  NonTerminalWithoutNext(String),
  #[error("step '{0}' has max_visits=0")]
  ZeroMaxVisits(String),
  #[error("step '{step}' has duplicate check id '{check}'")]
  DuplicateCheck { step: String, check: String },
  #[error("step '{step}' has check with empty id")]
  EmptyCheckId { step: String },
  #[error("step '{step}' command check '{check}' has empty command")]
  EmptyCommandCheck { step: String, check: String },
  #[error("step '{0}' is unreachable from start")]
  UnreachableStep(String),
  #[error("current step '{0}' not in workflow")]
  UnknownCurrentStep(String),
  #[error("target step '{0}' not in workflow")]
  UnknownTargetStep(String),
  #[error("first workflow step must be start step '{start}', got '{got}'")]
  InvalidStartTransition { start: String, got: String },
  #[error("invalid transition: '{from}' -> '{to}'. Allowed next from '{from}': {allowed:?}")]
  InvalidTransition {
    from: String,
    to: String,
    allowed: Vec<String>,
  },
  #[error("leaving step '{step}' requires completed checks: {missing:?}")]
  RequiredChecksPending { step: String, missing: Vec<String> },
  #[error("step '{step}' is gated; transition to '{to}' requires a reason")]
  GateReasonRequired { step: String, to: String },
  #[error("step '{step}' would exceed max_visits ({visits}/{max})")]
  MaxVisitsExceeded { step: String, visits: u32, max: u32 },
  #[error("check '{check}' not found in step '{step}'")]
  UnknownCheck { step: String, check: String },
  #[error("manual check '{check}' in step '{step}' requires evidence")]
  EvidenceRequired { step: String, check: String },
  #[error("waived check '{check}' in step '{step}' requires waiver_reason and waiver_risk")]
  WaiverDetailsRequired { step: String, check: String },
  #[error("workflow has not started")]
  NotStarted,
  #[error("current step '{0}' is not terminal")]
  NotTerminal(String),
}

fn default_version() -> u32 {
  1
}

impl Workflow {
  pub fn validate(&self) -> Result<(), WorkflowError> {
    if self.id.trim().is_empty() {
      return Err(WorkflowError::MissingId);
    }
    if self.name.trim().is_empty() {
      return Err(WorkflowError::MissingName);
    }
    if self.start.trim().is_empty() {
      return Err(WorkflowError::MissingStart);
    }
    if self.steps.is_empty() {
      return Err(WorkflowError::EmptySteps);
    }
    if !self.steps.contains_key(&self.start) {
      return Err(WorkflowError::UnknownStart(self.start.clone()));
    }
    if !self.steps.values().any(|step| step.terminal) {
      return Err(WorkflowError::NoTerminalStep);
    }

    for (step_id, step) in &self.steps {
      if !step.terminal && step.next.is_empty() {
        return Err(WorkflowError::NonTerminalWithoutNext(step_id.clone()));
      }
      if step.max_visits == Some(0) {
        return Err(WorkflowError::ZeroMaxVisits(step_id.clone()));
      }
      for next in &step.next {
        if !self.steps.contains_key(next) {
          return Err(WorkflowError::UnknownNext {
            step: step_id.clone(),
            next: next.clone(),
          });
        }
      }

      let mut seen = HashSet::new();
      for check in &step.checks {
        if check.id.trim().is_empty() {
          return Err(WorkflowError::EmptyCheckId {
            step: step_id.clone(),
          });
        }
        if !seen.insert(check.id.clone()) {
          return Err(WorkflowError::DuplicateCheck {
            step: step_id.clone(),
            check: check.id.clone(),
          });
        }
        if check.kind == CheckKind::Command
          && check
            .command
            .as_ref()
            .is_some_and(|cmd| cmd.trim().is_empty())
        {
          return Err(WorkflowError::EmptyCommandCheck {
            step: step_id.clone(),
            check: check.id.clone(),
          });
        }
      }
    }

    let reachable = self.reachable_steps();
    for step_id in self.steps.keys() {
      if !reachable.contains(step_id) {
        return Err(WorkflowError::UnreachableStep(step_id.clone()));
      }
    }
    Ok(())
  }

  fn reachable_steps(&self) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut stack = vec![self.start.clone()];
    while let Some(step_id) = stack.pop() {
      if !seen.insert(step_id.clone()) {
        continue;
      }
      if let Some(step) = self.steps.get(&step_id) {
        stack.extend(step.next.iter().cloned());
      }
    }
    seen
  }
}

impl WorkflowState {
  pub fn new(definition: Workflow) -> Self {
    Self {
      definition,
      current_step: None,
      visits: HashMap::new(),
      check_results: HashMap::new(),
      transition_log: Vec::new(),
    }
  }

  pub fn enter_step(
    &mut self,
    step_id: &str,
    reason: &str,
    timestamp_ms: u64,
  ) -> Result<(), WorkflowError> {
    self.definition.validate()?;
    let step_id = step_id.trim();
    let target = self
      .definition
      .steps
      .get(step_id)
      .ok_or_else(|| WorkflowError::UnknownTargetStep(step_id.to_string()))?;

    if let Some(current_id) = self.current_step.clone() {
      let current = self
        .definition
        .steps
        .get(&current_id)
        .ok_or_else(|| WorkflowError::UnknownCurrentStep(current_id.clone()))?;
      if !current.next.iter().any(|next| next == step_id) {
        return Err(WorkflowError::InvalidTransition {
          from: current_id.clone(),
          to: step_id.to_string(),
          allowed: current.next.clone(),
        });
      }
      if current.gate && reason.trim().is_empty() {
        return Err(WorkflowError::GateReasonRequired {
          step: current_id.clone(),
          to: step_id.to_string(),
        });
      }
      self.ensure_required_checks_satisfied(&current_id)?;
    } else if step_id != self.definition.start {
      return Err(WorkflowError::InvalidStartTransition {
        start: self.definition.start.clone(),
        got: step_id.to_string(),
      });
    }

    if let Some(max) = target.max_visits {
      let visits = self.visits.get(step_id).copied().unwrap_or(0) + 1;
      if visits > max {
        return Err(WorkflowError::MaxVisitsExceeded {
          step: step_id.to_string(),
          visits,
          max,
        });
      }
    }

    let from = self.current_step.clone();
    self.current_step = Some(step_id.to_string());
    *self.visits.entry(step_id.to_string()).or_insert(0) += 1;
    self.transition_log.push(TransitionRecord {
      from,
      to: step_id.to_string(),
      reason: reason.trim().to_string(),
      timestamp_ms,
    });
    Ok(())
  }

  pub fn record_check(&mut self, input: ManualCheckInput<'_>) -> Result<(), WorkflowError> {
    let step_id = input.step_id;
    let check_id = input.check_id;
    let check = self.find_check(step_id, check_id)?;
    if input.status != CheckStatus::Waived && input.evidence.trim().is_empty() {
      return Err(WorkflowError::EvidenceRequired {
        step: step_id.to_string(),
        check: check_id.to_string(),
      });
    }
    if input.status == CheckStatus::Waived
      && (input.waiver_reason.trim().is_empty() || input.waiver_risk.trim().is_empty())
    {
      return Err(WorkflowError::WaiverDetailsRequired {
        step: step_id.to_string(),
        check: check_id.to_string(),
      });
    }
    let result = CheckResult {
      step_id: step_id.to_string(),
      check_id: check.id.clone(),
      status: input.status,
      evidence: input.evidence.trim().to_string(),
      waiver_reason: input.waiver_reason.trim().to_string(),
      waiver_risk: input.waiver_risk.trim().to_string(),
      command: None,
      exit_code: None,
      timestamp_ms: input.timestamp_ms,
    };
    self
      .check_results
      .entry(step_id.to_string())
      .or_default()
      .insert(check_id.to_string(), result);
    Ok(())
  }

  pub fn record_command_check(
    &mut self,
    step_id: &str,
    check_id: &str,
    command: &str,
    exit_code: i32,
    evidence: &str,
    timestamp_ms: u64,
  ) -> Result<(), WorkflowError> {
    let check = self.find_check(step_id, check_id)?;
    if check.kind != CheckKind::Command {
      return Err(WorkflowError::UnknownCheck {
        step: step_id.to_string(),
        check: check_id.to_string(),
      });
    }
    let status = if exit_code == 0 {
      CheckStatus::Passed
    } else {
      CheckStatus::Failed
    };
    let result = CheckResult {
      step_id: step_id.to_string(),
      check_id: check.id.clone(),
      status,
      evidence: evidence.trim().to_string(),
      waiver_reason: String::new(),
      waiver_risk: String::new(),
      command: Some(command.trim().to_string()),
      exit_code: Some(exit_code),
      timestamp_ms,
    };
    self
      .check_results
      .entry(step_id.to_string())
      .or_default()
      .insert(check_id.to_string(), result);
    Ok(())
  }

  pub fn command_for_check(
    &self,
    step_id: &str,
    check_id: &str,
  ) -> Result<Option<String>, WorkflowError> {
    Ok(self.find_check(step_id, check_id)?.command.clone())
  }

  pub fn ensure_current_step_is_terminal(&self) -> Result<(), WorkflowError> {
    let step_id = self
      .current_step
      .as_ref()
      .ok_or(WorkflowError::NotStarted)?;
    let step = self
      .definition
      .steps
      .get(step_id)
      .ok_or_else(|| WorkflowError::UnknownCurrentStep(step_id.clone()))?;
    if !step.terminal {
      return Err(WorkflowError::NotTerminal(step_id.clone()));
    }
    self.ensure_required_checks_satisfied(step_id)
  }

  pub fn reminder_text(&self) -> String {
    let mut s = format!(
      "Active: {} ({})\nInstructions: {}",
      self.definition.name,
      self.definition.id,
      empty_dash(&self.definition.instructions)
    );
    match &self.current_step {
      None => {
        s.push_str(&format!(
          "\nCurrent step: none\nStart step: {}",
          self.definition.start
        ));
      }
      Some(step_id) => {
        if let Some(step) = self.definition.steps.get(step_id) {
          let visits = self.visits.get(step_id).copied().unwrap_or(0);
          s.push_str(&format!(
            "\nCurrent step: {step_id}\nTitle: {}\nStep instructions: {}\nVisits: {visits}",
            empty_dash(&step.title),
            empty_dash(&step.instructions)
          ));
          if step.terminal {
            s.push_str("\nTerminal: true");
          } else {
            s.push_str(&format!("\nAllowed next: {:?}", step.next));
          }
          if step.gate {
            s.push_str("\nGate: transition requires reason");
          }
          if !step.checks.is_empty() {
            s.push_str("\nChecks:");
            for check in &step.checks {
              let status = self
                .check_results
                .get(step_id)
                .and_then(|checks| checks.get(&check.id))
                .map(|r| format!("{:?}", r.status).to_lowercase())
                .unwrap_or_else(|| "pending".to_string());
              s.push_str(&format!(
                "\n- {} ({:?}, required={}): {}",
                check.id, check.kind, check.required, status
              ));
            }
          }
        } else {
          s.push_str(&format!("\nCurrent step: {step_id} (unknown)"));
        }
      }
    }
    s
  }

  pub fn render_status(&self) -> String {
    format!("[Workflow]\n{}", self.reminder_text())
  }

  fn ensure_required_checks_satisfied(&self, step_id: &str) -> Result<(), WorkflowError> {
    let step = self
      .definition
      .steps
      .get(step_id)
      .ok_or_else(|| WorkflowError::UnknownCurrentStep(step_id.to_string()))?;
    let results = self.check_results.get(step_id);
    let missing: Vec<String> = step
      .checks
      .iter()
      .filter(|check| check.required)
      .filter(|check| {
        !results
          .and_then(|r| r.get(&check.id))
          .is_some_and(|result| result.status.satisfies_required())
      })
      .map(|check| check.id.clone())
      .collect();
    if missing.is_empty() {
      Ok(())
    } else {
      Err(WorkflowError::RequiredChecksPending {
        step: step_id.to_string(),
        missing,
      })
    }
  }

  fn find_check(&self, step_id: &str, check_id: &str) -> Result<&WorkflowCheck, WorkflowError> {
    let step = self
      .definition
      .steps
      .get(step_id)
      .ok_or_else(|| WorkflowError::UnknownTargetStep(step_id.to_string()))?;
    step
      .checks
      .iter()
      .find(|check| check.id == check_id)
      .ok_or_else(|| WorkflowError::UnknownCheck {
        step: step_id.to_string(),
        check: check_id.to_string(),
      })
  }
}

fn empty_dash(s: &str) -> &str {
  if s.trim().is_empty() { "-" } else { s }
}

pub fn load_workflow(selector: &str) -> anyhow::Result<Workflow> {
  let content = std::fs::read_to_string(resolve_workflow_path(selector)?)?;
  let workflow: Workflow = serde_yaml::from_str(&content)?;
  workflow.validate()?;
  Ok(workflow)
}

fn resolve_workflow_path(selector: &str) -> anyhow::Result<PathBuf> {
  let explicit = PathBuf::from(selector);
  if explicit.exists() {
    return Ok(explicit);
  }
  let builtin = Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("workflows")
    .join(format!("{selector}.yaml"));
  if builtin.exists() {
    return Ok(builtin);
  }
  anyhow::bail!(
    "workflow '{selector}' not found. Use a file path or a built-in workflow name from workflows/"
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  fn test_workflow() -> Workflow {
    load_workflow("common-sw").unwrap()
  }

  #[test]
  fn validates_common_sw() {
    test_workflow().validate().unwrap();
  }

  #[test]
  fn invalid_transition_rejected() {
    let mut ws = WorkflowState::new(test_workflow());
    ws.enter_step("intake", "", 1).unwrap();
    ws.record_check(ManualCheckInput {
      step_id: "intake",
      check_id: "scope",
      status: CheckStatus::Passed,
      evidence: "scope ok",
      waiver_reason: "",
      waiver_risk: "",
      timestamp_ms: 2,
    })
    .unwrap();
    let err = ws.enter_step("verify", "", 3).unwrap_err();
    assert!(err.to_string().contains("invalid transition"));
  }

  #[test]
  fn first_step_must_be_start() {
    let mut ws = WorkflowState::new(test_workflow());
    let err = ws.enter_step("execute", "", 1).unwrap_err();
    assert!(err.to_string().contains("first workflow step"));
  }

  #[test]
  fn required_checks_block_leaving_step() {
    let mut ws = WorkflowState::new(test_workflow());
    ws.enter_step("intake", "", 1).unwrap();
    let err = ws.enter_step("execute", "", 2).unwrap_err();
    assert!(err.to_string().contains("requires completed checks"));
  }

  #[test]
  fn gate_requires_reason() {
    let mut ws = WorkflowState::new(test_workflow());
    ws.enter_step("intake", "", 1).unwrap();
    ws.record_check(ManualCheckInput {
      step_id: "intake",
      check_id: "scope",
      status: CheckStatus::Passed,
      evidence: "scope ok",
      waiver_reason: "",
      waiver_risk: "",
      timestamp_ms: 2,
    })
    .unwrap();
    ws.enter_step("execute", "", 3).unwrap();
    ws.record_check(ManualCheckInput {
      step_id: "execute",
      check_id: "work_done",
      status: CheckStatus::Passed,
      evidence: "work ok",
      waiver_reason: "",
      waiver_risk: "",
      timestamp_ms: 4,
    })
    .unwrap();
    ws.enter_step("verify", "", 5).unwrap();
    ws.record_check(ManualCheckInput {
      step_id: "verify",
      check_id: "verification",
      status: CheckStatus::Passed,
      evidence: "tests ok",
      waiver_reason: "",
      waiver_risk: "",
      timestamp_ms: 6,
    })
    .unwrap();
    let err = ws.enter_step("review", "", 7).unwrap_err();
    assert!(err.to_string().contains("requires a reason"));
  }

  #[test]
  fn complete_requires_terminal_step() {
    let mut ws = WorkflowState::new(test_workflow());
    assert!(ws.ensure_current_step_is_terminal().is_err());
    ws.enter_step("intake", "", 1).unwrap();
    assert!(ws.ensure_current_step_is_terminal().is_err());
  }

  #[test]
  fn terminal_step_allows_complete() {
    let mut ws = WorkflowState::new(test_workflow());
    ws.enter_step("intake", "", 1).unwrap();
    ws.record_check(ManualCheckInput {
      step_id: "intake",
      check_id: "scope",
      status: CheckStatus::Passed,
      evidence: "scope ok",
      waiver_reason: "",
      waiver_risk: "",
      timestamp_ms: 2,
    })
    .unwrap();
    ws.enter_step("execute", "", 3).unwrap();
    ws.record_check(ManualCheckInput {
      step_id: "execute",
      check_id: "work_done",
      status: CheckStatus::Passed,
      evidence: "work ok",
      waiver_reason: "",
      waiver_risk: "",
      timestamp_ms: 4,
    })
    .unwrap();
    ws.enter_step("verify", "", 5).unwrap();
    ws.record_check(ManualCheckInput {
      step_id: "verify",
      check_id: "verification",
      status: CheckStatus::Passed,
      evidence: "tests ok",
      waiver_reason: "",
      waiver_risk: "",
      timestamp_ms: 6,
    })
    .unwrap();
    ws.enter_step("review", "tests passed", 7).unwrap();
    ws.record_check(ManualCheckInput {
      step_id: "review",
      check_id: "self_review",
      status: CheckStatus::Passed,
      evidence: "diff ok",
      waiver_reason: "",
      waiver_risk: "",
      timestamp_ms: 8,
    })
    .unwrap();
    ws.enter_step("done", "ready", 9).unwrap();
    ws.ensure_current_step_is_terminal().unwrap();
  }
}
