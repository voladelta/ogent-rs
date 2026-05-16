# Director Mode Implementation Plan

## Goal

Transform ogent from a repo-aware worker into a Director agent. There is no mode switch. `ogent` IS the Director.

The Director does not edit files. It designs workflows, dispatches workers, manages state, and exits when a terminal status is written. The Director's last assistant message is the user-facing final output.

Workers (subprocesses spawned by ogent) do the actual file editing, research, and verification.

## Design Decisions

| Decision | Choice |
|---|---|
| State storage | JSON state map at `.ogent/sessions/{session_id}/states.json`; worker state map at `.ogent/sessions/{session_id}/workers/{worker_id}/states.json` |
| System prompt | Always `DIRECTOR_PROMPT.md`. No normal/worker mode for the main agent. |
| Director-mode spec docs | Treat `director-mode/` as source material for this plan and prompts only. It will become obsolete after the final prompt/spec merge. |
| File edit policy | Main agent cannot `write_file` or `edit_hash_anchors`. Workspace mutations go through workers. |
| Director shell policy | Director `bash` is allowlisted to `colgrep` and `rg` commands only. Workers keep normal `bash`. |
| Search | Director searches with allowlisted `bash` commands: `colgrep ...` and `rg ...` |
| Worker creation | `role: "factory"` in `dispatch_workers` triggers `CONTRACTOR_FACTORY.md` architect prompt |
| Worker CLI | Worker subprocesses use `--worker=<parent_session_id>` plus `OGENT_WORKER_ID` for their scoped state/transcript directory. |
| Exit mechanism | Agent loop reads the `status` key from `.ogent/sessions/{session_id}/states.json` after each turn. If it exactly equals `done`, `blocked`, `failed`, or `partial`, the session exits and prints the Director's last assistant message. |
| Old tools | `set_goal`, `update_phase`, `update_todo`, `workflow_*`, `complete` removed from main agent. Replaced by `state` tool. |

## New Files (Prompts)

### `prompts/DIRECTOR_PROMPT.md`
The only system prompt for ogent. Adapted from `director-mode/SYSTEM_PROMPT_DIRECTOR.md`.

### `prompts/CONTRACTOR_FACTORY.md`
Architect prompt for generating temporary specialist workers. Used when `dispatch_workers` receives an unknown role or `role: "factory"`.

### `prompts/workers/implementer.md`
Implementation worker. Produces artifacts and code changes.

### `prompts/workers/verifier.md`
Gathers proof: tests, builds, checks.

### `prompts/workers/debugger.md`
Finds root cause. Does not fix unless explicitly asked.

### `prompts/workers/researcher.md`
Gathers context from codebase and docs.

### `prompts/workers/writer.md`
Produces clear written content.

### `prompts/workers/critic.md`
Judges quality, flags contract drift.

### `prompts/workers/designer.md`
Evaluates visual/structural design.

### `prompts/workers/summarizer.md`
Compresses complex outputs into concise summaries.

### Updated `prompts/workers/reviewer.md`
Restructured to match director worker format with Operating Kernel and structured output.

## Changes by File

### `src/prompts.rs`

**Replace constants:**
- `SYSTEM_PROMPT` remains for potential override, but default system prompt becomes `DIRECTOR_PROMPT`
- `DIRECTOR_PROMPT` — loaded from `prompts/DIRECTOR_PROMPT.md`
- `CONTRACTOR_FACTORY` — loaded from `prompts/CONTRACTOR_FACTORY.md`
- Worker prompt constants for all 9 roles

**Update functions:**
- `build_messages(prompt)` — now always uses `DIRECTOR_PROMPT` instead of `load_system_prompt()`
- `get_builtin_worker_prompt(name)` — add mappings for: `implementer`, `verifier`, `debugger`, `researcher`, `writer`, `critic`, `designer`, `summarizer`
- `load_system_prompt()` — optionally check for `.ogent/DIRECTOR_PROMPT.md` override before falling back to built-in
- Preserve `enrich_initial_messages()` and `load_skill_content()` so startup skill/context injection continues, especially the `colgrep` skill.
- Preserve the `load_skill` tool as a read-only tool. It is not part of task/state tracking and should not be removed with the old workflow/task tools.

