# Reference

CLI flags, model profiles, tools, and runtime behavior.

## CLI

```text
ogent [OPTIONS] [PROMPT...]
```

Common options:

| Option | Description |
|---|---|
| `--profile <name>` | Model profile. Default: `ds-flash` |
| `--steer` | Start interactive TUI steering mode |
| `--autocompact <percent>` | Auto-compact context when usage crosses threshold. Default: `80`. `-1` to disable |
| `--resume [<session>]` | Resume the latest or named non-worker session and save back into that same session |
| `--fork [<session>]` | Load the latest or named non-worker session, then save the run into a new child session |
| `--worker` | Internal worker mode. Reads system prompt from stdin |
| `--temp` | Ephemeral mode: run without persisting session state to disk |

Non-steer mode requires a prompt unless `--resume` or `--fork` is used.

`resume` and `fork` can also be used as command-style aliases when they are the first argument after `ogent`:

```bash
ogent resume 1778216383-2028 "Continue this session"
ogent fork 1778216383-2028 "Try a different approach"
```

## Profiles

| Profile | Backend | Model | Key env | Context | Max output | Thinking |
|---|---|---|---|---|---|---|
| `ds-flash` *(default)* | DeepSeek | `deepseek-v4-flash` | `DEEPSEEK_API_KEY` | 1M | 393216 | `thinking:{type:enabled}` + `reasoning_effort=high` |
| `ds-flash-max` | DeepSeek | `deepseek-v4-flash` | `DEEPSEEK_API_KEY` | 1M | 393216 | `thinking:{type:enabled}` + `reasoning_effort=max` |
| `ds-pro` | DeepSeek | `deepseek-v4-pro` | `DEEPSEEK_API_KEY` | 1M | 393216 | `thinking:{type:enabled}` + `reasoning_effort=high` |
| `ds-pro-max` | DeepSeek | `deepseek-v4-pro` | `DEEPSEEK_API_KEY` | 1M | 393216 | `thinking:{type:enabled}` + `reasoning_effort=max` |
| `kimi` | Baseten | `moonshotai/Kimi-K2.6` | `BASETEN_API_KEY` | 256K | 262144 | `enable_thinking=true` |
| `glm` | Z.ai | `glm-5.1` | `Z_API_KEY` | 200K | 131072 | interleaved + preserved |

Select a profile with `--profile`:

```bash
cargo run -- --profile kimi "Explain this repository"
```

## Tools

| Tool | Description |
|---|---|
| `read_file` | Read a workspace file or allowed config file such as `~/.ogent` (1 MB max). Optional `start`/`end` line range (1-indexed, inclusive) |
| `write_file` | Write a new file; creates parent directories. Existing files require `overwrite_existing=true`; prefer `edit_hash_anchors` for normal edits |
| `read_hash_anchors` | Read workspace files with `line:hash\|content` prefixes for anchored editing. Optional `start`/`end` line range (1-indexed, inclusive) |
| `edit_hash_anchors` | Anchored edits via an `ops` array. Batch multiple edits to the same file in one call so anchors are resolved against one snapshot |
| `bash` | Run a shell command in the workspace; returns combined stdout/stderr. Default timeout: 120s; max timeout: 600s |
| `repo_map` | Display a tree map of the workspace or allowed config roots such as `~/.ogent`. Use instead of `bash` with `ls`/`eza` |
| `web_search` | Search the web via Exa; returns titles, URLs, and highlights |
| `web_read` | Read page content from URLs via Exa; returns full text as markdown |
| `code_web_context` | Search real code for syntax, APIs, and patterns to avoid hallucinating implementation details. Not for general web search or URL reading |
| `load_skill` | Load a skill from `.ogent/skills/`, `.skills/`, or `~/.ogent/skills/` and inject its content |
| `dispatch_worker` | Hire a specialist coworker. `template` selects the worker role (generic, coder, tester, reviewer, validator); `task` states the concrete assignment. ogent generates the system prompt via an architect LLM call unless a built-in template is used. The worker runs as a separate process and returns a Markdown summary |
| `start_workers` | Start a batch of specialist coworkers asynchronously and return worker IDs immediately |
| `check_workers` | Wait for active async coworkers, collect their summaries/errors, and clear the batch |
| `set_goal` | Initialize runtime task tracking with one Goal (single-use) |
| `revise_goal` | Revise the Goal and record prior goal + reason |
| `update_phase` | Upsert one Phase under the current Goal |
| `update_todo` | Upsert one Todo under an existing Phase |
| `complete` | Finish the run with a structured Markdown session summary |

