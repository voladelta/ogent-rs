mod agent;
mod client;
mod config;
mod hashline;
mod prompts;
mod providers;
mod session;
mod sse;
mod symbol_tree;
mod tools;
mod types;
mod workers;
mod workspace;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser};
use std::env;

use agent::{Agent, CompactState};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser, Debug)]
struct Args {
  #[arg(long)]
  profile: Option<String>,
  #[arg(long)]
  autocompact: Option<i32>,
  #[arg(long)]
  role: Option<String>,
  prompt: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
  let args = parse_args();
  if let Err(err) = tools::ensure_exa_api_key_set() {
    eprintln!("{err}");
    std::process::exit(2);
  }
  let workspace = crate::workspace::Workspace::from_current_dir();
  let config = config::load_or_exit(workspace.root());
  let profile_name = args
    .profile
    .clone()
    .unwrap_or_else(|| config.default_profile.clone());
  let autocompact = args.autocompact.unwrap_or(config.autocompact);
  if let Err(err) = ensure_run_mode_flags(&args) {
    if args.prompt.is_empty() {
      let mut cmd = Args::command();
      cmd.print_help()?;
      println!();
      return Ok(());
    }
    return Err(err);
  }
  let profile = config
    .get_profile(&profile_name)
    .with_context(|| format!("unknown profile: {}", profile_name))?;
  let provider = config
    .provider_for(profile)
    .context("missing provider config for profile")?;
  let context_limit = profile.context_limit;
  let client = providers::new_client(profile, provider)?;
  let role = args.role.as_deref().unwrap_or("ogent");
  run_worker_cli(WorkerCliRun {
    workspace,
    client,
    profile_name: &profile_name,
    context_limit,
    autocompact,
    role,
    task: &args.prompt.join(" "),
  })
  .await
}

fn ensure_run_mode_flags(args: &Args) -> Result<()> {
  if args.prompt.join(" ").trim().is_empty() {
    bail!("a task prompt is required");
  }
  Ok(())
}

struct WorkerCliRun<'a> {
  workspace: crate::workspace::Workspace,
  client: crate::client::Client,
  profile_name: &'a str,
  context_limit: usize,
  autocompact: i32,
  role: &'a str,
  task: &'a str,
}

async fn run_worker_cli(run: WorkerCliRun<'_>) -> Result<()> {
  let (system_prompt, task_prompt) =
    workers::resolve_worker_prompts(run.role, run.task, "").await?;
  let session_id = session::generate_session_id();
  let messages = workers::build_worker_messages(&system_prompt, &task_prompt, &session_id);
  let compact = if run.autocompact >= 0 {
    CompactState::new(f64::from(run.autocompact) / 100.0, run.context_limit)
  } else {
    CompactState::disabled()
  };
  let meta = session::SessionMeta {
    session_id: session_id.clone(),
    parent_session: None,
    title: None,
    profile: run.profile_name.to_string(),
    mode: "worker".to_string(),
    flags: session::SessionFlags {
      steer: false,
      worker: true,
      autocompact: run.autocompact,
      resume: false,
      temp: true,
    },
    usage: session::SessionUsage { total_tokens: 0 },
    draft_input: None,
    start_ts: Some(session::timestamp_ms()),
    end_ts: None,
  };
  let mut agent = Agent::new(
    run.workspace,
    run.client,
    messages,
    tools::configured_worker_tools_for_role(run.role),
    compact,
    meta,
    None,
    Some("worker".to_string()),
  );
  agent.set_output_sink(Some(agent::cli_output_sink()));
  agent.dirty = true;
  let loop_result = agent.run_loop().await;
  if let Err(e) = loop_result {
    agent.persist_if_dirty()?;
    return Err(e.into());
  }
  agent.persist_if_dirty()?;
  Ok(())
}

fn parse_args() -> Args {
  let mut raw: Vec<String> = env::args().collect();
  parse_args_from(&mut raw)
}

fn parse_args_from(raw: &mut [String]) -> Args {
  Args::parse_from(raw.iter())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn parse_test_args(raw: &[&str]) -> Args {
    let mut raw = raw.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
    parse_args_from(&mut raw)
  }

  #[test]
  fn parses_run_role_and_task() {
    let args = parse_test_args(&["ogent", "--role", "implementer", "fix the parser"]);
    assert_eq!(args.role.as_deref(), Some("implementer"));
    assert_eq!(args.prompt, vec!["fix the parser"]);
    assert!(ensure_run_mode_flags(&args).is_ok());
  }

  #[test]
  fn parses_run_with_profile_override() {
    let args = parse_test_args(&[
      "ogent",
      "--role",
      "reviewer",
      "--profile",
      "kimi",
      "review it",
    ]);
    assert_eq!(args.role.as_deref(), Some("reviewer"));
    assert_eq!(args.profile.as_deref(), Some("kimi"));
    assert_eq!(args.prompt, vec!["review it"]);
    assert!(ensure_run_mode_flags(&args).is_ok());
  }

  #[test]
  fn defaults_to_ogent_without_explicit_role() {
    let args = parse_test_args(&["ogent", "fix it"]);
    assert_eq!(args.role, None);
    assert_eq!(args.prompt, vec!["fix it"]);
    assert!(ensure_run_mode_flags(&args).is_ok());
  }

  #[test]
  fn run_requires_task_prompt() {
    let args = parse_test_args(&["ogent", "--role", "implementer"]);
    assert!(ensure_run_mode_flags(&args).is_err());
  }
}