### `src/session.rs`

Session persistence remains, but worker subprocess persistence now needs parent-scoped paths.

**Preserve:**
- `.ogent/sessions/{session_id}/meta.json`
- `.ogent/sessions/{session_id}/messages.jsonl`

**Add helpers:**
- `state_path(session_id) -> .ogent/sessions/{session_id}/states.json`
- `worker_dir(parent_session_id, worker_id) -> .ogent/sessions/{parent_session_id}/workers/{worker_id}/`
- `worker_state_path(parent_session_id, worker_id) -> .../states.json`
- `worker_messages_path(parent_session_id, worker_id) -> .../messages.jsonl`

`meta.json` stays because the current resume/find-latest logic depends on it.

### `src/main.rs`

**Remove fields:**
- Remove `--workflow` flag (no YAML workflow engine)

**Preserve fields:**
- `--steer` stays. Steer mode is now "steer the Director" — the TUI lets the user guide a Director agent instead of a worker.
- Worker mode becomes `--worker=<parent_session_id>` for subprocess workers spawned by the Director.
- `--create-skill` stays.

**Update logic:**
- Always call `prompts::build_messages(&prompt)` which now returns Director messages
- Always use `tools::configured_director_tools()` instead of `configured_coder_tools`
- Do not load YAML workflow state
- In steer mode, the Director still uses `configured_director_tools()` and cannot edit files directly
- In worker mode (`--worker=<parent_session_id>`):
  - Read `OGENT_WORKER_ID` from environment
  - Persist worker transcript to `.ogent/sessions/{parent_session_id}/workers/{worker_id}/messages.jsonl`
  - Use `tools::configured_worker_tools()` including the shared `state` tool
  - Initialize `state` tool with the worker's scoped state file: `.ogent/sessions/{parent_session_id}/workers/{worker_id}/states.json`

**Resume behavior:**
- Resume loads `messages.jsonl` only (same as today)
- The Director can use `state` tool on its first turn to read `states.json` and orient itself
- No special rehydration or state injection into the transcript

### `src/tools.rs`

**Remove from main toolset:**
- `write_file`
- `read_hash_anchors`
- `edit_hash_anchors`
- `dispatch_worker`
- `start_workers`
- `check_workers`
- `set_goal`
- `revise_goal`
- `update_phase`
- `update_todo`
- `workflow_status`
- `workflow_enter_step`
- `workflow_record_check`
- `workflow_run_check`
- `complete`
- `worker_complete`

**Keep but move to worker-only:**
- `write_file`, `read_hash_anchors`, `edit_hash_anchors` — available to workers, not Director

**Preserve read-only skill tool:**
- `load_skill` remains available. Startup skill/context injection is also preserved.

**Add state tool:**
- `state({action, path, content})` where:
  - `action`: `"read" | "write" | "append" | "list"`
  - `path`: logical key inside the current agent's `states.json`
  - Director scope: `.ogent/sessions/{session_id}/states.json`
  - Worker scope: `.ogent/sessions/{parent_session_id}/workers/{worker_id}/states.json`
  - The state file is a JSON object mapping `path -> content`
  - `read` returns the key content or null
  - `list` returns matching keys; with an empty or omitted path, return all keys
  - `write` sets/replaces the key
  - `append` appends to the existing string value, or creates it if absent
  - Reject empty paths for read/write/append. `list` may use an empty path.
  - `state` is an internal mutating tool, but it only mutates ogent state, not the workspace. It is allowed for both Director and workers.

**Add Director bash allowlist:**
- Director `bash` must reject commands whose executable is not `colgrep` or `rg`.
- This is the code-level enforcement for "Director does not edit files."
- Worker `bash` keeps existing behavior.

**Add worker dispatch tools:**
- `dispatch_workers({workers})` where:
  - `workers`: array of `{role, task}`
  - Input shape is only `{ workers: Worker[] }`; there is no `sync: bool`.
  - Spawns all requested workers, waits for every worker in that batch to finish, then returns results.
  - The Director has no separate work to do while a batch is running, so there is no async polling API in V1.
  - Return order must match the input `workers` array order.
  - Each result must include the input index, role, worker ID, status, and the worker process's last assistant message.
  - Sequential chains are expressed as separate `dispatch_workers` calls after inspecting the previous batch results/state.

