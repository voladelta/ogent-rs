# ogent

`ogent` is a minimal task agent with thinking-mode LLMs, anchored file editing, autonomous 10x-coder execution, and TUI-based steering.

## Overview

`ogent` is a terminal-based autonomous agent that turns user intent into file reads, edits, shell commands, tests, debugging, and worker delegation.

The default **10x coder** mode owns the work directly. It delegates to worker subprocesses only when a specialist or parallel work stream adds value.

The design priorities are:

- Safe file edits: edits are anchored by content hashes, so stale edits are rejected.
- Observable execution: reasoning, assistant output, tool calls, token usage, and session history are visible.
- Small architecture: the code is split into focused modules with explicit boundaries.
- Interactive steering: long-running work can be corrected through a TUI without waiting for the current model call to finish.

## Quick Start

Requires a Rust toolchain with Cargo.

```bash
export DEEPSEEK_API_KEY="sk-..."

cargo build --release
./target/release/ogent "Add a divide function to src/math.rs"
```

Or run without building first:

```bash
cargo run -- "Add a divide function to src/math.rs"
```

## How It Works

```text
User prompt
    |
    v
10x Coder (read -> plan -> act -> checkpoint)
    |
    v
Need specialist? -> dispatch_worker / start_workers
    |
    v
Worker subprocess -> worker_complete({summary})
    |
    v
10x Coder reads report -> integrate -> continue or finalize
```

The 10x coder is the default mode. It reads files, writes code, runs tests, debugs issues, and hires workers only when useful.

Workers run as child `ogent --worker` processes with a custom system prompt and task supplied by the parent agent.

## 10x Coder

The 10x coder works in **phases**, writing short in-session checkpoints and hiring specialist coworkers when needed.

### Checkpoints

At meaningful in-session boundaries, the agent may write a short `<checkpoint>` note for its own context management:

```xml
<checkpoint>
- Evidence: ...
- State: ...
- Decisions: ...
- Risks: ...
- Next: ...
</checkpoint>
```

Checkpoints help preserve working state across phase changes, delegation, compaction, and handoff. They are model-facing context notes only: runtime code does not parse them, save them as durable memory, or load them on future runs.

### Skills

Skills are loaded from:

- `.ogent/skills/<name>/SKILL.md`
- `.skills/<name>/SKILL.md`
- `~/.ogent/skills/<name>/SKILL.md`

At startup, available skills are discovered and listed in the user message. The agent can call `load_skill` to inject a skill body into the next turn.

The `colgrep` and `codectx` skills are preloaded: if their `SKILL.md` files exist in a skill root, ogent auto-injects their full body into the initial user message after the skills list. This gives the 10x coder semantic code search and repo context instructions without spending a turn on `load_skill`.

Install the search CLIs you want the agent to use for efficient codebase discovery:

```bash
# macOS
brew install ripgrep ast-grep

# Install colgrep separately if you use semantic repo search.
brew install lightonai/tap/colgrep

# Then add its skill file:
mkdir -p ~/.ogent/skills/colgrep
$EDITOR ~/.ogent/skills/colgrep/SKILL.md
```

Recommended search behavior:
- `colgrep` for intent-based code search, system exploration, and symbol discovery.
- `rg` for exact text and regex matching.
- `ast-grep` for syntax-aware structural search.

### Hiring coworkers

The 10x coder uses `dispatch_worker` when:
- The task has parallel independent work streams
- A specialist perspective is needed (security review, docs, tests)
- The task is large enough that splitting context helps

**Golden rule:** Give the worker JUST ENOUGH context — but it must be the RIGHT context. A worker without file paths or commands will fail silently.

**Worker prompt templates** in `prompts/templates/` (`generic`, `tester`, `reviewer`) are starting points for the worker `system_prompt`. The 10x coder customizes one of them for the worker's role, scope, constraints, and summary format, then puts the concrete assignment in the separate `task` argument. All `{{PLACEHOLDERS}}` must be filled before dispatch.

**Dispatch checklist:**
- [ ] You actually need a worker (prefer direct action for <3 turns of work)
- [ ] `system_prompt` defines role, allowed tools/actions, read/write scope, constraints, commands, and summary format
- [ ] `task` states the exact assignment, expected output, success criteria, and immediate next step
- [ ] All file paths are exact relative paths
- [ ] Commands are exact and copied into the worker scope
- [ ] Invariants/constraints from the current checkpoint or task context are included

The worker runs in isolation with your prompt. When done, it calls `worker_complete` with a structured Markdown summary. That summary is returned to the parent coder. You decide what to do next.

### Question tool (turn 1 only)

The `question` tool is available **only on the first turn** of the 10x coder for initial requirement clarification. After turn 1, the agent makes decisions autonomously. Workers cannot ask the human directly; they use `worker_question` to ask the parent coder when blocked.

## Creating skills

Skills are **domain knowledge packages** stored as `.ogent/skills/<name>/SKILL.md`:

