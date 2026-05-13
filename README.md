# ogent

`ogent` is a minimal task agent with thinking-mode LLMs, anchored file editing, autonomous agent execution, and TUI-based steering.

## Overview

`ogent` is a terminal-based autonomous agent that turns user intent into file reads, edits, shell commands, tests, debugging, and worker delegation.

The default **agent** mode owns the work directly. It delegates to worker subprocesses only when a specialist or parallel work stream adds value.

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
Agent (read -> plan -> act -> checkpoint)
    |
    v
Need specialist? -> dispatch_worker / start_workers
    |
    v
Worker subprocess -> worker_complete({summary})
    |
    v
Agent reads report -> integrate -> continue or finalize
```

The agent is the default mode. It reads files, writes code, runs tests, debugs issues, and hires workers only when useful.

Workers run as child `ogent --worker` processes. The parent provides a `template` (worker role), `task` (concrete assignment), and `context` (project info, files, constraints); ogent generates the worker's system prompt via an architect LLM call unless a built-in template is used.

## Documentation

- [Agent Guide](docs/agent-guide.md) — agent internals, checkpoints, task tracking, skills, and hiring coworkers
- [Reference](docs/reference.md) — CLI flags, model profiles, tools, hashline editing, sessions, context budget
- [Steer Mode](docs/steer-mode.md) — Interactive TUI, commands, and navigation
- [Architecture](ARCHITECTURE.md) — Module map, data flow, and design invariants

## Development

```bash
# Type-check without emitting
cargo check

# Lint
cargo clippy

# Format check
cargo fmt -- --check

# Auto-format
cargo fmt

# Full check
cargo test
```

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

# Resume the latest session and save back into that same session
cargo run -- resume "Add more tests"

# Resume a specific session and save back into that same session
cargo run -- resume 1778216383-2028 "Add more tests"

# Fork a specific session into a new child session
cargo run -- fork 1778216383-2028 "Try a different approach"

# Auto-compact context at 50% and continue automatically
cargo run -- --autocompact 50 "Large refactoring task"

# Disable autocompact
cargo run -- --autocompact -1 "Quick task"

# Ephemeral session (no session state written to disk)
cargo run -- --temp "Quick one-off query"

# Try different backends
cargo run -- --profile kimi "Explain the fnv1a hashline logic"
cargo run -- --profile glm "Summarize this repository"

# Max DeepSeek reasoning effort
cargo run -- --profile ds-pro-max "Design a caching layer"

# TUI steer mode
cargo run -- --steer --profile ds-pro "Write a small web server"

# TUI steer mode, waiting for first input
cargo run -- --steer
```
