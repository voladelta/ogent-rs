use anyhow::{Context, Result, bail};
use serde::Deserialize;

const HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Debug, Clone, Copy)]
struct Anchor<'a> {
  line: usize,
  hash: &'a str,
}

pub fn render_hashlines(source: &str, start: Option<usize>, end: Option<usize>) -> String {
  let lines = source_lines_ref(source);
  let slice_start = start
    .map(|s| s.saturating_sub(1))
    .unwrap_or(0)
    .min(lines.len());
  let slice_end = end.unwrap_or(lines.len()).clamp(slice_start, lines.len());
  let slice = &lines[slice_start..slice_end];
  let estimated: usize = slice.iter().map(|l| l.len() + 12).sum();
  let mut out = String::with_capacity(estimated);
  let mut hbuf = [0u8; 4];
  for (i, line) in slice.iter().enumerate() {
    let line_no = slice_start + i + 1;
    line_hash_into(line, &mut hbuf);
    append_decimal(&mut out, line_no);
    out.push(':');
    out.push_str(std::str::from_utf8(&hbuf).unwrap());
    out.push('|');
    out.push_str(line);
    out.push('\n');
  }
  out
}

fn append_decimal(out: &mut String, mut n: usize) {
  if n == 0 {
    out.push('0');
    return;
  }
  let mut buf = [0u8; 20];
  let mut pos = 20;
  while n > 0 {
    pos -= 1;
    buf[pos] = b'0' + (n % 10) as u8;
    n /= 10;
  }
  out.push_str(std::str::from_utf8(&buf[pos..]).unwrap());
}

fn source_lines_ref(source: &str) -> Vec<&str> {
  let s = source.strip_suffix('\n').unwrap_or(source);
  if s.is_empty() {
    return Vec::new();
  }
  s.split('\n').collect()
}

pub fn source_lines(source: &str) -> Vec<String> {
  let s = source.strip_suffix('\n').unwrap_or(source);
  if s.is_empty() {
    return Vec::new();
  }
  s.split('\n').map(String::from).collect()
}

#[inline]
fn line_hash_into(line: &str, buf: &mut [u8; 4]) {
  let h = (fnv1a64(line.as_bytes()) >> 48) as u16;
  buf[0] = HEX[((h >> 12) & 0xf) as usize];
  buf[1] = HEX[((h >> 8) & 0xf) as usize];
  buf[2] = HEX[((h >> 4) & 0xf) as usize];
  buf[3] = HEX[(h & 0xf) as usize];
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
  pub start_at: String,
  #[serde(default)]
  pub end_at: String,
  pub action: String,
  #[serde(default)]
  pub content: String,
}

#[derive(Clone)]
enum InsertMode {
  Replace,
  Before,
  After,
}

#[derive(Clone)]
struct ResolvedEdit {
  start_idx: usize,
  end_idx: Option<usize>,
  replacement: Vec<String>,
  mode: InsertMode,
}

pub fn apply_anchor_edits(source: &str, ops: &[EditOp]) -> Result<String> {
  let lines = source_lines(source);
  let mut edits = Vec::with_capacity(ops.len());
  for (i, op) in ops.iter().enumerate() {
    edits.push(resolve_edit(&lines, op).with_context(|| format!("op[{i}]"))?);
  }
  let result = apply_resolved(lines, edits)?;
  let mut out = result.join("\n");
  if source.ends_with('\n') {
    out.push('\n');
  }
  Ok(out)
}

fn resolve_edit(lines: &[String], op: &EditOp) -> Result<ResolvedEdit> {
  let start = parse_anchor(&op.start_at)?;
  validate_anchor(lines, start)?;
  let end = if op.end_at.is_empty() {
    None
  } else {
    let ea = parse_anchor(&op.end_at)?;
    validate_anchor(lines, ea)?;
    if ea.line < start.line {
      bail!("end anchor precedes start anchor");
    }
    Some(ea)
  };
  let replacement = source_lines(&op.content);
  let start_idx = start.line - 1;
  Ok(match op.action.as_str() {
    "insert_before" => {
      if end.is_some() {
        bail!("insert edits cannot use end");
      }
      ResolvedEdit {
        start_idx,
        end_idx: None,
        replacement,
        mode: InsertMode::Before,
      }
    }
    "insert_after" => {
      if end.is_some() {
        bail!("insert edits cannot use end");
      }
      ResolvedEdit {
        start_idx,
        end_idx: None,
        replacement,
        mode: InsertMode::After,
      }
    }
    "replace" => ResolvedEdit {
      start_idx,
      end_idx: Some(end.map_or(start_idx, |a| a.line - 1)),
      replacement,
      mode: InsertMode::Replace,
    },
    "delete" => ResolvedEdit {
      start_idx,
      end_idx: Some(end.map_or(start_idx, |a| a.line - 1)),
      replacement: Vec::new(),
      mode: InsertMode::Replace,
    },
    other => bail!("action must be replace, delete, insert_before, or insert_after, got: {other}"),
  })
}

fn parse_anchor(value: &str) -> Result<Anchor<'_>> {
  let (line, hash) = value
    .split_once(':')
    .with_context(|| format!("invalid anchor, expected line:hash: {value}"))?;
  let line: usize = line
    .parse()
    .with_context(|| format!("invalid anchor line number: {value}"))?;
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
  let mut hbuf = [0u8; 4];
  line_hash_into(&lines[idx], &mut hbuf);
  let current = std::str::from_utf8(&hbuf).unwrap();
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

fn apply_resolved(mut lines: Vec<String>, mut edits: Vec<ResolvedEdit>) -> Result<Vec<String>> {
  edits.sort_by_key(|e| std::cmp::Reverse(e.start_idx));
  for pair in edits.windows(2) {
    let upper = &pair[0];
    let lower = &pair[1];
    let lower_end = lower.end_idx.unwrap_or(lower.start_idx);
    if lower_end >= upper.start_idx {
      bail!(
        "overlapping edits: lines {}-{} and {}-{}",
        lower.start_idx + 1,
        lower_end + 1,
        upper.start_idx + 1,
        upper.end_idx.unwrap_or(upper.start_idx) + 1
      );
    }
  }
  for e in edits {
    match e.mode {
      InsertMode::Before => lines.splice(e.start_idx..e.start_idx, e.replacement),
      InsertMode::After => lines.splice(e.start_idx + 1..e.start_idx + 1, e.replacement),
      InsertMode::Replace => lines.splice(e.start_idx..=e.end_idx.unwrap(), e.replacement),
    };
  }
  Ok(lines)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn hashline_matches_go_fnv_prefix() {
    assert_eq!(render_hashlines("hello\n", None, None), "1:a430|hello\n");
  }

  #[test]
  fn hashline_empty_when_start_exceeds_end() {
    assert_eq!(render_hashlines("hello\n", Some(60), Some(15)), "");
  }

  #[test]
  fn edit_op_allows_missing_end_anchor_for_inserts() {
    let op: EditOp =
      serde_json::from_str(r#"{"start_at":"1:a430","action":"insert_after","content":"world"}"#)
        .expect("missing end_at should default to empty");

    assert!(op.end_at.is_empty());
  }
}
