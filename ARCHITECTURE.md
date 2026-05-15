# Architecture

`ogent` is a terminal-based autonomous coding agent. It turns natural language prompts into file reads, shell commands, code edits, and worker delegation. The core abstraction is an agent loop that processes assistant responses and executes tools; a main driver sets up state and dispatches to the appropriate loop.

## Bird's Eye View

The system has two layers: the agent loops and the main driver. The agent loops (`src/agent.rs`) implement turn logic, tool call dispatch, context-budget reminders, compaction, and child-session logic. The main driver (`src/main.rs`) parses CLI flags, loads profiles, sets up agent state, and starts the correct loop (`run_loop` or `steer_loop`). This split keeps agent logic contained and the entry point explicit.
## Codemap

Coarse-grained modules and their responsibilities:

- `main.rs` — CLI entry point, profile selection, optional workflow loading, creator mode dispatch, session setup, loop selection.
- `artifact_creator.rs` — One-shot skill/workflow generation via the selected profile, artifact validation, and local `.ogent` writes.
- `agent.rs` — Standard and steer loops, turn handling, workflow reminders, compaction (in-session compaction to a new child session), and task tracker preservation.
- `client.rs` — HTTP streaming client with SSE parsing and retry behavior with exponential backoff.
- `providers.rs` — DeepSeek, Kimi, and Z/GLM request builders.
- `profiles.rs` — Named model profiles.
- `types.rs` — Domain types for messages, tools, responses, and tool calls.
- `sse.rs` — SSE parser and streamed response accumulation.
- `tools.rs` — Tool registry, conditional JSON schemas, and implementations.
- `task_tracker.rs` — Runtime task tracker state, validation, reminders, and compact-brief serialization.
- `workflow.rs` — Optional workflow schema, validation, persisted state, check evidence, and transition enforcement.
- `workers.rs` — Worker subprocess execution and async worker manager.
- `hashline.rs` — FNV-1a hash anchors and validated anchored edits.
- `prompts.rs` — Prompt loading, skill discovery, and skill injection.
- `session.rs` — Session persistence, child session snapshots, journal, and metadata.
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
    +--> session.rs -> .ogent/sessions
```

User prompt or steer input enters through `main.rs`, which builds initial messages and creates an `Agent`. If `--create-skill` or `--create-workflow` is supplied, `main.rs` enters creator mode, calls `artifact_creator.rs`, writes one validated local artifact, and exits without creating a session. If `--workflow <name-or-path>` is supplied, `main.rs` loads and validates one active workflow before selecting the tool schema. Workflow names resolve to local `.ogent/workflows/`, global `~/.ogent/workflows/`, or built-in YAML files in `workflows/`; explicit file paths are also supported. `run_loop` or `steer_loop` calls `client.chat`, which streams an SSE response. Tool calls in the response are executed through `tools.rs`. Workers are spawned via `workers.rs` as child `ogent --worker` processes. Sessions and optional workflow state are persisted via `session.rs`.

## Invariants and Boundaries

- **Agent loop resilience**: `agent.rs` never performs I/O directly on the LLM stream; it delegates to `client.rs`. Individual tool failures are caught and returned as `ERROR: ...` strings to the model. They do not crash the agent loop.
- **Read-only batching**: `tools.rs` batches contiguous read-only tool calls in parallel. A mutating tool or barrier flushes the batch serially.
- **Workspace boundary**: `workspace.rs` validates all file paths before FS access. Tools cannot read or write outside the workspace or `~/.ogent`.
- **Workflow is optional**: Workflow tools are included in the model tool schema only when a workflow is active. Sessions without `--workflow` do not pay schema/context cost and behave normally.
- **Workflow authority**: When active, workflow state controls process transitions and completion gating. Task tracker phases are progress display; they do not drive workflow transitions.
- **Workflow evidence**: Required workflow checks must pass or be waived before leaving a step. Command checks store command, exit code, output evidence, and timestamp.
- **Creator validation**: Creator mode writes only after the model output parses and passes the skill or workflow validation contract. Existing artifacts are not overwritten.

## Cross-cutting Concerns

- **Error handling**: The agent loop is resilient — individual tool errors are caught and fed back to the model. Unhandled exceptions crash the process. The HTTP client retries transient errors with exponential backoff.
- **Cancellation**: In-flight LLM requests can be cancelled via `CancellationToken` in steer mode. Partial SSE responses are preserved.
- **Session persistence**: Every turn's state is saved to `.ogent/sessions/`. The journal appends completion summaries to `.ogent/journal.md`.
- **Workflow persistence**: Active workflow state is saved to `.ogent/sessions/<id>/workflow-state.json` and reloaded on resume/fork. Compaction preserves the same workflow state in the child session.
- **Task tracking**: Runtime-owned `Goal -> Phases -> Todos` hierarchy maintained through tool calls, not free-form prose. Phases may carry **validation contracts** (behavioral assertions that define "done" before implementation starts).
- **Workflow and skills**: Skills are reusable capability instructions loaded with `load_skill`. Skills do not define or activate workflows; workflows are explicit session policies loaded with `--workflow`.
- **Adversarial validation**: The `validator` worker template enforces behavioral verification. Validators are dispatched with a different model profile when possible and verify against contracts without seeing implementation reasoning. Structured handoffs (per-contract pass/fail with evidence) enable programmatic root cause diagnosis in corrective loops.
