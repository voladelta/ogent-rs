# ogent

`ogent` is a Director-first coding agent with a direct one-off worker mode.

The main agent is the Director. It frames the task, inspects the repo, manages state, dispatches workers, integrates results, and reports the outcome. Workspace edits are done by workers.

## Quick Start

1. Copy `config.yaml.sample` to your repo's `.ogent/config.yaml` or to `~/.ogent/config.yaml`.
2. Set the required API keys:

```bash
export DEEPSEEK_API_KEY="sk-..."
export EXA_API_KEY="..."
```

3. Run:

```bash
cargo run -- "Fix the failing tests without overcomplicating"
```

## CLI

```bash
# Director run
ogent "Implement feature X"

# Direct one-off worker run
ogent --run reviewer --profile kimi "Review the staged diff"

# WebSocket server mode
ogent --serve 127.0.0.1:9876

```

## WebSocket Mode

`--serve` starts a localhost-friendly WebSocket server. Each connection starts unbound and creates exactly one Director after a setup message:

```json
{"type":"start","repo":"/path/to/repo","profile":"ds-flash","autocompact":80}
{"type":"fork","repo":"/path/to/repo","session":"<session_id>","profile":"ds-flash"}
{"type":"resume","repo":"/path/to/repo","session":"<session_id>","profile":"ds-flash"}
```

After setup, send normal control messages:

```json
{"type":"message","content":"Fix the failing tests"}
{"type":"cancel"}
{"type":"compact","focus":"handoff for next step"}
{"type":"exit"}
```

Each WebSocket Director owns an immutable workspace root from `repo`. Tools, workers, state, and session files are scoped to that workspace, so one server process can host connections for different repos.

## Key Behavior

- Director tools include: `repo_map`, `code_map`, restricted `bash` (`colgrep`/`rg` only), `load_skill`, `state`, `set_title`, `dispatch_workers`, `wait_workers`.
- Worker tools include editing tools (`write_file`, `read_hash_anchors`, `edit_hash_anchors`) plus read/web/bash/state tools.
- `--run <role>` starts one worker-mode agent directly with the worker prompt and worker tools. It bypasses the Director and uses temporary session mode.
- `dispatch_workers` starts a worker batch and returns worker IDs immediately.
- `wait_workers` returns completed worker results as soon as any worker finishes, or reports still-running workers after a short wait.
- Running worker statuses include `progress`. Workers are prompted to write `progress/current` in their state; until they do, progress is `Starting`.
- A run ends when the Director sends a final assistant message (no tool calls).
- WebSocket `resume` rejects duplicate active sessions inside the same serve process.

## Runtime Layout

```txt
{workspace_root}/.ogent/
  sessions/
    {session_id}/
      meta.json
      messages.jsonl
      states.json
      workers/
        {worker_id}/
          messages.jsonl
          states.json
```

## Skill Creator

`--create-skill` is available:

```bash
ogent --create-skill repo-audit "Review repositories for correctness and maintainability."
```

## Development

```bash
cargo fmt
cargo check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## Docs

- [docs/reference.md](docs/reference.md)
- [docs/agent-guide.md](docs/agent-guide.md)
- [ARCHITECTURE.md](ARCHITECTURE.md)
