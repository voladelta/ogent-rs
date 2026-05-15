# ogent Agent Guide

Use this file as the first routing layer for coding work in this repository.
It should help an agent identify the responsible files, make the smallest
correct change, and update documentation when behavior changes.

`ogent` is a Rust 2024 terminal coding agent. It turns user prompts into file
reads, shell commands, anchored edits, tests, worker delegation, session
persistence, workflows, skills, and optional TUI steering.

## Table of Contents

1. [Operating Rules](#operating-rules)
2. [Project Mental Model](#project-mental-model)
3. [File Routing Map](#file-routing-map)
4. [Runtime State and Environment](#runtime-state-and-environment)
5. [Change Playbooks](#change-playbooks)
6. [Invariants to Preserve](#invariants-to-preserve)
7. [Documentation Maintenance](#documentation-maintenance)
8. [Verification](#verification)
9. [Search and Editing](#search-and-editing)
10. [Documentation Index](#documentation-index)

## Operating Rules

- Prefer the smallest change that fixes the real requested behavior.
- Read the responsible module before editing it.
- Preserve existing behavior unless the request requires changing it.
- Use `colgrep` first for behavior or intent search. Use `rg` for exact text.
- Do not edit generated or runtime state such as `.ogent/sessions/`,
  `.ogent/journal.md`, `.ogent/memory/`, `target/`, or local skill caches unless
  the user explicitly asks for it.
- Update docs in the same change when user-visible behavior, architecture, or
  routing changes.
- In the final response, state what changed, what was verified, and which docs
  were updated or why docs were not needed.

## Project Mental Model

Two layers matter most:

- `src/main.rs` sets up runtime state: CLI args, profile, client, session mode,
  worker mode, resume/fork, workflows, and loop selection.
- `src/agent.rs` owns the agent loops: turn execution, streaming response
  handling, tool dispatch, compaction, steer-mode cancellation, task tracking,
  and worker integration.

Main flow:

```text
CLI / TUI input
  -> src/main.rs
  -> src/agent.rs
  -> src/client.rs + src/providers.rs + src/sse.rs
  -> src/tools.rs / src/workers.rs / src/session.rs / src/workflow.rs
```

## File Routing Map

| Request area | Start here | Also check |
| --- | --- | --- |
| CLI flags, command aliases, mode selection, resume/fork/temp/worker setup | `src/main.rs` | `docs/reference.md`, `README.md` |
| Dependencies and package configuration | `Cargo.toml`, `Cargo.lock` | affected module docs only if behavior changes |
| Model profiles and provider selection | `src/profiles.rs` | `src/providers.rs`, `docs/reference.md` |
| Provider request bodies, API-specific fields, auth env vars | `src/providers.rs` | `src/client.rs`, `docs/reference.md` |
| HTTP streaming, retries, SSE request lifecycle | `src/client.rs` | `src/sse.rs`, `docs/reference.md` |
| SSE parsing and streamed response accumulation | `src/sse.rs` | `src/types.rs` |
| Agent loop behavior, turn handling, tool-call execution order, compaction | `src/agent.rs` | `docs/agent-guide.md`, `ARCHITECTURE.md` |
| Steer-mode cancellation and loop behavior | `src/agent.rs` | `src/tui.rs`, `docs/steer-mode.md` |
| TUI rendering, input handling, steer commands | `src/tui.rs` | `src/agent.rs`, `docs/steer-mode.md` |
| Tool schema, tool registry, tool implementation | `src/tools.rs` | `src/types.rs`, `docs/reference.md` |
| Read/write/bash/web tool behavior | `src/tools.rs` | `src/workspace.rs`, `docs/reference.md` |
| Anchored editing and hashline validation | `src/hashline.rs` | `src/tools.rs`, `docs/reference.md` |
| Workspace path validation and allowed roots | `src/workspace.rs` | `src/tools.rs`, `ARCHITECTURE.md` |
| Runtime goal, phase, todo tracking | `src/task_tracker.rs` | `src/tools.rs`, `docs/agent-guide.md` |
| Workflow schema, checks, transitions, persistence model | `src/workflow.rs` | `workflows/*.yaml`, `src/tools.rs`, `docs/agent-guide.md` |
| Built-in workflow definitions | `workflows/common-sw.yaml`, `workflows/auto-iteration.yaml` | `src/workflow.rs`, `docs/agent-guide.md` |
| Session save/load, child sessions, journal, metadata | `src/session.rs` | `src/main.rs`, `src/agent.rs`, `docs/reference.md` |
| Worker subprocess execution and async worker manager | `src/workers.rs` | `src/tools.rs`, `prompts/workers/*`, `docs/agent-guide.md` |
| Worker role behavior | `prompts/workers/*.md` | `src/workers.rs`, `src/tools.rs` |
| System prompt, architect prompt, skill discovery/loading | `src/prompts.rs`, `prompts/*` | `docs/agent-guide.md` |
| Shared response, message, tool, and domain types | `src/types.rs` | caller module that owns behavior |
| User-facing examples and project overview | `README.md` | `docs/reference.md` |
| Architecture map and cross-module invariants | `ARCHITECTURE.md` | this file |

## Runtime State and Environment

Runtime artifacts normally should not be edited as source:

- `.ogent/sessions/` stores session transcripts and metadata.
- `.ogent/journal.md` stores completion summaries.
- `.ogent/memory/` is reserved for memory implementation artifacts.
- `target/` is Cargo build output.

Provider and web tools depend on environment variables:

- `DEEPSEEK_API_KEY` for DeepSeek profiles.
- `BASETEN_API_KEY` for Kimi/Baseten profiles.
- `Z_API_KEY` for Z.ai/GLM profiles.
- `EXA_API_KEY` for web tools.

Do not assume credentials are present. If a verification step needs network or
paid API access, state that explicitly and prefer compile/unit coverage when it
is sufficient.

## Change Playbooks

### Add or Change a CLI Flag

1. Edit `Args` and startup wiring in `src/main.rs`.
2. If persisted runtime metadata changes, update `src/session.rs` types/usages.
3. Update `docs/reference.md`.
4. Update `README.md` only when quick-start examples or common usage change.
5. Verify with `cargo check` and a focused CLI smoke command when practical.

### Add or Change a Model Profile

1. Edit `src/profiles.rs`.
2. If the provider request shape changes, edit `src/providers.rs`.
3. If streaming, retry, or auth behavior changes, check `src/client.rs`.
4. Update `docs/reference.md` profile table.
5. Verify with `cargo check`; run a real API smoke test only when credentials and
   cost are acceptable.

### Add or Change Dependencies or Package Config

1. Edit `Cargo.toml`.
2. Let Cargo update `Cargo.lock`; do not hand-edit lockfile entries.
3. Check the modules that use the dependency for feature flags, runtime behavior,
   or platform assumptions.
4. Update docs only when behavior, setup, or supported usage changes.
5. Verify with `cargo check`; run `cargo test` when behavior changes.

### Add or Change a Tool

1. Edit dispatch in `execute_tool` in `src/tools.rs`.
2. Add or update the tool schema in `build_coder_tools`.
3. Decide whether workers may use the tool; update `WORKER_EXCLUDED` if needed.
4. If the tool needs agent state, thread it through `ToolContext`.
5. Add or update tests in `src/tools.rs` or the owning module.
6. Update `docs/reference.md`; update `ARCHITECTURE.md` if tool boundaries or
   execution ordering change.
7. Verify with `cargo test` for behavioral changes.

### Change File Editing Behavior

1. Start in `src/hashline.rs` for anchor generation and edit application.
2. Check `src/tools.rs` for tool argument parsing and user-visible errors.
3. Preserve stale-anchor rejection unless intentionally changing the contract.
4. Update `docs/reference.md` hashline section.
5. Verify with focused hashline tests and then `cargo test`.

### Change Agent Loop Behavior

1. Start in `src/agent.rs`.
2. Check whether the change affects tools, sessions, task tracking, workflows,
   workers, or steer-mode cancellation.
3. Update `docs/agent-guide.md`.
4. Update `ARCHITECTURE.md` if module boundaries, data flow, or invariants
   change.
5. Verify with `cargo test`; add focused tests where the behavior can be made
   deterministic.

### Change Steer Mode

1. Start in `src/tui.rs` for UI/input behavior.
2. Check `src/agent.rs` for cancellation, partial-response preservation, and
   loop behavior.
3. If stream cancellation or partial response handling changes, also check
   `src/client.rs` and `src/sse.rs`.
4. Update `docs/steer-mode.md`.
5. Update `README.md` examples only for common invocation changes.
6. Verify with `cargo check`; manually smoke test `cargo run -- --steer` when UI
   behavior changes.

### Change Session Persistence

1. Start in `src/session.rs`.
2. Check `src/main.rs` for resume/fork/temp setup.
3. Check `src/agent.rs` for compaction and child-session behavior.
4. Update `docs/reference.md`; update `docs/agent-guide.md` for compaction
   semantics.
5. Verify with `cargo test` and a resume/fork smoke test when practical.

### Change Workers

1. Start in `src/workers.rs` for subprocess execution and manager behavior.
2. Check `src/tools.rs` for `dispatch_worker`, `start_workers`, and
   `check_workers`.
3. Remember that coder and worker tool schemas differ; update both paths when a
   tool availability change affects workers.
4. Edit `prompts/workers/*.md` for role behavior.
5. Update `docs/agent-guide.md`.
6. Verify with `cargo test`; run a small worker task when practical.

### Change Workflows

1. Start in `src/workflow.rs` for state, validation, transitions, and checks.
2. Edit `workflows/*.yaml` for built-in workflow definitions.
3. Check `src/tools.rs` for workflow tool behavior and schemas.
   Workflow tools are `workflow_status`, `workflow_enter_step`,
   `workflow_record_check`, and `workflow_run_check`.
4. Update `docs/agent-guide.md`; update `docs/reference.md` if tool behavior or
   CLI usage changes.
5. Verify transition/check behavior with command or unit coverage, then run
   `cargo test`.

### Change Task Tracking

1. Start in `src/task_tracker.rs`.
2. Check `src/tools.rs` for `set_goal`, `revise_goal`, `update_phase`, and
   `update_todo`.
3. Check `src/agent.rs` for compaction and workflow mirroring.
4. Update `docs/agent-guide.md`.
5. Verify with focused task-tracker tests and `cargo test`.

### Change Prompts or Skills

1. Start in `prompts/SYSTEM_PROMPT.md`, `prompts/ARCHITECT_PROMPT.md`, or
   `prompts/workers/*.md`.
2. Check `src/prompts.rs` if loading, discovery, or injection behavior changes.
3. Update `docs/agent-guide.md` if skill or worker prompting behavior changes.
4. Verify with `cargo check`; run a small prompt-driven smoke task when useful.

### Change Architecture or Module Ownership

1. Make the code change first.
2. Update `ARCHITECTURE.md`.
3. Update this `AGENTS.md` routing map and any affected playbook.
4. Verify with the command appropriate to the code change.

## Invariants to Preserve

- `src/agent.rs` delegates HTTP streaming to `src/client.rs`; do not mix provider
  transport details into the agent loop.
- Tool failures that are normal runtime errors should be returned to the model
  as error strings, not crash the whole loop.
- Contiguous read-only tool calls may be batched; mutating or blocking tools are
  barriers.
- File access must pass workspace validation in `src/workspace.rs`.
- Anchored edits must reject stale anchors.
- Workflow tools are available only when a workflow is active.
- Workflow state controls workflow transitions; task tracker state is progress
  display and planning state.
- Workers do not get parent-only tools such as worker dispatch, task tracking,
  workflow control, or `complete`.
- Tool availability differs between coder and worker modes; schema changes must
  preserve that split intentionally.
- Resume writes back to the loaded session. Fork creates a child session.
- Compaction preserves the parent session and starts a child session from a
  handoff brief.
- Steer-mode cancellation preserves accumulated partial streamed responses.
  Changes touching `src/tui.rs`, `src/agent.rs`, `src/client.rs`, or `src/sse.rs`
  should preserve that behavior unless explicitly changing it.

## Documentation Maintenance

Docs are part of done when behavior changes.

Update docs when you:

- add, remove, or rename CLI flags
- add, remove, or change tools
- change agent-loop, compaction, worker, session, workflow, task-tracker, or
  steer-mode behavior
- add, remove, or change model profiles or providers
- change user-visible commands, examples, errors, or runtime guarantees
- change module ownership or architectural boundaries

Do not update docs for purely internal refactors that preserve behavior, unless
file ownership or routing guidance changes.

| Change | Docs to check |
| --- | --- |
| CLI flag, profile, or session behavior | `README.md`, `docs/reference.md` |
| Tool schema or tool behavior | `docs/reference.md`, `ARCHITECTURE.md` if boundaries change |
| Agent loop, compaction, task tracker, workers | `docs/agent-guide.md`, `ARCHITECTURE.md` |
| TUI or steer commands | `docs/steer-mode.md`, `README.md` if examples change |
| Workflows | `docs/agent-guide.md`, `docs/reference.md`, `workflows/*.yaml` |
| Module boundary or major data flow | `ARCHITECTURE.md`, this file |
| User-facing quick start or examples | `README.md` |

Before finishing a code change:

- Run the smallest useful verification command.
- Check whether documentation needs updates.
- Update this file if future agents need different routing.
- State tests run and docs updated in the final response.

## Verification

Common commands:

```bash
cargo fmt -- --check
cargo fmt
cargo check
cargo clippy
cargo test
```

Use the smallest command that gives real evidence:

- Docs-only change: no Rust verification required unless examples or generated
  content changed.
- Type or wiring change: `cargo check`.
- Tool, workflow, session, hashline, or task-tracker behavior change:
  `cargo test`.
- Style-only Rust change: `cargo fmt -- --check`.
- TUI behavior change: `cargo check` plus a manual `cargo run -- --steer` smoke
  test when practical.

Focused test locations:

- `src/hashline.rs` for anchored edit tests.
- `src/workflow.rs` for workflow validation, transitions, checks, and
  persistence-adjacent behavior.
- `src/tools.rs` for tool schema, dispatch, worker-tool exclusion, and
  tool-level behavior.
- `src/task_tracker.rs` for goal, phase, todo, and validation-contract behavior.
- `src/session.rs` for session persistence helpers.
- `src/workers.rs` for worker command construction and manager behavior.
- `src/sse.rs` and `src/providers.rs` for streaming and provider request
  behavior.
- `src/tui.rs` for TUI helper behavior.

## Search and Editing

- Use `colgrep "<behavior description>" -k 20` for semantic search.
- Use `colgrep -e "<exact text>" "<intent>"` for hybrid search.
- Use `rg` for exact symbol or string search.
- Use `cargo fmt` only when Rust files were changed.
- Keep edits local to the owning module.
- Clean up only imports, variables, or helpers orphaned by your own change.
- Do not refactor adjacent code unless the requested behavior requires it.

## Documentation Index

- `README.md` - user-facing overview, quick start, common examples.
- `ARCHITECTURE.md` - module map, data flow, invariants, cross-cutting behavior.
- `docs/reference.md` - CLI flags, profiles, tools, hashline editing, sessions,
  context budget.
- `docs/agent-guide.md` - agent internals, task tracking, skills, workers,
  workflows, compaction.
- `docs/steer-mode.md` - TUI behavior, commands, cancellation, compaction.
