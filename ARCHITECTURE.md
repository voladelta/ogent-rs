# Architecture

## Runtime Shape

`ogent` currently runs as a single worker-mode runtime from the CLI.

```text
main.rs
  -> config.rs
  -> agent.rs
    -> workspace.rs
    -> client.rs / providers.rs / sse.rs
    -> tools.rs
      -> session.rs
  -> workers.rs
```

## Module Ownership

- `src/main.rs`
  - CLI parsing and worker runtime launch.
- `src/config.rs`
  - `config.yaml` loader with repo-level (`{workspace}/.ogent/config.yaml`) then home (`~/.ogent/config.yaml`) fallback.
  - Holds `profiles`, `providers`, `default_profile`, and `autocompact`.
- `src/agent.rs`
  - Worker turn loop, tool-call execution, and lightweight compaction reminders.
  - Owns an immutable `Workspace` used by tools, workers, and session persistence.
- `src/workspace.rs`
  - Workspace root abstraction and safe path resolution.
  - Provides current-dir compatibility wrappers for CLI paths.
- `src/tools.rs`
  - Tool schemas and implementations.
  - Full worker toolset used by CLI runtime.
  - Executes filesystem and shell tools against the active `Workspace`.
- `src/workers.rs`
  - Worker prompt resolution.
- `src/session.rs`
  - Session meta/messages persistence.
  - Workspace-scoped state and transcript paths.
- `src/prompts.rs`
  - Built-in `SYSTEM_PROMPT.md` prompt and skill injection.

## State Layout

Session persistence supports direct worker paths:

```txt
{workspace_root}/.ogent/
  sessions/
    {session_id}/
      meta.json
      messages.jsonl
      states.json
```

Active CLI runs set `temp: true`, so `persist_if_dirty` skips `meta.json` and `messages.jsonl`. The `state` tool can still create `states.json` during a direct CLI run.

Embedded worker scopes can still persist under a parent session:

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

- CLI launches a worker-mode agent directly.
- Worker-mode runs use worker prompts and worker tools only.
- Each Agent has one immutable workspace root from process current directory.
- Tool execution, bash current directory, state paths, and session files are workspace-scoped.
- Worker file edits are done via worker toolset (`write_file`, `edit_hash_anchors`).
- A run ends when the worker sends a final assistant message (no tool calls).

## Purge Status

This pass removes redundant CLI surfaces (`--serve`, `--resume`, `--fork`, `--create-skill`) and deletes the old websocket, steer, and artifact-creator source modules from the active tree. Nested-worker orchestration is no longer exposed by the worker CLI.
