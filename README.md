# ogent

`ogent` is a Director-mode terminal agent.

The main agent does not edit workspace files directly. It plans work, manages state, dispatches workers, integrates results, and exits when a terminal status is written.

## Quick Start

```bash
export DEEPSEEK_API_KEY="sk-..."

cargo run -- "Fix the failing tests without overcomplicating"
```

## CLI

```bash
# Director run
ogent "Implement feature X"

# Steer-mode Director (TUI)
ogent --steer

# Internal worker subprocess mode
ogent --worker=<parent_session_id> "<task prompt>"
```

## Key Behavior

- Director tools include: `read_file`, `repo_map`, restricted `bash` (`colgrep`/`rg` only), web tools, `load_skill`, `state`, `dispatch_workers`.
- Worker tools include editing tools (`write_file`, `read_hash_anchors`, `edit_hash_anchors`) plus read/web/bash/state tools.
- Director exits only when state key `status` is exactly one of:
  - `done`
  - `blocked`
  - `failed`
  - `partial`

## Runtime Layout

```txt
.ogent/
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

`--create-skill` remains available:

```bash
ogent --create-skill repo-audit "Review repositories for correctness and maintainability."
```

## Development

```bash
cargo fmt
cargo check
cargo test
```

## Docs

- [docs/reference.md](docs/reference.md)
- [docs/agent-guide.md](docs/agent-guide.md)
- [docs/steer-mode.md](docs/steer-mode.md)
- [ARCHITECTURE.md](ARCHITECTURE.md)
