# PARALLEL_WORK.md

## Purpose

The Director may dispatch multiple workers at once when a task can be decomposed into non-overlapping chunks.

Parallelism is powerful, but only when the Director prevents overlap, drift, and integration failure.

## Core rule

```txt
One worker owns one bounded contract.
The Director owns the whole goal.
An Integrator owns the merge.
A Verifier checks the final combined outcome.
```

Workers should not all "help with the task." They should each own a clear slice.

## Good decomposition

Bad:

```txt
Worker A: Improve frontend
Worker B: Improve frontend
Worker C: Make UI better
```

Good:

```txt
Worker A: Implement frontend API client
Worker B: Implement backend endpoint
Worker C: Update UI loading/error states
Worker D: Review integration contract
```

Each worker needs:

```txt
owned scope
forbidden scope
interface contract
expected output
integration notes
```

## Parallel Director loop

```txt
1. Frame global task
2. Decompose into independent work packages
3. Write shared contracts if needed
4. Dispatch workers concurrently with dispatch_workers
5. Wait for results
6. Review each result locally
7. Integrate results globally
8. Verify whole task
9. Revise only failed slices
10. Final report
```

## Dispatch primitive

Use:

```ts
dispatch_workers({
  workers: [
    { role: "implementer", task: "# Task
Backend work..." },
    { role: "implementer", task: "# Task
Frontend work..." },
    { role: "writer", task: "# Task
Docs work..." }
  ]
});
```

Default is async parallel dispatch.

Use `sync: true` for sequential chains:

```ts
dispatch_workers({
  sync: true,
  workers: [
    { role: "debugger", task: "# Task
Find root cause." },
    { role: "implementer", task: "# Task
Fix the root cause." },
    { role: "verifier", task: "# Task
Verify the fix." }
  ]
});
```

## Coding isolation

For coding tasks, do not let multiple implementers mutate the same working tree at the same time.

Recommended rule:

```txt
Read/review workers can share the main workspace.
Implementer workers should use isolated worktrees.
Parallel implementers must use isolated worktrees.
```

Patch diffs are still useful, but as artifacts. Workers should have a real filesystem where they can edit and run checks.

## Worktree flow

```txt
main repo
  ↓
Director creates isolated worker worktree
  ↓
Worker edits inside its own worktree
  ↓
Worker runs checks inside its own worktree
  ↓
Worker returns branch/worktree summary + diff
  ↓
Integrator merges selected work into main/integration worktree
  ↓
Verifier checks final integrated result
```

Example worktree:

```txt
../.director-worktrees/run-001-backend-001
branch: director/run-001/backend-001
```

Create with `run_command`:

```ts
run_command({
  command: "git worktree add ../.director-worktrees/run-001-backend-001 -b director/run-001/backend-001"
});
```

Run checks with `cwd`:

```ts
run_command({
  command: "go test ./...",
  cwd: "../.director-worktrees/run-001-backend-001"
});
```

## File ownership contract

Example backend contract:

```md
# Task

Implement backend config loading.

# Owned scope

You may modify:

- src/config/**
- src/server/**
- tests/config/**

# Forbidden scope

Do not modify:

- src/ui/**
- docs/**

# Required output

Return:

1. Worktree path
2. Branch name
3. Files changed
4. Commands run
5. Test result
6. Git diff
7. Risks
```

Example frontend contract:

```md
# Task

Implement frontend config UI.

# Owned scope

You may modify:

- src/ui/config/**
- src/api/configClient.ts
- tests/ui/config/**

# Forbidden scope

Do not modify:

- src/server/**
- src/config/**
```

## Shared interface contracts

When workers must connect, write a shared interface first.

Example:

```txt
contracts/shared/config-api.md
```

Both frontend and backend workers reference the same file.

## Writing parallelism

Parallel writing works when the Director owns the outline.

Bad:

```txt
Worker A: write about the topic
Worker B: write about the topic
Worker C: write about the topic
```

Good:

```txt
Director creates outline first.
Worker A: Introduction
Worker B: Literature review
Worker C: Case analysis
Worker D: Counterarguments
Worker E: Conclusion
Worker F: Editor/Integrator
```

Without an editor/integrator, the result will feel stitched together.

## Safe vs risky parallelism

Safe to parallelize:

```txt
research
review
brainstorming
chapter drafting
design exploration
test investigation
```

Be careful with:

```txt
code edits
schema changes
public API changes
shared state changes
large refactors
```

## Final verification

A worker can be right locally and wrong globally.

Always verify the integrated result.

Pattern:

```txt
local review → local verification → integration → global verification
```