```
.ogent/skills/
├── rust-refactor/
│   └── SKILL.md          # Rust refactoring procedures
├── string-utils/
│   └── SKILL.md          # Rust string utility patterns
└── ...                   # User-created skills
```

Each `SKILL.md` has YAML frontmatter (`name`, `description`) and Markdown instructions. The description helps the agent decide when to apply the skill. The full body is loaded only when triggered (progressive disclosure).

```bash
mkdir -p .ogent/skills/my-skill
cat > .ogent/skills/my-skill/SKILL.md << 'EOF'
---
name: my-skill
description: What this skill does and when to use it.
---

## Brief
Compressed procedure.

## Context
What to assume.

## Constraints
Hard limits.

## Procedure
1. Step one
2. Step two

## Verification
How to confirm success.
EOF
```

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

## CLI

```text
ogent [OPTIONS] [PROMPT...]
```

Common options:

| Option | Description |
|---|---|
| `--profile <name>` | Model profile. Default: `ds-pro` |
| `--steer` | Start interactive TUI steering mode |
| `--auto` | In steer mode, start with auto-continuation enabled |
| `--retry <n>` | Retry transient API errors. Default: `5` |
| `--max-turns <n>` | Limit agent turns. Default: `-1` for unlimited |
| `--autocompact <percent>` | Start handoff/compaction when remaining context crosses the threshold |
| `--handoff` | Exit after writing a handoff during compaction |
| `--continue` | Resume from the newest `.ogent/handoffs/*.md` file |
| `--worker` | Internal worker mode. Reads system prompt from stdin |

Non-steer mode requires a prompt unless `--continue` is used.

## Tools

| Tool | Description |
|---|---|
| `read_file` | Read a workspace file or allowed config file such as `~/.ogent` (1 MB max). Optional `start`/`end` line range (0-indexed, inclusive/exclusive) |
| `write_file` | Write a new file; creates parent directories. Existing files require `overwrite_existing=true`; prefer `edit_hash_anchors` for normal edits |
| `read_hash_anchors` | Read workspace files with `line:hash\|content` prefixes for anchored editing. Optional `start`/`end` line range (0-indexed, inclusive/exclusive) |
| `edit_hash_anchors` | Anchored edits via an `ops` array. Batch multiple edits to the same file in one call so anchors are resolved against one snapshot |
| `bash` | Run a shell command in the workspace; returns combined stdout/stderr. Default timeout: 120s; max timeout: 600s |
| `repo_map` | Display a tree map of the workspace or allowed config roots such as `~/.ogent`. Use instead of `bash` with `ls`/`eza` |
| `question` | Ask the user for clarification. **Only available on turn 1 of the 10x coder.** Workers use `worker_question` to ask the parent coder |
| `web_search` | Search the web via Exa; returns titles, URLs, and highlights |
| `web_read` | Read page content from URLs via Exa; returns full text as markdown |
| `code_web_context` | Semantic code search across the web (GitHub, docs, Stack Overflow) |
| `load_skill` | Load a skill from `.ogent/skills/`, `.skills/`, or `~/.ogent/skills/` and inject its content |
| `dispatch_worker` | Hire a specialist coworker. system_prompt shapes worker behavior/scope; task states the concrete assignment. The worker runs as a separate process and returns a Markdown summary |
| `start_workers` | Start a batch of specialist coworkers asynchronously and return worker IDs immediately |
| `check_workers` | Wait for active async coworkers, collect their summaries/errors, and clear the batch |
| `handoff` | Write a session handoff brief under `.ogent/handoffs/` |
| `complete` | Finish the run with a structured Markdown session summary |

Web tools require `EXA_API_KEY`.

Workers use the same toolset except `dispatch_worker`, `start_workers`, `check_workers`, `handoff`, `complete`, and `question`. Instead, workers have `worker_question` to ask the parent 10x coder when blocked and `worker_complete` to return their final Markdown summary.

Tool calls are evaluated in order. Contiguous read-only calls (`read_file`, `read_hash_anchors`, `repo_map`, web tools, `load_skill`) may run in parallel. Mutating or blocking calls (`write_file`, `edit_hash_anchors`, `bash`, workers, `handoff`, questions) act as barriers and run serially.

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

`--retry=5` is the default. Transient API errors retry with linear backoff (`1s, 2s, 3s...`).

HTTP `429 Rate Limit` is terminal and is not retried.

## Session Persistence

After each run, the full conversation is written to `.ogent/sessions/*.jsonl`.

Worker sessions include `worker` in the filename.

When the coder calls `complete`, its structured Markdown summary is appended to `.ogent/journal.md`. Journal entries are retrospective experience notes, not instructions loaded into future runs.

Handoffs are written to `.ogent/handoffs/*.md`. Continue from the newest handoff:

```bash
cargo run -- --continue
```

## Turn Limits

```bash
cargo run -- --max-turns 20 "Add auth middleware"
```

`--max-turns=-1` is unlimited.

Worker limits can be set by the parent agent through the `max_turns` field in `dispatch_worker` or async worker specs.

## Token Reporting

