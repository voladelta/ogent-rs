# Reference

## CLI Flags

| Flag | Meaning |
| --- | --- |
| `--profile <name>` | Model/profile selection |
| `--steer` | Start TUI steer mode (Director) |
| `--worker=<parent_session_id>` | Internal worker subprocess mode |
| `--autocompact <percent>` | Auto-compaction threshold (`-1` disables) |
| `--resume[=<session_id>]` | Resume existing session |
| `--fork[=<session_id>]` | Fork existing session into a child session |
| `--temp` | Ephemeral mode (no session persistence) |
| `--create-skill <name>` | Generate/update `.ogent/skills/<name>/SKILL.md` |

`--create-skill` cannot be combined with `--resume`, `--fork`, `--worker`, or `--steer`.

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
- State storage:
  - Director: `.ogent/sessions/{session_id}/states.json`
  - Worker: `.ogent/sessions/{parent_session_id}/workers/{worker_id}/states.json`

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
      "status": "running"
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
      "status": "running"
    }
  ]
}
```

## Terminal Completion

A run ends when the Director sends a final assistant message (no tool calls). The final user-facing output is the Director’s last assistant message.
