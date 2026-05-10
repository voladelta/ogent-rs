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
  auto: bool,
  #[arg(long = "retry", default_value_t = 5)]
  retry: usize,
  #[arg(long, default_value_t = false)]
  worker: bool,
  #[arg(long = "max-turns", default_value_t = -1)]
  max_turns: i32,
  #[arg(long, default_value_t = -1)]
  autocompact: i32,
  #[arg(long, default_value_t = false)]
  handoff: bool,
  #[arg(long = "continue", default_value_t = false)]
  continue_flag: bool,
  #[arg(long, default_value_t = false)]
  resume: bool,
  #[arg(long, value_name = "SESSION")]
  resume_session: Option<String>,
  prompt: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
  tracing_subscriber::fmt::try_init().ok();
  let args = Args::parse();
  let profile = profiles::get_profile(&args.profile)
    .with_context(|| format!("unknown profile: {}", args.profile))?;
  let client = providers::new_client(profile, args.retry)?;
  let compact = if args.autocompact >= 0 {
    CompactState::new(
      args.autocompact as f64 / 100.0,
      args.handoff,
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
    max_turns: args.max_turns,
    turn: 0,
    flags: session::SessionFlags {
      steer: args.steer,
      auto: args.auto,
      worker: args.worker,
      autocompact: args.autocompact,
      handoff: args.handoff,
      retry: args.retry,
      continue_flag: args.continue_flag,
      resume: args.resume,
    },
    usage: session::SessionUsage {
      prompt_tokens: 0,
      completion_tokens: 0,
    },
    prompt: None,
    start_ts: None,
    end_ts: None,
  };
  let mut old_session_id: Option<String> = None;

  let is_resume = args.resume;
  let prompt = args.prompt.join(" ");
  let wait_for_steer_input =
    args.steer && !args.worker && !args.continue_flag && !is_resume && prompt.is_empty();

  let (mut messages, tools, mut task_tracker, workflow_state) = if args.worker {
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
  } else if args.continue_flag {
    let path = session::find_latest_handoff(".ogent/handoffs").context("no handoff found")?;
    eprintln!("[continue] resuming from {path}");
    let data = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let mut task_tracker = crate::task_tracker::TaskTracker::from_handoff_text(&data);
    if let Some(tracker) = task_tracker.as_mut() {
      tracker.mark_restored();
    }
    let stripped = crate::task_tracker::TaskTracker::strip_handoff_state_block(&data);
    let content =
      format!("## Previous Session Handoff\n\n{stripped}\n\nPlease continue from this handoff.");
    let mut messages = prompts::build_10x_coder_messages("");
    messages.push(Message {
      role: "user".into(),
      content,
      ..Default::default()
    });
    (
      messages,
      tools::configured_coder_tools(args.steer),
      task_tracker,
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
    let mut messages = prompts::build_10x_coder_messages(&prompt);
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
  if args.worker || args.continue_flag || is_resume || !prompt.is_empty() {
    agent.dirty = true;
  }
  let loop_result = if args.steer {
    let tui = tui::start(args.profile.clone(), profile.model.to_string(), args.auto)?;
    agent
      .steer_loop(args.max_turns, args.auto, tui, wait_for_steer_input)
      .await
  } else if args.worker {
    agent.run_loop(args.max_turns, false, true).await
  } else {
    agent.run_loop(args.max_turns, true, true).await
  };

  let final_messages = match loop_result {
    Ok(msgs) => msgs,
    Err(e) => {
      if agent.dirty {
        agent.meta.usage.prompt_tokens = agent.total_prompt;
        agent.meta.usage.completion_tokens = agent.total_completion;
        session::write_meta(&agent.meta)?;
        session::persist_session(&agent.messages, &agent.meta.session_id)?;
      }
      return Err(e.into());
    }
  };
  if agent.dirty {
    agent.meta.usage.prompt_tokens = agent.total_prompt;
    agent.meta.usage.completion_tokens = agent.total_completion;
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
