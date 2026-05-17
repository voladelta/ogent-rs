# Architecture

## Runtime Shape

`ogent` is Director-first.

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
  - Turn loop, stream handling, tool-call execution, compaction, terminal status exit.
- `src/tools.rs`
  - Tool schemas and implementations.
  - Director/worker toolset split.
  - Director `bash` allowlist enforcement (`colgrep`/`rg`).
- `src/workers.rs`
  - Worker prompt resolution.
  - Worker subprocess spawn (`--worker=<parent_session_id>`, `OGENT_WORKER_ID`).
  - Batch dispatch and ordered result collation.
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
- `dispatch_workers` waits for the full batch and returns ordered `results`.
- Worker subprocess state/transcript are scoped under parent session + worker ID.
- Director loop exits only when state key `status` is exactly `done`, `blocked`, `failed`, or `partial`.
