use super::{Symbol, make_symbol, signature_text};
use anyhow::{Context, Result};
use tree_sitter::Node;

pub fn parse(source: &str) -> Result<Vec<Symbol>> {
  let mut parser = tree_sitter::Parser::new();
  let lang = tree_sitter::Language::new(tree_sitter_python::LANGUAGE);
  parser.set_language(&lang)?;
  let tree = parser.parse(source, None).context("python parse failed")?;
  Ok(extract_block(source, tree.root_node()))
}

fn extract_block(source: &str, node: Node) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if let Some(sym) = node_to_symbol(source, child) {
      out.push(sym);
    }
  }
  out
}

fn signature(source: &str, node: Node) -> String {
  signature_text(source, node, &["block"])
}

fn node_to_symbol(source: &str, node: Node) -> Option<Symbol> {
  match node.kind() {
    "function_definition" => {
      let name = node
        .child_by_field_name("name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(make_symbol(
        source,
        node,
        "fn",
        name,
        signature(source, node),
        vec![],
      ))
    }
    "class_definition" => {
      let name = node
        .child_by_field_name("name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(make_symbol(
        source,
        node,
        "class",
        name,
        signature(source, node),
        extract_class_body(source, node),
      ))
    }
    "decorated_definition" => {
      let def = node.child_by_field_name("definition")?;
      let inner = node_to_symbol(source, def)?;
      Some(make_symbol(
        source,
        node,
        inner.kind,
        inner.name,
        inner.signature,
        inner.children,
      ))
    }
    "expression_statement" => {
      let mut cursor = node.walk();
      let child = node.children(&mut cursor).next()?;
      if child.kind() == "assignment" {
        node_to_symbol(source, child)
      } else {
        None
      }
    }
    "assignment" => {
      let left = node.child_by_field_name("left")?;
      if left.kind() == "identifier" {
        let name = left.utf8_text(source.as_bytes()).ok()?.to_string();
        Some(make_symbol(
          source,
          node,
          "var",
          name,
          signature(source, node),
          vec![],
        ))
      } else {
        None
      }
    }
    _ => None,
  }
}

fn extract_class_body(source: &str, node: Node) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if child.kind() == "block" {
      let mut inner = child.walk();
      for member in child.children(&mut inner) {
        if let Some(sym) = node_to_symbol(source, member) {
          out.push(sym);
        }
      }
    }
  }
  out
}
