use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;
use similar::TextDiff;
use std::fs;

use crate::hashline::{apply_anchor_edits as hashline_apply_anchor_edits, render_hashlines};
use crate::tools::{ToolContext, parse_args, require_nonempty};

const PREVIEW_DIFF_MAX_CHARS: usize = 10_000;
const PREVIEW_DIFF_TRUNCATED_MARKER: &str =
  "\n... preview truncated to stay under the tool output cap ...\n";

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

#[derive(Deserialize)]
pub struct ReadLinesArgs {
  pub path: String,
  pub start_line: usize,
  pub end_line: usize,
}

pub fn read_file(ctx: ToolContext, args: &str) -> Result<String> {
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

pub fn read_lines(ctx: ToolContext, args: &str) -> Result<String> {
  let args: ReadLinesArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  if args.start_line == 0 {
    bail!("start_line must be >= 1");
  }
  if args.end_line < args.start_line {
    bail!("end_line must be >= start_line");
  }
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
  let source = fs::read_to_string(&path).with_context(|| format!("read {}", args.path))?;
  let lines: Vec<&str> = source.split_inclusive('\n').collect();
  if args.end_line > lines.len() {
    bail!(
      "line range {}-{} is outside file with {} lines",
      args.start_line,
      args.end_line,
      lines.len()
    );
  }
  Ok(lines[(args.start_line - 1)..args.end_line].concat())
}

#[derive(Deserialize)]
struct WriteFileArgs {
  path: String,
  content: String,
  #[serde(default)]
  overwrite_existing: bool,
}

pub fn write_file(ctx: ToolContext, args: &str) -> Result<String> {
  let args: WriteFileArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  let path = ctx.workspace.workspace_path(&args.path)?;
  if path.exists() && !args.overwrite_existing {
    bail!(
      "file {} already exists; use apply_anchor_edits for anchored edits or set overwrite_existing=true for intentional full-file replacement",
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

pub fn read_hash_anchors(ctx: ToolContext, args: &str) -> Result<String> {
  let args: ReadHashAnchorsArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  let path = ctx.workspace.workspace_path(&args.path)?;
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

pub fn apply_anchor_edits(ctx: ToolContext, args: &str) -> Result<String> {
  let args: EditHashAnchorsArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  if args.ops.is_empty() {
    bail!("ops array is required");
  }
  let path = ctx.workspace.workspace_path(&args.path)?;
  let source = fs::read_to_string(&path).with_context(|| format!("read {}", args.path))?;
  let out = hashline_apply_anchor_edits(&source, &args.ops)?;
  fs::write(&path, out).with_context(|| format!("write {}", args.path))?;
  Ok(format!("Applied {} edits to {}", args.ops.len(), args.path))
}

pub fn preview_anchor_edits(ctx: ToolContext, args: &str) -> Result<String> {
  let args: EditHashAnchorsArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  if args.ops.is_empty() {
    bail!("ops array is required");
  }
  let path = ctx.workspace.workspace_path(&args.path)?;
  let source = fs::read_to_string(&path).with_context(|| format!("read {}", args.path))?;
  let out = hashline_apply_anchor_edits(&source, &args.ops)?;
  Ok(render_compact_unified_diff(&args.path, &source, &out))
}

fn render_compact_unified_diff(path: &str, before: &str, after: &str) -> String {
  if before == after {
    return format!("No changes to {path}\n");
  }

  let next_header = format!("{path} (preview)");
  let diff = TextDiff::from_lines(before, after);
  let preview = diff
    .unified_diff()
    .context_radius(3)
    .header(path, &next_header)
    .to_string();
  truncate_preview(preview)
}

fn truncate_preview(preview: String) -> String {
  let marker_len = PREVIEW_DIFF_TRUNCATED_MARKER.chars().count();
  let max_body = PREVIEW_DIFF_MAX_CHARS.saturating_sub(marker_len);
  if preview.chars().count() <= PREVIEW_DIFF_MAX_CHARS {
    return preview;
  }
  let mut out: String = preview.chars().take(max_body).collect();
  out.push_str(PREVIEW_DIFF_TRUNCATED_MARKER);
  out
}

#[derive(Deserialize)]
struct AppendFileArgs {
  path: String,
  content: String,
}

pub fn append_file(ctx: ToolContext, args: &str) -> Result<String> {
  use std::io::Write;
  let args: AppendFileArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  let path = ctx.workspace.workspace_path(&args.path)?;
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
  }
  let mut file = fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open(&path)
    .with_context(|| format!("open {}", args.path))?;
  file
    .write_all(args.content.as_bytes())
    .with_context(|| format!("append {}", args.path))?;
  Ok(format!(
    "Appended {} bytes to {}",
    args.content.len(),
    args.path
  ))
}

#[derive(Deserialize)]
struct FileInfoArgs {
  path: String,
}

pub fn file_info(ctx: ToolContext, args: &str) -> Result<String> {
  let args: FileInfoArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  let path = ctx.workspace.readable_path(&args.path)?;
  let meta = fs::metadata(&path).with_context(|| format!("stat {}", args.path))?;
  let size_bytes = meta.len();
  // Count lines without loading the whole file into memory when it's large
  let line_count: u64 = if size_bytes <= (1 << 20) {
    let content = fs::read_to_string(&path).with_context(|| format!("read {}", args.path))?;
    content.lines().count() as u64
  } else {
    use std::io::{BufRead, BufReader};
    let file = fs::File::open(&path).with_context(|| format!("open {}", args.path))?;
    BufReader::new(file).lines().count() as u64
  };
  Ok(serde_json::to_string(&json!({
    "path": args.path,
    "size_bytes": size_bytes,
    "line_count": line_count,
  }))?)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn preview_diff_truncates_with_visible_marker() {
    let before = (0..300)
      .map(|i| format!("before-{i}-{}", "x".repeat(80)))
      .collect::<Vec<_>>()
      .join("\n")
      + "\n";
    let after = (0..300)
      .map(|i| format!("after-{i}-{}", "y".repeat(80)))
      .collect::<Vec<_>>()
      .join("\n")
      + "\n";

    let preview = render_compact_unified_diff("large.rs", &before, &after);

    assert!(preview.contains(PREVIEW_DIFF_TRUNCATED_MARKER));
    assert!(preview.chars().count() <= PREVIEW_DIFF_MAX_CHARS);
  }
}
