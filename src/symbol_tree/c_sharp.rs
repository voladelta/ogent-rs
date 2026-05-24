use super::{Symbol, byte_to_line, group_or_single, make_symbol, node_name, signature_text};
use anyhow::{Context, Result};
use tree_sitter::Node;

pub fn parse(source: &str) -> Result<Vec<Symbol>> {
  let mut parser = tree_sitter::Parser::new();
  let lang = tree_sitter::Language::new(tree_sitter_c_sharp::LANGUAGE);
  parser.set_language(&lang)?;
  let tree = parser.parse(source, None).context("c_sharp parse failed")?;

  let root = tree.root_node();
  Ok(
    file_scoped_namespace_symbol(source, root)
      .map(|s| vec![s])
      .unwrap_or_else(|| extract_block(source, root)),
  )
}

fn file_scoped_namespace_symbol(source: &str, root: Node) -> Option<Symbol> {
  let mut file_scoped_ns = None;
  let mut cursor = root.walk();
  for child in root.children(&mut cursor) {
    if child.kind() == "file_scoped_namespace_declaration" {
      file_scoped_ns = Some(child);
      break;
    }
  }

  let ns_node = file_scoped_ns?;
  let name = node_name(source, ns_node)?;

  let mut children = Vec::new();
  let mut inner_cursor = root.walk();
  for child in root.children(&mut inner_cursor) {
    if child.kind() != "file_scoped_namespace_declaration" && child.kind() != "using_directive" {
      if let Some(sym) = node_to_symbol(source, child) {
        children.push(sym);
      } else {
        match child.kind() {
          "declaration_list" | "class_body" | "struct_body" | "interface_body" => {
            children.extend(extract_block(source, child));
          }
          _ => {}
        }
      }
    }
  }
  let mut ns_sym = make_symbol(
    source,
    ns_node,
    "namespace",
    name,
    signature(source, ns_node),
    children,
  );
  ns_sym.line_end = byte_to_line(source, root.end_byte());
  Some(ns_sym)
}

fn extract_block(source: &str, node: Node) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if let Some(sym) = node_to_symbol(source, child) {
      out.push(sym);
    } else {
      match child.kind() {
        "declaration_list" | "class_body" | "struct_body" | "interface_body"
        | "compilation_unit" => {
          out.extend(extract_block(source, child));
        }
        _ => {}
      }
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
      "arrow_expression_clause",
      "declaration_list",
      "class_body",
      "struct_body",
      "interface_body",
      "enum_body",
    ],
  )
}

fn collect_variable_declarators(source: &str, node: Node) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if child.kind() == "variable_declaration" {
      let mut inner_cursor = child.walk();
      for var_decl in child.children(&mut inner_cursor) {
        if var_decl.kind() == "variable_declarator"
          && let Some(name) = node_name(source, var_decl)
        {
          let sig = signature_text(source, var_decl, &[]);
          out.push(make_symbol(source, var_decl, "field", name, sig, vec![]));
        }
      }
    }
  }
  out
}

