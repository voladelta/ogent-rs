use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use crate::tools::{ToolContext, parse_args, require_nonempty};

pub const MAX_LOADED_ARTIFACT_BYTES: usize = 24 * 1024;

#[derive(Deserialize)]
struct LoadArtifactArgs {
  name: String,
}

#[derive(Clone, Copy)]
enum ArtifactKind {
  Workflow,
  ContextShard,
}

impl ArtifactKind {
  fn singular(self) -> &'static str {
    match self {
      Self::Workflow => "workflow",
      Self::ContextShard => "context shard",
    }
  }

  fn plural_title(self) -> &'static str {
    match self {
      Self::Workflow => "Available Workflows",
      Self::ContextShard => "Available Context Shards",
    }
  }

  fn xml_tag(self) -> &'static str {
    match self {
      Self::Workflow => "workflow",
      Self::ContextShard => "context_shard",
    }
  }

  fn load_function(self) -> &'static str {
    match self {
      Self::Workflow => "load_workflow(name)",
      Self::ContextShard => "load_context_shard(name)",
    }
  }

  fn repo_dir(self) -> &'static str {
    match self {
      Self::Workflow => ".ogent/workflows",
      Self::ContextShard => ".ogent/context",
    }
  }

  fn home_dir(self) -> &'static str {
    match self {
      Self::Workflow => ".ogent/workflows",
      Self::ContextShard => ".ogent/context",
    }
  }

  fn roots(self, ctx: &ToolContext) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(repo_root) = ctx.workspace.readable_path(self.repo_dir()) {
      roots.push(repo_root);
    }
    if std::env::var_os("HOME").is_some() {
      let home_root = format!("~/{}", self.home_dir());
      if let Ok(home_root) = ctx.workspace.readable_path(&home_root) {
        roots.push(home_root);
      }
    }
    roots
  }
}

#[derive(Clone)]
struct ArtifactInfo {
  name: String,
  description: String,
  path: PathBuf,
  bytes: u64,
  oversized: bool,
}

struct LoadedArtifact {
  info: ArtifactInfo,
  content: String,
}

pub fn ensure_prompt_artifact_fits(kind: &str, name: &str, content: &str) -> Result<()> {
  if content.len() > MAX_LOADED_ARTIFACT_BYTES {
    bail!(
      "{kind} {name} exceeds max loaded artifact size ({} > {} bytes); split it into smaller artifacts",
      content.len(),
      MAX_LOADED_ARTIFACT_BYTES
    );
  }
  Ok(())
}

pub fn ensure_prompt_output_fits(label: &str, content: &str) -> Result<()> {
  if content.len() > MAX_LOADED_ARTIFACT_BYTES {
    bail!(
      "{label} exceeds max prompt output size ({} > {} bytes); narrow the request or split the artifacts",
      content.len(),
      MAX_LOADED_ARTIFACT_BYTES
    );
  }
  Ok(())
}

pub fn list_workflows(ctx: ToolContext, _args: &str) -> Result<String> {
  list_artifacts(ctx, ArtifactKind::Workflow)
}

pub fn load_workflow(ctx: ToolContext, args: &str) -> Result<String> {
  load_artifact(ctx, args, ArtifactKind::Workflow)
}

pub fn list_context_shards(ctx: ToolContext, _args: &str) -> Result<String> {
  list_artifacts(ctx, ArtifactKind::ContextShard)
}

pub fn load_context_shard(ctx: ToolContext, args: &str) -> Result<String> {
  load_artifact(ctx, args, ArtifactKind::ContextShard)
}

fn list_artifacts(ctx: ToolContext, kind: ArtifactKind) -> Result<String> {
  let artifacts = discover_artifacts(&ctx, kind);
  let mut out = String::new();
  writeln!(out, "# {}", kind.plural_title())?;
  if artifacts.is_empty() {
    let out = out.trim_end().to_string();
    ensure_prompt_output_fits(kind.plural_title(), &out)?;
    return Ok(out);
  }

  writeln!(out, "Use `{}` to load one.\n", kind.load_function())?;
  for artifact in artifacts {
    writeln!(out, "## {}", artifact.name)?;
    writeln!(out, "- **Path**: `{}`", artifact.path.to_string_lossy())?;
    if artifact.description.is_empty() {
      writeln!(out, "- **Description**: ")?;
    } else {
      writeln!(out, "- **Description**: {}", artifact.description)?;
    }
    writeln!(out, "- **Bytes**: {}", artifact.bytes)?;
    if artifact.oversized {
      writeln!(
        out,
        "- **Loadable**: no, exceeds {} byte limit",
        MAX_LOADED_ARTIFACT_BYTES
      )?;
    }
    out.push('\n');
  }
  let out = out.trim_end().to_string();
  ensure_prompt_output_fits(kind.plural_title(), &out)?;
  Ok(out)
}

