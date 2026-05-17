# Architecture

## Runtime Shape

`ogent` is built around a Director/worker split.

```text
main.rs
  -> agent.rs
    -> client.rs / providers.rs / sse.rs
    -> tools.rs
      -> workers.rs
      -> session.rs
```

## Module Ownership

- `src/main.rs`
  - CLI parsing, mode wiring, resume/fork, steer boot, skill creation mode.
- `src/agent.rs`
  - Turn loop, stream handling, tool-call execution, compaction.
- `src/tools.rs`
  - Tool schemas and implementations.
  - Director/worker toolset split.
  - Director `bash` allowlist enforcement (`colgrep`/`rg`).
- `src/workers.rs`
  - Worker prompt resolution.
  - Shared worker progress prompt injection.
  - Worker subprocess spawn (`--worker=<parent_session_id>`, `OGENT_WORKER_ID`).
  - Batch dispatch, async worker tracking, progress polling, and result collation.
- `src/session.rs`
  - Session meta/messages persistence.
  - Director/worker state and worker transcript paths.
- `src/prompts.rs`
  - Director prompt loading, factory prompt, builtin worker prompts, skill injection.

## State Layout

```txt
.ogent/
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
- Worker file edits are done via worker toolset (`write_file`, `edit_hash_anchors`).
- `dispatch_workers` starts workers and returns worker IDs immediately.
- `wait_workers` long-polls for completed worker results and reports still-running workers after a short wait.
- Running worker reports include `progress`, read from worker state key `progress/current`; missing or empty progress is reported as `Starting`.
- Worker subprocess state/transcript are scoped under parent session + worker ID.
- A run ends when the Director sends a final assistant message (no tool calls).