fn node_to_symbol(source: &str, node: Node) -> Option<Symbol> {
  match node.kind() {
    "namespace_declaration" | "file_scoped_namespace_declaration" => {
      let name = node_name(source, node)?;
      Some(make_symbol(
        source,
        node,
        "namespace",
        name,
        signature(source, node),
        extract_block(source, node),
      ))
    }
    "class_declaration" | "record_declaration" => {
      let name = node_name(source, node)?;
      Some(make_symbol(
        source,
        node,
        "class",
        name,
        signature(source, node),
        extract_block(source, node),
      ))
    }
    "struct_declaration" => {
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
    "interface_declaration" => {
      let name = node_name(source, node)?;
      Some(make_symbol(
        source,
        node,
        "interface",
        name,
        signature(source, node),
        extract_block(source, node),
      ))
    }
    "enum_declaration" => {
      let name = node_name(source, node)?;
      Some(make_symbol(
        source,
        node,
        "enum",
        name,
        signature(source, node),
        vec![],
      ))
    }
    "method_declaration" | "constructor_declaration" | "destructor_declaration" => {
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
    "property_declaration" => {
      let name = node_name(source, node)?;
      Some(make_symbol(
        source,
        node,
        "field",
        name,
        signature(source, node),
        vec![],
      ))
    }
    "field_declaration" => {
      let children = collect_variable_declarators(source, node);
      group_or_single(source, node, "field", signature(source, node), children)
    }
    _ => None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn c_sharp_simple_file() {
    let src = r#"
using System;

namespace Enterprise.App
{
    public interface IGreet
    {
        string Greet(string name);
    }

    public class Greeter : IGreet
    {
        public string Title { get; set; }
        private string greetingPrefix;

        public Greeter(string prefix)
        {
            greetingPrefix = prefix;
        }

        public string Greet(string name)
        {
            return $"{greetingPrefix} {name}";
        }
    }

    public struct Point
    {
        public int X;
        public int Y;
    }

    public enum Mode
    {
        Active,
        Inactive
    }
}
"#;
    let syms = parse(src).unwrap();
    let ns = syms
      .iter()
      .find(|s| s.kind == "namespace" && s.name == "Enterprise.App")
      .unwrap();

    let interface = ns
      .children
      .iter()
      .find(|s| s.kind == "interface" && s.name == "IGreet")
      .unwrap();
    assert!(
      interface
        .children
        .iter()
        .any(|s| s.kind == "fn" && s.name == "Greet")
    );

    let greeter = ns
      .children
      .iter()
      .find(|s| s.kind == "class" && s.name == "Greeter")
      .unwrap();
    assert!(
      greeter
        .children
        .iter()
        .any(|s| s.kind == "field" && s.name == "Title")
    );
    assert!(
      greeter
        .children
        .iter()
        .any(|s| s.kind == "field" && s.name == "greetingPrefix")
    );
    assert!(
      greeter
        .children
        .iter()
        .any(|s| s.kind == "fn" && s.name == "Greeter")
    );
    assert!(
      greeter
        .children
        .iter()
        .any(|s| s.kind == "fn" && s.name == "Greet")
    );

    let point = ns
      .children
      .iter()
      .find(|s| s.kind == "struct" && s.name == "Point")
      .unwrap();
    assert!(
      point
        .children
        .iter()
        .any(|s| s.kind == "field" && s.name == "X")
    );
    assert!(
      point
        .children
        .iter()
        .any(|s| s.kind == "field" && s.name == "Y")
    );

    assert!(
      ns.children
        .iter()
        .any(|s| s.kind == "enum" && s.name == "Mode")
    );
  }

  #[test]
  fn c_sharp_file_scoped_namespace_with_multiple_declarations() {
    let src = r#"
namespace App.Core;

class A {}
struct B {}
enum C { One }
public record PersonRecord(string Name, int Age);
"#;
    let syms = parse(src).unwrap();
    let ns = syms
      .iter()
      .find(|s| s.kind == "namespace" && s.name == "App.Core")
      .unwrap();
    assert_eq!(ns.line_start, 2);
    assert_eq!(ns.line_end, 8);

    assert!(
      ns.children
        .iter()
        .any(|s| s.kind == "class" && s.name == "A")
    );
    assert!(
      ns.children
        .iter()
        .any(|s| s.kind == "struct" && s.name == "B")
    );
    assert!(
      ns.children
        .iter()
        .any(|s| s.kind == "enum" && s.name == "C")
    );
    assert!(
      ns.children
        .iter()
        .any(|s| s.kind == "class" && s.name == "PersonRecord")
    );
  }

  #[test]
  fn c_sharp_multiple_field_declarators() {
    let src = r#"
class Config {
    const int A = 1, B = 2;
    static string X, Y;
}
"#;
    let syms = parse(src).unwrap();
    let config = syms
      .iter()
      .find(|s| s.kind == "class" && s.name == "Config")
      .unwrap();

    let const_group = config
      .children
      .iter()
      .find(|s| s.kind == "field" && s.signature.contains("const"))
      .unwrap();
    assert!(
      const_group
        .children
        .iter()
        .any(|s| s.kind == "field" && s.name == "A")
    );
    assert!(
      const_group
        .children
        .iter()
        .any(|s| s.kind == "field" && s.name == "B")
    );

    let static_group = config
      .children
      .iter()
      .find(|s| s.kind == "field" && s.signature.contains("static"))
      .unwrap();
    assert!(
      static_group
        .children
        .iter()
        .any(|s| s.kind == "field" && s.name == "X")
    );
    assert!(
      static_group
        .children
        .iter()
        .any(|s| s.kind == "field" && s.name == "Y")
    );
  }
}
