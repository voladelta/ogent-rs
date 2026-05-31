# ogent

`ogent` is a single CLI agent process designed to perform focused, bounded tasks (implementing a feature, debugging a failure, reviewing changes, etc.) within a workspace.

## Quick Start

1. Ensure an `.ogent` directory exists at the repo level (or at `~/.ogent` for global config). It should contain `config.yaml` and skill directories.
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
| `--profile <name>` | Model/profile selection, overriding the default in `config.yaml`. Available profiles: `ds-flash`, `ds-flash-max`, `ds-pro`, `ds-pro-max`, `kimi`, `glm`, `mimo`, `mimo-pro`. |
| `-v`, `--verbose`  | Show full thinking reasoning trace (`[thinking]`), Lua execution code, and tool returns. Default mode only prints actor explanation reasons, task updates, and final assistant replies. |
| `-t`, `--temp` | Run a temporary session. The transcript is not persisted on exit. |
| `-r`, `--resume <session_id>` | Resume a previous session from `.ogent/sessions/{session_id}.jsonl`. If the session does not exist, `ogent` prints an error and exits. |

---

## Agent Runtime & Key Behavior

- **Agent Process**: Each invocation runs one standalone CLI agent process.
- **System Prompt**: It relies on [PROMPT_SYSTEM.md](PROMPT_SYSTEM.md) for its prompt loop, state, and result formatting guidelines.
- **Session Persistence**: CLI runs persist conversation transcripts to `.ogent/sessions/{session_id}.jsonl` on exit unless `--temp` is set.
- **Session Resume**: Non-temporary runs print a resume hint (`ogent -r {session_id}`) after completion. `ogent -r {session_id}` loads that transcript and continues without rebuilding the initial system/tool messages. You may also pass a new prompt after the session ID to append a human turn before continuing.
- **Run Completion**: A run terminates when the agent returns a final message without calling any more tools.

---

## Agent Tools Reference

The outer LLM agent loop is strictly limited to exactly two tools:
* `exec`: Executes a stateless, one-off Lua 5.5 script.
* `eval`: Executes a stateful Lua 5.5 script within the persistent session (retains globals/functions).

Within the Lua sandbox, scripts invoke workspace operations via global functions: filesystem editing (`read_file`, `read_lines`, `write_file`, `apply_anchor_edits`, `preview_anchor_edits`), repo exploration (`repo_map`, `glob`, `search_text`, `outline` for Rust/Go/Python files), shell, structured git operations (`git_status`, `git_diff`, `git_changes`, `git_show`, `git_log`), web search, skills loading, and subagent DSL (`agent`, `parallel`, `task_update`). See PROMPT_TOOLSET.md for the full API.


---

## Developer Guide & Operating Rules

When developing or pair programming with `ogent`:

### Operating Rules
- **Smallest Correct Change**: Always optimize for the smallest correct delta.
- **Search First**: Prefer `colgrep` for behavioral/intent search, and `rg` for exact text.
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

Agent-facing exact/regex search inside `ogent` Lua:
```lua
search_text{pattern = "apply_anchor_edits", paths = {"src"}}
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