After each run, prompt/completion/total tokens are reported:

```
tokens: prompt=4057 completion=625 total=4682
```

## Steer Mode

`--steer` starts an interactive terminal UI.

```bash
cargo run -- --steer --profile ds-pro "Write a small web server"
```

The TUI shows:

- a status bar with profile, model, turn, token count, and auto mode
- a scrollable log of reasoning summaries, assistant content, tool calls, and worker updates
- an input box for steering messages and commands

Supported commands:

| Input | Effect |
|---|---|
| `/auto` | Enable auto-continuation |
| `/stop` | Disable auto-continuation after the current turn |
| `/complete` | Ask the agent to summarize the session, call `complete`, save the journal entry, and exit |
| `/cancel` | Cancel the in-flight model request |
| `/q`, `/quit`, `quit`, `exit`, `Esc`, `Ctrl-C` | Exit steer mode |
| any other text | Abort the in-flight model request, append the text as a new user message, and re-prompt |

Navigation:

- `Up` / `Down`: scroll one line
- `PageUp` / `PageDown`: scroll one page
- `Home` / `End`: jump to top or follow bottom
- mouse wheel: scroll log

If you run steer mode without an initial prompt, the TUI waits for your first message:

```bash
cargo run -- --steer
```

When a steering message arrives during an LLM stream, the agent cancels the in-flight request, preserves any partial assistant content/tool calls already accumulated, appends your message, and starts the next turn.

## Architecture

### Design Principles

1. **Focused modules**: each module owns one part of the agent loop, tool system, provider layer, or UI.
2. **Simple behavior**: the control flow is explicit and avoids hidden orchestration.
3. **Explicit contracts**: tool schemas, provider requests, and worker boundaries are spelled out in code.
4. **Content-addressed edits**: hashline validation protects against stale file edits.
5. **Graceful cancellation**: in-flight streaming requests can be cancelled, and partial responses are preserved.
6. **TUI steering**: interactivity is local and terminal-native.

### File Structure

| File | Purpose |
|---|---|
| `src/main.rs` | CLI entry point, profile selection, session setup, loop selection |
| `src/agent.rs` | Standard loop, steer loop, turn handling, compaction |
| `src/client.rs` | HTTP streaming client and retry behavior |
| `src/providers.rs` | DeepSeek, Kimi, and Z/GLM request builders |
| `src/profiles.rs` | Named model profiles |
| `src/types.rs` | Domain types for messages, tools, responses, and tool calls |
| `src/sse.rs` | SSE parser and streamed response accumulation |
| `src/tools.rs` | Tool registry and JSON schemas |
| `src/toolimpl.rs` | Tool implementations |
| `src/workers.rs` | Worker subprocess execution and async worker manager |
| `src/hashline.rs` | Hash anchors and validated anchored edits |
| `src/prompts.rs` | Embedded prompts, skill discovery, skill loading |
| `src/session.rs` | Sessions, handoffs, timestamps |
| `src/tui.rs` | Ratatui/Crossterm steering UI |
| `src/workspace.rs` | Workspace path validation and readable path rules |

### Data Flow

```text
User prompt / TUI message
    |
    v
main.rs
    |
    v
agent.rs
    |
    +--> client.rs -> providers.rs -> SSE stream -> sse.rs
    |
    +--> tools.rs -> toolimpl.rs
    |
    +--> workers.rs -> child ogent --worker
    |
    +--> session.rs -> .ogent/sessions, .ogent/handoffs
```

### Agent Loops

`run_loop` is the standard non-steer loop used by the parent coder and workers. It processes assistant responses, executes tools, checks workers, handles handoffs/compaction, and repeats until the model returns a final response or the turn limit is reached.

`steer_loop` is the interactive loop used by `--steer`. It starts `tui::start`, receives `SteerEvent`s from the UI, cancels in-flight requests when needed, preserves partial responses, and applies user steering messages as new turns.

## Examples

```bash
# Simple task
cargo run -- "Add a divide function to src/math.rs"

# Research task
cargo run -- "How does Tokio scheduling work?"

# Multi-step with worker delegation
cargo run -- "Add auth module, then review it for security issues"

# Parallel worker-friendly task
cargo run -- "Add tests for src/auth.rs and write README documentation"

# Continue from handoff
cargo run -- --continue

# Auto-compact context at 10%, write handoff, then exit
cargo run -- --autocompact 10 --handoff "Large refactoring task"

# Auto-compact context at 5% and continue automatically
cargo run -- --autocompact 5 "Large refactoring task"

# Try different backends
cargo run -- --profile kimi "Explain the fnv1a hashline logic"
cargo run -- --profile glm "Summarize this repository"

# Max DeepSeek reasoning effort
cargo run -- --profile ds-pro-max "Design a caching layer"

# Limit turns
cargo run -- --max-turns 15 "Add a simple health check endpoint"

# TUI steer mode
cargo run -- --steer --profile ds-pro "Write a small web server"

# TUI steer mode, waiting for first input
cargo run -- --steer
```
