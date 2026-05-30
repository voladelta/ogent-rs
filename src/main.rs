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
mod util;
mod workspace;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use std::env;

use agent::Agent;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser, Debug)]
struct Args {
  #[arg(long)]
  profile: Option<String>,
  #[arg(short, long)]
  verbose: bool,
  prompt: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
  let args = parse_args();
  if let Err(err) = tools::ensure_exa_api_key_set() {
    eprintln!("{err}");
    std::process::exit(2);
  }
  let mut workspace = crate::workspace::Workspace::from_current_dir();
  if let Ok(home) = env::var("HOME") {
    workspace.add_allowed_root(std::path::PathBuf::from(home).join(".ogent"));
  }
  let config = config::load_or_exit(workspace.root());
  let profile_name = args
    .profile
    .clone()
    .unwrap_or_else(|| config.default_profile.clone());
  if args.prompt.is_empty() {
    let mut cmd = Args::command();
    cmd.print_help()?;
    println!();
    return Ok(());
  }
  let profile = config
    .get_profile(&profile_name)
    .with_context(|| format!("unknown profile: {}", profile_name))?;
  let provider = config
    .provider_for(profile)
    .context("missing provider config for profile")?;
  let client = providers::new_client(profile, provider)?;
  run_agent_cli(workspace, client, &args.prompt.join(" "), args.verbose).await
}

async fn run_agent_cli(
  mut workspace: crate::workspace::Workspace,
  client: crate::client::Client,
  task: &str,
  verbose: bool,
) -> Result<()> {
  let skill_store = std::sync::Arc::new(skills::SkillStore::new(workspace.root()));
  for root in skill_store.skill_roots() {
    workspace.add_allowed_root(root.clone());
  }

  let messages = prompts::build_initial_messages(task);
  let mut agent = Agent::new(
    workspace,
    client,
    messages,
    tools::configured_agent_tools(),
    session::generate_session_id(),
    skill_store,
    "director".to_string(),
    verbose,
    0,
  );
  agent.set_output_sink(Some(agent::cli_output_sink()));
  let loop_result = agent.run_loop().await;
  if let Err(e) = loop_result {
    agent.persist()?;
    return Err(e.into());
  }
  agent.persist()?;
  Ok(())
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
  fn run_requires_task_prompt() {
    let args = parse_test_args(&["ogent"]);
    assert!(args.prompt.is_empty());
  }
}
