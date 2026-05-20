# Agent Guide

## Director-First Runtime

`ogent` runs as a Director by default. `--run <role>` is the exception: it starts one worker-mode agent directly for a temporary one-off task.

The Director:

- maps and searches context
- plans
- dispatches workers
- writes/reads runtime state
- integrates worker results
- synthesizes already available evidence
- discusses routing, tradeoffs, and decisions directly
- exits on terminal state

The Director does not directly edit workspace files.

## Direct Worker Run

`ogent --run <role> "task"` bypasses the Director and starts a single worker-mode `Agent`.

Direct worker runs:

- resolve `<role>` through the normal worker prompt resolver
- use the worker toolset only
- do not expose Director worker-management tools
- use the configured default profile unless `--profile` is passed
- use the configured autocompact default
- force temporary mode, so no resumable session is persisted

## Worker Runtime

Workers are async tasks spawned by the Director. Each worker runs an independent `Agent` loop with the worker toolset.

Worker persistence:

- transcript: `{workspace_root}/.ogent/sessions/{parent_session_id}/workers/{worker_id}/messages.jsonl`
- state: `{workspace_root}/.ogent/sessions/{parent_session_id}/workers/{worker_id}/states.json`

Worker waiting:

- `dispatch_workers` starts workers and returns worker IDs immediately.
- `wait_workers` returns completed worker results as soon as any worker finishes, or reports still-running workers after a short wait.
- Running worker reports include `progress`.
  Workers are prompted to write concise phase updates to state key `progress/current`; `wait_workers` reports `Starting` until that key has a non-empty value.

Worker contracts are Markdown tasks with the smallest useful structure: task, scope, acceptance criteria, required evidence, verification, and output format. The contract output format overrides a worker role's default output format. The default output format is a concise worker result: status, summary, changed files, evidence, verification, risks, open questions, and next action.

Dispatch workers in one batch only when their scopes are independent. Independent means they do not need the same evidence-gathering step in order to do useful work, unless duplicate independent analysis is intentional. If one worker needs another worker's output, wait, integrate that result, then dispatch the dependent worker.

## Prompts

- Main system prompt: `prompts/SYSTEM_PROMPT.md`
- Factory prompt: `prompts/CONTRACTOR_FACTORY.md`
- Built-in worker roles:
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

Unknown role or `factory` role uses contractor-factory generation.
All worker system prompts include the shared progress-reporting nudge, including factory-generated roles.

## State and Exit

Director state lives in `{workspace_root}/.ogent/sessions/{session_id}/states.json`.

For non-trivial runs, the Director may keep a compact `decision/current` packet with goal, assumptions, worker IDs, acceptance criteria, evidence, and next decision.

A run ends when the Director sends a final assistant message (no tool calls).

## Tool Split

Director-only:

- `dispatch_workers`
- `wait_workers`
- `set_title`
- restricted `bash` (`colgrep` / `rg`)

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

## Compaction

Compaction is auto triggered at `autocompact` threshold.

Compaction creates a child session from a handoff brief and preserves the parent session.
