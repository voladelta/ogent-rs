use anyhow::{Result, bail};
use serde::Deserialize;
use std::fmt::Write;

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
    let line_no = start_line + slice_start + i;
    let hash = line_hash(line);
    let _ = writeln!(out, "{line_no}:{hash}|{line}");
  }
  out
}

pub fn source_lines(source: &str) -> Vec<String> {
  let has_trailing = source.ends_with('\n');
  let s = if has_trailing { &source[..source.len() - 1] } else { source };
  if s.is_empty() {
    return Vec::new();
  }
  s.split('\n').map(String::from).collect()
}

fn line_hash(line: &str) -> String {
  format!("{:04x}", fnv1a64(line.as_bytes()) >> 48)
}

fn fnv1a64(bytes: &[u8]) -> u64 {
  let mut hash = 0xcbf2_9ce4_8422_2325u64;
  for b in bytes {
    hash ^= u64::from(*b);
    hash = hash.wrapping_mul(0x0100_0000_01b3);
  }
  hash
}

#[derive(Clone, Deserialize)]
pub struct EditOp {
  pub anchor: String,
  #[serde(default)]
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
      end_idx: Some(end.map_or(start_idx, |a| a.line - 1)),
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
  if line == 0 || hash.len() != 4 {
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
  if !current.eq_ignore_ascii_case(anchor.hash) {
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
      Some(end) => lines.splice(e.start_idx..=end, e.replacement),
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

  #[test]
  fn edit_op_allows_missing_end_anchor_for_inserts() {
    let op: EditOp =
      serde_json::from_str(r#"{"anchor":"1:a430","action":"after","new_string":"world"}"#)
        .expect("missing end_anchor should default to empty");

    assert!(op.end_anchor.is_empty());
  }
}
