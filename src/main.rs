mod agent;
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
use clap::Parser;

use agent::{Agent, CompactState};
use types::Message;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser, Debug)]
struct Args {
  #[arg(long, default_value = "ds-pro")]
  profile: String,
  #[arg(long, default_value_t = false)]
  steer: bool,
  #[arg(long, default_value_t = false)]
  worker: bool,
  #[arg(long, default_value_t = -1)]
  autocompact: i32,
  #[arg(long, default_value_t = false)]
  resume: bool,
  #[arg(long, default_value_t = false)]
  temp: bool,
  #[arg(long, value_name = "SESSION")]
  resume_session: Option<String>,
  prompt: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
  let args = Args::parse();
  let profile = profiles::get_profile(&args.profile)
    .with_context(|| format!("unknown profile: {}", args.profile))?;
  let client = providers::new_client(profile)?;
  let compact = if args.autocompact >= 0 {
    CompactState::new(
      f64::from(args.autocompact) / 100.0,
      profile.context_limit,
    )
  } else {
    CompactState::disabled()
  };
  let session_id = session::generate_session_id();
  let mode = if args.worker {
    "worker"
  } else if args.steer {
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
      steer: args.steer,
      worker: args.worker,
      autocompact: args.autocompact,
      resume: args.resume,
      temp: args.temp,
    },
    usage: session::SessionUsage { total_tokens: 0 },
    prompt: None,
    start_ts: None,
    end_ts: None,
  };
  let mut old_session_id: Option<String> = None;

  let is_resume = args.resume;
  let prompt = args.prompt.join(" ");
  let wait_for_steer_input =
    args.steer && !args.worker && !is_resume && prompt.is_empty();

  let (mut messages, tools, mut task_tracker, workflow_state) = if args.worker {
    let system_prompt = read_stdin().await?.trim().to_string();
    if system_prompt.is_empty() {
      bail!("--worker requires system prompt on stdin");
    }
    (
      build_worker_messages(&system_prompt, &prompt, &session_id),
      tools::configured_worker_tools(),
      None as Option<crate::task_tracker::TaskTracker>,
      None,
    )
  } else if is_resume {
    let path = if let Some(name) = args.resume_session {
      format!(".ogent/sessions/{name}.jsonl")
    } else {
      session::find_latest_session(".ogent/sessions").context("no session found")?
    };
    old_session_id = Some(
      path
        .strip_prefix(".ogent/sessions/")
        .and_then(|p| p.strip_suffix(".jsonl"))
        .unwrap_or(&path)
        .to_string(),
    );
    eprintln!("[resume] loading {path}");
    let mut loaded = session::load_session(&path)?;
    if !prompt.is_empty() {
      loaded.push(Message {
        role: "user".into(),
        content: prompt.clone(),
        ..Default::default()
      });
    }
    (
      loaded,
      tools::configured_coder_tools(args.steer),
      None,
      None,
    )
  } else {
    if prompt.is_empty() && !args.steer {
      bail!("usage: ogent [--profile ...] [--steer] <prompt>");
    }
    let mut messages = prompts::build_messages(&prompt);
    let workflow_state = prompts::enrich_initial_messages(&mut messages);
    (
      messages,
      tools::configured_coder_tools(args.steer),
      None,
      workflow_state,
    )
  };
  if !prompt.is_empty() {
    meta.prompt = Some(prompt.clone());
    meta.start_ts = Some(session::timestamp_ms());
  }
  if let Some(ref sid) = old_session_id {
    meta.parent_session = Some(sid.clone());
    if let Ok(old_meta) = session::read_meta(sid) {
      eprintln!(
        "[resume] parent session {sid} (profile: {}, mode: {})",
        old_meta.profile, old_meta.mode
      );
    }
  }
  if let Some(tracker) = task_tracker.as_mut()
    && let Some(reminder) = tracker.take_reminder()
  {
    messages.push(Message {
      role: "user".into(),
      content: reminder,
      ..Default::default()
    });
  }

  let mut agent = Agent::new(
    client,
    messages,
    tools,
    compact,
    task_tracker,
    workflow_state,
    meta,
  );
  if args.worker || is_resume || !prompt.is_empty() {
    agent.dirty = true;
  }
    let loop_result = if args.steer {
    let tui = tui::start(args.profile.clone(), profile.model.to_string())?;
    agent
      .steer_loop(tui, wait_for_steer_input)
      .await
  } else {
    agent.run_loop().await
  };

  let final_messages = match loop_result {
    Ok(msgs) => msgs,
    Err(e) => {
      if agent.dirty && !agent.meta.flags.temp {
        agent.meta.usage.total_tokens = agent.total_tokens;
        session::write_meta(&agent.meta)?;
        session::persist_session(&agent.messages, &agent.meta.session_id)?;
      }
      return Err(e.into());
    }
  };
  if agent.dirty && !agent.meta.flags.temp {
    agent.meta.usage.total_tokens = agent.total_tokens;
    session::write_meta(&agent.meta)?;
    session::persist_session(&final_messages, &agent.meta.session_id)?;
    if let Some(summary) = agent.completion_summary.as_deref() {
      if args.worker {
        print!("{summary}");
      } else {
        session::append_journal(&agent.meta.session_id, summary)?;
      }
    }
  }
  Ok(())
}

fn build_worker_messages(system_prompt: &str, prompt: &str, session_id: &str) -> Vec<Message> {
  vec![
    Message {
      role: "system".into(),
      content: system_prompt.to_string(),
      ..Default::default()
    },
    Message {
      role: "user".into(),
      content: format!("[session: {session_id}]\n\n{prompt}"),
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