**Update `configured_coder_tools`:**
- Rename or replace with `configured_director_tools()`
- Returns: `read_file`, `repo_map`, restricted `bash`, `web_search`, `web_read`, `web_code_context`, `load_skill`, `state`, `dispatch_workers`

**Update `configured_worker_tools`:**
- Workers get: all read tools + `write_file`, `edit_hash_anchors`, `read_hash_anchors`, unrestricted `bash`, `repo_map`, `web_search`, `web_read`, `web_code_context`, `load_skill`, `state`
- Exclude from workers: `dispatch_workers` (workers cannot dispatch other workers)

### `src/workers.rs`

**Add structs:**
- `DispatchWorkersArgs`:
  ```rust
  struct DispatchWorkersArgs {
    workers: Vec<WorkerDispatch>,
  }
  struct WorkerDispatch {
    role: String,
    task: String,
  }
  ```
**Update WorkerManager:**
- `dispatch(&self, args: DispatchWorkersArgs) -> Result<String>`:
  - Validate `workers` is non-empty.
  - For each worker in `workers`, preserve its input index and requested role.
  - Resolve each role to prompts via `resolve_worker_prompts(role, task, "")`.
  - Spawn all workers in the batch.
  - Await every worker in the batch before returning.
  - Return structured JSON with `results` in the same order as the input array.
  - Each result includes `{index, role, worker_id, status, output, error}`.
  - `output` is the worker process's last assistant message.
  - If one worker fails, still collect all other worker results and mark the failed worker's result with `status: "failed"`.

**Update worker spawn:**
- `WorkerProcessArgs` gains `parent_session_id: String` and `worker_id: String`
- When spawning a worker subprocess:
  - Invoke `ogent --worker=<parent_session_id> <task_prompt>`
  - Set `OGENT_WORKER_ID`
  - The worker binary uses `<parent_session_id>` and `OGENT_WORKER_ID` to persist under `.ogent/sessions/{parent_session_id}/workers/{worker_id}/`

**Update `resolve_worker_prompts`:**
- If `role` matches builtin → use builtin prompt (same as today)
- If `role` is `"factory"` or unknown → use `CONTRACTOR_FACTORY` as the architect system prompt (instead of `ARCHITECT_PROMPT`)
- The user message to the architect is the worker's `task` (the hiring request)
- Parse output for `<system_prompt>` and `<task_prompt>` tags (same as today)

**Remove old WorkerManager methods:**
- `start()` → functionality merged into `dispatch()`
- `check()` → no longer exposed; batch `dispatch()` waits and returns results directly
- Or keep them internally only if useful, but expose only `dispatch_workers` to tools

### `src/agent.rs`

**Remove fields / old semantics:**
- `task_tracker` field — tools `set_goal`/`revise_goal`/`update_phase`/`update_todo` are removed, so this is dead code
- `workflow_state` field — YAML workflow engine is removed, so this is dead code
- Old `completion_summary` writes from `complete`/`worker_complete` tools. Completion is now based on terminal `state.status` for the Director and natural final assistant output for workers.

**Keep fields:**
- `compact` field stays — autocompact is preserved
- `worker_manager` field stays — Director dispatches workers
- `messages`, `tools`, `client`, `total_tokens`, `meta`, `dirty` all stay

**Update `finish_turn`:**
- After existing checks, read `status` from `.ogent/sessions/{session_id}/states.json`:
  ```rust
  if let Some(status) = read_state_key(&self.meta.session_id, "status")? {
    let status = status.trim().to_ascii_lowercase();
    if matches!(status.as_str(), "done" | "blocked" | "failed" | "partial") {
      self.completion_summary = self.last_assistant_message().or(Some(status.clone()));
      return Ok(true);
    }
  }
  ```
- If terminal status is set but no assistant message exists, fall back to the status string.
- In worker mode, do not require terminal status. When the worker loop naturally stops, print the worker's last assistant message to stdout so the parent Director receives it as the worker result.

