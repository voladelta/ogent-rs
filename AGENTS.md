# ogent Agent Guide

`ogent` runs as a Director. The main agent plans, dispatches workers, tracks state, synthesizes evidence, integrates results, and reports the outcome.

## Operating Rules

- Make the smallest correct change.
- Use `colgrep` first for behavior/intent search. Use `rg` for exact text.
- Do not edit runtime artifacts (`.ogent/sessions/`, `.ogent/journal.md`, `target/`) unless requested.
- Update docs when behavior changes.
- In final handoff: list changed files, verification, and doc updates.

## Project Mental Model

Main flow:

```text
CLI / TUI
  -> src/main.rs
  -> src/agent.rs
  -> src/client.rs + src/providers.rs + src/sse.rs
  -> src/tools.rs + src/workers.rs + src/session.rs
```

- `src/main.rs`: CLI wiring, resume/fork, worker subprocess mode, steer mode, skill creation.
- `src/agent.rs`: turn loop, streaming handling, tool dispatch integration, compaction.
- `src/tools.rs`: Director/worker tool schemas and execution (`state`, `dispatch_workers`, `wait_workers`, read/write/web/bash/hashline).
- `src/workers.rs`: worker batch dispatch/waiting, role prompt resolution, subprocess spawning.
- `src/session.rs`: session/meta/messages persistence plus Director/worker state file paths.
- `src/prompts.rs`: Director system prompt, factory prompt, built-in worker prompts, skill discovery/injection.

## File Routing Map

| Request area | Start here | Also check |
| --- | --- | --- |
| CLI flags, resume/fork/temp/worker/create-skill | `src/main.rs` | `docs/reference.md`, `README.md` |
| Director loop/exit rules/compaction | `src/agent.rs` | `docs/agent-guide.md`, `ARCHITECTURE.md` |
| Tool schema/behavior | `src/tools.rs` | `src/workers.rs`, `docs/reference.md` |
| Worker dispatch/wait/spawn/prompt resolution | `src/workers.rs` | `prompts/workers/*.md`, `docs/agent-guide.md` |
| Session/state pathing | `src/session.rs` | `src/main.rs`, `src/tools.rs`, `docs/reference.md` |
| Prompt loading and built-ins | `src/prompts.rs` | `prompts/*`, `docs/agent-guide.md` |
| Anchored editing | `src/hashline.rs` | `src/tools.rs`, `docs/reference.md` |
| Workspace path validation | `src/workspace.rs` | `src/tools.rs`, `ARCHITECTURE.md` |
| Skill artifact creation (`--create-skill`) | `src/artifact_creator.rs` | `prompts/SKILL_CREATOR_PROMPT.md` |

## Runtime State

- Director transcript/meta:
  - `.ogent/sessions/{session_id}/messages.jsonl`
  - `.ogent/sessions/{session_id}/meta.json`
- Director state map:
  - `.ogent/sessions/{session_id}/states.json`
- Worker transcript/state:
  - `.ogent/sessions/{parent_session_id}/workers/{worker_id}/messages.jsonl`
  - `.ogent/sessions/{parent_session_id}/workers/{worker_id}/states.json`

## Key Invariants

- Main agent is Director (no direct file-edit tools in Director toolset).
- Director `bash` allows only `colgrep` and `rg`.
- Workspace edits happen through worker subprocesses.
- `dispatch_workers` takes `{ workers: [{ role, task }] }`, starts workers, and returns worker IDs immediately.
- `wait_workers` waits briefly, returns completed worker results as soon as any worker finishes, and reports still-running workers otherwise.
- Running worker statuses include `progress`, read from each worker's `progress/current` state key. Workers are prompted to update that key during non-trivial work; missing or empty progress is reported as `Starting`.
- A run ends when the Director sends a final assistant message (no tool calls).
- Workers do not dispatch workers.
- `load_skill` tool and startup skill injection stay enabled.

## Verification

Use the smallest useful command set:

```bash
cargo fmt
cargo check
cargo test
```

For tool/loop/worker behavior changes, run `cargo test`.

## Search Quick Reference

```bash
colgrep "<intent>" -k 20
colgrep -e "<exact text>" "<intent>"
rg "<exact symbol>"
```
