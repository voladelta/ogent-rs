use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;
use std::fs;

use crate::hashline::{apply_anchor_edits, render_hashlines};
use crate::tools::{Handler, ToolContext, ToolDef, parse_args, require_nonempty};

pub fn tools() -> Vec<ToolDef> {
  vec![
    ToolDef {
      name: "read_file",
      description: "Read a file from the local filesystem. Use start and end as 1-indexed line numbers; omit both for the full file.",
      parameters: json!({"type":"object","properties":{"path":{"type":"string"},"start":{"type":"integer","description":"1-indexed start line (inclusive)"},"end":{"type":"integer","description":"1-indexed end line (inclusive)"}},"required":["path"],"additionalProperties":false}),
      handler: Handler::Sync(read_file),
    },
    ToolDef {
      name: "write_file",
      description: "Write content to a new file. For existing files, prefer edit_hash_anchors; set overwrite_existing=true only for intentional full-file replacement.",
      parameters: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},"overwrite_existing":{"type":"boolean"}},"required":["path","content"],"additionalProperties":false}),
      handler: Handler::Sync(write_file),
    },
    ToolDef {
      name: "read_hash_anchors",
      description: "Read a file with each line prefixed as <line>:<hash>|content, where the 4-char hash is derived from line content. Use before edit_hash_anchors to generate stable anchors.",
      parameters: json!({"type":"object","properties":{"path":{"type":"string"},"start":{"type":"integer","description":"1-indexed start line (inclusive)"},"end":{"type":"integer","description":"1-indexed end line (inclusive)"}},"required":["path"],"additionalProperties":false}),
      handler: Handler::Sync(read_hash_anchors),
    },
    ToolDef {
      name: "edit_hash_anchors",
      description: "Edit a file using hashline anchors from read_hash_anchors. Anchors must be <line>:<4-char-hash> (e.g., \"15:af63\"); use end_anchor for multi-line ranges. new_string replaces the entire anchored line(s).",
      parameters: json!({"type":"object","properties":{"path":{"type":"string"},"ops":{"type":"array","items":{"type":"object","properties":{"anchor":{"type":"string","description":"Anchor in <line-number>:<4-char-hash> format (e.g., 15:af63)"},"end_anchor":{"type":"string","description":"Optional end anchor in <line-number>:<4-char-hash> format for range replacement"},"action":{"type":"string","enum":["replace","insert_before","insert_after"]},"new_string":{"type":"string"}},"required":["anchor","action","new_string"]}}},"required":["path","ops"],"additionalProperties":false}),
      handler: Handler::Sync(edit_hash_anchors),
    },
  ]
}

#[derive(Deserialize)]
pub struct ReadFileArgs {
  pub path: String,
  pub start: Option<usize>,
  pub end: Option<usize>,
}

fn read_file(ctx: ToolContext, args: &str) -> Result<String> {
  let args: ReadFileArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  let path = ctx.workspace.readable_path(&args.path)?;
  let meta = fs::metadata(&path).with_context(|| format!("stat {}", args.path))?;
  if meta.len() > (1 << 20) {
    bail!(
      "file {} exceeds size limit ({} > {} bytes)",
      args.path,
      meta.len(),
      1 << 20
    );
  }
  let content = fs::read_to_string(&path).with_context(|| format!("read {}", args.path))?;
  if args.start.is_none() && args.end.is_none() {
    return Ok(content);
  }
  let lines: Vec<&str> = content.split('\n').collect();
  if args.start == Some(0) {
    bail!("start line must be >= 1 (lines are 1-indexed)");
  }
  if args.end == Some(0) {
    bail!("end line must be >= 1 (lines are 1-indexed)");
  }
  let start = args.start.unwrap_or(1);
  let end = args.end.unwrap_or(lines.len());
  let slice_start = (start - 1).min(lines.len());
  let slice_end = end.min(lines.len());
  if slice_start > slice_end {
    bail!("start line {start} exceeds end line {end}");
  }
  Ok(lines[slice_start..slice_end].join("\n"))
}

#[derive(Deserialize)]
struct WriteFileArgs {
  path: String,
  content: String,
  #[serde(default)]
  overwrite_existing: bool,
}

fn write_file(ctx: ToolContext, args: &str) -> Result<String> {
  let args: WriteFileArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  let path = ctx.workspace.workspace_path(&args.path)?;
  if path.exists() && !args.overwrite_existing {
    bail!(
      "file {} already exists; use edit_hash_anchors for anchored edits or set overwrite_existing=true for intentional full-file replacement",
      args.path
    );
  }
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
  }
  fs::write(&path, &args.content).with_context(|| format!("write {}", args.path))?;
  Ok(format!(
    "Wrote {} bytes to {}",
    args.content.len(),
    args.path
  ))
}

fn read_hash_anchors(ctx: ToolContext, args: &str) -> Result<String> {
  let args: ReadFileArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  if args.start == Some(0) || args.end == Some(0) {
    bail!("start and end line numbers must be >= 1 (1-indexed)");
  }
  let path = ctx.workspace.workspace_path(&args.path)?;
  let source = fs::read_to_string(&path).with_context(|| format!("read {}", args.path))?;
  Ok(render_hashlines(&source, args.start, args.end))
}

#[derive(Deserialize)]
struct EditHashAnchorsArgs {
  path: String,
  ops: Vec<crate::hashline::EditOp>,
}

fn edit_hash_anchors(ctx: ToolContext, args: &str) -> Result<String> {
  let args: EditHashAnchorsArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  if args.ops.is_empty() {
    bail!("ops array is required");
  }
  let path = ctx.workspace.workspace_path(&args.path)?;
  let source = fs::read_to_string(&path).with_context(|| format!("read {}", args.path))?;
  let out = apply_anchor_edits(&source, &args.ops)?;
  fs::write(&path, out).with_context(|| format!("write {}", args.path))?;
  Ok(format!("Applied {} edits to {}", args.ops.len(), args.path))
}
