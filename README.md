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

The outer LLM agent loop is strictly limited to exactly two tools:
* `exec`: Executes a stateless, one-off Lua 5.5 script.
* `eval`: Executes a stateful Lua 5.5 script within the persistent session (retains globals/functions).

Within the Lua execution sandbox, scripts can invoke workspace operations directly using positional global functions or standard table-argument functions:

### Filesystem & Editing
* `read_file(path, offset, limit)`: Reads a file from the workspace starting at a byte offset with a max byte limit.
* `write_file{path=..., content=..., overwrite_existing=...}`: Writes content to a file.
* `read_hash_anchors(path, offset, limit)`: Reads a file with line FNV-1a hashes prefixed (e.g. `<line>:<hash>|content`).
* `apply_anchor_edits(ops)` or `apply_anchor_edits(path, ops)`: Applies a batch array of `EditOp` tables all at once without re-calculating anchors (infers path from the last `read_hash_anchors` call if omitted).

### Skills & Asset Loading
* `list_skills()`: Lists all discovered skills in Markdown format with their names, root directories, and descriptions.
* `load_skill(name)`: Loads a pre-configured skill prompt template.
* `load_skill_asset(root, path)`: Securely reads asset files from a whitelisted skill directory (under `cwd/` or `~/`), rejecting traversal attempts.

### Shell & Repository Maps
* `shell{command=..., timeout_seconds=...}`: Runs bounded commands (max 600s) inside the workspace root (e.g. `cargo test`, `git diff`).
* `repo_map{}` / `repo_map()`: Displays the directory structure tree of the workspace.

### Web Search (Exa)
* `web_search{query=...}` / `web_read{url=...}` / `web_code_context{query=...}`: Queries the web, reads highlight summaries, or fetches real-world code snippets.

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
