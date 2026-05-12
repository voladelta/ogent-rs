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
  #[arg(long, default_value_t = false)]
  temp: bool,
  prompt: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
  let args = Args::parse();
  let profile = profiles::get_profile(&args.profile)
    .with_context(|| format!("unknown profile: {}", args.profile))?;
  let client = providers::new_client(profile)?;
  let compact = if args.autocompact >= 0 {
    CompactState::new(f64::from(args.autocompact) / 100.0, profile.context_limit)
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
      resume: args.resume.is_some(),
      temp: args.temp,
    },
    usage: session::SessionUsage { total_tokens: 0 },
    prompt: None,
    start_ts: None,
    end_ts: None,
  };
  let mut old_session_id: Option<String> = None;

  let is_resume = args.resume.is_some();
  let prompt = args.prompt.join(" ");
  let wait_for_steer_input = args.steer && !args.worker && !is_resume && prompt.is_empty();

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
  } else if is_resume {
    let path = match args.resume {
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
    eprintln!("[resume] loading {path}");
    let mut loaded = session::load_session(&path)?;
    if !prompt.is_empty() {
      loaded.push(Message {
        role: "user".into(),
        content: prompt.clone(),
        ..Default::default()
      });
    }
    (loaded, tools::configured_coder_tools(), None, None)
  } else {
    if prompt.is_empty() && !args.steer {
      bail!("usage: ogent [--profile ...] [--steer] <prompt>");
    }
    let mut messages = prompts::build_messages(&prompt);
    prompts::enrich_initial_messages(&mut messages);
    (messages, tools::configured_coder_tools(), None, None)
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
  if !args.worker && !args.temp {
    eprintln!("\nogent resume {} to continue this session", agent.meta.session_id);
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
