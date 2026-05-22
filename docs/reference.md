# Reference

## CLI Flags

| Flag | Meaning |
| --- | --- |
| `--profile <name>` | Model/profile selection, overriding `config.yaml` |
| `--autocompact <percent>` | Auto-compaction threshold; `-1` disables it |

## Worker Run

`ogent` starts one worker-mode agent.

```bash
ogent "Fix the failing parser test"
ogent --profile kimi "Review the staged diff"
```

Worker runs:

- use the root `SYSTEM_PROMPT.md`
- expose the full worker toolset
- run in temporary mode, so transcript and metadata are not persisted by `persist_if_dirty`
- write state under `.ogent/sessions/{session_id}/states.json` when the `state` tool is used
- exit when the model sends a final assistant message with no tool calls

## Worker Tools

Every worker run receives the same full worker toolset:

Available worker tools are:

- `read_file`
- `write_file`
- `bash`
- `repo_map`
- `code_map`
- `read_hash_anchors`
- `edit_hash_anchors`
- `web_search`
- `web_read`
- `web_code_context`
- `load_skill`
- `state`

Director orchestration tools such as `dispatch_workers`, `wait_workers`, `inspect_worker`,
`cancel_workers`, and `set_title` are not part of the active CLI runtime.

## Runtime State

```txt
{workspace_root}/.ogent/
  sessions/
    {session_id}/
      meta.json
      messages.jsonl
      states.json
```

Direct CLI runs do not persist `messages.jsonl` or `meta.json`. `states.json` is worker-owned runtime state, including `progress/current` when the worker reports progress.
