use anyhow::{Context, Result, bail};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use tree_sitter::{Language, Node, Parser, TreeCursor};

use crate::tools::{ToolContext, parse_args, require_nonempty};

const MAX_CONTEXT: usize = 5;
const DEFAULT_MAX_MATCHES: usize = 100;
const MAX_MATCHES: usize = 500;
const MAX_SEARCH_FILE_BYTES: u64 = 1 << 20;

#[derive(Deserialize)]
struct SearchTextArgs {
  pattern: String,
  paths: Option<Vec<String>>,
  #[serde(default)]
  regex: bool,
  #[serde(default = "default_true")]
  case_sensitive: bool,
  #[serde(default)]
  context: usize,
  #[serde(default)]
  max_matches: usize,
}

#[derive(Deserialize, Serialize)]
struct TextMatch {
  path: String,
  line: usize,
  column: usize,
  text: String,
  before: Vec<String>,
  after: Vec<String>,
}

pub fn search_text(ctx: ToolContext, args: &str) -> Result<String> {
  let args: SearchTextArgs = parse_args(args)?;
  require_nonempty(&args.pattern, "pattern")?;
  let context = args.context.min(MAX_CONTEXT);
  let max_matches = if args.max_matches == 0 {
    DEFAULT_MAX_MATCHES
  } else {
    args.max_matches.min(MAX_MATCHES)
  };
  let paths = args.paths.unwrap_or_else(|| vec![".".to_string()]);
  let matcher = build_matcher(&args.pattern, args.regex, args.case_sensitive)?;
  let mut out = Vec::new();

  for requested in paths {
    require_nonempty(&requested, "paths entries")?;
    let start = ctx.workspace.workspace_path(&requested)?;
    if !start.is_file() && !start.is_dir() {
      bail!("path {requested} is not a file or directory");
    }

    let walk_root = if start.is_file() {
      ctx.workspace.root()
    } else {
      &start
    };
    let walker = ignore::WalkBuilder::new(walk_root)
      .sort_by_file_name(|a, b| a.cmp(b))
      .build();
    for entry in walker {
      let entry = entry?;
      if out.len() >= max_matches {
        break;
      }
      if start.is_file() && entry.path() != start {
        continue;
      }
      if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
        search_file(
          ctx.workspace.root(),
          entry.path(),
          &matcher,
          context,
          max_matches,
          &mut out,
        )?;
      }
    }
    if out.len() >= max_matches {
      break;
    }
  }

  Ok(serde_json::to_string(&out)?)
}

