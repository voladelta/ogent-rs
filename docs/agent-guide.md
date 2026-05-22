# Agent Guide

## Worker Runtime

`ogent` now runs as a single worker-mode agent from the CLI.

`ogent --role <role> "task"` starts a single worker-mode `Agent`.
`ogent "task"` does the same with default role `ogent`.

Direct worker runs:

- resolve `<role>` through the normal worker prompt resolver
- use the worker toolset only
- do not expose Director worker-management tools (`dispatch_workers`, `wait_workers`, `inspect_worker`, `cancel_workers`, `set_title`)
- use the configured default profile unless `--profile` is passed
- use the configured autocompact default
- force temporary mode, so no resumable session is persisted

## Prompts

- Worker preset prompts: `workers/*.md`
- Built-in worker roles:
  - `ogent`
  - `implementer`
  - `verifier`
  - `debugger`
  - `researcher`
  - `writer`
  - `critic`
  - `visual_designer`
  - `database_architect`
  - `system_architect`
  - `summarizer`
  - `reviewer`
  - `qa_writer`

Unknown roles return an error from the worker prompt resolver.
All worker system prompts append the shared integrity, progress, and result-reporting instructions; role files should describe role goals, constraints, and evidence focus rather than duplicating the final format.

## State and Exit

Direct CLI worker runs set `temp: true`, so transcript and metadata files are not persisted by `persist_if_dirty`. The `state` tool can still write `{workspace_root}/.ogent/sessions/{session_id}/states.json`.

A run ends when the worker sends a final assistant message (no tool calls).

## Tool Split

Worker-only edits:

- `write_file`
- `read_hash_anchors`
- `edit_hash_anchors`

Shared:

- `repo_map`
- `code_map`
- `load_skill`
- `state`

Worker-only context gathering:

- `read_file`
- `web_search`
- `web_read`
- `web_code_context`

## Purge Note

Legacy Director, websocket, nested-worker, and skill-creator entrypoints have been removed from the active runtime. This document describes the worker-only CLI surface.
