use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Workflow {
  #[serde(default)]
  pub phases: HashMap<String, PhaseDef>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PhaseDef {
  #[serde(default)]
  pub next: Vec<String>,
  #[serde(default)]
  pub terminal: bool,
  #[serde(default)]
  pub gate: bool,
  #[serde(default)]
  pub max_visits: Option<u32>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum WorkflowError {
  #[error("current phase '{0}' not in workflow graph")]
  UnknownCurrentPhase(String),
  #[error("Invalid transition: '{from}' -> '{to}'. Allowed next from '{from}': {allowed:?}")]
  InvalidTransition {
    from: String,
    to: String,
    allowed: Vec<String>,
  },
  #[error("phase '{phase}' would exceed max_visits ({visits}/{max})")]
  MaxVisitsExceeded {
    phase: String,
    visits: u32,
    max: u32,
  },
}

#[derive(Debug, Clone)]
pub struct WorkflowState {
  pub definition: Workflow,
  pub current_phase: Option<String>,
  pub visits: HashMap<String, u32>,
}

impl WorkflowState {
  pub fn new(definition: Workflow) -> Self {
    Self {
      definition,
      current_phase: None,
      visits: HashMap::new(),
    }
  }

  pub fn transition_to(&mut self, phase: &str) -> Result<(), WorkflowError> {
    // Validate transition from current phase
    if let Some(ref current) = self.current_phase {
      let def = self
        .definition
        .phases
        .get(current)
        .ok_or_else(|| WorkflowError::UnknownCurrentPhase(current.clone()))?;
      if !def.next.is_empty() && !def.next.contains(&phase.to_string()) {
        return Err(WorkflowError::InvalidTransition {
          from: current.clone(),
          to: phase.to_string(),
          allowed: def.next.clone(),
        });
      }
    }

    // Check max visits for target phase BEFORE entering
    if let Some(def) = self.definition.phases.get(phase)
      && let Some(max) = def.max_visits
    {
      let visits = self.visits.get(phase).unwrap_or(&0) + 1;
      if visits > max {
        return Err(WorkflowError::MaxVisitsExceeded {
          phase: phase.to_string(),
          visits,
          max,
        });
      }
    }

    self.current_phase = Some(phase.to_string());
    *self.visits.entry(phase.to_string()).or_insert(0) += 1;
    Ok(())
  }

  pub fn reminder_text(&self) -> String {
    match &self.current_phase {
      None => "[Workflow] No current phase. Awaiting first update_phase.".to_string(),
      Some(phase) => {
        let def = self.definition.phases.get(phase);
        let visits = self.visits.get(phase).unwrap_or(&0);
        let mut s = format!("[Workflow] Phase: {}. Visits: {}.", phase, visits);
        if let Some(d) = def {
          if d.terminal {
            s.push_str(" Terminal. You may call complete.");
          } else if !d.next.is_empty() {
            s.push_str(&format!(" Next: {:?}.", d.next));
          }
          if d.gate {
            s.push_str(" Gate: explicit branch required.");
          }
        }
        s
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn test_workflow() -> Workflow {
    let mut phases = HashMap::new();
    phases.insert(
      "plan".to_string(),
      PhaseDef {
        next: vec!["implement".to_string()],
        ..Default::default()
      },
    );
    phases.insert(
      "implement".to_string(),
      PhaseDef {
        next: vec!["test".to_string()],
        ..Default::default()
      },
    );
    phases.insert(
      "test".to_string(),
      PhaseDef {
        next: vec!["done".to_string()],
        ..Default::default()
      },
    );
    phases.insert(
      "done".to_string(),
      PhaseDef {
        terminal: true,
        ..Default::default()
      },
    );
    phases.insert(
      "loop".to_string(),
      PhaseDef {
        next: vec!["verify".to_string()],
        max_visits: Some(2),
        ..Default::default()
      },
    );
    phases.insert(
      "verify".to_string(),
      PhaseDef {
        next: vec!["done".to_string(), "loop".to_string()],
        gate: true,
        ..Default::default()
      },
    );
    Workflow { phases }
  }

  #[test]
  fn valid_transition_sequence() {
    let mut ws = WorkflowState::new(test_workflow());
    assert!(ws.transition_to("plan").is_ok());
    assert!(ws.transition_to("implement").is_ok());
    assert!(ws.transition_to("test").is_ok());
    assert!(ws.transition_to("done").is_ok());
  }

  #[test]
  fn invalid_transition_rejected() {
    let mut ws = WorkflowState::new(test_workflow());
    ws.transition_to("plan").unwrap();
    let err = ws.transition_to("done").unwrap_err();
    assert!(err.to_string().contains("Invalid transition"));
    assert!(err.to_string().contains("plan"));
    assert!(err.to_string().contains("done"));
    assert!(err.to_string().contains("implement"));
  }

  #[test]
  fn max_visits_enforced() {
    let mut ws = WorkflowState::new(test_workflow());
    ws.transition_to("loop").unwrap();
    ws.transition_to("verify").unwrap();
    ws.transition_to("loop").unwrap(); // visit 2
    ws.transition_to("verify").unwrap();
    let err = ws.transition_to("loop").unwrap_err();
    assert!(err.to_string().contains("max_visits"));
    assert!(err.to_string().contains("3/2"));
  }

  #[test]
  fn max_visits_not_exceeded_when_staying_under() {
    let mut ws = WorkflowState::new(test_workflow());
    assert!(ws.transition_to("loop").is_ok());
    assert!(ws.transition_to("verify").is_ok());
    assert!(ws.transition_to("loop").is_ok());
    assert!(ws.transition_to("verify").is_ok());
  }

  #[test]
  fn reminder_for_terminal_phase() {
    let mut ws = WorkflowState::new(test_workflow());
    ws.transition_to("done").unwrap();
    let text = ws.reminder_text();
    assert!(text.contains("done"));
    assert!(text.contains("Terminal"));
  }

  #[test]
  fn reminder_for_gate_phase() {
    let mut ws = WorkflowState::new(test_workflow());
    ws.transition_to("verify").unwrap();
    let text = ws.reminder_text();
    assert!(text.contains("Gate"));
  }

  #[test]
  fn reminder_tracks_visits() {
    let mut ws = WorkflowState::new(test_workflow());
    ws.transition_to("loop").unwrap();
    assert!(ws.reminder_text().contains("Visits: 1"));
    ws.transition_to("verify").unwrap();
    ws.transition_to("loop").unwrap();
    assert!(ws.reminder_text().contains("Visits: 2"));
  }
}
