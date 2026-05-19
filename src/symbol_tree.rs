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

pub fn collect_source_files(path: &Path) -> Vec<PathBuf> {
  let mut files = Vec::new();
  if let Err(e) = collect_files_inner(path, &mut files) {
    eprintln!("symbol_tree: walk error: {}", e);
  }
  files
}

fn collect_files_inner(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
  if path.is_file() {
    if let Some(ext) = path.extension()
      && (ext == "rs" || ext == "go")
    {
      files.push(path.to_path_buf());
    }
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
  let files = collect_source_files(path);
  if files.is_empty() {
    bail!("no .rs or .go files found at {}", path.display());
  }

  let mut out = String::new();
  for file in &files {
    if let Ok(text) = process_file(file) {
      out.push_str(&text);
      out.push('\n');
    }
  }
  Ok(out)
}

fn process_file(path: &Path) -> Result<String> {
  let source = fs::read_to_string(path)?;

  let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
  let syms = match ext {
    "rs" => parse_rust(&source)?,
    "go" => parse_go(&source)?,
    _ => bail!("unsupported extension: {}", ext),
  };

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

fn signature_text(source: &str, node: Node, body_kinds: &[&str]) -> String {
  let mut end_byte = node.end_byte();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if body_kinds.contains(&child.kind()) {
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

fn format_symbol(out: &mut String, sym: &Symbol, depth: usize) {
  let indent = "  ".repeat(depth);
  let sig = sym.signature.trim();
  let sig_compact: String = sig.lines().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
  if sig_compact.is_empty() {
    out.push_str(&format!(
      "{}{} {}@{}:{}\n",
      indent, sym.kind, sym.name, sym.line_start, sym.line_end
    ));
  } else {
    let rest = display_rest(sym.kind, &sig_compact);
    out.push_str(&format!(
      "{}{} @{}:{} {}\n",
      indent, sym.kind, sym.line_start, sym.line_end, rest
    ));
  }
  for child in &sym.children {
    format_symbol(out, child, depth + 1);
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

// ── Rust ────────────────────────────────────────────────────────────────────

fn parse_rust(source: &str) -> Result<Vec<Symbol>> {
  let mut parser = tree_sitter::Parser::new();
  let lang = tree_sitter::Language::new(tree_sitter_rust::LANGUAGE);
  parser.set_language(&lang)?;
  let tree = parser.parse(source, None).context("rust parse failed")?;
  Ok(extract_rust_block(source, tree.root_node()))
}

fn extract_rust_block(source: &str, node: Node) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if let Some(sym) = rust_node_to_symbol(source, child) {
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
      out.extend(extract_rust_block(source, child));
    }
  }
  out
}

fn rust_signature(source: &str, node: Node) -> String {
  signature_text(
    source,
    node,
    &[
      "block",
      "field_declaration_list",
      "ordered_field_declaration_list",
      "enum_variant_list",
      "declaration_list",
    ],
  )
}

fn rust_node_to_symbol(source: &str, node: Node) -> Option<Symbol> {
  match node.kind() {
    "function_item" | "function_signature_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(Symbol {
        kind: "fn",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: rust_signature(source, node),
        children: vec![],
      })
    }
    "struct_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(Symbol {
        kind: "struct",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: rust_signature(source, node),
        children: extract_rust_block(source, node),
      })
    }
    "enum_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(Symbol {
        kind: "enum",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: rust_signature(source, node),
        children: extract_rust_block(source, node),
      })
    }
    "trait_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(Symbol {
        kind: "trait",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: rust_signature(source, node),
        children: extract_rust_block(source, node),
      })
    }
    "impl_item" => {
      let type_node = child_by_field(node, "type")?;
      let type_name = type_node.utf8_text(source.as_bytes()).ok()?.to_string();
      let trait_name = child_by_field(node, "trait")
        .and_then(|n| Some(n.utf8_text(source.as_bytes()).ok()?.to_string()));
      let name = if let Some(t) = trait_name {
        format!("{} for {}", t, type_name)
      } else {
        type_name
      };
      Some(Symbol {
        kind: "impl",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: rust_signature(source, node),
        children: extract_rust_block(source, node),
      })
    }
    "mod_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(Symbol {
        kind: "mod",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: rust_signature(source, node),
        children: extract_rust_block(source, node),
      })
    }
    "type_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(Symbol {
        kind: "type",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: rust_signature(source, node),
        children: vec![],
      })
    }
    "const_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(Symbol {
        kind: "const",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: rust_signature(source, node),
        children: vec![],
      })
    }
    "static_item" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(Symbol {
        kind: "static",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: rust_signature(source, node),
        children: vec![],
      })
    }
    "macro_definition" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(Symbol {
        kind: "macro",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: rust_signature(source, node),
        children: vec![],
      })
    }
    _ => None,
  }
}

