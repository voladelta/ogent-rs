use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use crate::tools::{Handler, ToolContext, ToolDef, parse_args, require_nonempty};

pub fn tools() -> Vec<ToolDef> {
  vec![ToolDef {
    name: "bash",
    description: "Execute a shell command in the workspace root and return stdout and stderr combined. Default timeout is 120s if omitted or 0; max is 600s.",
    parameters: json!({"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer","description":"Max seconds. Default: 120 if 0 or omitted. Max: 600."}},"required":["command"],"additionalProperties":false}),
    handler: Handler::Async(Box::new(|ctx, args| {
      let args = args.to_owned();
      Box::pin(async move { bash(ctx, &args).await })
    })),
  }]
}

#[derive(Deserialize)]
struct BashArgs {
  command: String,
  #[serde(default)]
  timeout_seconds: u64,
}

fn check_bash_cds(workspace: &crate::workspace::Workspace, command: &str) -> Result<()> {
  let cmd = strip_heredoc_bodies(command);
  let cmd = split_shell_separators(&cmd);
  let base = workspace.root();
  let tmp = Path::new("/tmp");
  let mut cwd = base.to_path_buf();
  for line in cmd.split('\n') {
    let mut words = line.split_whitespace();
    if words.next() == Some("cd") {
      let path = words.next().unwrap_or("");
      if path.is_empty() {
        bail!(
          "cd without argument is not allowed (would go to $HOME). Use a relative path within the workspace (e.g., ./foo) or /tmp."
        );
      }
      let target = resolve_cd_target(&cwd, path)?;
      let norm = crate::workspace::normalize(&target);
      let in_workspace = norm.starts_with(base);
      let in_tmp = norm.starts_with(tmp);
      if !in_workspace && !in_tmp {
        bail!(
          "cd to {path} is not allowed. You cannot cd outside the workspace or /tmp. Use relative paths within the workspace (e.g., ./foo or foo)."
        );
      }
      cwd = norm;
    }
  }
  Ok(())
}

fn split_shell_separators(command: &str) -> String {
  let mut cmd = command.to_string();
  for sep in ["&&", "||", "|", ";", "\n", "\r"] {
    cmd = cmd.replace(sep, "\n");
  }
  cmd
}

fn strip_heredoc_bodies(command: &str) -> String {
  let mut out = String::new();
  let mut lines = command.lines();
  while let Some(line) = lines.next() {
    out.push_str(line);
    out.push('\n');

    let Some(marker) = heredoc_marker(line) else {
      continue;
    };

    for body_line in lines.by_ref() {
      if body_line.trim() == marker {
        out.push_str(body_line);
        out.push('\n');
        break;
      }
    }
  }
  out
}

fn heredoc_marker(line: &str) -> Option<String> {
  let marker = line.split_once("<<")?.1.trim_start();
  let marker = marker
    .split_whitespace()
    .next()?
    .trim_matches(|c| matches!(c, '\'' | '"'));
  if marker.is_empty() {
    None
  } else {
    Some(marker.to_string())
  }
}

fn resolve_cd_target(base: &Path, path: &str) -> Result<PathBuf> {
  if path == "~" {
    return std::env::var_os("HOME").map(PathBuf::from).context(
      "cd to ~ is not allowed. Use a relative path within the workspace (e.g., ./foo) or /tmp.",
    );
  }
  if let Some(rest) = path.strip_prefix("~/") {
    let home = std::env::var_os("HOME").context(
      "cd to ~/... is not allowed. Use a relative path within the workspace (e.g., ./foo) or /tmp.",
    )?;
    return Ok(PathBuf::from(home).join(rest));
  }
  if path.starts_with('/') {
    return Ok(PathBuf::from(path));
  }
  Ok(base.join(path))
}

async fn bash(ctx: ToolContext, args: &str) -> Result<String> {
  let args: BashArgs = parse_args(args)?;
  require_nonempty(&args.command, "command")?;
  check_bash_cds(&ctx.workspace, &args.command)?;
  let secs = if args.timeout_seconds == 0 {
    120
  } else {
    args.timeout_seconds
  };
  if secs > 600 {
    bail!("timeout_seconds must be <= 600");
  }
  let mut cmd = Command::new("sh");
  cmd
    .arg("-c")
    .arg(&args.command)
    .current_dir(ctx.workspace.root())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
  let output = timeout(Duration::from_secs(secs), cmd.output()).await;
  match output {
    Err(_) => bail!("command timed out after {secs}s"),
    Ok(Err(e)) => bail!("exec: {e}"),
    Ok(Ok(out)) => {
      let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
      combined.push_str(&String::from_utf8_lossy(&out.stderr));
      if !out.status.success() {
        bail!("exit err: {}\n{combined}", out.status);
      }
      Ok(combined)
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::path::PathBuf;

  fn test_workspace(root: &str) -> crate::workspace::Workspace {
    crate::workspace::Workspace::from_root(PathBuf::from(root))
  }

  #[test]
  fn check_bash_cds_tracks_cwd_after_tmp_cd() {
    let ws = test_workspace("/tmp/demo");

    let err = check_bash_cds(&ws, "cd /tmp && cd ..").unwrap_err();

    assert!(err.to_string().contains("cd to .. is not allowed"));
  }

  #[test]
  fn check_bash_cds_allows_relative_tmp_child_after_tmp_cd() {
    let ws = test_workspace("/workspace/project");

    assert!(check_bash_cds(&ws, "cd /tmp && cd src").is_ok());
  }

  #[test]
  fn check_bash_cds_tracks_workspace_relative_cd_chain() {
    let ws = test_workspace("/workspace/project");

    assert!(check_bash_cds(&ws, "cd src && cd ..").is_ok());
    assert!(check_bash_cds(&ws, "cd src && cd ../..").is_err());
  }

  #[test]
  fn check_bash_cds_ignores_heredoc_body_examples() {
    let ws = test_workspace("/workspace/project");
    let command = "cat <<'EOF'\ncd /tmp && cd ..\nEOF";

    assert!(check_bash_cds(&ws, command).is_ok());
  }
}
