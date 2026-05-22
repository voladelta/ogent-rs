# Agent Guide

## Worker Runtime

`ogent` now runs as a single worker-mode agent from the CLI.

`ogent --role <role> "task"` starts a single worker-mode `Agent`.
`ogent "task"` does the same with default role `ogent`.

Direct worker runs:

- resolve `<role>` through the normal worker prompt resolver
- use the role's scoped worker tool group
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
  - `visual_designer`
  - `database_architect`
  - `system_architect`
  - `summarizer`
  - `reviewer`

Unknown roles return an error from the worker prompt resolver.
All worker system prompts append the shared integrity, progress, and result-reporting instructions; role files should describe role goals, constraints, and evidence focus rather than duplicating the final format.

## State and Exit

Direct CLI worker runs set `temp: true`, so transcript and metadata files are not persisted by `persist_if_dirty`. The `state` tool can still write `{workspace_root}/.ogent/sessions/{session_id}/states.json`.

A run ends when the worker sends a final assistant message (no tool calls).

## Tool Groups

Worker tools are grouped by role to keep prompts smaller and reduce accidental tool use. `ogent` receives the full worker toolset.

| Group | Roles | Tools |
| --- | --- | --- |
| generalist | `ogent` | all worker tools |
| coder | `implementer` | `state`, `load_skill`, repo/code read tools, file write/edit tools, `bash`, `web_code_context` |
| diagnostic | `debugger` | `state`, `load_skill`, repo/code read tools, `bash`, `web_code_context` |
| review | `reviewer` | `state`, `load_skill`, repo/code read tools, `bash` |
| evidence | `verifier` | `state`, `load_skill`, repo/code read tools, `bash`, `web_search`, `web_read` |
| research | `researcher` | `state`, `load_skill`, `read_file`, `write_file`, web tools |
| writing | `writer`, `visual_designer` | `state`, `load_skill`, `read_file`, `write_file`, `web_search`, `web_read` |
| architecture | `system_architect`, `database_architect` | `state`, `load_skill`, repo/code read tools, `write_file` |
| summary | `summarizer` | `state`, `load_skill`, `read_file`, `write_file` |

Every specialist group includes `state` for progress reporting and `load_skill` for task-specific guidance. Writing and summary roles can create requested files with `write_file`; code-editing anchors stay with the coder/generalist groups.

## Purge Note

Legacy Director, websocket, nested-worker, and skill-creator entrypoints have been removed from the active runtime. This document describes the worker-only CLI surface.
