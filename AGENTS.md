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
  -> src/steer.rs
  -> src/websocket.rs (serve mode)
  -> src/workspace.rs
  -> src/client.rs + src/providers.rs + src/sse.rs
  -> src/tools.rs + src/workers.rs + src/session.rs
```

- `src/main.rs`: CLI wiring, resume/fork, worker subprocess mode, steer mode, websocket serve mode, skill creation.
- `src/agent.rs`: turn loop, streaming handling, tool dispatch integration, compaction, transport-neutral steer logic, Agent-owned `Workspace`.
- `src/steer.rs`: steer transport boundary and adapters.
- `src/websocket.rs`: websocket server, lazy `start`/`fork`/`resume` setup, per-connection Director runtime.
- `src/workspace.rs`: explicit workspace root and path resolution.
- `src/tools.rs`: Director/worker tool schemas and execution (`state`, `dispatch_workers`, `wait_workers`, read/write/web/bash/hashline).
- `src/workers.rs`: worker batch dispatch/waiting, role prompt resolution, workspace-aware subprocess spawning.
- `src/session.rs`: workspace-scoped session/meta/messages persistence plus Director/worker state file paths.
- `src/prompts.rs`: Director system prompt, factory prompt, built-in worker prompts, skill discovery/injection.

## File Routing Map

| Request area | Start here | Also check |
| --- | --- | --- |
| CLI flags, resume/fork/temp/worker/create-skill | `src/main.rs` | `docs/reference.md`, `README.md` |
| Director loop/exit rules/compaction | `src/agent.rs` | `src/steer.rs`, `docs/agent-guide.md`, `ARCHITECTURE.md` |
| Websocket serve mode/protocol | `src/websocket.rs` | `src/main.rs`, `src/workspace.rs`, `docs/reference.md` |
| Tool schema/behavior | `src/tools.rs` | `src/workers.rs`, `docs/reference.md` |
| Worker dispatch/wait/spawn/prompt resolution | `src/workers.rs` | `prompts/workers/*.md`, `docs/agent-guide.md` |
| Session/state pathing | `src/session.rs` | `src/workspace.rs`, `src/main.rs`, `src/tools.rs`, `docs/reference.md` |
| Prompt loading and built-ins | `src/prompts.rs` | `prompts/*`, `docs/agent-guide.md` |
| Anchored editing | `src/hashline.rs` | `src/tools.rs`, `docs/reference.md` |
| Workspace path validation | `src/workspace.rs` | `src/tools.rs`, `ARCHITECTURE.md` |
| Skill artifact creation (`--create-skill`) | `src/artifact_creator.rs` | `prompts/SKILL_CREATOR_PROMPT.md` |

## Runtime State

- Director transcript/meta:
  - `{workspace_root}/.ogent/sessions/{session_id}/messages.jsonl`
  - `{workspace_root}/.ogent/sessions/{session_id}/meta.json`
- Director state map:
  - `{workspace_root}/.ogent/sessions/{session_id}/states.json`
- Worker transcript/state:
  - `{workspace_root}/.ogent/sessions/{parent_session_id}/workers/{worker_id}/messages.jsonl`
  - `{workspace_root}/.ogent/sessions/{parent_session_id}/workers/{worker_id}/states.json`

## Key Invariants

- Main agent is Director (no direct file-edit tools in Director toolset).
- In websocket serve mode, each connection starts unbound and initializes one Director Agent with `start`, `fork`, or `resume`.
- Every Agent owns one immutable `Workspace`. WebSocket setup gets it from `repo`; CLI/TUI use current dir; worker mode uses `OGENT_WORKSPACE_ROOT` when set.
- Director `bash` allows only `colgrep` and `rg`.
- Workspace edits happen through worker subprocesses.
- Tools, state, sessions, and worker subprocesses must use the Agent workspace, not process-global cwd.
- `dispatch_workers` takes `{ workers: [{ role, task }] }`, starts workers, and returns worker IDs immediately.
- `wait_workers` waits briefly, returns completed worker results as soon as any worker finishes, and reports still-running workers otherwise.
- Running worker statuses include `progress`, read from each worker's `progress/current` state key. Workers are prompted to update that key during non-trivial work; missing or empty progress is reported as `Starting`.
- A run ends when the Director sends a final assistant message (no tool calls).
- Workers do not dispatch workers.
- `load_skill` tool and startup skill injection stay enabled.
- `--resume` acquires a workspace-scoped per-session lock file; concurrent resume on the same session is rejected.
- WebSocket `resume` uses an in-process active-session registry scoped by repo and session id.

## Verification

Use the smallest useful command set:

```bash
cargo fmt
cargo check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

For tool/loop/worker behavior changes, run `cargo test`.

## Search Quick Reference

```bash
colgrep "<intent>" -k 20
colgrep -e "<exact text>" "<intent>"
rg "<exact symbol>"
```
