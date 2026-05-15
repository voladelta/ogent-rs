mod agent;
mod artifact_creator;
mod client;
mod hashline;
mod profiles;
mod prompts;
mod providers;
mod session;
mod sse;
mod task_tracker;
mod tools;
mod tui;
mod types;
mod workers;
mod workflow;
mod workspace;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser};
use std::env;
use std::io::{self, Write};

use agent::{Agent, CompactState};
use artifact_creator::ArtifactAction;
use types::{Message, MessageOrigin};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser, Debug)]
struct Args {
  #[arg(long, default_value = "ds-flash")]
  profile: String,
  #[arg(long, default_value_t = false)]
  steer: bool,
  #[arg(long, default_value_t = false)]
  worker: bool,
  #[arg(long, default_value_t = 80)]
  autocompact: i32,
  #[arg(long)]
  resume: Option<Option<String>>,
  #[arg(long)]
  fork: Option<Option<String>>,
  #[arg(long, default_value_t = false)]
  temp: bool,
  #[arg(long)]
  workflow: Option<String>,
  #[arg(long, value_name = "NAME")]
  create_skill: Option<String>,
  #[arg(long, value_name = "NAME")]
  create_workflow: Option<String>,
  prompt: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
  let args = parse_args();
  if args.resume.is_some() && args.fork.is_some() {
    bail!("use either resume or fork, not both");
  }
  if args.create_skill.is_some() && args.create_workflow.is_some() {
    bail!("use either --create-skill or --create-workflow, not both");
  }
  let creator_mode = args.create_skill.is_some() || args.create_workflow.is_some();
  if creator_mode {
    ensure_creator_mode_flags(&args)?;
  }
  let profile = profiles::get_profile(&args.profile)
    .with_context(|| format!("unknown profile: {}", args.profile))?;
  let client = providers::new_client(profile)?;
  if let Some(name) = args.create_skill.as_deref() {
    let objective = args.prompt.join(" ");
    let result = artifact_creator::create_skill(&client, name, &objective).await?;
    println!(
      "{} skill: {}",
      artifact_action_verb(result.action),
      result.path.display()
    );
    return Ok(());
  }
  if let Some(name) = args.create_workflow.as_deref() {
    let objective = args.prompt.join(" ");
    let result = artifact_creator::create_workflow(&client, name, &objective).await?;
    println!(
      "{} workflow: {}",
      artifact_action_verb(result.action),
      result.path.display()
    );
    return Ok(());
  }
  let compact = if args.autocompact >= 0 {
    CompactState::new(f64::from(args.autocompact) / 100.0, profile.context_limit)
  } else {
    CompactState::disabled()
  };
  let session_id = session::generate_session_id();
  let mut run_steer = args.steer && !args.worker;
  let mut mode = if args.worker {
    "worker"
  } else if run_steer {
    "steer"
  } else {
    "default"
  };
  let mut meta = session::SessionMeta {
    session_id: session_id.clone(),
    parent_session: None,
    profile: args.profile.clone(),
    mode: mode.to_string(),
    flags: session::SessionFlags {
      steer: run_steer,
      worker: args.worker,
      autocompact: args.autocompact,
      resume: args.resume.is_some(),
      temp: args.temp,
    },
    usage: session::SessionUsage { total_tokens: 0 },
    draft_input: None,
    start_ts: None,
    end_ts: None,
  };
  let mut old_session_id: Option<String> = None;

  let is_resume = args.resume.is_some();
  let is_fork = args.fork.is_some();
  let is_loaded_session = is_resume || is_fork;
  let prompt = args.prompt.join(" ");

