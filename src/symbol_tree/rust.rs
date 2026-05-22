use super::{Symbol, make_symbol, node_name, signature_text};
use anyhow::{Context, Result};
use tree_sitter::Node;

pub fn parse(source: &str) -> Result<Vec<Symbol>> {
  let mut parser = tree_sitter::Parser::new();
  let lang = tree_sitter::Language::new(tree_sitter_rust::LANGUAGE);
  parser.set_language(&lang)?;
  let tree = parser.parse(source, None).context("rust parse failed")?;
  Ok(extract_block(source, tree.root_node()))
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

fn signature(source: &str, node: Node) -> String {
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

fn node_to_symbol(source: &str, node: Node) -> Option<Symbol> {
  match node.kind() {
    "function_item" | "function_signature_item" => {
      let name = node_name(source, node)?;
      Some(make_symbol(
        source,
        node,
        "fn",
        name,
        signature(source, node),
        vec![],
      ))
    }
    "struct_item" => {
      let name = node_name(source, node)?;
      Some(make_symbol(
        source,
        node,
        "struct",
        name,
        signature(source, node),
        extract_block(source, node),
      ))
    }
    "enum_item" => {
      let name = node_name(source, node)?;
      Some(make_symbol(
        source,
        node,
        "enum",
        name,
        signature(source, node),
        extract_block(source, node),
      ))
    }
    "trait_item" => {
      let name = node_name(source, node)?;
      Some(make_symbol(
        source,
        node,
        "trait",
        name,
        signature(source, node),
        extract_block(source, node),
      ))
    }
    "impl_item" => {
      let type_name = node
        .child_by_field_name("type")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      let trait_name = node
        .child_by_field_name("trait")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok().map(|s| s.to_string()));
      let name = match trait_name {
        Some(t) => format!("{} for {}", t, type_name),
        None => type_name,
      };
      Some(make_symbol(
        source,
        node,
        "impl",
        name,
        signature(source, node),
        extract_block(source, node),
      ))
    }
    "mod_item" => {
      let name = node_name(source, node)?;
      Some(make_symbol(
        source,
        node,
        "mod",
        name,
        signature(source, node),
        extract_block(source, node),
      ))
    }
    "type_item" => {
      let name = node_name(source, node)?;
      Some(make_symbol(
        source,
        node,
        "type",
        name,
        signature(source, node),
        vec![],
      ))
    }
    "const_item" => {
      let name = node_name(source, node)?;
      Some(make_symbol(
        source,
        node,
        "const",
        name,
        signature(source, node),
        vec![],
      ))
    }
    "static_item" => {
      let name = node_name(source, node)?;
      Some(make_symbol(
        source,
        node,
        "static",
        name,
        signature(source, node),
        vec![],
      ))
    }
    "macro_definition" => {
      let name = node_name(source, node)?;
      Some(make_symbol(
        source,
        node,
        "macro",
        name,
        signature(source, node),
        vec![],
      ))
    }
    _ => None,
  }
}
