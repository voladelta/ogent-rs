# Architecture

`ogent` is a terminal-based autonomous coding agent. It turns natural language prompts into file reads, shell commands, code edits, and worker delegation. The core abstraction is an agent loop that processes assistant responses and executes tools; a main driver sets up state and dispatches to the appropriate loop.

## Bird's Eye View

The system has two layers: the agent loops and the main driver. The agent loops (`src/agent.rs`) implement turn logic, tool call dispatch, budget reminders, compaction, and handoff logic. The main driver (`src/main.rs`) parses CLI flags, loads profiles, sets up agent state, and starts the correct loop (`run_loop` or `steer_loop`). This split keeps agent logic contained and the entry point explicit.
## Codemap

Coarse-grained modules and their responsibilities:

- `main.rs` — CLI entry point, profile selection, session setup, loop selection.
- `agent.rs` — Standard and steer loops, turn handling, compaction (in-session handoff to new child session), and task tracker preservation.
- `client.rs` — HTTP streaming client with SSE parsing and retry behavior with exponential backoff.
- `providers.rs` — DeepSeek, Kimi, and Z/GLM request builders.
- `profiles.rs` — Named model profiles.
- `types.rs` — Domain types for messages, tools, responses, and tool calls.
- `sse.rs` — SSE parser and streamed response accumulation.
- `tools.rs` — Tool registry, JSON schemas, and implementations.
- `task_tracker.rs` — Runtime task tracker state, validation, reminders, and handoff serialization.
- `workflow.rs` — Workflow graph parsing and phase transition validation.
- `workers.rs` — Worker subprocess execution and async worker manager.
- `hashline.rs` — FNV-1a hash anchors and validated anchored edits.
- `prompts.rs` — Prompt loading, skill discovery, and skill injection.
- `session.rs` — Session persistence, handoff files, journal, and metadata.
- `tui.rs` — Ratatui/Crossterm interactive TUI for steer mode.
- `workspace.rs` — Workspace path validation and readable path rules.

To find the implementation of a specific behavior, search for the named type or function (e.g., `run_loop`, `steer_loop`, `execute_tool`, `Client`, `WorkerManager`).

## Data Flow

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
    +--> tools.rs
    |
    +--> workers.rs -> child ogent --worker
    |
    +--> session.rs -> .ogent/sessions, .ogent/handoffs
```

User prompt or steer input enters through `main.rs`, which builds initial messages and creates an `Agent`. `run_loop` or `steer_loop` calls `client.chat`, which streams an SSE response. Tool calls in the response are executed through `tools.rs`. Workers are spawned via `workers.rs` as child `ogent --worker` processes. Sessions and handoffs are persisted via `session.rs`.

## Invariants and Boundaries

- **Agent loop resilience**: `agent.rs` never performs I/O directly on the LLM stream; it delegates to `client.rs`. Individual tool failures are caught and returned as `ERROR: ...` strings to the model. They do not crash the agent loop.
- **Read-only batching**: `tools.rs` batches contiguous read-only tool calls in parallel. A mutating tool or barrier flushes the batch serially.
- **Workspace boundary**: `workspace.rs` validates all file paths before FS access. Tools cannot read or write outside the workspace or `~/.ogent`.
- **Handoff immutability**: Handoff files are write-once. Restoration reads the file, reconstructs state, and starts a fresh session with a new session ID.

## Cross-cutting Concerns

- **Error handling**: The agent loop is resilient — individual tool errors are caught and fed back to the model. Unhandled exceptions crash the process. The HTTP client retries transient errors with exponential backoff.
- **Cancellation**: In-flight LLM requests can be cancelled via `CancellationToken` in steer mode. Partial SSE responses are preserved.
- **Session persistence**: Every turn's state is saved to `.ogent/sessions/`. Handoffs go to `.ogent/handoffs/`. The journal appends completion summaries to `.ogent/journal.md`.
- **Task tracking**: Runtime-owned `Goal -> Phases -> Todos` hierarchy maintained through tool calls, not free-form prose. Phases may carry **validation contracts** (behavioral assertions that define "done" before implementation starts).
- **Adversarial validation**: The `validator` worker template enforces behavioral verification. Validators are dispatched with a different model profile when possible and verify against contracts without seeing implementation reasoning. Structured handoffs (per-contract pass/fail with evidence) enable programmatic root cause diagnosis in corrective loops.