  let (messages, tools, task_tracker, workflow_state): (
    Vec<Message>,
    Vec<crate::types::Tool>,
    Option<crate::task_tracker::TaskTracker>,
    Option<_>,
  ) = if args.worker {
    let system_prompt = read_stdin().await?.trim().to_string();
    if system_prompt.is_empty() {
      bail!("--worker requires system prompt on stdin");
    }
    (
      build_worker_messages(&system_prompt, &prompt, &session_id),
      tools::configured_worker_tools(),
      None,
      None,
    )
  } else if is_loaded_session {
    let path = match args.resume.or(args.fork) {
      Some(Some(name)) => format!(".ogent/sessions/{name}.jsonl"),
      Some(None) => session::find_latest_session(".ogent/sessions").context("no session found")?,
      None => unreachable!(),
    };
    old_session_id = Some(
      path
        .strip_prefix(".ogent/sessions/")
        .and_then(|p| p.strip_suffix(".jsonl"))
        .unwrap_or(&path)
        .to_string(),
    );
    let load_action = if is_fork { "fork" } else { "resume" };
    eprintln!("[{load_action}] loading {path}");
    let mut loaded = session::load_session(&path)?;
    loaded.retain(|m| {
      !(m.role == "user"
        && m.content.is_empty()
        && m.reasoning_content.is_empty()
        && m.tool_calls.is_empty()
        && m.tool_call_id.is_empty())
    });
    if is_resume {
      meta.session_id = old_session_id.clone().expect("loaded session id");
    }
    if !prompt.is_empty() {
      loaded.push(Message {
        role: "user".into(),
        content: prompt.clone(),
        origin: MessageOrigin::Human,
        ..Default::default()
      });
    }
    let mut workflow_state =
      session::read_workflow_state(old_session_id.as_ref().expect("loaded session id"))?;
    if workflow_state.is_none()
      && let Some(selector) = args.workflow.as_deref()
    {
      workflow_state = Some(crate::workflow::WorkflowState::new(
        crate::workflow::load_workflow(selector)
          .with_context(|| format!("load workflow {selector}"))?,
      ));
    }
    (
      loaded,
      tools::configured_coder_tools(workflow_state.is_some()),
      None,
      workflow_state,
    )
  } else {
    if prompt.is_empty() && !args.steer && !args.worker && !is_loaded_session {
      let mut cmd = Args::command();
      cmd.print_help()?;
      println!();
      return Ok(());
    }
    let mut messages = prompts::build_messages(&prompt);
    prompts::enrich_initial_messages(&mut messages);
    let workflow_state = if let Some(selector) = args.workflow.as_deref() {
      Some(crate::workflow::WorkflowState::new(
        crate::workflow::load_workflow(selector)
          .with_context(|| format!("load workflow {selector}"))?,
      ))
    } else {
      None
    };
    (
      messages,
      tools::configured_coder_tools(workflow_state.is_some()),
      None,
      workflow_state,
    )
  };
  if !prompt.is_empty() {
    meta.start_ts = Some(session::timestamp_ms());
  }
  if let Some(ref sid) = old_session_id {
    let old_session_meta = session::read_meta(sid).ok();
    if is_fork {
      meta.parent_session = Some(sid.clone());
    } else if let Some(ref old_meta) = old_session_meta {
      meta.parent_session = old_meta.parent_session.clone();
      meta.start_ts = old_meta.start_ts;
      meta.end_ts = old_meta.end_ts;
      meta.draft_input = old_meta.draft_input.clone();
      if !args.worker && !args.steer && old_meta.mode == "steer" {
        run_steer = true;
        mode = "steer";
      }
    }
    if let Some(old_meta) = old_session_meta.as_ref() {
      eprintln!(
        "[{}] {} session {sid} (profile: {}, mode: {})",
        if is_fork { "fork" } else { "resume" },
        if is_fork { "parent" } else { "continuing" },
        old_meta.profile,
        old_meta.mode
      );
    }
  }
  if !prompt.is_empty() {
    meta.draft_input = None;
  }
  meta.mode = mode.to_string();
  meta.flags.steer = run_steer;
  let wait_for_steer_input = run_steer && prompt.is_empty();
  let initial_draft_input = meta.draft_input.clone();
  let mut agent = Agent::new(
    client,
    messages,
    tools,
    compact,
    task_tracker,
    workflow_state,
    meta,
  );
  if args.worker || is_loaded_session || !prompt.is_empty() {
    agent.dirty = true;
  }
  let loop_result = if run_steer {
    let tui = tui::start(
      args.profile.clone(),
      profile.model.to_string(),
      crate::prompts::discover_skill_names(),
      initial_draft_input,
    )?;
    if args.autocompact >= 0 {
      tui.status.set_compact_threshold(args.autocompact);
      tui.status.set_context_limit(profile.context_limit);
    }
    agent.steer_loop(tui, wait_for_steer_input).await
  } else {
    agent.run_loop().await
  };

  if let Err(e) = loop_result {
    agent.persist_if_dirty()?;
    return Err(e.into());
  }
  agent.persist_if_dirty()?;
  if let Some(summary) = agent.completion_summary.as_deref() {
    if args.worker {
      print!("{summary}");
    } else {
      session::append_journal(&agent.meta.session_id, summary)?;
    }
  }
  if !args.worker && agent.dirty && !args.temp {
    io::stdout().flush()?;
    let steer_flag = if agent.meta.mode == "steer" {
      "--steer "
    } else {
      ""
    };
    eprintln!(
      "\nogent {steer_flag}--resume={} to continue this session",
      agent.meta.session_id
    );
  }
  Ok(())
}

fn ensure_creator_mode_flags(args: &Args) -> Result<()> {
  if args.resume.is_some()
    || args.fork.is_some()
    || args.worker
    || args.steer
    || args.workflow.is_some()
  {
    bail!(
      "creator mode cannot be combined with --resume, --fork, --worker, --steer, or --workflow"
    );
  }
  if args.prompt.join(" ").trim().is_empty() {
    bail!("creator mode requires a description/objective prompt");
  }
  Ok(())
}

fn artifact_action_verb(action: ArtifactAction) -> &'static str {
  match action {
    ArtifactAction::Created => "created",
    ArtifactAction::Updated => "updated",
  }
}

fn parse_args() -> Args {
  let mut raw: Vec<String> = env::args().collect();
  if raw.len() > 1 && (raw[1] == "resume" || raw[1] == "fork") {
    raw[1] = format!("--{}", raw[1]);
  }
  Args::parse_from(raw)
}

fn build_worker_messages(system_prompt: &str, prompt: &str, session_id: &str) -> Vec<Message> {
  vec![
    Message {
      role: "system".into(),
      content: system_prompt.to_string(),
      origin: MessageOrigin::Internal,
      ..Default::default()
    },
    Message {
      role: "user".into(),
      content: format!("[session: {session_id}]\n\n{prompt}"),
      origin: MessageOrigin::Human,
      ..Default::default()
    },
  ]
}

async fn read_stdin() -> Result<String> {
  use tokio::io::AsyncReadExt;
  let mut s = String::new();
  tokio::io::stdin().read_to_string(&mut s).await?;
  Ok(s)
}