// ── Go ──────────────────────────────────────────────────────────────────────

fn parse_go(source: &str) -> Result<Vec<Symbol>> {
  let mut parser = tree_sitter::Parser::new();
  let lang = tree_sitter::Language::new(tree_sitter_go::LANGUAGE);
  parser.set_language(&lang)?;
  let tree = parser.parse(source, None).context("go parse failed")?;
  Ok(extract_go_block(source, tree.root_node()))
}

fn extract_go_block(source: &str, node: Node) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if let Some(sym) = go_node_to_symbol(source, child) {
      out.push(sym);
    }
  }
  out
}

fn go_signature(source: &str, node: Node) -> String {
  let mut end_byte = node.end_byte();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if matches!(
      child.kind(),
      "block"
        | "field_declaration_list"
        | "interface_body"
        | "struct_type"
        | "interface_type"
        | "map_type"
        | "channel_type"
        | "function_type"
        | "array_type"
        | "slice_type"
        | "pointer_type"
    ) {
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

fn go_node_to_symbol(source: &str, node: Node) -> Option<Symbol> {
  match node.kind() {
    "package_clause" => {
      let name = node
        .children(&mut node.walk())
        .find(|c| c.kind() == "package_identifier")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      let sig = go_signature(source, node);
      Some(Symbol {
        kind: "package",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: sig,
        children: vec![],
      })
    }
    "function_declaration" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      let sig = go_signature(source, node);
      Some(Symbol {
        kind: "fn",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: sig,
        children: vec![],
      })
    }
    "method_declaration" => {
      let name = child_by_field(node, "name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      let sig = go_signature(source, node);
      Some(Symbol {
        kind: "fn",
        name,
        line_start: byte_to_line(source, node.start_byte()),
        line_end: byte_to_line(source, node.end_byte()),
        signature: sig,
        children: vec![],
      })
    }
    "type_declaration" => {
      // type_declaration may contain one or more type_spec
      let mut cursor = node.walk();
      let mut out = Vec::new();
      for child in node.children(&mut cursor) {
        if child.kind() == "type_spec"
          && let Some(sym) = go_type_spec_to_symbol(source, child)
        {
          out.push(sym);
        }
      }
      // Return a pseudo-symbol with children if multiple, else unwrap
      if out.len() == 1 {
        return out.into_iter().next();
      }
      if out.is_empty() {
        return None;
      }
      let line_start = byte_to_line(source, node.start_byte());
      let line_end = byte_to_line(source, node.end_byte());
      Some(Symbol {
        kind: "type",
        name: "(group)".to_string(),
        line_start,
        line_end,
        signature: go_signature(source, node),
        children: out,
      })
    }
    "const_declaration" => {
      let mut out = Vec::new();
      go_collect_specs(source, node, "const_spec", "const", &mut out);
      if out.len() == 1 {
        return out.into_iter().next();
      }
      if out.is_empty() {
        return None;
      }
      let line_start = byte_to_line(source, node.start_byte());
      let line_end = byte_to_line(source, node.end_byte());
      Some(Symbol {
        kind: "const",
        name: "(group)".to_string(),
        line_start,
        line_end,
        signature: go_signature(source, node),
        children: out,
      })
    }
    "var_declaration" => {
      let mut out = Vec::new();
      go_collect_specs(source, node, "var_spec", "var", &mut out);
      if out.len() == 1 {
        return out.into_iter().next();
      }
      if out.is_empty() {
        return None;
      }
      let line_start = byte_to_line(source, node.start_byte());
      let line_end = byte_to_line(source, node.end_byte());
      Some(Symbol {
        kind: "var",
        name: "(group)".to_string(),
        line_start,
        line_end,
        signature: go_signature(source, node),
        children: out,
      })
    }
    _ => None,
  }
}

fn go_type_spec_to_symbol(source: &str, node: Node) -> Option<Symbol> {
  let name = child_by_field(node, "name")?
    .utf8_text(source.as_bytes())
    .ok()?
    .to_string();
  let line_start = byte_to_line(source, node.start_byte());
  let line_end = byte_to_line(source, node.end_byte());

  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    match child.kind() {
      "struct_type" => {
        return Some(Symbol {
          kind: "struct",
          name,
          line_start,
          line_end,
          signature: go_signature(source, node),
          children: extract_go_struct_fields(source, child),
        });
      }
      "interface_type" => {
        return Some(Symbol {
          kind: "interface",
          name,
          line_start,
          line_end,
          signature: go_signature(source, node),
          children: extract_go_interface_methods(source, child),
        });
      }
      _ => {}
    }
  }
  // Type alias or other type
  Some(Symbol {
    kind: "type",
    name,
    line_start,
    line_end,
    signature: go_signature(source, node),
    children: vec![],
  })
}

fn extract_go_struct_fields(source: &str, node: Node) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if (child.kind() == "field_declaration" || child.kind() == "field_declaration_list")
      && let Some(name) = child.child_by_field_name("name")
      && let Ok(name_str) = name.utf8_text(source.as_bytes())
    {
      let sig = go_signature(source, child);
      out.push(Symbol {
        kind: "field",
        name: name_str.to_string(),
        line_start: byte_to_line(source, child.start_byte()),
        line_end: byte_to_line(source, child.end_byte()),
        signature: sig,
        children: vec![],
      });
    }
  }
  out
}

