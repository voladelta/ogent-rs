use super::{Symbol, group_or_single, make_symbol, node_name, signature_text};
use anyhow::{Context, Result};
use tree_sitter::Node;

pub fn parse(source: &str) -> Result<Vec<Symbol>> {
  let mut parser = tree_sitter::Parser::new();
  let lang = tree_sitter::Language::new(tree_sitter_go::LANGUAGE);
  parser.set_language(&lang)?;
  let tree = parser.parse(source, None).context("go parse failed")?;
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
  signature_text(
    source,
    node,
    &[
      "block",
      "field_declaration_list",
      "interface_body",
      "struct_type",
      "interface_type",
      "map_type",
      "channel_type",
      "function_type",
      "array_type",
      "slice_type",
      "pointer_type",
    ],
  )
}

fn node_to_symbol(source: &str, node: Node) -> Option<Symbol> {
  match node.kind() {
    "package_clause" => {
      let name = node
        .children(&mut node.walk())
        .find(|c| c.kind() == "package_identifier")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(make_symbol(source, node, "package", name, signature(source, node), vec![]))
    }
    "function_declaration" | "method_declaration" => {
      let name = node_name(source, node)?;
      Some(make_symbol(source, node, "fn", name, signature(source, node), vec![]))
    }
    "type_declaration" => {
      let mut cursor = node.walk();
      let children: Vec<Symbol> = node
        .children(&mut cursor)
        .filter(|c| c.kind() == "type_spec")
        .filter_map(|c| type_spec_to_symbol(source, c))
        .collect();
      group_or_single(source, node, "type", signature(source, node), children)
    }
    "const_declaration" => {
      let mut children = Vec::new();
      collect_specs(source, node, "const_spec", "const", &mut children);
      group_or_single(source, node, "const", signature(source, node), children)
    }
    "var_declaration" => {
      let mut children = Vec::new();
      collect_specs(source, node, "var_spec", "var", &mut children);
      group_or_single(source, node, "var", signature(source, node), children)
    }
    _ => None,
  }
}

fn type_spec_to_symbol(source: &str, node: Node) -> Option<Symbol> {
  let name = node_name(source, node)?;
  let sig = signature(source, node);
  let mut cursor = node.walk();
  let (kind, children) = node
    .children(&mut cursor)
    .find_map(|child| match child.kind() {
      "struct_type" => Some(("struct", extract_struct_fields(source, child))),
      "interface_type" => Some(("interface", extract_interface_methods(source, child))),
      _ => None,
    })
    .unwrap_or(("type", vec![]));
  Some(make_symbol(source, node, kind, name, sig, children))
}

fn extract_struct_fields(source: &str, node: Node) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if (child.kind() == "field_declaration" || child.kind() == "field_declaration_list")
      && let Some(name) = node_name(source, child)
    {
      out.push(make_symbol(
        source,
        child,
        "field",
        name,
        signature(source, child),
        vec![],
      ));
    }
  }
  out
}

fn extract_interface_methods(source: &str, node: Node) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if (child.kind() == "method_elem" || child.kind() == "method_spec")
      && let Some(name) = node_name(source, child)
    {
      out.push(make_symbol(
        source,
        child,
        "fn",
        name,
        signature(source, child),
        vec![],
      ));
    }
  }
  out
}

fn spec_to_symbol(source: &str, node: Node, kind: &'static str) -> Option<Symbol> {
  let name = node_name(source, node)?;
  Some(make_symbol(source, node, kind, name, signature(source, node), vec![]))
}

fn collect_specs(
  source: &str,
  node: Node,
  spec_kind: &str,
  symbol_kind: &'static str,
  out: &mut Vec<Symbol>,
) {
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if child.kind() == spec_kind {
      if let Some(sym) = spec_to_symbol(source, child, symbol_kind) {
        out.push(sym);
      }
    } else {
      // Recurse into lists/groups
      collect_specs(source, child, spec_kind, symbol_kind, out);
    }
  }
}
