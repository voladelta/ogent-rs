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
Memento emitted? -> auto-continue next phase
    |
    v
Need specialist? -> dispatch_worker / start_workers
    |
    v
Worker subprocess -> report artifact
    |
    v
10x Coder reads report -> integrate -> continue or finalize
```

The 10x coder is the default mode. It reads files, writes code, runs tests, debugs issues, and hires workers only when useful.

Workers run as child `ogent --worker` processes with a custom system prompt and task supplied by the parent agent.

## Profiles

| Profile | Backend | Model | Key env | Context | Max output | Thinking |
|---|---|---|---|---:|---:|---|
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

## Memento Protocol

At meaningful boundaries, the agent may emit a `<memento>` block:

```xml
<memento>
- Invariants: ...
- State: ...
- Decisions: ...
- Next: ...
</memento>
```

Mementos are saved under `.ogent/mementos/` and loaded on future runs. If a memento has a `Next` step that is not done, the parent loop can auto-continue.

## Skills

Skills are loaded from:

- `.ogent/skills/<name>/SKILL.md`
- `.skills/<name>/SKILL.md`
- `~/.ogent/skills/<name>/SKILL.md`

At startup, available skills are discovered and listed in the user message. The agent can call `load_skill` to inject a skill body into the next turn.

The `colgrep` skill is special-cased: if found, its full body is auto-injected into the initial user message so the agent has semantic code search instructions immediately.

Recommended search tools for agent workflows:

```bash
brew install ripgrep ast-grep
brew install lightonai/tap/colgrep
```

## Tools

| Tool | Description |
|---|---|
| `read_file` | Read a workspace file or allowed config file, with optional line range |
| `write_file` | Write a new file; existing files require explicit overwrite |
| `read_hash_anchors` | Read lines as `line:hash|content` anchors |
| `edit_hash_anchors` | Apply anchored edits after validating hashes |
| `bash` | Run a shell command in the workspace |
| `repo_map` | Show a repository tree map |
| `web_search` | Search the web through Exa. Requires `EXA_API_KEY` |
| `web_read` | Read web pages through Exa. Requires `EXA_API_KEY` |
| `code_web_context` | Retrieve code-oriented web context through Exa. Requires `EXA_API_KEY` |
| `load_skill` | Load a skill body from a skill root |
| `dispatch_worker` | Run one specialist worker subprocess and return its report |
| `start_workers` | Start a batch of async workers |
| `check_workers` | Wait for active async workers and collect reports |
| `handoff` | Write a session handoff under `.ogent/handoffs/` |
| `question` | First-turn clarification signal; the current loop exits cleanly when it is called |

Workers receive a reduced toolset: no worker dispatch, async worker management, handoff, or question tool. They receive `worker_question` for reporting blockers to the parent.

Read-only tool calls can be processed in parallel. Mutating, blocking, or worker-related calls are serialized.

## Hashline Editing

Read anchors:

```text
read_hash_anchors({"path":"src/main.rs"})
```

Output format:

```text
1:5502|fn main() {
2:cbf2|  println!("hello");
3:9a8b|}
```

Edit with anchors:

```text
edit_hash_anchors({
  "path": "src/main.rs",
  "ops": [
    {
      "anchor": "2:cbf2",
      "action": "replace",
      "new_string": "  println!(\"hello, agent\");"
    }
  ]
})
```

The hash is FNV-1a 64-bit truncated to 4 hex characters. `edit_hash_anchors` recomputes hashes against the current file snapshot. If any anchor is stale, the whole batch is rejected.

After a successful edit, old anchors for that file are stale. Re-read before editing the same file again.

## Retry Behavior

`--retry=5` is the default. Transient API errors retry with linear backoff.

HTTP `429 Rate Limit` is terminal and is not retried.

## Session Persistence

After each run, the full conversation is written to `.ogent/sessions/*.jsonl`.

Worker sessions include `worker` in the filename.

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
| `src/agent.rs` | Standard loop, steer loop, turn handling, mementos, compaction |
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
| `src/session.rs` | Sessions, mementos, handoffs, timestamps |
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
    +--> session.rs -> .ogent/sessions, .ogent/mementos, .ogent/handoffs
```

### Agent Loops

`run_loop` is the standard non-steer loop used by the parent coder and workers. It processes assistant responses, executes tools, handles memento auto-continuation, checks workers, and repeats until the model returns a final response or the turn limit is reached.

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