fn extract_go_interface_methods(source: &str, node: Node) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if (child.kind() == "method_elem" || child.kind() == "method_spec")
      && let Some(name) = child.child_by_field_name("name")
      && let Ok(name_str) = name.utf8_text(source.as_bytes())
    {
      let sig = go_signature(source, child);
      out.push(Symbol {
        kind: "fn",
        name: name_str.to_string(),
        line_start: byte_to_line(source, child.start_byte()),
        line_end: byte_to_line(source, child.end_byte()),
        signature: sig,
        children: vec![],
      });
    }
  }
  out
}

fn go_const_var_spec_to_symbol(source: &str, node: Node, kind: &'static str) -> Option<Symbol> {
  let name = child_by_field(node, "name")?
    .utf8_text(source.as_bytes())
    .ok()?
    .to_string();
  let sig = go_signature(source, node);
  Some(Symbol {
    kind,
    name,
    line_start: byte_to_line(source, node.start_byte()),
    line_end: byte_to_line(source, node.end_byte()),
    signature: sig,
    children: vec![],
  })
}

fn go_collect_specs(
  source: &str,
  node: Node,
  spec_kind: &str,
  symbol_kind: &'static str,
  out: &mut Vec<Symbol>,
) {
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if child.kind() == spec_kind {
      if let Some(sym) = go_const_var_spec_to_symbol(source, child, symbol_kind) {
        out.push(sym);
      }
    } else {
      // Recurse into lists/groups
      go_collect_specs(source, child, spec_kind, symbol_kind, out);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn go_simple_file() {
    let src = r#"package main

import "fmt"

const MaxSize = 100

var GlobalVar = 42

type Person struct {
	Name string
	Age  int
}

type Stringer interface {
	String() string
}

type (
	Counter int
	ID      string
)

func Hello(name string) string {
	return "Hello " + name
}

func (p *Person) String() string {
	return p.Name
}

func main() {
	fmt.Println("Hello")
}
"#;
    let syms = parse_go(src).unwrap();
    assert!(!syms.is_empty());
    // Should find package, const, var, types, funcs
    let kinds: Vec<_> = syms.iter().map(|s| s.kind).collect();
    assert!(kinds.contains(&"package"));
    assert!(kinds.contains(&"const"));
    assert!(kinds.contains(&"var"));
    assert!(kinds.contains(&"struct"));
    assert!(kinds.contains(&"interface"));
    assert!(kinds.contains(&"fn"));
  }

  #[test]
  fn go_group_declarations() {
    let src = r#"package foo

type (
	A struct { X int }
	B interface { M() }
)

const (
	One = 1
	Two = 2
)

var (
	X = "x"
	Y = "y"
)
"#;
    let syms = parse_go(src).unwrap();
    let names: Vec<_> = syms.iter().map(|s| (s.kind, s.name.as_str())).collect();
    // Should unwrap group declarations into individual symbols when single, or group when multiple
    assert!(names.contains(&("type", "(group)")));
    assert!(names.contains(&("const", "(group)")));
    assert!(names.contains(&("var", "(group)")));
  }

  #[test]
  fn go_format_output() {
    let src = r#"package main

const MaxSize = 100

var GlobalVar = 42

type Person struct {
	Name string
	Age  int
}

type Stringer interface {
	String() string
}

func Hello(name string) string {
	return "Hello " + name
}

func (p *Person) String() string {
	return p.Name
}
"#;
    let syms = parse_go(src).unwrap();
    let mut out = String::new();
    out.push_str("test.go\n");
    for sym in syms {
      format_symbol(&mut out, &sym, 1);
    }
    assert!(out.contains("package"));
    assert!(out.contains("struct"));
    assert!(out.contains("interface"));
    assert!(out.contains("fn"));
  }
}