#[derive(Deserialize)]
struct OutlineArgs {
  path: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct OutlineEntry {
  pub(crate) name: String,
  pub(crate) kind: String,
  pub(crate) start_line: usize,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub(crate) end_line: Option<usize>,
  pub(crate) signature: String,
}

pub fn outline(ctx: ToolContext, args: &str) -> Result<String> {
  let args: OutlineArgs = parse_args(args)?;
  require_nonempty(&args.path, "path")?;
  let path = ctx.workspace.workspace_path(&args.path)?;
  let entries = outline_entries_for_path(&path, &args.path)?;
  Ok(serde_json::to_string(&entries)?)
}

pub(crate) fn outline_entries_for_path(
  path: &Path,
  display_path: &str,
) -> Result<Vec<OutlineEntry>> {
  let language = OutlineLanguage::from_path(display_path)?;
  let meta = fs::metadata(path).with_context(|| format!("stat {}", display_path))?;
  if meta.len() > MAX_SEARCH_FILE_BYTES {
    bail!(
      "file {} exceeds size limit ({} > {} bytes)",
      display_path,
      meta.len(),
      MAX_SEARCH_FILE_BYTES
    );
  }
  let source = fs::read_to_string(path).with_context(|| format!("read {}", display_path))?;
  let mut parser = Parser::new();
  parser
    .set_language(&language.tree_sitter_language())
    .with_context(|| format!("load tree-sitter parser for {}", language.name()))?;
  let tree = parser
    .parse(&source, None)
    .with_context(|| format!("parse {}", display_path))?;

  Ok(language.outline_entries(&source, tree.root_node()))
}

fn build_matcher(pattern: &str, regex: bool, case_sensitive: bool) -> Result<Regex> {
  let pattern = if regex {
    pattern.to_string()
  } else {
    regex::escape(pattern)
  };
  RegexBuilder::new(&pattern)
    .case_insensitive(!case_sensitive)
    .build()
    .with_context(|| format!("invalid regex pattern {pattern:?}"))
}

fn search_file(
  root: &Path,
  path: &Path,
  matcher: &Regex,
  context: usize,
  max_matches: usize,
  out: &mut Vec<TextMatch>,
) -> Result<()> {
  if out.len() >= max_matches {
    return Ok(());
  }
  let meta = match fs::metadata(path) {
    Ok(meta) => meta,
    Err(_) => return Ok(()),
  };
  if meta.len() > MAX_SEARCH_FILE_BYTES {
    return Ok(());
  }
  let source = match fs::read_to_string(path) {
    Ok(source) => source,
    Err(_) => return Ok(()),
  };
  let lines: Vec<&str> = source.lines().collect();
  let rel_path = path.strip_prefix(root).unwrap_or(path).to_string_lossy();

  for (idx, line) in lines.iter().enumerate() {
    if let Some(found) = matcher.find(line) {
      let before_start = idx.saturating_sub(context);
      let after_end = (idx + 1 + context).min(lines.len());
      out.push(TextMatch {
        path: rel_path.to_string(),
        line: idx + 1,
        column: found.start() + 1,
        text: (*line).to_string(),
        before: lines[before_start..idx]
          .iter()
          .map(|line| (*line).to_string())
          .collect(),
        after: lines[(idx + 1)..after_end]
          .iter()
          .map(|line| (*line).to_string())
          .collect(),
      });
      if out.len() >= max_matches {
        break;
      }
    }
  }
  Ok(())
}

fn compact_signature(line: &str) -> String {
  line.split_whitespace().collect::<Vec<_>>().join(" ")
}

enum OutlineLanguage {
  Rust,
  Go,
  Python,
}

impl OutlineLanguage {
  fn from_path(path: &str) -> Result<Self> {
    match Path::new(path).extension().and_then(|e| e.to_str()) {
      Some("rs") => Ok(Self::Rust),
      Some("go") => Ok(Self::Go),
      Some("py") => Ok(Self::Python),
      _ => bail!("outline supports .rs, .go, and .py files only"),
    }
  }

  fn name(&self) -> &'static str {
    match self {
      Self::Rust => "Rust",
      Self::Go => "Go",
      Self::Python => "Python",
    }
  }

  fn tree_sitter_language(&self) -> Language {
    match self {
      Self::Rust => tree_sitter_rust::LANGUAGE.into(),
      Self::Go => tree_sitter_go::LANGUAGE.into(),
      Self::Python => tree_sitter_python::LANGUAGE.into(),
    }
  }

  fn outline_entries(&self, source: &str, root: Node<'_>) -> Vec<OutlineEntry> {
    let mut entries = Vec::new();
    let mut cursor = root.walk();
    self.visit_node(source, root, &mut cursor, &mut entries);
    entries
  }

  fn visit_node(
    &self,
    source: &str,
    node: Node<'_>,
    cursor: &mut TreeCursor<'_>,
    entries: &mut Vec<OutlineEntry>,
  ) {
    if let Some(entry) = self.entry_for_node(source, node) {
      entries.push(entry);
    }

    if cursor.goto_first_child() {
      loop {
        self.visit_node(source, cursor.node(), cursor, entries);
        if !cursor.goto_next_sibling() {
          break;
        }
      }
      cursor.goto_parent();
    }
  }

  fn entry_for_node(&self, source: &str, node: Node<'_>) -> Option<OutlineEntry> {
    let (kind, name) = match self {
      Self::Rust => rust_entry(source, node)?,
      Self::Go => go_entry(source, node)?,
      Self::Python => python_entry(source, node)?,
    };
    Some(OutlineEntry {
      name,
      kind: kind.to_string(),
      start_line: node.start_position().row + 1,
      end_line: Some(node.end_position().row + 1),
      signature: node_signature(source, node),
    })
  }
}

fn default_true() -> bool {
  true
}

