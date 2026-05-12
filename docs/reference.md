# Reference

CLI flags, model profiles, tools, and runtime behavior.

## CLI

```text
ogent [OPTIONS] [PROMPT...]
```

Common options:

| Option | Description |
|---|---|
| `--profile <name>` | Model profile. Default: `ds-pro` |
| `--steer` | Start interactive TUI steering mode |
| `--retry <n>` | Retry transient API errors. Default: `5` |
| `--max-turns <n>` | Limit agent turns. Default: `-1` for unlimited |
| `--autocompact <percent>` | Start compaction when remaining context crosses the threshold |
| `--resume` | Resume from the latest non-worker session (`.ogent/sessions/*.jsonl`) |
| `--resume-session <name>` | Resume from a specific session file by name (without `.jsonl`) |
| `--worker` | Internal worker mode. Reads system prompt from stdin |
| `--temp` | Ephemeral mode: run without persisting session state to disk |

Non-steer mode requires a prompt unless `--resume` is used.

## Profiles

| Profile | Backend | Model | Key env | Context | Max output | Thinking |
|---|---|---|---|---|---|---|
| `ds-flash` | DeepSeek | `deepseek-v4-flash` | `DEEPSEEK_API_KEY` | 1M | 393216 | `thinking:{type:enabled}` + `reasoning_effort=high` |
| `ds-flash-max` | DeepSeek | `deepseek-v4-flash` | `DEEPSEEK_API_KEY` | 1M | 393216 | `thinking:{type:enabled}` + `reasoning_effort=max` |
| `ds-pro` *(default)* | DeepSeek | `deepseek-v4-pro` | `DEEPSEEK_API_KEY` | 1M | 393216 | `thinking:{type:enabled}` + `reasoning_effort=high` |
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
| `read_file` | Read a workspace file or allowed config file such as `~/.ogent` (1 MB max). Optional `start`/`end` line range (0-indexed, inclusive/exclusive) |
| `write_file` | Write a new file; creates parent directories. Existing files require `overwrite_existing=true`; prefer `edit_hash_anchors` for normal edits |
| `read_hash_anchors` | Read workspace files with `line:hash\|content` prefixes for anchored editing. Optional `start`/`end` line range (0-indexed, inclusive/exclusive) |
| `edit_hash_anchors` | Anchored edits via an `ops` array. Batch multiple edits to the same file in one call so anchors are resolved against one snapshot |
| `bash` | Run a shell command in the workspace; returns combined stdout/stderr. Default timeout: 120s; max timeout: 600s |
| `repo_map` | Display a tree map of the workspace or allowed config roots such as `~/.ogent`. Use instead of `bash` with `ls`/`eza` |
| `web_search` | Search the web via Exa; returns titles, URLs, and highlights |
| `web_read` | Read page content from URLs via Exa; returns full text as markdown |
| `code_web_context` | Semantic code search across the web (GitHub, docs, Stack Overflow) |
| `load_skill` | Load a skill from `.ogent/skills/`, `.skills/`, or `~/.ogent/skills/` and inject its content |
| `dispatch_worker` | Hire a specialist coworker. system_prompt shapes worker behavior/scope; task states the concrete assignment. The worker runs as a separate process and returns a Markdown summary |
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
- `action="before"` with `anchor` to insert before a line
- `action="after"` with `anchor` to insert after a line

Example:

```text
edit_hash_anchors(
  path="src/main.rs",
  ops=[
    {"anchor":"10:abc1","action":"before","new_string":"// header"},
    {"anchor":"20:def2","action":"replace","new_string":"updated"},
    {"anchor":"30:9a8b","end_anchor":"34:cc02","action":"replace","new_string":"replacement block"}
  ]
)
```

Hash is FNV-1a 64-bit truncated to 4 hex chars.

## Retry Behavior

`--retry=5` is the default. Transient API errors retry with exponential backoff (`1s, 2s, 4s, 8s...` up to 60s max).

HTTP `429 Rate Limit` is terminal and is not retried.

## Session Persistence

After each run, the full conversation is written to `.ogent/sessions/*.jsonl`.

Worker sessions include `worker` in the filename.

When the coder calls `complete`, its structured Markdown summary is appended to `.ogent/journal.md`. Journal entries are retrospective experience notes, not instructions loaded into future runs. If tracked work is still open, the first `complete` call returns a warning; a second `complete` must include explicit limitation and intent.

### Resume from Session

```bash
# Resume the latest non-worker session
cargo run -- --resume "Now add a type hint to the function"

# Resume a specific session by name (without .jsonl)
cargo run -- --resume --resume-session 1778216383-2028 "Add a main block"
```

Sessions are saved even when the run hits `--max-turns`. If the turn limit is reached, the exit message prints:

```
Reached max turns (N). Session saved. Resume with ogent --resume.
```

## Turn Limits

```bash
cargo run -- --max-turns 20 "Add auth middleware"
```

`--max-turns=-1` is unlimited.

Worker limits can be set by the parent agent through the `max_turns` field in `dispatch_worker` or async worker specs.

### Turn Budget Reminders

The agent receives contextual reminders at key points in the turn budget to guide behavior:

| Reminder | When | Guidance |
|---|---|---|
| Turn 1 | Always | "Use turns deliberately. Delegate coworkers now if work is parallelizable." |
| 50% used | `max_turns >= 10`, remaining = `max_turns/2` | "If useful work is parallelizable and delegatable, delegate coworkers now." |
| 75% used | `max_turns >= 10`, remaining = `max_turns/4` (>= 5) | "Focus on verification and completion. Avoid new delegation." |
| 3 left | `remaining == 3` | "Finish current chunk and prepare to summarize for human review." |
| 2 left | `remaining == 2` | "No new work. `complete` or prepare a summary." |
| FINAL | `remaining == 1` | "`complete` if done. Otherwise summarize progress for human review." |

These reminders help the agent avoid overcommitting on the final turns and prioritize completion when the turn budget is exhausted.

## Token Reporting

After each run, prompt/completion/total tokens are reported:

```
tokens: prompt=4057 completion=625 total=4682
```
