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

use anyhow::{Context, Result, bail};
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
  #[arg(short, long)]
  image: Option<String>,
  prompt: Vec<String>,
}

#[tokio::main]
async fn main() {
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
  if args.prompt.is_empty() && args.resume.is_none() && args.image.is_none() {
    let mut cmd = Args::command();
    if let Err(err) = cmd.print_help() {
      eprintln!("Error printing help: {err}");
      std::process::exit(1);
    }
    println!();
    std::process::exit(0);
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
  let profile = match config.get_profile(&profile_name) {
    Some(p) => p,
    None => {
      eprintln!("Error: unknown profile: {profile_name}");
      std::process::exit(3);
    }
  };
  let provider = match config.provider_for(profile) {
    Some(p) => p,
    None => {
      eprintln!("Error: missing provider config for profile");
      std::process::exit(3);
    }
  };
  let client = match providers::new_client(profile, provider) {
    Ok(c) => c,
    Err(err) => {
      let err_msg = err.to_string();
      eprintln!("Error: {err_msg}");
      if err_msg.contains("is not set") {
        std::process::exit(3);
      } else {
        std::process::exit(1);
      }
    }
  };
  let image_url = if let Some(img_arg) = &args.image {
    match process_image_arg(img_arg) {
      Ok(url) => Some(url),
      Err(err) => {
        eprintln!("Error: {err}");
        std::process::exit(1);
      }
    }
  } else {
    None
  };
  match run_agent_cli(
    workspace,
    client,
    &args.prompt.join(" "),
    args.verbose,
    args.temp,
    args.resume.as_deref(),
    image_url,
  )
  .await
  {
    Ok(()) => {
      std::process::exit(0);
    }
    Err(err) => {
      eprintln!("Error: {err}");
      let exit_code = determine_exit_code(&err);
      std::process::exit(exit_code);
    }
  }
}

async fn run_agent_cli(
  mut workspace: crate::workspace::Workspace,
  client: crate::client::Client,
  task: &str,
  verbose: bool,
  temporary: bool,
  resume_session_id: Option<&str>,
  image_url: Option<String>,
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
    let mut msgs = prompts::build_initial_messages(task);
    if let Some(ref img_url) = image_url {
      if let Some(msg) = msgs.last_mut() {
        if msg.role == crate::types::Role::User && msg.origin == crate::types::MessageOrigin::Human {
          msg.image_url = Some(img_url.clone());
          if msg.content.trim().is_empty() {
            msg.content = "What does this image show?".to_string();
          }
        }
      }
    }
    msgs
  };
  if resume_session_id.is_some() && (!task.trim().is_empty() || image_url.is_some()) {
    let text = if task.trim().is_empty() {
      "What does this image show?"
    } else {
      task.trim()
    };
    let mut msg = crate::types::Message::user(
      text,
      crate::types::MessageOrigin::Human,
    );
    if let Some(ref img_url) = image_url {
      msg.image_url = Some(img_url.clone());
    }
    messages.push(msg);
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
    eprintln!("\n\nResume this session later with: ogent -r {session_id}");
  }
  loop_result.map_err(Into::into)
}

fn load_config_or_exit(workspace_root: &std::path::Path) -> config::Config {
  match config::load_config(workspace_root) {
    Ok(cfg) => cfg,
    Err(err) => {
      eprintln!("Error: {err}");
      std::process::exit(3);
    }
  }
}

fn determine_exit_code(err: &anyhow::Error) -> i32 {
  if let Some(agent_err) = err.downcast_ref::<crate::agent::AgentError>() {
    match agent_err {
      crate::agent::AgentError::Client(client_err) => {
        return determine_client_exit_code(client_err);
      }
      crate::agent::AgentError::Other(inner_err) => {
        return determine_exit_code(inner_err);
      }
    }
  }
  if let Some(client_err) = err.downcast_ref::<crate::client::ClientError>() {
    return determine_client_exit_code(client_err);
  }
  1
}

fn determine_client_exit_code(client_err: &crate::client::ClientError) -> i32 {
  match client_err {
    crate::client::ClientError::RateLimited { .. } => 4,
    crate::client::ClientError::ApiError { .. } => 5,
    crate::client::ClientError::Http(..) => 6,
    crate::client::ClientError::Sse(..) => 7,
    crate::client::ClientError::BuildRequest(..) => 1,
  }
}

fn parse_args() -> Args {
  Args::parse_from(env::args())
}

fn process_image_arg(image_path: &str) -> Result<String> {
  if image_path.starts_with("http://") || image_path.starts_with("https://") {
    return Ok(image_path.to_string());
  }

  let path = std::path::Path::new(image_path);
  if !path.exists() {
    bail!("image file not found: {}", image_path);
  }

  let bytes = std::fs::read(path)
    .with_context(|| format!("failed to read image file: {}", image_path))?;

  let extension = path
    .extension()
    .and_then(|ext| ext.to_str())
    .unwrap_or("")
    .to_lowercase();

  let mime_type = match extension.as_str() {
    "png" => "image/png",
    "jpg" | "jpeg" => "image/jpeg",
    "gif" => "image/gif",
    "webp" => "image/webp",
    _ => "image/jpeg",
  };

  use base64::prelude::*;
  let base64_data = BASE64_STANDARD.encode(&bytes);
  Ok(format!("data:{};base64,{}", mime_type, base64_data))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_process_image_arg_remote() {
    let url = "https://example.com/test.jpg";
    let res = process_image_arg(url).unwrap();
    assert_eq!(res, url);
  }

  #[test]
  fn test_process_image_arg_local() {
    use std::io::Write;
    let mut temp = tempfile::NamedTempFile::new().unwrap();
    write!(temp, "dummy content").unwrap();

    let temp_path = temp.path().with_extension("png");
    std::fs::write(&temp_path, b"hello png").unwrap();

    let path_str = temp_path.to_str().unwrap();
    let res = process_image_arg(path_str).unwrap();
    assert!(res.starts_with("data:image/png;base64,"));

    let _ = std::fs::remove_file(temp_path);
  }

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
