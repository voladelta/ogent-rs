use super::{Symbol, byte_to_line, group_or_single, make_symbol, signature_text};
use anyhow::{Context, Result};
use tree_sitter::Node;

pub fn parse(source: &str) -> Result<Vec<Symbol>> {
  let mut parser = tree_sitter::Parser::new();
  let lang = tree_sitter::Language::new(tree_sitter_cpp::LANGUAGE);
  parser.set_language(&lang)?;
  let tree = parser.parse(source, None).context("cpp parse failed")?;
  Ok(extract_block(source, tree.root_node(), false))
}

fn extract_block(source: &str, node: Node, is_member: bool) -> Vec<Symbol> {
  let mut out = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    let syms = symbols_for_node(source, child, is_member);
    if !syms.is_empty() {
      out.extend(syms);
    } else {
      match child.kind() {
        "declaration_list"
        | "field_declaration_list"
        | "translation_unit"
        | "linkage_specification" => {
          out.extend(extract_block(source, child, is_member));
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
      "compound_statement",
      "field_declaration_list",
      "enumerator_list",
      "declaration_list",
    ],
  )
}

fn find_declarator_name(node: Node, source: &str) -> Option<String> {
  match node.kind() {
    "identifier"
    | "field_identifier"
    | "scoped_identifier"
    | "qualified_identifier"
    | "destructor_name"
    | "operator_name" => node
      .utf8_text(source.as_bytes())
      .ok()
      .map(|s| s.to_string()),
    "template_function" => {
      if let Some(name_node) = node.child_by_field_name("name") {
        find_declarator_name(name_node, source)
      } else {
        node
          .utf8_text(source.as_bytes())
          .ok()
          .map(|s| s.to_string())
      }
    }
    "function_declarator"
    | "pointer_declarator"
    | "reference_declarator"
    | "array_declarator"
    | "parenthesized_declarator" => {
      if let Some(decl) = node.child_by_field_name("declarator") {
        find_declarator_name(decl, source)
      } else {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
          if child.kind() != "parameter_list"
            && let Some(name) = find_declarator_name(child, source)
          {
            return Some(name);
          }
        }
        None
      }
    }
    _ => {
      if let Some(decl) = node.child_by_field_name("declarator") {
        find_declarator_name(decl, source)
      } else {
        None
      }
    }
  }
}

fn is_function_declarator(node: Node) -> bool {
  match node.kind() {
    "function_declarator" => true,
    _ => {
      if let Some(decl) = node.child_by_field_name("declarator") {
        is_function_declarator(decl)
      } else {
        false
      }
    }
  }
}

fn get_declarators(node: Node) -> Vec<Node> {
  let mut decls = Vec::new();
  let mut cursor = node.walk();
  for child in node.children(&mut cursor) {
    if child.kind() == "init_declarator" {
      decls.push(child);
    }
  }
  if decls.is_empty()
    && let Some(decl) = node.child_by_field_name("declarator")
  {
    decls.push(decl);
  }
  decls
}