Web tools require `EXA_API_KEY`.

Workers use the same toolset except `dispatch_worker`, `start_workers`, `check_workers`, `set_goal`, `revise_goal`, `update_phase`, `update_todo`, and `complete`. Workers have `worker_complete` to return their final Markdown summary.

Tool calls are evaluated in order. Contiguous read-only calls (`read_file`, `read_hash_anchors`, `repo_map`, web tools, `load_skill`) may run in parallel. Mutating or blocking calls (`write_file`, `edit_hash_anchors`, `bash`, workers) act as barriers and run serially.

## Hashline Editing

Read a file to get stable anchors, then edit by line reference:

```text
read_hash_anchors({"path":"src/main.rs"})
```

Output format:

```text
1:5502|fn main() {
2:cbf2|  println!("hello");
3:9a8b|}
```

Then edit using an `ops` array of `line:hash` anchors. Prefer one `edit_hash_anchors` call per file with all intended edits. This is safer and cheaper: anchors are validated against the same file snapshot, then edits are applied bottom-to-top in one write so earlier edits do not shift later anchors.

After any write or edit to a file, all previous anchors for that file are stale. Re-read `read_hash_anchors` before making another edit call to the same file.

Each op supports:

- `action="replace"` with `anchor` for one line
- `action="replace"` with `anchor` and `end_anchor` for a range
- `action="insert_before"` with `anchor` to insert before a line
- `action="insert_after"` with `anchor` to insert after a line

Example:

```text
edit_hash_anchors(
  path="src/main.rs",
  ops=[
    {"anchor":"10:abc1","action":"insert_before","new_string":"// header"},
    {"anchor":"20:def2","action":"replace","new_string":"updated"},
    {"anchor":"30:9a8b","end_anchor":"34:cc02","action":"replace","new_string":"replacement block"}
  ]
)
```

Hash is FNV-1a 64-bit truncated to 4 hex chars.

## Retry Behavior

Transient API errors retry with exponential backoff (`1s, 2s, 4s, 8s...` up to 60s max). Up to 5 retries.

HTTP `429 Rate Limit` is terminal and is not retried.

## Session Persistence

After each run, the full conversation is written to `.ogent/sessions/*.jsonl`.

Worker sessions include `worker` in the filename.

When the coder calls `complete`, its structured Markdown summary is appended to `.ogent/journal.md`. Journal entries are retrospective experience notes, not instructions loaded into future runs. If tracked work is still open, the first `complete` call returns a warning; a second `complete` must include explicit limitation and intent.

### Resume from Session

Resume continues the selected session in place. If you resume `1778216383-2028`, the next save also goes to `1778216383-2028`.

```bash
# Resume the latest non-worker session
cargo run -- resume "Now add a type hint to the function"

# Resume a specific session by name (without .jsonl)
cargo run -- resume 1778216383-2028 "Add a main block"

# Flag form is equivalent
cargo run -- --resume 1778216383-2028 "Add a main block"
```

### Fork from Session

Fork loads the selected session as context, then writes the run to a new session id. The new session records the source session as `parent_session`.

```bash
# Fork the latest non-worker session
cargo run -- fork "Try a different implementation"

# Fork a specific session by name (without .jsonl)
cargo run -- fork 1778216383-2028 "Try a different implementation"

# Flag form is equivalent
cargo run -- --fork 1778216383-2028 "Try a different implementation"
```

## Context Budget Reminders

When `--autocompact` is enabled and token usage crosses the threshold, the agent receives escalating reminders:

| Urgency | Trigger | Guidance |
|---|---|---|
| 1 | Ratio >= threshold | "Finish the current chunk. Do not start unrelated work." |
| 2 | Ratio >= threshold (again) | "Approaching the limit. Finish only critical in-progress work. Do not delegate new work." |
| 3+ | Ratio >= threshold (again) | "EXHAUSTED. Do not write more files, delegate, or start new work. Call `complete` immediately." |

These are injected as user-visible reminders, not hard stops.

## Token Reporting

After each run, total tokens are reported:

```
tokens: 4682
```
