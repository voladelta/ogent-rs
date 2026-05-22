# Reference

## CLI Flags

| Flag | Meaning |
| --- | --- |
| `--profile <name>` | Model/profile selection, overriding `config.yaml` |
| `--autocompact <percent>` | Auto-compaction threshold; `-1` disables it |
| `--role <role>` | Run with an explicit worker role; default is `ogent` |

## Worker Run

`ogent` starts one worker-mode agent.

```bash
ogent "Fix the failing parser test"
ogent --role reviewer --profile kimi "Review the staged diff"
```

Worker runs:

- resolve the requested role through built-in worker prompts
- expose the role's scoped worker tool group
- run in temporary mode, so transcript and metadata are not persisted by `persist_if_dirty`
- write state under `.ogent/sessions/{session_id}/states.json` when the `state` tool is used
- exit when the model sends a final assistant message with no tool calls

## Worker Tool Groups

`ogent` receives all worker tools. Specialist roles receive smaller capability groups:

- `implementer`: repo/code read tools, file write/edit tools, `bash`, `web_code_context`, `state`, `load_skill`
- `debugger`: repo/code read tools, `bash`, `web_code_context`, `state`, `load_skill`
- `reviewer`: repo/code read tools, `bash`, `state`, `load_skill`
- `verifier`: repo/code read tools, `bash`, web read/search tools, `state`, `load_skill`
- `researcher`: `read_file`, `write_file`, web tools, `state`, `load_skill`
- `writer`, `visual_designer`: `read_file`, `write_file`, web read/search tools, `state`, `load_skill`
- `system_architect`, `database_architect`: repo/code read tools, `write_file`, `state`, `load_skill`
- `summarizer`: `read_file`, `write_file`, `state`, `load_skill`

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