**Preserve `steer_loop`:**
- Steer mode (`--steer`) stays fully intact as a TUI layer
- The agent underneath is now Director instead of worker
- `steer_loop` uses `configured_director_tools()` and Director system prompt
- User steers a Director that dispatches workers instead of editing files directly
- Remove `/complete` behavior. The Director finishes by writing terminal `status` via `state`, then sending the final answer as its last assistant message.

**Update `run_loop`:**
- Uses `configured_director_tools()` and Director system prompt

## Tool Availability Matrix

| Tool | Director (main agent) | Worker (subprocess) |
|---|---|---|
| `read_file` | yes | yes |
| `repo_map` | yes | yes |
| `bash` | yes, restricted to `colgrep`/`rg` | yes |
| `web_search` | yes | yes |
| `web_read` | yes | yes |
| `web_code_context` | yes | yes |
| `load_skill` | yes | yes |
| `state` | yes | yes, scoped to worker state |
| `dispatch_workers` | yes | **no** |
| `write_file` | **no** | yes |
| `read_hash_anchors` | **no** | yes |
| `edit_hash_anchors` | **no** | yes |

Removed entirely: `dispatch_worker`, `start_workers`, `check_workers`, `wait_workers`, `set_goal`, `revise_goal`, `update_phase`, `update_todo`, `workflow_status`, `workflow_enter_step`, `workflow_record_check`, `workflow_run_check`, `complete`, `worker_complete`.

## State Directory Layout

```
.ogent/
  sessions/
    {session_id}/
      meta.json              # existing
      messages.jsonl         # existing
      states.json            # Director state key/value map
      workers/
        {worker_id}/
          messages.jsonl      # worker transcript
          states.json         # worker state key/value map
  journal.md                   # existing
```

The `state` tool manages keys inside the current scope's `states.json`.

Recommended Director keys:
- `goal`
- `task_contract`
- `workflow`
- `status`
- `next_action`
- `risks`
- `decision_packet`
- `worker_batch_summary`
- `evidence`
- `ownership_map`

Recommended worker keys:
- `task_contract`
- `progress`
- `evidence`
- `files_changed`
- `risks`
- `summary`

## Verification Plan

1. `cargo fmt`
2. `cargo check`
3. `cargo test`

Tests to update/add:
- Old tool list tests: remove assertions for deleted tools (`complete`, `workflow_*`, etc.)
- New tool list tests: assert Director gets `state`, `dispatch_workers`; does not get `wait_workers`, `write_file`, `edit_hash_anchors`
- Worker tool list tests: assert workers get `write_file`, `edit_hash_anchors`, `state`; do not get `dispatch_workers`
- Director `bash` allowlist: accepts `colgrep` and `rg`; rejects mutating commands and other executables
- `state` round-trip: write → read → list → append
- `state` scope: Director writes `.ogent/sessions/{session_id}/states.json`; worker writes `.ogent/sessions/{parent_session_id}/workers/{worker_id}/states.json`
- `state` terminal status: exact `status` key values `done`, `blocked`, `failed`, `partial` exit; substrings do not
- `resolve_worker_prompts`: known role returns builtin, `"factory"` uses CONTRACTOR_FACTORY
- `dispatch_workers`: spawns the batch, waits for every worker, and returns results in input order with index/role/worker_id mapping
- Worker output: worker's last assistant message is returned to the parent Director
- Director output: Director's last assistant message is printed on terminal status
- Worker subprocess: `--worker=<parent_session_id>` and `OGENT_WORKER_ID` route transcript/state to the worker directory

## CLI Behavior

```bash
# One-shot director
ogent "fix the failing tests without overcomplicating"

# Steer-mode director (TUI steering)
ogent --steer

# Worker subprocess (internal, spawned by Director)
ogent --worker=<parent_session_id> <task_prompt>
```

There is no `--director` flag because ogent IS the director.

## Migration Notes

- This is a breaking change for anyone relying on ogent as a hands-on coding agent
- The old main-agent direct edit behavior is gone; workspace file edits must go through `dispatch_workers` with `role: "implementer"`
- Session transcript format is unchanged for the Director
- `.ogent/sessions/{session_id}/states.json` is new
- Worker transcripts move under `.ogent/sessions/{session_id}/workers/{worker_id}/messages.jsonl`

