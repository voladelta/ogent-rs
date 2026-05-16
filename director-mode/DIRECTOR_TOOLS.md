# DIRECTOR_TOOLS.md

## Purpose

This document defines the primitive toolset for the Director runtime.

The tool layer should stay small. Most higher-level concepts such as contracts, evidence, decisions, reviews, failures, reports, checkpoints, and verification requests are not separate tools. They are protocol conventions stored in filesystem-like state or performed by workers.

## Design principle

Use primitive operations only.

```txt
tools = primitive operations
paths = meaning
files = memory
workers = processes
director = scheduler and judge
```

The runtime should not know too much about the Director's reasoning model.

The Director creates structure through:

- Markdown contracts
- state paths
- worker roles
- event logs
- evidence files
- final reports

## Primitive toolset

The Director runtime exposes exactly these tools:

```txt
tree
rg
read_file
write_file
apply_patch
run_command
state
dispatch_workers
hire_worker
wait_workers
```

That is the full primitive layer for v1.

---

# Workspace tools

Workspace tools should support `cwd` so the Director can use isolated worktrees.

```ts
type Cwd = {
  cwd?: string;
};
```

If `cwd` is omitted, use the run's root working directory.

## `tree`

Show a compact directory tree.

```ts
type TreeInput = {
  path?: string;
  cwd?: string;
};

type TreeOutput = {
  text: string;
};
```

Use this to understand the workspace shape before deeper inspection.

## `rg`

Search files using ripgrep-like semantics.

```ts
type RgInput = {
  pattern: string;
  path?: string;
  cwd?: string;
};

type RgOutput = {
  text: string;
};
```

Use `rg` as the main search primitive.

Do not add semantic search in v1. If needed later, make it an optional index over files.

## `read_file`

Read one file.

```ts
type ReadFileInput = {
  path: string;
  cwd?: string;
};

type ReadFileOutput = {
  content: string;
};
```

The runtime may enforce size limits.

## `write_file`

Write a file.

```ts
type WriteFileInput = {
  path: string;
  content: string;
  cwd?: string;
};

type WriteFileOutput = {
  path: string;
  bytes_written: number;
};
```

Use `write_file` for:

- new files
- generated artifacts
- reports
- snapshots
- prompts
- documentation

Avoid using `write_file` to replace existing source files unless the file is small and the replacement is clearly intended.

For existing source files, prefer `apply_patch`.

## `apply_patch`

Apply a patch.

```ts
type ApplyPatchInput = {
  patch: string;
  cwd?: string;
};

type ApplyPatchOutput = {
  applied: boolean;
  output: string;
};
```

Use `apply_patch` for normal code changes.

Patches are preferred because they are:

- auditable
- reviewable
- smaller
- easier to revert
- less likely to rewrite unrelated code

## `run_command`

Run a shell command.

```ts
type RunCommandInput = {
  command: string;
  cwd?: string;
};

type RunCommandOutput = {
  exit_code: number;
  stdout: string;
  stderr: string;
};
```

Use `run_command` for:

- tests
- builds
- linting
- typechecking
- benchmarks
- git status/diff/worktree/checkpoint
- scripts
- local probes

Examples:

```ts
run_command({ command: "git status --short" });
run_command({ command: "git diff" });
run_command({ command: "cargo test" });
run_command({ command: "go test ./..." });
run_command({ command: "pnpm typecheck" });
```

For worktrees:

```ts
run_command({
  command: "go test ./...",
  cwd: "../.director-worktrees/run-001-backend-001"
});
```

The Director should ask the user before destructive or irreversible commands.

Examples requiring caution:

```txt
rm -rf
git reset --hard
database migrations
deploy commands
secret/credential operations
production writes
```

---

# State tool

## `state`

Filesystem-like key/value state.

```ts
type StateInput = {
  action: "read" | "write" | "append" | "list";
  path: string;
  content?: string;
};

type StateOutput =
  | { action: "read"; path: string; content: string | null }
  | { action: "write"; path: string; bytes_written: number }
  | { action: "append"; path: string; bytes_written: number }
  | { action: "list"; path: string; entries: string[] };
```

State content is plain text. Store Markdown, JSONL, diffs, command output, or generated reports.

Do not require `kind`, `type`, `context`, or structured payloads in the primitive API. The path gives meaning.

Examples:

```ts
state({
  action: "write",
  path: "snapshots/goal/current.md",
  content: "Fix failing tests with the smallest correct change."
});
```

```ts
state({
  action: "append",
  path: "logs/events.jsonl",
  content: "{"event":"task.framed"}
"
});
```

```ts
state({
  action: "write",
  path: "workers/outputs/verifier-001.md",
  content: "pnpm test failed in parseConfig.test.ts."
});
```

## Recommended state layout

```txt
.director/
  snapshots/
    direction/current.md
    goal/current.md
    task/current.md
    workflow/current.md
    status/current.md
    next_action/current.md
    risks/current.md

  logs/
    events.jsonl
    decisions.jsonl

  contracts/
    task.md
    shared/
      api.md
      design-system.md
      paper-outline.md
    workers/
      implementer-001.md
      verifier-001.md
      reviewer-001.md

  workers/
    hired/
      rust-macro-reviewer-001.md
    outputs/
      verifier-001.md
      implementer-001.md
      reviewer-001.md

  artifacts/
    evidence/
      test-run-001.md
      benchmark-before.md
      benchmark-after.md
    patches/
      patch-001.diff
    reports/
      final.md
```

