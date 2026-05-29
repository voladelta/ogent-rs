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
      description: "Read a file from the local filesystem with optional byte offset and limit.",
      parameters: json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","description":"0-indexed byte offset (inclusive)"},"limit":{"type":"integer","description":"max bytes to read"}},"required":["path"],"additionalProperties":false}),
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
      description: "Read a file with each line prefixed as <line>:<hash>|content, filtered by optional byte offset and limit.",
      parameters: json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","description":"0-indexed byte offset (inclusive)"},"limit":{"type":"integer","description":"max bytes to read"}},"required":["path"],"additionalProperties":false}),
      handler: Handler::Sync(read_hash_anchors),
    },
    ToolDef {
      name: "edit_hash_anchors",
      description: "Edit a file using hashline anchors from read_hash_anchors. Anchors must be <line>:<4-char-hash> (e.g., \"15:af63\"); use end_at for multi-line ranges.",
      parameters: json!({
        "type": "object",
        "properties": {
          "path": {"type": "string"},
          "ops": {
            "type": "array",
            "items": {
              "type": "object",
              "properties": {
                "start_at": {"type": "string", "description": "Anchor in <line-number>:<4-char-hash> format (e.g., 15:af63)"},
                "end_at": {"type": "string", "description": "Optional end anchor in <line-number>:<4-char-hash> format for range replacement"},
                "action": {"type": "string", "enum": ["replace", "delete", "insert_before", "insert_after"]},
                "content": {"type": "string", "description": "new content to insert/replace"}
              },
              "required": ["start_at", "action"]
            }
          }
        },
        "required": ["path", "ops"],
        "additionalProperties": false
      }),
      handler: Handler::Sync(edit_hash_anchors),
    },
  ]
}

#[derive(Deserialize)]
pub struct ReadFileArgs {
  pub path: String,
  pub offset: Option<usize>,
  pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct ReadHashAnchorsArgs {
  pub path: String,
  pub offset: Option<usize>,
  pub limit: Option<usize>,
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
  let bytes = fs::read(&path).with_context(|| format!("read {}", args.path))?;
  let offset = args.offset.unwrap_or(0).min(bytes.len());
  let limit = args.limit.unwrap_or(bytes.len()).min(bytes.len() - offset);
  let slice = &bytes[offset..(offset + limit)];
  Ok(String::from_utf8_lossy(slice).into_owned())
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
  let args: ReadHashAnchorsArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  let path = ctx.workspace.workspace_path(&args.path)?;
  let bytes = fs::read(&path).with_context(|| format!("read {}", args.path))?;
  let source = String::from_utf8_lossy(&bytes);

  let offset = args.offset.unwrap_or(0).min(bytes.len());
  let limit = args.limit.unwrap_or(bytes.len()).min(bytes.len() - offset);
  let end_byte = offset + limit;

  let mut start_line = None;
  let mut end_line = None;
  let mut current_byte_idx = 0;
  for (i, line) in source.split('\n').enumerate() {
    let line_len = line.len() + 1; // +1 for the \n
    let next_byte_idx = current_byte_idx + line_len;

    if current_byte_idx < end_byte && next_byte_idx > offset {
      if start_line.is_none() {
        start_line = Some(i + 1);
      }
      end_line = Some(i + 1);
    }
    current_byte_idx = next_byte_idx;
  }

  Ok(render_hashlines(&source, start_line, end_line))
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