fn rust_entry<'a>(source: &str, node: Node<'a>) -> Option<(&'static str, String)> {
  match node.kind() {
    "function_item" => Some((
      if has_ancestor_kind(node, "impl_item") {
        "method"
      } else {
        "function"
      },
      node_name(source, node)?,
    )),
    "struct_item" => Some(("struct", node_name(source, node)?)),
    "enum_item" => Some(("enum", node_name(source, node)?)),
    "trait_item" => Some(("trait", node_name(source, node)?)),
    "mod_item" => Some(("mod", node_name(source, node)?)),
    "impl_item" => Some(("impl", rust_impl_name(source, node))),
    _ => None,
  }
}

fn go_entry(source: &str, node: Node<'_>) -> Option<(&'static str, String)> {
  match node.kind() {
    "function_declaration" => Some(("function", node_name(source, node)?)),
    "method_declaration" => Some(("method", node_name(source, node)?)),
    "type_declaration" => Some(("type", node_name(source, node)?)),
    "type_spec" => {
      let kind = match node.child_by_field_name("type").map(|n| n.kind()) {
        Some("struct_type") => "struct",
        Some("interface_type") => "interface",
        _ => "type",
      };
      Some((kind, node_name(source, node)?))
    }
    _ => None,
  }
}

fn python_entry(source: &str, node: Node<'_>) -> Option<(&'static str, String)> {
  match node.kind() {
    "function_definition" => Some(("function", node_name(source, node)?)),
    "class_definition" => Some(("class", node_name(source, node)?)),
    _ => None,
  }
}

fn node_name(source: &str, node: Node<'_>) -> Option<String> {
  node
    .child_by_field_name("name")
    .and_then(|name| name.utf8_text(source.as_bytes()).ok())
    .map(|name| name.to_string())
}

fn rust_impl_name(source: &str, node: Node<'_>) -> String {
  if let Some(type_node) = node.child_by_field_name("type")
    && let Ok(name) = type_node.utf8_text(source.as_bytes())
  {
    return compact_signature(name);
  }

  node
    .utf8_text(source.as_bytes())
    .ok()
    .and_then(|text| text.split('{').next())
    .map(|head| compact_signature(head.trim_start_matches("impl").trim()))
    .filter(|name| !name.is_empty())
    .unwrap_or_else(|| "impl".to_string())
}

fn has_ancestor_kind(mut node: Node<'_>, kind: &str) -> bool {
  while let Some(parent) = node.parent() {
    if parent.kind() == kind {
      return true;
    }
    node = parent;
  }
  false
}

