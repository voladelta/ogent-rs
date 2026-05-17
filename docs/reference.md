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

- `read_file`
- `repo_map`
- `bash` (allowlisted to `colgrep` and `rg`)
- `web_search`
- `web_read`
- `web_code_context`
- `load_skill`
- `state`
- `dispatch_workers`

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

Workers do **not** get `dispatch_workers`.

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
- Waits for all workers in that batch.
- Returns ordered results matching input order.

Output shape:

```json
{
  "results": [
    {
      "index": 0,
      "role": "implementer",
      "worker_id": "worker-1",
      "status": "completed | failed",
      "output": "last assistant message",
      "error": null
    }
  ]
}
```

## Terminal Completion

Director exits when state key `status` is exactly:

- `done`
- `blocked`
- `failed`
- `partial`

The final user-facing output is the Director’s last assistant message.
