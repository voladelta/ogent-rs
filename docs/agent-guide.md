# Agent Guide

## Director-First Runtime

`ogent` is always a Director in the main process.

The Director:

- reads context
- plans
- dispatches workers
- writes/reads runtime state
- integrates worker results
- exits on terminal state

The Director does not directly edit workspace files.

## Worker Runtime

Workers are subprocesses spawned by the Director:

```bash
ogent --worker=<parent_session_id> "<task prompt>"
```

Environment:

- `OGENT_WORKER_ID` is required in worker mode.

Worker persistence:

- transcript: `.ogent/sessions/{parent_session_id}/workers/{worker_id}/messages.jsonl`
- state: `.ogent/sessions/{parent_session_id}/workers/{worker_id}/states.json`

## Prompts

- Main system prompt: `prompts/DIRECTOR_PROMPT.md`
- Factory prompt: `prompts/CONTRACTOR_FACTORY.md`
- Built-in worker roles:
  - `implementer`
  - `verifier`
  - `debugger`
  - `researcher`
  - `writer`
  - `critic`
  - `designer`
  - `summarizer`
  - `reviewer`

Unknown role or `factory` role uses contractor-factory generation.

## State and Exit

Director state lives in `.ogent/sessions/{session_id}/states.json`.

After each turn, the agent checks key `status`.
If `status` is exactly one of `done`, `blocked`, `failed`, `partial`, the Director loop exits.

The final output shown to the user is the Director’s last assistant message.

## Tool Split

Director-only:

- `dispatch_workers`
- restricted `bash` (`colgrep` / `rg`)

Worker-only edits:

- `write_file`
- `read_hash_anchors`
- `edit_hash_anchors`

Shared:

- read/web/search tools
- `repo_map`
- `load_skill`
- `state`

## Compaction

Autocompaction is still available.
Compaction creates a child session from a handoff brief and preserves the parent session.
