mod agent;
mod artifact_creator;
mod client;
mod hashline;
mod profiles;
mod prompts;
mod providers;
mod session;
mod sse;
mod steer;
mod tools;
mod types;
mod websocket;
mod workers;
mod workspace;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser};
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;

use agent::{Agent, CompactState};
use artifact_creator::ArtifactAction;
use types::{Message, MessageOrigin};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser, Debug)]
struct Args {
  #[arg(long, default_value = "ds-flash")]
  profile: String,
  #[arg(long, value_name = "PARENT_SESSION_ID")]
  worker: Option<String>,
  #[arg(long, default_value_t = 80)]
  autocompact: i32,
  #[arg(long)]
  resume: Option<Option<String>>,
  #[arg(long)]
  fork: Option<Option<String>>,
  #[arg(long, default_value_t = false)]
  temp: bool,
  #[arg(long, value_name = "NAME")]
  create_skill: Option<String>,
  #[arg(long, value_name = "ADDR")]
  serve: Option<String>,
  prompt: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
  let args = parse_args();
  let workspace = if let Some(root) = std::env::var_os("OGENT_WORKSPACE_ROOT") {
    crate::workspace::Workspace::from_root(PathBuf::from(root))
  } else {
    crate::workspace::Workspace::from_current_dir()
  };
  if args.resume.is_some() && args.fork.is_some() {
    bail!("use either resume or fork, not both");
  }
  if args.serve.is_some()
    && (args.worker.is_some()
      || args.resume.is_some()
      || args.fork.is_some()
      || args.create_skill.is_some())
  {
    bail!("--serve cannot be combined with --worker, --resume, --fork, or --create-skill");
  }
  if args.serve.is_some() && !args.prompt.is_empty() {
    bail!("--serve does not accept a prompt; send messages over the websocket connection");
  }
  if let Some(addr) = args.serve.as_deref() {
    return websocket::serve(addr, &args.profile, args.autocompact, args.temp).await;
  }
  let creator_mode = args.create_skill.is_some();
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
  let compact = if args.autocompact >= 0 {
    CompactState::new(f64::from(args.autocompact) / 100.0, profile.context_limit)
  } else {
    CompactState::disabled()
  };
  let session_id = args
    .worker
    .clone()
    .unwrap_or_else(session::generate_session_id);
  let mode = if args.worker.is_some() {
    "worker"
  } else {
    "default"
  };
  let mut meta = session::SessionMeta {
    session_id: session_id.clone(),
    parent_session: None,
    profile: args.profile.clone(),
    mode: mode.to_string(),
    flags: session::SessionFlags {
      steer: false,
      worker: args.worker.is_some(),
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
  let mut session_lock: Option<session::SessionLock> = None;

  let is_resume = args.resume.is_some();
  let is_fork = args.fork.is_some();
  let is_loaded_session = is_resume || is_fork;
  let prompt = args.prompt.join(" ");

  let mut worker_parent_session_id = None;
  let mut worker_id = None;
  let (messages, tools): (Vec<Message>, Vec<crate::types::Tool>) =
    if let Some(parent_session_id) = args.worker.as_deref() {
      let system_prompt = read_stdin().await?.trim().to_string();
      if system_prompt.is_empty() {
        bail!("--worker requires system prompt on stdin");
      }
      let wid = std::env::var("OGENT_WORKER_ID")
        .context("--worker requires OGENT_WORKER_ID environment variable")?;
      worker_parent_session_id = Some(parent_session_id.to_string());
      worker_id = Some(wid);
      (
        build_worker_messages(&system_prompt, &prompt, parent_session_id),
        tools::configured_worker_tools(),
      )
    } else if is_loaded_session {
      let path = match args.resume.or(args.fork) {
        Some(Some(name)) => name,
        Some(None) => {
          session::find_latest_session(&workspace.root().join(".ogent/sessions").to_string_lossy())
            .context("no session found")?
        }
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
      let mut loaded = session::load_session_in(&workspace, &path)?;
      loaded.retain(|m| {
        !(m.role == "user"
          && m.content.is_empty()
          && m.reasoning_content.is_empty()
          && m.tool_calls.is_empty()
          && m.tool_call_id.is_empty())
      });
      if is_resume {
        meta.session_id = old_session_id.clone().expect("loaded session id");
        session_lock = Some(session::try_acquire_session_lock_in(
          &workspace,
          &meta.session_id,
        )?);
      }
      if !prompt.is_empty() {
        loaded.push(Message {
          role: "user".into(),
          content: prompt.clone(),
          origin: MessageOrigin::Human,
          ..Default::default()
        });
      }
      (loaded, tools::configured_director_tools())
    } else {
      if prompt.is_empty() {
        let mut cmd = Args::command();
        cmd.print_help()?;
        println!();
        return Ok(());
      }
      let mut messages = prompts::build_messages(&prompt);
      prompts::enrich_initial_messages(&mut messages);
      (messages, tools::configured_director_tools())
    };
  if !prompt.is_empty() {
    meta.start_ts = Some(session::timestamp_ms());
  }
  if let Some(ref sid) = old_session_id {
    let old_session_meta = session::read_meta_in(&workspace, sid).ok();
    if is_fork {
      meta.parent_session = Some(sid.clone());
    } else if let Some(ref old_meta) = old_session_meta {
      meta.parent_session = old_meta.parent_session.clone();
      meta.start_ts = old_meta.start_ts;
      meta.end_ts = old_meta.end_ts;
      meta.draft_input = old_meta.draft_input.clone();
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
  meta.flags.steer = false;
  let mut agent = Agent::new(
    workspace.clone(),
    client,
    messages,
    tools,
    compact,
    meta,
    worker_parent_session_id,
    worker_id,
  );
  if args.worker.is_some() || is_loaded_session || !prompt.is_empty() {
    agent.dirty = true;
  }
  let loop_result = agent.run_loop().await;

  if let Err(e) = loop_result {
    agent.persist_if_dirty()?;
    drop(session_lock);
    return Err(e.into());
  }
  agent.persist_if_dirty()?;
  drop(session_lock);
  if args.worker.is_some() {
    if let Some(last) = agent.last_assistant_message() {
      print!("{last}");
    }
  } else if let Some(last) = agent.last_assistant_message() {
    session::append_journal(&agent.meta.session_id, &last)?;
  }
  if args.worker.is_none() && agent.dirty && !args.temp {
    io::stdout().flush()?;
    eprintln!(
      "\nogent --resume={} to continue this session",
      agent.meta.session_id
    );
  }
  Ok(())
}

fn ensure_creator_mode_flags(args: &Args) -> Result<()> {
  if args.resume.is_some() || args.fork.is_some() || args.worker.is_some() || args.serve.is_some() {
    bail!("creator mode cannot be combined with --resume, --fork, --worker, or --serve");
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