fn node_signature(source: &str, node: Node<'_>) -> String {
  let start = node.start_byte();
  let line_end = source[start..]
    .find('\n')
    .map(|offset| start + offset)
    .unwrap_or_else(|| node.end_byte());
  compact_signature(&source[start..line_end.min(node.end_byte())])
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::skills::SkillStore;
  use crate::workspace::Workspace;
  use std::path::PathBuf;
  use std::sync::Arc;

  fn test_context(root: PathBuf) -> ToolContext {
    let workspace = Workspace::from_root(root);
    let skill_store = Arc::new(SkillStore::new(workspace.root()));
    let client = crate::client::Client::new(
      crate::client::ClientConfig {
        url: "http://localhost".to_string(),
        api_key: "dummy".into(),
        request_timeout_secs: 30,
        require_sse_done: true,
      },
      |_, _| Ok(serde_json::Value::Null),
    )
    .unwrap();
    ToolContext {
      workspace,
      skill_store,
      lua_session: Arc::new(parking_lot::Mutex::new(None)),
      client,
      output_sink: None,
      verbose: false,
      actor_id: "director".to_string(),
      agent_depth: 0,
    }
  }

  #[test]
  fn search_text_respects_gitignore_and_bounds_results() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::write(root.join(".gitignore"), "ignored/\n")?;
    fs::create_dir(root.join(".git"))?;
    fs::create_dir(root.join("ignored"))?;
    fs::write(root.join("ignored/file.rs"), "needle\n")?;
    fs::write(root.join("a.rs"), "one\nNeedle\nneedle\n")?;

    let res = search_text(
      test_context(root.to_path_buf()),
      r#"{"pattern":"needle","case_sensitive":false,"context":1,"max_matches":1}"#,
    )?;
    let matches: Vec<TextMatch> = serde_json::from_str(&res)?;
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].path, "a.rs");
    assert_eq!(matches[0].line, 2);
    assert_eq!(matches[0].column, 1);
    assert_eq!(matches[0].before, vec!["one"]);

    let res = search_text(
      test_context(root.to_path_buf()),
      r#"{"pattern":"needle","paths":["ignored/file.rs"]}"#,
    )?;
    let matches: Vec<TextMatch> = serde_json::from_str(&res)?;
    assert!(matches.is_empty());

    Ok(())
  }

  #[test]
  fn outline_returns_tree_sitter_rust_items() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::write(
      root.join("lib.rs"),
      "pub mod api;\n\npub struct Thing {\n  value: i32,\n}\n\nimpl Thing {\n  pub fn new() -> Self {\n    Self { value: 1 }\n  }\n}\n",
    )?;

    let res = outline(test_context(root.to_path_buf()), r#"{"path":"lib.rs"}"#)?;
    let entries: Vec<OutlineEntry> = serde_json::from_str(&res)?;
    assert!(entries.iter().any(|e| e.kind == "mod" && e.name == "api"));
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "struct" && e.name == "Thing" && e.end_line == Some(5))
    );
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "impl" && e.name == "Thing")
    );
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "method" && e.name == "new")
    );

    Ok(())
  }

  #[test]
  fn outline_returns_tree_sitter_go_items() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::write(
      root.join("main.go"),
      "package main\n\ntype Server struct {\n  addr string\n}\n\ntype Runner interface {\n  Run() error\n}\n\nfunc NewServer() *Server {\n  return &Server{}\n}\n\nfunc (s *Server) Start() {}\n",
    )?;

    let res = outline(test_context(root.to_path_buf()), r#"{"path":"main.go"}"#)?;
    let entries: Vec<OutlineEntry> = serde_json::from_str(&res)?;
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "struct" && e.name == "Server" && e.end_line == Some(5))
    );
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "interface" && e.name == "Runner")
    );
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "function" && e.name == "NewServer")
    );
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "method" && e.name == "Start")
    );

    Ok(())
  }

  #[test]
  fn outline_returns_tree_sitter_python_items() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::write(
      root.join("app.py"),
      "class Service:\n    def run(self):\n        return 1\n\nasync def build():\n    return Service()\n",
    )?;

    let res = outline(test_context(root.to_path_buf()), r#"{"path":"app.py"}"#)?;
    let entries: Vec<OutlineEntry> = serde_json::from_str(&res)?;
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "class" && e.name == "Service" && e.end_line == Some(3))
    );
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "function" && e.name == "run")
    );
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "function" && e.name == "build")
    );
    assert!(
      entries
        .iter()
        .all(|e| !e.signature.contains("return Service()"))
    );

    Ok(())
  }

  #[test]
  fn outline_is_best_effort_for_parse_errors() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let source = "class Service:\n    def run(self):\n        return 1\n\nif True\n";
    fs::write(root.join("app.py"), source)?;

    let mut parser = Parser::new();
    let language: Language = tree_sitter_python::LANGUAGE.into();
    parser.set_language(&language)?;
    let tree = parser
      .parse(source, None)
      .expect("python parser returns a tree");
    assert!(tree.root_node().has_error());

    let res = outline(test_context(root.to_path_buf()), r#"{"path":"app.py"}"#)?;
    let entries: Vec<OutlineEntry> = serde_json::from_str(&res)?;
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "class" && e.name == "Service")
    );
    assert!(
      entries
        .iter()
        .any(|e| e.kind == "function" && e.name == "run")
    );

    Ok(())
  }

  #[test]
  fn outline_rejects_unsupported_extensions() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::write(root.join("notes.txt"), "hello\n")?;

    let err = outline(test_context(root.to_path_buf()), r#"{"path":"notes.txt"}"#)
      .expect_err("unsupported extension should fail");
    assert!(err.to_string().contains(".rs, .go, and .py"));

    Ok(())
  }
}