## Cleanup

Dead code, stale references, and orphaned artifacts to remove or update after the primary changes land.

### Dead modules

- `src/task_tracker.rs` — all consumers removed. Delete the file and `mod task_tracker` from `main.rs`.
- `src/workflow.rs` — all consumers removed. Delete the file and `mod workflow` from `main.rs`. This also removes `WorkflowState`, `ManualCheckInput`, `CheckStatus`, and all workflow validation logic.

### `src/session.rs`

- Delete `write_workflow_state()` and `read_workflow_state()`.
- Delete `use crate::workflow::WorkflowState` import.

### `src/agent.rs`

- Remove `task_tracker` field, `workflow_state` field, `complete_open_work_warned` field from `Agent` struct.
- Remove `task_tracker` and `workflow_state` params from `Agent::new()`.
- Remove `record_task_tracking_turn()`, `push_task_tracking_reminder()`.
- Remove `refresh_workflow_reminder()` — workflow state no longer exists. Delete the `WORKFLOW_MARKER` constant.
- Update `check_compact()` urgency messages: replace references to `complete` tool with the status/last-message exit rule.
- Remove `MANUAL_COMPLETE_REMINDER` if it is only used for `/complete`.
- Update `SteerEvent::New` handler (`agent.rs:627`): replace `configured_coder_tools(false)` with `configured_director_tools()`. Replace `prompts::build_messages("")` (already returns Director messages). Remove `self.workflow_state = None`. Remove `self.task_tracker = None`.
- Update `SteerEvent::Compact` handler: remove the `if let Some(tracker) = &self.task_tracker` block that serializes the task plan into the compact message.
- Remove `use crate::task_tracker::{TaskTracker, is_tracking_tool_name}` import.

### `src/tools.rs`

- Remove `use crate::task_tracker::{...}` import.
- Remove `use crate::workflow::{CheckStatus, ManualCheckInput}` import.
- Remove function implementations: `set_goal`, `revise_goal`, `update_phase`, `update_todo`, `workflow_status`, `workflow_enter_step`, `workflow_record_check`, `workflow_run_check`, `complete`, `worker_complete`, `dispatch_worker`.
- Remove `start_workers` and `check_workers` dispatch cases from `execute_tool`.
- Remove `build_workflow_tools()` function entirely.
- Remove `CODER_TOOLS_WITH_WORKFLOW` static. Remove `configured_coder_tools(workflow_enabled)` function.
- Keep `load_skill` as a read-only tool.
- Remove `is_tracking_tool_name` usage (function moves or is deleted with task_tracker).
- Remove `summary_has_limitation_and_intent()` helper (was only used by `complete`).
- Remove test assertions for deleted tools in `tools::tests`.
- Remove `WORKER_EXCLUDED` entries for deleted tools that no longer exist in any toolset. Update to exclude only `dispatch_workers` from workers.

### `src/prompts.rs`

- Remove `ARCHITECT_PROMPT` constant (replaced by `CONTRACTOR_FACTORY`).
- Remove `WORKER_PROMPT_CODER`, `WORKER_PROMPT_TESTER`, `WORKER_PROMPT_VALIDATOR` constants (replaced by new worker prompts).
- Keep `load_skill_content()` because both startup injection and the `load_skill` tool remain.
- Update `get_builtin_worker_prompt()` — remove old mappings (`coder`, `tester`, `validator`), add new mappings (`implementer`, `verifier`, `debugger`, `researcher`, `writer`, `critic`, `designer`, `summarizer`).

### `src/workers.rs`

- Remove `DispatchWorkerArgs`, `AsyncCoworkerArgs`, `StartWorkersArgs` structs (replaced by `DispatchWorkersArgs`).
- Remove `WorkerManager::start()` and `WorkerManager::check()` methods (replaced by batch `dispatch()`).
- Remove `validate_start_workers_args()` (replaced by validation in `dispatch()`).
- Remove `format_dispatch_worker_result()` if no longer called (check all call sites).

### `src/main.rs`

