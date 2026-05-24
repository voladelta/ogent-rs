use super::{Symbol, group_or_single, make_symbol, signature_text};
use anyhow::{Context, Result, bail};
use tree_sitter::Node;

pub fn parse(source: &str, ext: &str) -> Result<Vec<Symbol>> {
  let mut parser = tree_sitter::Parser::new();
  let lang = match ext {
    "ts" => tree_sitter::Language::new(tree_sitter_typescript::LANGUAGE_TYPESCRIPT),
    "tsx" => tree_sitter::Language::new(tree_sitter_typescript::LANGUAGE_TSX),
    "js" | "jsx" | "mjs" | "cjs" => tree_sitter::Language::new(tree_sitter_javascript::LANGUAGE),
    _ => bail!("unsupported ts/js extension: {}", ext),
  };
  parser.set_language(&lang)?;
  let tree = parser.parse(source, None).context("ts/js parse failed")?;
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
      "statement_block",
      "class_body",
      "interface_body",
      "enum_body",
      "object_type",
    ],
  )
}

fn node_to_symbol(source: &str, node: Node) -> Option<Symbol> {
  match node.kind() {
    "function_declaration" | "generator_function_declaration" | "function_signature" => {
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
    "class_declaration" | "abstract_class_declaration" => {
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
    "interface_declaration" => {
      let name = node
        .child_by_field_name("name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(make_symbol(
        source,
        node,
        "interface",
        name,
        signature(source, node),
        extract_interface_body(source, node),
      ))
    }
    "enum_declaration" => {
      let name = node
        .child_by_field_name("name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(make_symbol(
        source,
        node,
        "enum",
        name,
        signature(source, node),
        vec![],
      ))
    }
    "type_alias_declaration" => {
      let name = node
        .child_by_field_name("name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(make_symbol(
        source,
        node,
        "type",
        name,
        signature(source, node),
        vec![],
      ))
    }
    "variable_declaration" | "lexical_declaration" => {
      let children = collect_variable_declarators(source, node);
      group_or_single(source, node, "var", signature(source, node), children)
    }
    "export_statement" => {
      if let Some(decl) = node.child_by_field_name("declaration") {
        node_to_symbol(source, decl)
      } else {
        None
      }
    }
    _ => None,
  }
}

fn collect_variable_declarators(source: &str, node: Node) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if child.kind() == "variable_declarator"
      && let Some(sym) = variable_declarator_to_symbol(source, child)
    {
      out.push(sym);
    }
  }
  out
}

fn variable_declarator_to_symbol(source: &str, node: Node) -> Option<Symbol> {
  let name = node
    .child_by_field_name("name")?
    .utf8_text(source.as_bytes())
    .ok()?
    .to_string();
  let sig = signature_text(source, node, &[]);
  Some(make_symbol(source, node, "var", name, sig, vec![]))
}

fn extract_class_body(source: &str, node: Node) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if child.kind() == "class_body" {
      let mut inner = child.walk();
      for member in child.children(&mut inner) {
        if let Some(sym) = class_member_to_symbol(source, member) {
          out.push(sym);
        }
      }
    }
  }
  out
}

fn extract_interface_body(source: &str, node: Node) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if child.kind() == "interface_body" {
      let mut inner = child.walk();
      for member in child.children(&mut inner) {
        if let Some(sym) = interface_member_to_symbol(source, member) {
          out.push(sym);
        }
      }
    }
  }
  out
}

fn class_member_to_symbol(source: &str, node: Node) -> Option<Symbol> {
  match node.kind() {
    "method_definition" => {
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
    "abstract_method_signature" => {
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
    "field_definition" => {
      let name = node
        .child_by_field_name("property")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(make_symbol(
        source,
        node,
        "field",
        name,
        signature(source, node),
        vec![],
      ))
    }
    _ => None,
  }
}

fn interface_member_to_symbol(source: &str, node: Node) -> Option<Symbol> {
  match node.kind() {
    "method_signature" => {
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
    "property_signature" => {
      let name = node
        .child_by_field_name("name")?
        .utf8_text(source.as_bytes())
        .ok()?
        .to_string();
      Some(make_symbol(
        source,
        node,
        "field",
        name,
        signature(source, node),
        vec![],
      ))
    }
    "construct_signature" => Some(make_symbol(
      source,
      node,
      "fn",
      "constructor".to_string(),
      signature(source, node),
      vec![],
    )),
    _ => None,
  }
}
