use anyhow::{Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::Node;

mod go;
mod python;
mod rust;
mod typescript;

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
      && matches!(
        ext.to_str().unwrap_or(""),
        "rs" | "go" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py"
      )
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
    bail!(
      "no supported source files found at {} (expected .rs, .go, .ts, .tsx, .js, .jsx, .mjs, .cjs, or .py)",
      path.display()
    );
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
    "rs" => rust::parse(&source)?,
    "go" => go::parse(&source)?,
    "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => typescript::parse(&source, ext)?,
    "py" => python::parse(&source)?,
    _ => bail!("unsupported extension: {}", ext),
  };
  let mut out = String::new();
  out.push_str(&format!("{}\n", path.display()));
  for sym in syms {
    format_symbol(&mut out, &sym, 1);
  }
  Ok(out)
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
    s[pos + target.len()..].to_string()
  } else {
    s.to_string()
  }
}

pub(crate) fn byte_to_line(source: &str, byte: usize) -> usize {
  let end = byte.min(source.len());
  source[..end].chars().filter(|&c| c == '\n').count() + 1
}

pub(crate) fn signature_text(source: &str, node: Node, body_kinds: &[&str]) -> String {
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

pub(crate) fn node_name(source: &str, node: Node) -> Option<String> {
  node
    .child_by_field_name("name")?
    .utf8_text(source.as_bytes())
    .ok()
    .map(|s| s.to_string())
}

pub(crate) fn make_symbol(
  source: &str,
  node: Node,
  kind: &'static str,
  name: String,
  signature: String,
  children: Vec<Symbol>,
) -> Symbol {
  Symbol {
    kind,
    name,
    line_start: byte_to_line(source, node.start_byte()),
    line_end: byte_to_line(source, node.end_byte()),
    signature,
    children,
  }
}

// Collapse a child list into a single symbol, a group wrapper, or None.
pub(crate) fn group_or_single(
  source: &str,
  node: Node,
  kind: &'static str,
  signature: String,
  children: Vec<Symbol>,
) -> Option<Symbol> {
  match children.len() {
    0 => None,
    1 => children.into_iter().next(),
    _ => Some(make_symbol(
      source,
      node,
      kind,
      "(group)".to_string(),
      signature,
      children,
    )),
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
    let syms = go::parse(src).unwrap();
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
    let syms = go::parse(src).unwrap();
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
    let syms = go::parse(src).unwrap();
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

  #[test]
  fn typescript_simple_file() {
    let src = r#"
export interface Greeter {
  greet(name: string): string;
  readonly label: string;
}

type ID = string | number;

export enum Mode {
  Fast,
  Slow,
}

export class Service {
  start(): void {}
}

export function make(id: ID): Service {
  return new Service();
}

const version = "1";
"#;
    let syms = typescript::parse(src, "ts").unwrap();
    let names: Vec<_> = syms.iter().map(|s| (s.kind, s.name.as_str())).collect();
    assert!(names.contains(&("interface", "Greeter")));
    assert!(names.contains(&("type", "ID")));
    assert!(names.contains(&("enum", "Mode")));
    assert!(names.contains(&("class", "Service")));
    assert!(names.contains(&("fn", "make")));
    assert!(names.contains(&("var", "version")));

    let interface = syms.iter().find(|s| s.name == "Greeter").unwrap();
    assert!(
      interface
        .children
        .iter()
        .any(|s| s.kind == "fn" && s.name == "greet")
    );

    let class = syms.iter().find(|s| s.name == "Service").unwrap();
    assert!(
      class
        .children
        .iter()
        .any(|s| s.kind == "fn" && s.name == "start")
    );
  }

  #[test]
  fn javascript_simple_file() {
    let src = r#"
export function load() {
  return true;
}

class Widget {
  render() {}
}

const cached = () => true;
"#;
    let syms = typescript::parse(src, "js").unwrap();
    let names: Vec<_> = syms.iter().map(|s| (s.kind, s.name.as_str())).collect();
    assert!(names.contains(&("fn", "load")));
    assert!(names.contains(&("class", "Widget")));
    assert!(names.contains(&("var", "cached")));

    let class = syms.iter().find(|s| s.name == "Widget").unwrap();
    assert!(
      class
        .children
        .iter()
        .any(|s| s.kind == "fn" && s.name == "render")
    );
  }

  #[test]
  fn python_simple_file() {
    let src = r#"
MAX_SIZE = 100

class Worker:
    @classmethod
    async def build(cls):
        return cls()

    def run(self):
        pass

def main():
    pass
"#;
    let syms = python::parse(src).unwrap();
    let names: Vec<_> = syms.iter().map(|s| (s.kind, s.name.as_str())).collect();
    assert!(names.contains(&("var", "MAX_SIZE")));
    assert!(names.contains(&("class", "Worker")));
    assert!(names.contains(&("fn", "main")));

    let class = syms.iter().find(|s| s.name == "Worker").unwrap();
    assert!(
      class
        .children
        .iter()
        .any(|s| s.kind == "fn" && s.name == "build")
    );
    assert!(
      class
        .children
        .iter()
        .any(|s| s.kind == "fn" && s.name == "run")
    );
  }

  #[test]
  fn format_path_includes_new_language_extensions() {
    let unique = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    let dir = std::env::temp_dir().join(format!("ogent-symbol-tree-{unique}"));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.ts"), "export function typed(): void {}\n").unwrap();
    std::fs::write(dir.join("b.js"), "function scripted() {}\n").unwrap();
    std::fs::write(dir.join("c.py"), "def scripted_py():\n    pass\n").unwrap();
    std::fs::write(dir.join("ignored.txt"), "def ignored():\n    pass\n").unwrap();

    let out = format_path(&dir).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    assert!(out.contains("a.ts"));
    assert!(out.contains("typed"));
    assert!(out.contains("b.js"));
    assert!(out.contains("scripted"));
    assert!(out.contains("c.py"));
    assert!(out.contains("scripted_py"));
    assert!(!out.contains("ignored.txt"));
  }
}
