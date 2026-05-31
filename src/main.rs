mod agent;
mod client;
mod config;
mod hashline;
mod prompts;
mod providers;
mod session;
mod skills;
mod sse;
mod tools;
mod types;
mod workspace;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use std::env;

use agent::Agent;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser, Debug)]
struct Args {
  #[arg(short, long)]
  profile: Option<String>,
  #[arg(short, long)]
  verbose: bool,
  #[arg(short, long)]
  temp: bool,
  #[arg(short, long)]
  resume: Option<String>,
  prompt: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
  let args = parse_args();
  let mut workspace = crate::workspace::Workspace::from_current_dir();
  if let Ok(home) = env::var("HOME") {
    workspace.add_allowed_root(std::path::PathBuf::from(home).join(".ogent"));
  }
  let config = load_config_or_exit(workspace.root());
  let profile_name = args
    .profile
    .clone()
    .unwrap_or_else(|| config.default_profile.clone());
  if args.prompt.is_empty() && args.resume.is_none() {
    let mut cmd = Args::command();
    cmd.print_help()?;
    println!();
    return Ok(());
  }
  if let Some(session_id) = &args.resume
    && let Err(err) = session::load_session_in(&workspace, session_id)
  {
    eprintln!("Error: {err}");
    std::process::exit(1);
  }
  if let Err(err) = tools::ensure_exa_api_key_set() {
    eprintln!("{err}");
    std::process::exit(2);
  }
  let profile = config
    .get_profile(&profile_name)
    .with_context(|| format!("unknown profile: {}", profile_name))?;
  let provider = config
    .provider_for(profile)
    .context("missing provider config for profile")?;
  let client = providers::new_client(profile, provider)?;
  run_agent_cli(
    workspace,
    client,
    &args.prompt.join(" "),
    args.verbose,
    args.temp,
    args.resume.as_deref(),
  )
  .await
}

async fn run_agent_cli(
  mut workspace: crate::workspace::Workspace,
  client: crate::client::Client,
  task: &str,
  verbose: bool,
  temporary: bool,
  resume_session_id: Option<&str>,
) -> Result<()> {
  let skill_store = std::sync::Arc::new(skills::SkillStore::new(workspace.root()));
  for root in skill_store.skill_roots() {
    workspace.add_allowed_root(root.clone());
  }

  let session_id = resume_session_id
    .map(str::to_string)
    .unwrap_or_else(session::generate_session_id);
  let mut messages = if let Some(session_id) = resume_session_id {
    session::load_session_in(&workspace, session_id)?
  } else {
    prompts::build_initial_messages(task)
  };
  if resume_session_id.is_some() && !task.trim().is_empty() {
    messages.push(crate::types::Message::user(
      task.trim(),
      crate::types::MessageOrigin::Human,
    ));
  }
  let mut agent = Agent::new(
    workspace,
    client,
    messages,
    tools::agent_tools(),
    session_id.clone(),
    skill_store,
    "director".to_string(),
    verbose,
    0,
  );
  agent.set_output_sink(Some(agent::cli_output_sink()));
  let loop_result = agent.run_loop().await;
  if !temporary && let Err(pe) = agent.persist() {
    eprintln!("warning: failed to persist session: {pe}");
  }
  if !temporary {
    println!("\n\nResume this session later with: ogent -r {session_id}");
  }
  loop_result.map_err(Into::into)
}

fn load_config_or_exit(workspace_root: &std::path::Path) -> config::Config {
  match config::load_config(workspace_root) {
    Ok(cfg) => cfg,
    Err(err) => {
      eprintln!("Error: {err}");
      std::process::exit(1);
    }
  }
}

fn parse_args() -> Args {
  Args::parse_from(env::args())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn parse_test_args(raw: &[&str]) -> Args {
    Args::parse_from(raw.iter().copied())
  }

  #[test]
  fn parses_run_task() {
    let args = parse_test_args(&["ogent", "fix the parser"]);
    assert_eq!(args.prompt, vec!["fix the parser"]);
  }

  #[test]
  fn parses_run_with_profile_override() {
    let args = parse_test_args(&["ogent", "--profile", "kimi", "review it"]);
    assert_eq!(args.profile.as_deref(), Some("kimi"));
    assert_eq!(args.prompt, vec!["review it"]);
  }

  #[test]
  fn parses_temporary_run() {
    let args = parse_test_args(&["ogent", "--temp", "scratch"]);
    assert!(args.temp);
    assert_eq!(args.prompt, vec!["scratch"]);
  }

  #[test]
  fn parses_resume_session() {
    let args = parse_test_args(&["ogent", "-r", "abc-123"]);
    assert_eq!(args.resume.as_deref(), Some("abc-123"));
    assert!(args.prompt.is_empty());
  }

  #[test]
  fn run_requires_task_prompt() {
    let args = parse_test_args(&["ogent"]);
    assert!(args.prompt.is_empty());
  }
}
