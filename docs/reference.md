# Reference

## CLI Flags

| Flag | Meaning |
| --- | --- |
| `--profile <name>` | Model/profile selection |
| `--autocompact <percent>` | Auto-compaction threshold (`-1` disables) |
| `--resume[=<session_id>]` | Resume existing session |
| `--fork[=<session_id>]` | Fork existing session into a child session |
| `--temp` | Ephemeral mode (no session persistence) |
| `--create-skill <name>` | Generate/update `.ogent/skills/<name>/SKILL.md` |
| `--serve <addr>` | WebSocket server mode (`ws://<addr>`) |

`--create-skill` cannot be combined with `--resume`, `--fork`, or `--serve`.
`--serve` cannot be combined with `--resume`, `--fork`, or an initial prompt.

## WebSocket Protocol (`--serve`)

Each websocket connection starts unbound. It does not create an Agent/session until setup succeeds.

Inbound JSON:

- `{"type":"start","repo":"/path/to/repo","temp":true,"profile":"ds-flash","autocompact":80}`
- `{"type":"fork","repo":"/path/to/repo","session":"<session_id>","profile":"ds-flash","autocompact":80}`
- `{"type":"resume","repo":"/path/to/repo","session":"<session_id>","profile":"ds-flash","autocompact":80}`
- `{"type":"message","content":"..."}`
- `{"type":"cancel"}`
- `{"type":"new"}`
- `{"type":"compact","focus":"optional focus task"}`
- `{"type":"profile","profile":"ds-flash"}`
- `{"type":"exit"}`

Setup rules:

- `start`, `fork`, and `resume` require `repo`.
- `repo` is canonicalized and must exist as a directory.
- `temp` is valid only on `start`; `fork`/`resume` reject it.
- `profile` and `autocompact` are valid on `start`/`fork`/`resume`; omitted values default to server startup values.
- Before setup, non-setup events are rejected with `error.code = "not_initialized"`.
- After setup, `start`/`fork`/`resume` are rejected with `error.code = "already_initialized"`.
- `resume` is rejected with `error.code = "session_active"` if that session is already active in this websocket server process.

Websocket active-session protection:

- The server keeps an in-process active session registry for websocket runs.
- `start` and `fork` create and register fresh session IDs.
- `resume` registers the target session only if it is not already active.
- IDs are unregistered when the connection/agent ends.

Outbound JSON:

- `session`: setup success payload:
  - `status`: `"ok"`
  - `session_id`
  - `mode`: `"start" | "fork" | "resume"`
  - `profile`
  - `repo`
- `status`: current agent state/tokens/profile/model
- `message`: transcript message from the Director or a worker:
  - `source`: `"director"` or a worker id such as `"worker-1"`
  - `role`: transcript role such as `"assistant"` or `"tool"`
  - `content`
  - `reasoning_content`
  - `tool_calls`
  - `tool_call_id`
- `error`: protocol/runtime error with machine-readable `code` and human-readable `message`

Disconnect behavior:

- The connection is treated as exit.
- Dirty session data is persisted unless `--temp`.
- In-flight request is cancelled via steer-loop exit path.

Known limitation:

- Tool execution, workers, state, and session files are scoped to the setup `repo`; skill discovery and custom system prompt discovery still use the server startup cwd and home config.

## Resume Locking

`--resume` acquires `{workspace_root}/.ogent/sessions/{session_id}/active.lock` for the process lifetime. A second resume attempt for the same active session fails fast.

## Director Tools

- `repo_map`
- `bash` (allowlisted to `colgrep` and `rg`)
- `load_skill`
- `state`
- `dispatch_workers`
- `wait_workers`

## Worker Tools

- `read_file`
- `write_file`
- `read_hash_anchors`
- `edit_hash_anchors`
- `repo_map`
- `bash` (normal behavior)
- `web_search`
- `web_read`
- `web_code_context`
- `load_skill`
- `state`

Workers do **not** get `dispatch_workers` or `wait_workers`.

## `state` Tool

Arguments:

```json
{
  "action": "read | write | append | list",
  "path": "string",
  "content": "string"
}
```

Rules:

- `read`, `write`, `append` require non-empty `path`.
- `write` and `append` require `content`.
- `list` allows empty `path`; empty means list all keys.
- Workers use state key `progress/current` for concise current-phase progress. `wait_workers` reads this key for running workers.
- State storage:
  - Director: `{workspace_root}/.ogent/sessions/{session_id}/states.json`
  - Worker: `{workspace_root}/.ogent/sessions/{parent_session_id}/workers/{worker_id}/states.json`

## `dispatch_workers` Tool

Input:

```json
{
  "workers": [
    { "role": "implementer", "task": "..." },
    { "role": "verifier", "task": "..." }
  ]
}
```

Behavior:

- Spawns the full batch.
- Returns worker IDs immediately.
- Does not wait for worker completion.
- Each running worker status includes `progress`, initially `Starting`.
- `completed` is usually empty; it contains only dispatch-time failures such as invalid worker arguments or prompt-resolution errors.
- Call `wait_workers` to collect completed worker results.

Output shape:

```json
{
  "message": "Workers dispatched successfully. Their results are not available yet. Next action: call `wait_workers`.",
  "batch_id": "batch-1",
  "workers": [
    {
      "batch_id": "batch-1",
      "index": 0,
      "role": "implementer",
      "worker_id": "worker-1",
      "status": "running",
      "progress": "Starting"
    }
  ],
  "completed": [
    {
      "batch_id": "batch-1",
      "index": 0,
      "role": "implementer",
      "worker_id": "worker-1",
      "status": "failed",
      "output": "",
      "error": "dispatch-time error"
    }
  ]
}
```

## `wait_workers` Tool

Input:

```json
{}
```

Behavior:

- Returns immediately if any unseen worker result is available.
- Otherwise waits about 10 seconds.
- If no worker finishes during that wait, returns the still-running workers.
- Running worker statuses include `progress`.
- `progress` is read from the worker state key `progress/current`.
- If that key is missing, empty, unreadable, or malformed, `progress` is `Starting`.
- Repeat until the workers needed for the Director decision have completed.

Output shape:

```json
{
  "message": "Completed workers are available. Some workers are still running; call `wait_workers` again before depending on unfinished workers.",
  "completed": [
    {
      "batch_id": "batch-1",
      "index": 0,
      "role": "implementer",
      "worker_id": "worker-1",
      "status": "completed | failed",
      "output": "last assistant message",
      "error": null
    }
  ],
  "running": [
    {
      "batch_id": "batch-1",
      "index": 1,
      "role": "verifier",
      "worker_id": "worker-2",
      "status": "running",
      "progress": "Reading subscription schemas"
    }
  ]
}
```

## Terminal Completion

A run ends when the Director sends a final assistant message (no tool calls). The final user-facing output is the Director’s last assistant message.
