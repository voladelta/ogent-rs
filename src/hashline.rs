use anyhow::{Result, bail};
use serde::Deserialize;

const ANCHOR_HASH_WIDTH: usize = 4;

#[derive(Debug, Clone, Copy)]
struct Anchor<'a> {
  line: usize,
  hash: &'a str,
}

pub fn render_hashlines(
  source: &str,
  start_line: usize,
  start: Option<usize>,
  end: Option<usize>,
) -> String {
  let lines = source_lines(source);
  let slice_start = start.unwrap_or(0).min(lines.len());
  let slice_end = end.unwrap_or(lines.len()).min(lines.len());
  let mut out = String::new();
  for (i, line) in lines[slice_start..slice_end].iter().enumerate() {
    out.push_str(&format!(
      "{}:{}|{}\n",
      start_line + slice_start + i,
      line_hash(line),
      line
    ));
  }
  out
}

pub fn source_lines(source: &str) -> Vec<String> {
  if source.is_empty() {
    return Vec::new();
  }
  let mut lines: Vec<String> = source.split('\n').map(ToString::to_string).collect();
  if lines.last().is_some_and(|s| s.is_empty()) {
    lines.pop();
  }
  lines
}

fn line_hash(line: &str) -> String {
  let hash = fnv1a64(line.as_bytes());
  format!("{hash:016x}")[..ANCHOR_HASH_WIDTH].to_string()
}

fn fnv1a64(bytes: &[u8]) -> u64 {
  let mut hash = 0xcbf29ce484222325u64;
  for b in bytes {
    hash ^= *b as u64;
    hash = hash.wrapping_mul(0x100000001b3);
  }
  hash
}

#[derive(Clone, Deserialize)]
pub struct EditOp {
  pub anchor: String,
  pub end_anchor: String,
  pub action: String,
  pub new_string: String,
}

#[derive(Clone)]
struct ResolvedEdit {
  start_idx: usize,
  end_idx: Option<usize>,
  replacement: Vec<String>,
  insert_mode: &'static str,
}

pub fn apply_anchor_edits(source: &str, ops: &[EditOp]) -> Result<String> {
  let lines = source_lines(source);
  if lines.is_empty() {
    bail!("file is empty");
  }
  let mut edits = Vec::with_capacity(ops.len());
  for (i, op) in ops.iter().enumerate() {
    edits.push(resolve_edit(&lines, op).map_err(|e| anyhow::anyhow!("op[{i}]: {e}"))?);
  }
  let result = apply_resolved(lines, edits)?;
  let mut out = result.join("\n");
  if source.ends_with('\n') {
    out.push('\n');
  }
  Ok(out)
}

fn resolve_edit(lines: &[String], op: &EditOp) -> Result<ResolvedEdit> {
  let start = parse_anchor(&op.anchor)?;
  validate_anchor(lines, start)?;
  let end = if op.end_anchor.is_empty() {
    None
  } else {
    let ea = parse_anchor(&op.end_anchor)?;
    validate_anchor(lines, ea)?;
    if ea.line < start.line {
      bail!("end anchor precedes start anchor");
    }
    Some(ea)
  };
  let replacement = source_lines(&op.new_string);
  let start_idx = start.line - 1;
  Ok(match op.action.as_str() {
    "before" | "insert_before" => {
      if end.is_some() {
        bail!("insert edits cannot use end");
      }
      ResolvedEdit {
        start_idx,
        end_idx: None,
        replacement,
        insert_mode: "before",
      }
    }
    "after" | "insert_after" => {
      if end.is_some() {
        bail!("insert edits cannot use end");
      }
      ResolvedEdit {
        start_idx: start_idx + 1,
        end_idx: None,
        replacement,
        insert_mode: "after",
      }
    }
    "replace" => ResolvedEdit {
      start_idx,
      end_idx: Some(end.map(|a| a.line - 1).unwrap_or(start_idx)),
      replacement,
      insert_mode: "",
    },
    other => bail!("action must be replace, before, or after, got: {other}"),
  })
}

fn parse_anchor(value: &str) -> Result<Anchor<'_>> {
  let (line, hash) = value
    .split_once(':')
    .ok_or_else(|| anyhow::anyhow!("invalid anchor, expected line:hash: {value}"))?;
  let line: usize = line
    .parse()
    .map_err(|_| anyhow::anyhow!("invalid anchor line number: {value}"))?;
  if line == 0 || hash.is_empty() || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
    bail!("invalid anchor: {value}");
  }
  Ok(Anchor { line, hash })
}

fn validate_anchor(lines: &[String], anchor: Anchor<'_>) -> Result<()> {
  let idx = anchor.line - 1;
  if idx >= lines.len() {
    bail!("anchor line is outside file: {}", anchor.line);
  }
  let current = line_hash(&lines[idx]);
  if current != anchor.hash.to_ascii_lowercase() {
    bail!(
      "anchor mismatch at line {}: expected {}, current {}\n{}:{}|{}",
      anchor.line,
      anchor.hash,
      current,
      anchor.line,
      current,
      lines[idx]
    );
  }
  Ok(())
}

fn effective_start(e: &ResolvedEdit) -> usize {
  if e.insert_mode == "after" {
    e.start_idx.saturating_sub(1)
  } else {
    e.start_idx
  }
}

fn effective_end(e: &ResolvedEdit) -> usize {
  e.end_idx.unwrap_or_else(|| effective_start(e))
}

fn apply_resolved(mut lines: Vec<String>, mut edits: Vec<ResolvedEdit>) -> Result<Vec<String>> {
  edits.sort_by_key(|b| std::cmp::Reverse(effective_start(b)));
  for pair in edits.windows(2) {
    let upper = &pair[0];
    let lower = &pair[1];
    if effective_end(lower) >= effective_start(upper) {
      bail!(
        "overlapping edits: lines {}-{} and {}-{}",
        effective_start(lower) + 1,
        effective_end(lower) + 1,
        effective_start(upper) + 1,
        effective_end(upper) + 1
      );
    }
  }
  for e in edits {
    match e.end_idx {
      None => lines.splice(e.start_idx..e.start_idx, e.replacement),
      Some(end) => lines.splice(e.start_idx..end + 1, e.replacement),
    };
  }
  Ok(lines)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn hashline_matches_go_fnv_prefix() {
    assert_eq!(render_hashlines("hello\n", 1, None, None), "1:a430|hello\n");
  }
}
