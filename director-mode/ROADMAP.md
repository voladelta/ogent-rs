# Roadmap

## V1: Minimal Director runtime

- One CLI command: `director "<task prompt>"`
- Filesystem Markdown state
- JSONL event/decision logs
- Primitive tools only
- Batch worker dispatch with `dispatch_workers`
- Temporary worker creation with `hire_worker`
- Basic worker registry
- Review/verification as worker contracts
- Final report

## V1.5: Coding isolation

- Git worktree creation via `run_command`
- Workspace-aware tools with `cwd`
- Parallel implementer worktrees
- Integrator worker
- Final integrated verification

## V2: Search and resume

- Resume from snapshots
- Better state compaction
- `rg` over `.director/`
- Optional SQLite FTS as rebuildable index

## V3: Semantic retrieval

- Optional semantic index over repo and `.director/`
- Similar past task retrieval
- Prior decision retrieval

## V4: Stronger orchestration

- Better decomposition heuristics
- Failure pattern detection
- Automatic specialist hiring
- Multi-run goal tracking
- More robust integration workflows
