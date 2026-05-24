# ogent

`ogent` is a single CLI agent process designed to perform focused, bounded tasks (implementing a feature, debugging a failure, reviewing changes, etc.) within a workspace.

## Quick Start

1. Rename the `dotogent` boilerplate directory to `.ogent` at the repo level or copy it to `~/.ogent` for global config. It contains `config.yaml` and the `colgrep` skill.
2. Edit `.ogent/config.yaml` or `~/.ogent/config.yaml` to configure your models and providers.
3. Set the required API keys (e.g. for DeepSeek and Exa search):
   ```bash
   export DEEPSEEK_API_KEY="sk-..."
   export EXA_API_KEY="..."
   ```
4. Run:
   ```bash
   cargo run -- "Fix the failing tests without overcomplicating"
   ```

## CLI Usage

Run a task with the default profile:
```bash
ogent "Fix the failing parser test"
```

Override the default profile/model:
```bash
ogent --profile kimi "Review the staged diff"
```

### CLI Flags

| Flag | Meaning |
| --- | --- |
| `--profile <name>` | Model/profile selection, overriding the default in `config.yaml`. Available profiles: `ds-flash`, `ds-flash-max`, `ds-pro`, `ds-pro-max`, `kimi`, `glm`. |

---

## Agent Runtime & Key Behavior

- **Agent Process**: Each invocation runs one standalone CLI agent process.
- **System Prompt**: It relies on [SYSTEM_PROMPT.md](SYSTEM_PROMPT.md) for its prompt loop, state, and result formatting guidelines.
- **Session Persistence**: CLI runs persist conversation transcripts to `.ogent/sessions/{session_id}.jsonl` on exit.
- **Run Completion**: A run terminates when the agent returns a final message without calling any more tools.

---

## Agent Tools Reference

Every agent run receives the full agent toolset:

* **Filesystem & Editing**:
  - `read_file`: Reads a file from the workspace (line-indexed, optional start/end bounds).
  - `write_file`: Writes content to a new file.
  - `read_hash_anchors`: Reads a file with line hashes prefixed (e.g. `<line>:<hash>|content`).
  - `edit_hash_anchors`: Performs safe edits using FNV-1a line-content hashes.
* **Code Search & Mapping**:
  - `repo_map`: Displays the directory structure tree of the workspace.
  - `code_map`: Renders a symbol map (structs, functions, enums, etc.) using tree-sitter for Rust, Go, TypeScript, JavaScript, Python, C++, and C#.
  - `bash`: Runs bounded commands (max 600s) inside the workspace root (e.g. cargo test, git diff).
* **Web Search**:
  - `web_search`: Queries the web for excerpts via Exa.
  - `web_read`: Reads key highlights or raw text from web URLs.
  - `web_code_context`: Searches real-world code for syntax/API context.
* **Utilities**:
  - `load_skill`: Dynamically loads pre-configured skill prompt/resources.

---

## Developer Guide & Operating Rules

When developing or pair programming with `ogent`:

### Operating Rules
- **Smallest Correct Change**: Always optimize for the smallest correct delta.
- **Search First**: Prefer `colgrep` for behavioral/intent search, `rg` for exact text, and `ast-grep` for structural queries.
- **Runtime Safety**: Do not edit runtime artifacts (`.ogent/sessions/`, `target/`) unless requested.
- **Final Handoff**: When completing a task, summarize changed files, verification performed, and any doc updates.

### Search Quick Reference
```bash
# Semantic search via colgrep
colgrep "<intent>" -k 20
colgrep -e "<exact text>" "<intent>"

# Exact string search via ripgrep
rg "<exact symbol>"

# Structure maps via code_map
code_map {"path": "src"}           # Map workspace/directory symbols
code_map {"path": "src/main.rs"}   # Map single file symbols
```

## Development Commands

```bash
cargo fmt
cargo check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## Docs
- [ARCHITECTURE.md](ARCHITECTURE.md)
