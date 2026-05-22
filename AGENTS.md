# ogent Agent Guide

`ogent` currently runs as a single worker-mode coding agent from the CLI. Codex or another outer caller is expected to prepare the task, launch one `ogent` worker, inspect its result, verify, and relaunch when needed.

## Operating Rules

- Make the smallest correct change.
- Use `colgrep` first for behavior/intent search. Use `rg` for exact text.
- Do not edit runtime artifacts (`.ogent/sessions/`, `.ogent/journal.md`, `target/`) unless requested.
- Update docs when behavior changes.
- In final handoff: list changed files, verification, and doc updates.

## Project Mental Model

Main flow:

```text
CLI
  -> src/main.rs
  -> src/agent.rs
  -> src/workspace.rs
  -> src/client.rs + src/providers.rs + src/sse.rs
  -> src/tools.rs + src/session.rs
```

- `src/main.rs`: CLI parsing and worker runtime launch.
- `src/agent.rs`: worker turn loop, tool-call execution, compaction reminders, Agent-owned `Workspace`.
- `src/workspace.rs`: explicit workspace root and path resolution.
- `src/client.rs`, `src/providers.rs`, `src/sse.rs`: provider request construction, HTTP client, and SSE response parsing.
- `src/tools.rs`: worker tool schemas and execution (`read_file`, `write_file`, `bash`, `repo_map`, `code_map`, web tools, `state`, hashline editing).
- `src/session.rs`: workspace-scoped session meta/messages/state paths and persistence.
- `src/prompts.rs`: built-in worker prompts and skill discovery/injection.
- `src/symbol_tree.rs`: tree-sitter based symbol extraction for `code_map` (Rust and Go).

Removed legacy surfaces:

- No Director runtime.
- No websocket server.
- No nested worker dispatch/wait/cancel tools.
- No `--resume`, `--fork`, `--serve`, or `--create-skill` CLI flow.

## File Routing Map

| Request area | Start here | Also check |
| --- | --- | --- |
| CLI flags and worker launch | `src/main.rs` | `docs/reference.md`, `README.md` |
| Worker loop / exit / compaction reminders | `src/agent.rs` | `docs/agent-guide.md`, `ARCHITECTURE.md` |
| Tool schema/behavior | `src/tools.rs` | `docs/reference.md` |
| System prompt / initial messages | `src/prompts.rs` | `SYSTEM_PROMPT.md` |
| Session/state pathing | `src/session.rs` | `src/workspace.rs`, `src/tools.rs` |
| Prompt loading and built-ins | `src/prompts.rs` | `SYSTEM_PROMPT.md`, `docs/agent-guide.md` |
| Anchored editing | `src/hashline.rs` | `src/tools.rs`, `docs/reference.md` |
| Workspace path validation | `src/workspace.rs` | `src/tools.rs`, `ARCHITECTURE.md` |
| Symbol extraction / `code_map` | `src/symbol_tree.rs` | `docs/reference.md` |

## Runtime State

Session persistence paths:

```txt
{workspace_root}/.ogent/
  sessions/
    {session_id}/
      meta.json
      messages.jsonl
      states.json
```

- Direct CLI runs set `temp: true`, so `meta.json` and `messages.jsonl` are not persisted by `persist_if_dirty`.
- The `state` tool can still write `states.json`, including `progress/current` when a task spans multiple tool calls.
- Non-temp or embedded runs can persist `meta.json` and `messages.jsonl`.
- Embedded worker scopes can still persist under `{parent_session}/workers/{worker_id}/`, but the active CLI launches one direct worker and does not expose worker-dispatch tools.

## Key Invariants

- CLI launches exactly one worker-mode agent.
- Worker-mode runs use worker prompts and worker tools only.
- Worker runs receive the full worker toolset.
- Every Agent owns one immutable `Workspace`; CLI uses the process current directory.
- Tool execution, bash current directory, state paths, and session files must use the Agent workspace, not process-global mutable cwd.
- Worker file edits happen through worker tools (`write_file`, `edit_hash_anchors`).
- Workers do not dispatch workers.
- A run ends when the worker sends a final assistant message with no tool calls.
- `load_skill` tool and startup skill injection stay enabled.
- The final worker answer must use the Markdown result sections defined in `SYSTEM_PROMPT.md`.

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
code_map {"path": "src"}           # Rust/Go symbol map
code_map {"path": "src/main.rs"}   # single-file symbol map
```