## State conventions

Snapshots represent current state. Use `write`.

Logs are append-only. Use `append`.

Contracts are structured Markdown files. Use `write`.

Worker outputs are Markdown files. Use `write`.

Evidence is Markdown or raw command output. Use `write`.

Reports are generated artifacts. Use `write`.

---

# Worker runtime tools

## `dispatch_workers`

Dispatch one or more existing workers.

```ts
type WorkerDispatch = {
  role: string;
  task: string; // structured Markdown
};

type DispatchWorkersInput = {
  workers: WorkerDispatch[];
  sync?: boolean;
};

type DispatchWorkersOutput =
  | {
      mode: "async";
      worker_ids: string[];
    }
  | {
      mode: "sync";
      results: WorkerResult[];
    };

type WorkerResult = {
  worker_id: string;
  role: string;
  output: string;
};
```

### Default: async parallel dispatch

If `sync` is omitted or false, workers are launched concurrently and `dispatch_workers` returns worker IDs.

```ts
dispatch_workers({
  workers: [
    {
      role: "implementer",
      task: "# Task
Implement backend config loading..."
    },
    {
      role: "implementer",
      task: "# Task
Implement frontend config UI..."
    },
    {
      role: "writer",
      task: "# Task
Update config documentation..."
    }
  ]
});
```

Then call:

```ts
wait_workers({
  worker_ids: ["implementer-001", "implementer-002", "writer-001"]
});
```

Use async parallel dispatch when contracts have non-overlapping ownership.

### Sequential dispatch

If `sync: true`, workers run sequentially in the order provided. The next worker starts only after the previous worker completes.

```ts
dispatch_workers({
  sync: true,
  workers: [
    {
      role: "debugger",
      task: "# Task
Find the root cause. Do not edit files."
    },
    {
      role: "implementer",
      task: "# Task
Fix the issue using the debugger output."
    },
    {
      role: "verifier",
      task: "# Task
Run relevant tests and report evidence."
    }
  ]
});
```

Use sequential dispatch when later workers depend on earlier outputs.

### Task format

`task` must be structural Markdown.

Default shape:

```md
# Task

What the worker should do.

# Goal

What success means.

# Constraints

- Hard rule 1
- Hard rule 2

# Owned scope

What this worker owns.

# Forbidden scope

What this worker must not touch.

# Inputs

Relevant files, state paths, command outputs, diffs, previous worker outputs, or assumptions.

# Required output

Exact output expected from the worker.

# Failure conditions

When the worker should stop and report blocked.
```

## `hire_worker`

Create a temporary specialist worker from a Markdown task.

```ts
type HireWorkerInput = {
  task: string; // structured Markdown
};

type HireWorkerOutput = {
  worker_id: string;
};
```

Use `hire_worker` when no existing worker role fits the task.

Example:

```ts
hire_worker({
  task: `
# Hire a temporary worker

Create a specialist reviewer for Rust macro_rules code.

# Specialist role

Rust macro_rules reviewer.

# Purpose

Review a macro implementation for:

- hygiene
- ambiguity
- diagnostics
- compile-time behavior
- API ergonomics

# Constraints

- Do not edit files.
- Do not suggest broad rewrites unless required.
- Focus on blocking issues first.

# Required output

Return the generated system prompt and review result.
`
});
```

The hired worker's prompt should be stored in state:

```txt
workers/hired/<worker-id>.md
```

## `wait_workers`

Wait for workers to complete.

```ts
type WaitWorkersInput = {
  worker_ids?: string[];
};

type WaitWorkersOutput = {
  completed: WorkerResult[];
  pending: string[];
  failed: WorkerFailure[];
};

type WorkerFailure = {
  worker_id: string;
  error: string;
};
```

Wait for all active workers:

```ts
wait_workers({});
```

Wait for specific workers:

```ts
wait_workers({
  worker_ids: ["verifier-001", "reviewer-001"]
});
```

---

# What is not a primitive tool

Do not add these as runtime primitives:

```txt
dispatch_worker
create_contract
close_contract
record_evidence
record_failure
record_decision
request_review
request_verification
git_status
git_diff
git_checkpoint
final_report
```

They are represented as conventions:

```txt
single dispatch = dispatch_workers with one item
contract = state write
evidence = state write
failure = state append
decision = state append
review = dispatch_workers(role: "reviewer")
verification = dispatch_workers(role: "verifier")
git = run_command
final_report = state write + Director response
```

---

# Coding worktree policy

For coding tasks, prefer isolated worktrees for implementers.

Rules:

```txt
Read-only workers may share the main workspace.
Single trivial edits may use the main workspace.
Non-trivial implementers should use worktrees.
Parallel implementers must use worktrees.
Integrated result must be verified again.
```

Worktree creation is done with `run_command`:

```ts
run_command({
  command: "git worktree add ../.director-worktrees/run-001-backend-001 -b director/run-001/backend-001"
});
```

Worker commands then use `cwd`:

```ts
run_command({
  command: "go test ./...",
  cwd: "../.director-worktrees/run-001-backend-001"
});
```

---

# Final tool list

```txt
tree
rg
read_file
write_file
apply_patch
run_command
state
dispatch_workers
hire_worker
wait_workers
```

This is the complete primitive toolset for v1.
