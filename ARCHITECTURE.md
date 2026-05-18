# Architecture

## Runtime Shape

`ogent` is built around a Director/worker split.

```text
main.rs
  -> agent.rs
    -> steer.rs
    -> workspace.rs
    -> client.rs / providers.rs / sse.rs
    -> tools.rs
      -> workers.rs
      -> session.rs
  -> websocket.rs (when `--serve`)
```

## Module Ownership

- `src/main.rs`
  - CLI parsing, mode wiring, resume/fork, websocket server mode, skill creation mode.
- `src/agent.rs`
  - Turn loop, stream handling, tool-call execution, compaction.
  - Transport-neutral steer loop over `SteerChannel`.
  - Owns an immutable `Workspace` used by tools, workers, and session persistence.
- `src/steer.rs`
  - Transport-neutral steer events/state plus the `SteerChannel` interface used by websocket control.
- `src/websocket.rs`
  - WebSocket listener and per-connection Director lifecycle.
  - JSON steer protocol mapping.
- `src/workspace.rs`
  - Workspace root abstraction and safe path resolution.
  - Provides current-dir compatibility wrappers for CLI paths.
- `src/tools.rs`
  - Tool schemas and implementations.
  - Director/worker toolset split.
  - Director `bash` allowlist enforcement (`colgrep`/`rg`).
  - Executes filesystem and shell tools against the active `Workspace`.
- `src/workers.rs`
  - Worker prompt resolution.
  - Shared worker progress prompt injection.
  - In-process worker dispatch via `Agent::run_loop` in spawned async tasks.
  - Batch dispatch, async worker tracking, progress polling, and result collation.
- `src/session.rs`
  - Session meta/messages persistence.
  - Workspace-scoped Director/worker state and worker transcript paths.
- `src/prompts.rs`
  - Director prompt loading, factory prompt, builtin worker prompts, skill injection.

## State Layout

```txt
{workspace_root}/.ogent/
  sessions/
    {session_id}/
      meta.json
      messages.jsonl
      states.json
      workers/
        {worker_id}/
          messages.jsonl
          states.json
```

## Invariants

- Main agent is Director and does not receive direct file-edit tools.
- In `--serve` mode, each websocket connection starts unbound and can initialize exactly one Director Agent via setup (`start`/`fork`/`resume`).
- Each Agent has one immutable workspace root. WebSocket setup derives it from `repo`; CLI runs use the process current directory.
- Tool execution, bash current directory, state paths, session files, and spawned workers are workspace-scoped. Do not use global `std::env::set_current_dir` for per-connection behavior.
- Worker file edits are done via worker toolset (`write_file`, `edit_hash_anchors`).
- `dispatch_workers` starts workers and returns worker IDs immediately.
- `wait_workers` long-polls for completed worker results and reports still-running workers after a short wait.
- Running worker reports include `progress`, read from worker state key `progress/current`; missing or empty progress is reported as `Starting`.
- Worker state/transcript are scoped under parent session + worker ID.
- `--resume` enforces single active process ownership with `{workspace_root}/.ogent/sessions/{session_id}/active.lock`.
- Websocket `resume` uses an in-process active-session registry (not lock files) to prevent duplicate active sessions inside the serve process.
- A run ends when the Director sends a final assistant message (no tool calls).
