mod agent;
mod artifact_creator;
mod client;
mod config;
mod hashline;
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

use agent::{Agent, CompactState};
use artifact_creator::ArtifactAction;
use types::{Message, MessageOrigin};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser, Debug)]
struct Args {
  #[arg(long)]
  profile: Option<String>,
  #[arg(long)]
  autocompact: Option<i32>,
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
  if let Err(err) = tools::ensure_exa_api_key_set() {
    eprintln!("{err}");
    std::process::exit(2);
  }
  let workspace = crate::workspace::Workspace::from_current_dir();
  let config = config::load_or_exit(workspace.root());
  let profile_name = args.profile.as_deref().unwrap_or(&config.default_profile);
  let autocompact = args.autocompact.unwrap_or(config.autocompact);
  if args.resume.is_some() && args.fork.is_some() {
    bail!("use either resume or fork, not both");
  }
  if args.serve.is_some()
    && (args.resume.is_some() || args.fork.is_some() || args.create_skill.is_some())
  {
    bail!("--serve cannot be combined with --resume, --fork, or --create-skill");
  }
  if args.serve.is_some() && !args.prompt.is_empty() {
    bail!("--serve does not accept a prompt; send messages over the websocket connection");
  }
  if let Some(addr) = args.serve.as_deref() {
    return websocket::serve(addr, profile_name, autocompact, args.temp, config.clone()).await;
  }
  let creator_mode = args.create_skill.is_some();
  if creator_mode {
    ensure_creator_mode_flags(&args)?;
  }
  let profile = config
    .get_profile(profile_name)
    .with_context(|| format!("unknown profile: {}", profile_name))?;
  let provider = config
    .provider_for(profile)
    .context("missing provider config for profile")?;
  let client = providers::new_client(profile, provider)?;
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
  let compact = if autocompact >= 0 {
    CompactState::new(f64::from(autocompact) / 100.0, profile.context_limit)
  } else {
    CompactState::disabled()
  };
  let session_id = session::generate_session_id();
  let mode = "default";
  let mut meta = session::SessionMeta {
    session_id: session_id.clone(),
    parent_session: None,
    title: None,
    profile: profile_name.to_string(),
    mode: mode.to_string(),
    flags: session::SessionFlags {
      steer: false,
      worker: false,
      autocompact,
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

  let (messages, tools): (Vec<Message>, Vec<crate::types::Tool>) = if is_loaded_session {
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
      meta.session_id = old_session_id
        .clone()
        .context("internal error: loaded session id missing during resume")?;
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
      meta.title = old_meta.title.clone();
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
    None,
    None,
    config.clone(),
  );
  agent.set_output_sink(Some(agent::cli_output_sink()));
  if is_loaded_session || !prompt.is_empty() {
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
  if let Some(last) = agent.last_assistant_message() {
    session::append_journal(&agent.meta.session_id, &last)?;
  }
  if agent.dirty && !args.temp {
    io::stdout().flush()?;
    eprintln!(
      "\nogent --resume={} to continue this session",
      agent.meta.session_id
    );
  }
  Ok(())
}

fn ensure_creator_mode_flags(args: &Args) -> Result<()> {
  if args.resume.is_some() || args.fork.is_some() || args.serve.is_some() {
    bail!("creator mode cannot be combined with --resume, --fork, or --serve");
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