fn symbols_for_node(source: &str, node: Node, is_member: bool) -> Vec<Symbol> {
  match node.kind() {
    "template_declaration" => {
      let mut decl = None;
      let mut cursor = node.walk();
      for child in node.children(&mut cursor) {
        if child.kind() != "template" && child.kind() != "template_parameter_list" {
          decl = Some(child);
          break;
        }
      }
      let Some(decl) = decl else {
        return vec![];
      };
      let mut syms = symbols_for_node(source, decl, is_member);
      for sym in &mut syms {
        sym.signature = signature(source, node);
        sym.line_start = byte_to_line(source, node.start_byte());
        sym.line_end = byte_to_line(source, node.end_byte());
      }
      syms
    }
    "namespace_definition" => {
      let body = match node.child_by_field_name("body") {
        Some(b) => b,
        None => return vec![],
      };
      if let Some(name_node) = node.child_by_field_name("name")
        && let Ok(name) = name_node.utf8_text(source.as_bytes()) {
          return vec![make_symbol(
            source,
            node,
            "namespace",
            name.to_string(),
            signature(source, node),
            extract_block(source, body, false),
          )];
        }
      // Anonymous namespace flattening
      extract_block(source, body, is_member)
    }
    "class_specifier" | "struct_specifier" => {
      let Some(name_node) = node.child_by_field_name("name") else {
        return vec![];
      };
      let Ok(name) = name_node.utf8_text(source.as_bytes()) else {
        return vec![];
      };
      let Some(body) = node.child_by_field_name("body") else {
        return vec![];
      };
      let kind = if node.kind() == "class_specifier" {
        "class"
      } else {
        "struct"
      };
      vec![make_symbol(
        source,
        node,
        kind,
        name.to_string(),
        signature(source, node),
        extract_block(source, body, true),
      )]
    }
    "enum_specifier" => {
      let Some(name_node) = node.child_by_field_name("name") else {
        return vec![];
      };
      let Ok(name) = name_node.utf8_text(source.as_bytes()) else {
        return vec![];
      };
      vec![make_symbol(
        source,
        node,
        "enum",
        name.to_string(),
        signature(source, node),
        vec![],
      )]
    }
    "function_definition" => {
      let Some(decl) = node.child_by_field_name("declarator") else {
        return vec![];
      };
      let Some(name) = find_declarator_name(decl, source) else {
        return vec![];
      };
      vec![make_symbol(
        source,
        node,
        "fn",
        name,
        signature(source, node),
        vec![],
      )]
    }
    "declaration" | "field_declaration" => {
      let decls = get_declarators(node);
      let mut children = Vec::new();
      for decl in decls {
        if let Some(name) = find_declarator_name(decl, source) {
          let kind = if is_function_declarator(decl) {
            "fn"
          } else if is_member {
            "field"
          } else {
            "var"
          };
          let sig = signature_text(source, decl, &[]);
          children.push(make_symbol(source, decl, kind, name, sig, vec![]));
        }
      }
      let default_kind = if is_member { "field" } else { "var" };
      group_or_single(
        source,
        node,
        default_kind,
        signature(source, node),
        children,
      )
      .into_iter()
      .collect()
    }
    "type_definition" => {
      let Some(decl) = node.child_by_field_name("declarator") else {
        return vec![];
      };
      let Some(name) = find_declarator_name(decl, source) else {
        return vec![];
      };
      vec![make_symbol(
        source,
        node,
        "type",
        name,
        signature(source, node),
        vec![],
      )]
    }
    "alias_declaration" => {
      let Some(name_node) = node.child_by_field_name("name") else {
        return vec![];
      };
      let Ok(name) = name_node.utf8_text(source.as_bytes()) else {
        return vec![];
      };
      vec![make_symbol(
        source,
        node,
        "type",
        name.to_string(),
        signature(source, node),
        vec![],
      )]
    }
    _ => vec![],
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn cpp_simple_file() {
    let src = r#"
#include <iostream>

int global_var = 42;
int mult1 = 1, mult2 = 2;

void global_func() {}

namespace Company::Project {
    class Calculator {
    public:
        int add(int a, int b) {
            return a + b;
        }
    private:
        int cached_result;
    };

    struct Point {
        double x;
        double y;
    };

    enum Color {
        Red,
        Green,
        Blue
    };
}
"#;
    let syms = parse(src).unwrap();

    assert!(
      syms
        .iter()
        .any(|s| s.kind == "var" && s.name == "global_var")
    );
    assert!(syms.iter().any(|s| s.kind == "var" && s.name == "(group)"));
    let group = syms
      .iter()
      .find(|s| s.kind == "var" && s.name == "(group)")
      .unwrap();
    assert!(
      group
        .children
        .iter()
        .any(|s| s.kind == "var" && s.name == "mult1")
    );
    assert!(
      group
        .children
        .iter()
        .any(|s| s.kind == "var" && s.name == "mult2")
    );

    assert!(
      syms
        .iter()
        .any(|s| s.kind == "fn" && s.name == "global_func")
    );

    let ns = syms
      .iter()
      .find(|s| s.kind == "namespace" && s.name == "Company::Project")
      .unwrap();

    let calc = ns
      .children
      .iter()
      .find(|s| s.kind == "class" && s.name == "Calculator")
      .unwrap();
    assert!(
      calc
        .children
        .iter()
        .any(|s| s.kind == "fn" && s.name == "add")
    );
    assert!(
      calc
        .children
        .iter()
        .any(|s| s.kind == "field" && s.name == "cached_result")
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
        .any(|s| s.kind == "field" && s.name == "x")
    );
    assert!(
      point
        .children
        .iter()
        .any(|s| s.kind == "field" && s.name == "y")
    );

    assert!(
      ns.children
        .iter()
        .any(|s| s.kind == "enum" && s.name == "Color")
    );
  }

  #[test]
  fn cpp_qualified_out_of_class_methods_keep_qualified_name() {
    let src = r#"
class Calculator {
public:
    int add(int a, int b) { return a + b; }
    int multiply(int a, int b);
};

int Calculator::multiply(int a, int b) {
    return a * b;
}
"#;
    let syms = parse(src).unwrap();
    let calc = syms
      .iter()
      .find(|s| s.kind == "class" && s.name == "Calculator")
      .unwrap();
    assert!(
      calc
        .children
        .iter()
        .any(|s| s.kind == "fn" && s.name == "add")
    );

    assert!(
      syms
        .iter()
        .any(|s| s.kind == "fn" && s.name == "Calculator::multiply")
    );
  }

  #[test]
  fn cpp_anonymous_namespace_flattening() {
    let src = r#"
namespace {
    void hidden() {}
    class Hidden {};
}
"#;
    let syms = parse(src).unwrap();
    assert!(!syms.iter().any(|s| s.name == "(anonymous)"));
    assert!(syms.iter().any(|s| s.kind == "fn" && s.name == "hidden"));
    assert!(syms.iter().any(|s| s.kind == "class" && s.name == "Hidden"));
  }

  #[test]
  fn cpp_template_function_and_class() {
    let src = r#"
template<typename T>
T identity(T value) { return value; }

template<typename T>
class Box {
    T value;
};
"#;
    let syms = parse(src).unwrap();
    assert!(syms.iter().any(|s| s.kind == "fn" && s.name == "identity"));
    let box_class = syms
      .iter()
      .find(|s| s.kind == "class" && s.name == "Box")
      .unwrap();
    assert!(
      box_class
        .children
        .iter()
        .any(|s| s.kind == "field" && s.name == "value")
    );
  }
}