fn load_artifact(ctx: ToolContext, args: &str, kind: ArtifactKind) -> Result<String> {
  let args: LoadArtifactArgs = parse_args(args)?;
  require_nonempty(&args.name, "name")?;
  let info = discover_artifacts(&ctx, kind)
    .into_iter()
    .find(|artifact| artifact.name == args.name)
    .ok_or_else(|| anyhow::anyhow!("{} {} not found", kind.singular(), args.name))?;

  if info.oversized {
    bail!(
      "{} {} exceeds max loaded artifact size ({} > {} bytes); split it into smaller artifacts",
      kind.singular(),
      info.name,
      info.bytes,
      MAX_LOADED_ARTIFACT_BYTES
    );
  }

  let content = read_loadable_file(&info.path).with_context(|| {
    format!(
      "failed to read {} at {}",
      kind.singular(),
      info.path.display()
    )
  })?;
  let artifact = LoadedArtifact { info, content };
  let formatted = format_loaded_artifact(kind, &artifact);
  ensure_prompt_artifact_fits(kind.singular(), &artifact.info.name, &formatted)?;
  Ok(formatted)
}

fn discover_artifacts(ctx: &ToolContext, kind: ArtifactKind) -> Vec<ArtifactInfo> {
  let mut artifacts = Vec::new();
  let mut seen = HashSet::new();

  for root in kind.roots(ctx) {
    let Ok(entries) = std::fs::read_dir(root) else {
      continue;
    };
    for entry in entries.flatten() {
      if !entry.file_type().is_ok_and(|t| t.is_file()) {
        continue;
      }
      let path = entry.path();
      if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        continue;
      }
      let Some(file_stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        continue;
      };
      let Ok(meta) = entry.metadata() else {
        continue;
      };
      let bytes = meta.len();
      let oversized = bytes > MAX_LOADED_ARTIFACT_BYTES as u64;
      let (name, description) = if oversized {
        (
          file_stem.to_string(),
          format!(
            "exceeds {} byte loaded artifact limit",
            MAX_LOADED_ARTIFACT_BYTES
          ),
        )
      } else {
        let content = match std::fs::read_to_string(&path) {
          Ok(content) => content,
          Err(_) => continue,
        };
        let (frontmatter_name, description) = parse_artifact_frontmatter(&content);
        let name = if frontmatter_name.is_empty() {
          file_stem.to_string()
        } else {
          frontmatter_name
        };
        (name, description)
      };

      if seen.insert(name.clone()) {
        artifacts.push(ArtifactInfo {
          name,
          description,
          path,
          bytes,
          oversized,
        });
      }
    }
  }

  artifacts
}

fn read_loadable_file(path: &Path) -> Result<String> {
  let meta = std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
  if !meta.is_file() {
    bail!("artifact path is not a file");
  }
  if meta.len() > MAX_LOADED_ARTIFACT_BYTES as u64 {
    bail!(
      "artifact exceeds max loaded artifact size ({} > {} bytes)",
      meta.len(),
      MAX_LOADED_ARTIFACT_BYTES
    );
  }
  std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

fn xml_escape(s: &str) -> String {
  let mut out = String::with_capacity(s.len());
  for c in s.chars() {
    match c {
      '&' => out.push_str("&amp;"),
      '"' => out.push_str("&quot;"),
      '<' => out.push_str("&lt;"),
      '>' => out.push_str("&gt;"),
      _ => out.push(c),
    }
  }
  out
}

fn format_loaded_artifact(kind: ArtifactKind, artifact: &LoadedArtifact) -> String {
  let tag = kind.xml_tag();
  format!(
    "<{} name=\"{}\" path=\"{}\">\n{}\n</{}>",
    tag,
    xml_escape(&artifact.info.name),
    xml_escape(&artifact.info.path.to_string_lossy()),
    artifact.content,
    tag
  )
}

#[derive(Deserialize, Default)]
struct ArtifactFrontmatter {
  #[serde(default)]
  name: String,
  #[serde(default)]
  description: String,
}

fn parse_frontmatter(content: &str) -> Option<&str> {
  content
    .strip_prefix("---")
    .and_then(|rest| rest.find("---").map(|end| &rest[..end]))
}

fn parse_artifact_frontmatter(content: &str) -> (String, String) {
  let fm = parse_frontmatter(content).unwrap_or("");
  let parsed = serde_yaml::from_str::<ArtifactFrontmatter>(fm).unwrap_or_default();
  (parsed.name, parsed.description)
}
