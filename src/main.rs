mod agent;
mod client;
mod hashline;
mod profiles;
mod prompts;
mod providers;
mod session;
mod sse;
mod toolimpl;
mod tools;
mod tui;
mod types;
mod workers;
mod workspace;

use anyhow::{Result, bail};
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
  prompt: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
  tracing_subscriber::fmt::try_init().ok();
  let args = Args::parse();
  let profile = profiles::get_profile(&args.profile)
    .ok_or_else(|| anyhow::anyhow!("unknown profile: {}", args.profile))?;
  let client = providers::new_client(&profile, args.retry)?;
  let compact = if args.autocompact >= 0 {
    CompactState::new(
      args.autocompact as f64 / 100.0,
      args.handoff,
      profile.context_limit,
    )
  } else {
    CompactState::disabled()
  };
  let session_id = format!("{}-{:04x}", session::timestamp(), std::process::id());

  let wait_for_steer_input =
    args.steer && !args.worker && !args.continue_flag && args.prompt.is_empty();

  let (mut messages, tools) = if args.worker {
    let system_prompt = read_stdin().await?.trim().to_string();
    if system_prompt.is_empty() {
      bail!("--worker requires system prompt on stdin");
    }
    let prompt = args.prompt.join(" ");
    (
      build_worker_messages(&system_prompt, &prompt, &session_id),
      tools::configured_worker_tools(),
    )
  } else if args.continue_flag {
    let path = session::find_latest_handoff(".ogent/handoffs")
      .ok_or_else(|| anyhow::anyhow!("no handoff found"))?;
    eprintln!("[continue] resuming from {path}");
    let data = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let content =
      format!("## Previous Session Handoff\n\n{data}\n\nPlease continue from this handoff.");
    let mut messages = build_10x_coder_messages("");
    messages.push(Message {
      role: "user".into(),
      content,
      ..Default::default()
    });
    (messages, tools::configured_coder_tools(args.steer))
  } else {
    if args.prompt.is_empty() && !args.steer {
      bail!("usage: ogent [--profile ...] [--steer] <prompt>");
    }
    let prompt = args.prompt.join(" ");
    let messages = build_10x_coder_messages(&prompt);
    (messages, tools::configured_coder_tools(args.steer))
  };

  append_to_last_user_message(&mut messages, &prompts::discover_skills_message());
  if let Ok((name, root, body)) = prompts::load_skill_content("colgrep") {
    append_to_last_user_message(
      &mut messages,
      &format!("<skill name=\"{name}\" root=\"{root}\">\n{body}\n</skill>"),
    );
  }
  let cwd_msg = current_working_directory_reminder();
  if !cwd_msg.is_empty() {
    append_to_last_user_message(&mut messages, &cwd_msg);
  }

  let mut agent = Agent::new(client, messages, tools, compact);
  let final_messages = if args.steer {
    let tui = tui::start(args.profile.clone(), profile.model.to_string(), args.auto)?;
    agent
      .steer_loop(args.max_turns, args.auto, tui, wait_for_steer_input)
      .await?
  } else if args.worker {
    agent.run_loop(args.max_turns, false, true).await?
  } else {
    agent.run_loop(args.max_turns, true, true).await?
  };
  session::persist_session(&final_messages, args.worker, &session_id)?;
  if !args.worker {
    if let Some(summary) = agent.completion_summary.as_deref() {
      session::append_journal(&session_id, summary)?;
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

fn build_10x_coder_messages(prompt: &str) -> Vec<Message> {
  vec![
    Message {
      role: "system".into(),
      content: prompts::TENX_CODER_SYSTEM_PROMPT.to_string(),
      ..Default::default()
    },
    Message {
      role: "user".into(),
      content: prompt.to_string(),
      ..Default::default()
    },
  ]
}

fn append_to_last_user_message(messages: &mut Vec<Message>, content: &str) {
  if content.is_empty() {
    return;
  }
  if let Some(message) = messages.iter_mut().rev().find(|m| m.role == "user") {
    if message.content.is_empty() {
      message.content = content.to_string();
    } else {
      message.content.push_str("\n\n");
      message.content.push_str(content);
    }
  }
}

fn current_working_directory_reminder() -> String {
  std::env::current_dir().map_or(String::new(), |cwd| format!(
    "<system_reminder kind=\"file_state\">\nmacOS: Tahoe 26.3\nCurrent working directory: {}\n</system_reminder>",
    cwd.display()
  ))
}

async fn read_stdin() -> Result<String> {
  use tokio::io::AsyncReadExt;
  let mut s = String::new();
  tokio::io::stdin().read_to_string(&mut s).await?;
  Ok(s)
}
