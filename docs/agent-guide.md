# Agent Guide

## Worker Runtime

`ogent` now runs as a single worker-mode agent from the CLI.

`ogent "task"` starts a single worker-mode `Agent`.

Direct worker runs:

- use the root `SYSTEM_PROMPT.md`
- use the full worker toolset
- do not expose Director worker-management tools (`dispatch_workers`, `wait_workers`, `inspect_worker`, `cancel_workers`, `set_title`)
- use the configured default profile unless `--profile` is passed
- use the configured autocompact default
- force temporary mode, so no resumable session is persisted

## Prompts

- Worker prompt: `SYSTEM_PROMPT.md`
- `SYSTEM_PROMPT.md` owns the integrity, progress, and result-reporting instructions.

## State and Exit

Direct CLI worker runs set `temp: true`, so transcript and metadata files are not persisted by `persist_if_dirty`. The `state` tool can still write `{workspace_root}/.ogent/sessions/{session_id}/states.json`.

A run ends when the worker sends a final assistant message (no tool calls).

## Worker Tools

All worker runs receive the full worker toolset. This keeps the runtime to one prompt and one capability surface.

## Purge Note

Legacy Director, websocket, nested-worker, and skill-creator entrypoints have been removed from the active runtime. This document describes the worker-only CLI surface.
