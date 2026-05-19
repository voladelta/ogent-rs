use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::Node;

pub struct Symbol {
  pub kind: &'static str,
  pub name: String,
  pub line_start: usize,
  pub line_end: usize,
  pub signature: String,
  pub children: Vec<Symbol>,
}

pub fn collect_rust_files(path: &Path) -> Vec<PathBuf> {
  let mut files = Vec::new();
  if let Err(e) = collect_files_inner(path, &mut files) {
    eprintln!("symbol_tree: walk error: {}", e);
  }
  files
}

fn collect_files_inner(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
  if path.is_file() && path.extension().map(|e| e == "rs").unwrap_or(false) {
    files.push(path.to_path_buf());
  } else if path.is_dir() {
    for entry in fs::read_dir(path)? {
      let entry = entry?;
      let name = entry.file_name();
      let name = name.to_string_lossy();
      if name.starts_with('.') || name == "target" || name == "node_modules" {
        continue;
      }
      collect_files_inner(&entry.path(), files)?;
    }
  }
  Ok(())
}

pub fn format_path(path: &Path) -> Result<String> {
  let files = collect_rust_files(path);
  if files.is_empty() {
    bail!("no .rs files found at {}", path.display());
  }

  let mut parser = tree_sitter::Parser::new();
  let lang = tree_sitter::Language::new(tree_sitter_rust::LANGUAGE);
  parser.set_language(&lang)?;

  let mut out = String::new();
  for file in &files {
    if let Ok(text) = process_file(&mut parser, file) {
      out.push_str(&text);
      out.push('\n');
    }
  }
  Ok(out)
}

fn process_file(parser: &mut tree_sitter::Parser, path: &Path) -> Result<String> {
  let source = fs::read_to_string(path)?;
  let tree = parser.parse(&source, None).context("parse failed")?;
  let root = tree.root_node();
  let syms = extract_block(&source, root);

  let mut out = String::new();
  out.push_str(&format!("{}\n", path.display()));
  for sym in syms {
    format_symbol(&mut out, &sym, 1);
  }
  Ok(out)
}

fn byte_to_line(source: &str, byte: usize) -> usize {
  let end = byte.min(source.len());
  source[..end].chars().filter(|&c| c == '\n').count() + 1
}

fn child_by_field<'a>(node: Node<'a>, name: &str) -> Option<Node<'a>> {
  node.child_by_field_name(name)
}

fn signature_text(source: &str, node: Node) -> String {
  let mut end_byte = node.end_byte();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if child.kind() == "block"
      || child.kind() == "field_declaration_list"
      || child.kind() == "ordered_field_declaration_list"
      || child.kind() == "enum_variant_list"
      || child.kind() == "declaration_list"
    {
      end_byte = child.start_byte();
      break;
    }
  }
  let text = &source[node.start_byte()..end_byte.min(source.len())];
  let text = text.trim_end();
  if end_byte < node.end_byte() {
    text.trim_end_matches(';').trim_end().to_string()
  } else {
    text.to_string()
  }
}

fn extract_block(source: &str, node: Node) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if let Some(sym) = node_to_symbol(source, child) {
      out.push(sym);
    } else if matches!(
      child.kind(),
      "declaration_list"
        | "enum_variant_list"
        | "field_declaration_list"
        | "ordered_field_declaration_list"
        | "macro_rule_list"
        | "extern_item_list"
    ) {
      out.extend(extract_block(source, child));
    }
  }
  out
}

fn node_to_symbol(source: &str, node: Node) -> Option<Symbol> {
  match node.kind() {
    "function_item" | "function_signature_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      let sig = signature_text(source, node);
      Some(Symbol {
        kind: "fn",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: sig,
        children: vec![],
      })
    }
    "struct_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      let sig = signature_text(source, node);
      Some(Symbol {
        kind: "struct",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: sig,
        children: extract_block(source, node),
      })
    }
    "enum_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      let sig = signature_text(source, node);
      Some(Symbol {
        kind: "enum",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: sig,
        children: extract_block(source, node),
      })
    }
    "trait_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      let sig = signature_text(source, node);
      Some(Symbol {
        kind: "trait",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: sig,
        children: extract_block(source, node),
      })
    }
    "impl_item" => {
      let type_node = child_by_field(node, "type")?;
      let type_name = type_node.utf8_text(source.as_bytes()).ok()?.to_string();
      let trait_name = child_by_field(node, "trait").and_then(|n| {
        Some(n.utf8_text(source.as_bytes()).ok()?.to_string())
      });
      let name = if let Some(t) = trait_name {
        format!("{} for {}", t, type_name)
      } else {
        type_name
      };
      let sig = signature_text(source, node);
      Some(Symbol {
        kind: "impl",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: sig,
        children: extract_block(source, node),
      })
    }
    "mod_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      let sig = signature_text(source, node);
      Some(Symbol {
        kind: "mod",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: sig,
        children: extract_block(source, node),
      })
    }
    "type_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      let sig = signature_text(source, node);
      Some(Symbol {
        kind: "type",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: sig,
        children: vec![],
      })
    }
    "const_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      let sig = signature_text(source, node);
      Some(Symbol {
        kind: "const",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: sig,
        children: vec![],
      })
    }
    "static_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      let sig = signature_text(source, node);
      Some(Symbol {
        kind: "static",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: sig,
        children: vec![],
      })
    }
    "macro_definition" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      let sig = signature_text(source, node);
      Some(Symbol {
        kind: "macro",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: sig,
        children: vec![],
      })
    }
    _ => None,
  }
}

fn display_rest(kind: &str, signature: &str) -> String {
  let s = signature.trim();
  let target = format!("{} ", kind);
  if let Some(pos) = s.find(&target) {
    let after = pos + target.len();
    s[after..].to_string()
  } else {
    s.to_string()
  }
}

fn format_symbol(out: &mut String, sym: &Symbol, depth: usize) {
  let indent = "  ".repeat(depth);
  let sig = sym.signature.trim();
  let sig_compact: String = sig.lines().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
  if sig_compact.is_empty() {
    out.push_str(&format!("{}{} {}@{}:{}\n", indent, sym.kind, sym.name, sym.line_start, sym.line_end));
  } else {
    let rest = display_rest(sym.kind, &sig_compact);
    out.push_str(&format!("{}{} @{}:{} {}\n", indent, sym.kind, sym.line_start, sym.line_end, rest));
  }
  for child in &sym.children {
    format_symbol(out, child, depth + 1);
  }
}