- Remove `--workflow` flag from `Args` struct.
- Keep `--create-skill` and its artifact creator path.
- Remove `--create-workflow` flag and its handler block.
- Keep `mod artifact_creator` if still needed for `--create-skill`.
- Remove `mod task_tracker` and `mod workflow` module declarations.
- Remove workflow state loading logic in the resume/fork branch.
- Update `Agent::new()` call sites to remove `task_tracker` and `workflow_state` args.

### Prompt files

- Delete `prompts/ARCHITECT_PROMPT.md` (replaced by `CONTRACTOR_FACTORY.md`).
- Delete `prompts/WORKFLOW_CREATOR_PROMPT.md` (workflow creation removed).
- Keep `prompts/SKILL_CREATOR_PROMPT.md` while `--create-skill` remains.
- Delete `prompts/workers/coder.md` (replaced by `implementer.md`).
- Delete `prompts/workers/tester.md` (replaced by `verifier.md`).
- Delete `prompts/workers/validator.md` (replaced by `verifier.md`).
- Delete `prompts/workers/generic.md` (no longer needed; all roles have explicit prompts or go through factory).

### `src/artifact_creator.rs`

- Keep the file while `--create-skill` remains.
- Remove only workflow-creation code if it becomes orphaned after deleting `--create-workflow`.

### Tests

- `src/agent.rs` tests: remove `complete_on_empty_session_stays_clean`, `complete_with_assistant_makes_dirty`, `new_clears_task_tracker` — these test removed functionality.
- `src/tools.rs` tests: remove `complete_schema_has_summary_required` and any `workflow_*` tool tests. Keep read-only classification coverage for `load_skill`.
- `src/workflow.rs` tests: deleted with the module.
- `src/task_tracker.rs` tests: deleted with the module.

### Documentation

- Remove workflow-related sections from `docs/reference.md`.
- Remove task-tracker tool docs from `docs/reference.md`.
- Update `docs/agent-guide.md` to remove workflow, task tracker, and old worker sections. Add Director state, dispatch, and worker sections.
- Update `docs/steer-mode.md` to reflect Director behavior (steer controls a Director, not a hands-on coder).
- Update `AGENTS.md` routing map to remove workflow and task tracker entries. Keep artifact creator routing for `--create-skill`; add Director state, dispatch, and new tool entries.
- Update `ARCHITECTURE.md` to reflect Director/worker split and new module boundaries.
- Update `README.md` to reflect Director-first usage.

## Open Issues

Known risks and unresolved design questions to resolve before or during implementation.

### `CONTRACTOR_FACTORY` output format vs architect parsing

`parse_architect_output` in `workers.rs` expects `<system_prompt>` and `<task_prompt>` XML tags. The `CONTRACTOR_FACTORY.md` prompt in `director-mode/prompts/` produces structured Markdown (Role, Scope, Task sections) without XML tags. The factory output must either:
1. Be updated to include `<system_prompt>` and `<task_prompt>` tags, or
2. Use a separate parse path, or
3. Bypass the architect round-trip entirely — the factory prompt IS the system prompt.

### `apply_patch` absent

The director-mode spec makes `apply_patch` the primary edit primitive. The plan has no `apply_patch` — workers use `edit_hash_anchors`. If `edit_hash_anchors` is the equivalent, state that. Otherwise, add `apply_patch` to worker tools.

### `hire_worker` folded into `dispatch_workers`

The spec defines `hire_worker` as a separate tool. The plan folds it into `dispatch_workers({role: "factory"})`. This is a good simplification but diverges from the spec. Document the rationale.

### Resume context waste

Resume loads the full `messages.jsonl` transcript. The new `states.json` `decision_packet` key gives the Director a compact orientation path, but there is still no special resume rehydration. Flag as a known V1 tradeoff.

### `ToolContext` needs session ID

The `state` tool needs Director vs worker scope. `ToolContext` currently only holds `Option<&mut Agent>`. Either extend `ToolContext` with explicit state scope, or derive it from `ctx.agent.meta` plus worker metadata captured from `--worker=<parent_session_id>` and `OGENT_WORKER_ID`.

## Future Work (out of scope)

- Worktree isolation for parallel implementers
- Resume from per-session state snapshots
- SQLite FTS over state
- Semantic retrieval
