# ogent

`ogent` is currently a single worker-runtime coding agent.

## Quick Start

1. Rename the `dotogent` boilerplate directory to `.ogent` at the repo level or to `~/.ogent` for global config. It contains `config.yaml` and the colgrep skill.
2. Edit `.ogent/config.yaml` or `~/.ogent/config.yaml` for your models and providers.
3. Set the required API keys:

```bash
export DEEPSEEK_API_KEY="sk-..."
export EXA_API_KEY="..."
```

4. Run:

```bash
cargo run -- "Fix the failing tests without overcomplicating"
```

## CLI

```bash
# Worker run with explicit role
ogent --role reviewer --profile kimi "Review the staged diff"

# Worker run with default role (ogent)
ogent "Implement feature X"
```

## Key Behavior

- Prompted runs execute in worker mode.
- `--role <role>` selects the worker role explicitly; without it, role defaults to `ogent`.
- Worker tools include editing tools (`write_file`, `read_hash_anchors`, `edit_hash_anchors`) plus read/web/bash/state tools.
- Director, websocket, nested-worker, and skill-creator CLI entrypoints have been removed from the active runtime.

## Runtime Layout

```txt
{workspace_root}/.ogent/
  sessions/
    {session_id}/
      meta.json
      messages.jsonl
      states.json
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
