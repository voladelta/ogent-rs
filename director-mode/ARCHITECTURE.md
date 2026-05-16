# Architecture

## 1. Overview

The Director runtime is a small control system around an LLM.

It does not need a large framework. It needs clean boundaries.

```txt
CLI
 ↓
Director Runtime
 ↓
Task Framer
Workflow Planner
State Store
Worker Dispatcher
Worker Registry
Hired Worker Factory
Decision Engine
Reporter
 ↓
Primitive Tools / Files / Shell / Model calls
```

## 2. Components

### 2.1 CLI

Responsible for:

- parsing command arguments
- setting cwd/model/budget/iteration limits
- creating or resuming a run
- printing final report

Primary command:

```bash
director "<task prompt>" --model=gpt-5.5
```

### 2.2 Director Runtime

The main loop.

Responsibilities:

- load state
- frame the task
- design workflow
- decompose work
- dispatch workers
- hire temporary workers when needed
- collect outputs
- request review/verification
- integrate results
- decide next action
- update snapshots/logs
- produce final report

### 2.3 Task Framer

Converts messy prompt into a task contract.

Input:

```txt
original user prompt
repo/document/context hints
runtime options
```

Output:

```txt
goal
definition_of_done
constraints
non_goals
risks
required_evidence
```

Write the task contract to:

```txt
contracts/task.md
```

### 2.4 Workflow Planner

Designs an adaptive workflow.

It should not produce a rigid pipeline. It should produce a starting plan and update it as evidence arrives.

Example workflow:

```txt
inspect → implement → review → verify → revise/report
```

Parallel workflow:

```txt
inspect → decompose → dispatch workers → wait → integrate → verify → report
```

### 2.5 State Store

State is a filesystem-like store.

Canonical truth is Markdown/JSONL files under `.director/`.

The runtime exposes one primitive:

```txt
state({ action, path, content })
```

Use:

```txt
write  = snapshots/contracts/reports
append = event logs/decision logs
list   = discover worker outputs/artifacts
read   = resume current state
```

### 2.6 Worker Registry

Stores reusable worker prompts.

Recommended path:

```txt
prompts/workers/
```

Each worker prompt should specify:

- purpose
- default behavior
- forbidden moves
- expected input contract
- output format
- failure modes

Tool permissions do not need to be passed as separate dispatch fields in v1. The worker's task Markdown should state boundaries and constraints.

### 2.7 Hired Worker Factory

Creates temporary specialist prompts on demand.

Hired workers should be:

- narrow
- scoped
- disposable
- contract-bound

A hired worker should expire after the task or subtask.

### 2.8 Worker Dispatcher

Dispatches one or more workers.

Primitive:

```ts
dispatch_workers({ workers, sync })
```

Default behavior is asynchronous parallel dispatch.

If `sync: true`, workers run sequentially in array order. The next worker starts only after the previous worker completes.

Use sequential dispatch when later workers need earlier worker outputs.

Use parallel dispatch when scopes do not overlap.

### 2.9 Decision Engine

Compares current state and evidence against the task contract.

Possible decisions:

```txt
continue
revise
hire_worker
review
verify
integrate
ask_user
accept
block
fail
```

These are decisions, not primitive tools.

### 2.10 Reporter

Produces concise user-facing output.

The report should not be a raw transcript.

It should include:

- result status
- work completed
- verification/evidence
- files/artifacts
- risks
- next step if needed

## 3. Suggested module layout

Language-neutral shape:

```txt
src/
  cli
  director
  state
  dispatcher
  workers
  hired_workers
  tools
  reporting
```

For Rust:

```txt
src/
  main.rs
  cli.rs
  director.rs
  state.rs
  dispatcher.rs
  workers.rs
  hired_workers.rs
  tools.rs
  reporting.rs
```

For Go:

```txt
cmd/director/main.go
internal/director
internal/state
internal/dispatcher
internal/workers
internal/hiredworkers
internal/tools
internal/reporting
```

For JS/TS:

```txt
src/
  cli.ts
  director.ts
  state.ts
  dispatcher.ts
  workers.ts
  hiredWorkers.ts
  tools.ts
  reporting.ts
```

## 4. Data flow

```txt
User prompt
  ↓
TaskFramer.frame()
  ↓
state.write("contracts/task.md")
  ↓
WorkflowPlanner.plan()
  ↓
dispatch_workers()
  ↓
wait_workers()
  ↓
state.write("workers/outputs/...")
  ↓
Director evaluates evidence
  ↓
Loop or Report
```

## 5. Pseudocode

```ts
async function runDirector(prompt, options) {
  const run = await initializeRun(prompt, options);

  await state.write("contracts/task.md", await frameTask(prompt, options));
  await state.write("snapshots/workflow/current.md", await designWorkflow(run));

  for (let i = 0; i < options.maxIterations; i++) {
    const next = await chooseNextMove(run);

    if (next.kind === "dispatch") {
      const started = await dispatch_workers({
        workers: next.workers,
        sync: next.sync ?? false,
      });

      const results = next.sync
        ? started.results
        : await wait_workers({ worker_ids: started.worker_ids });

      await recordWorkerResults(results);
    }

    if (next.kind === "hire") {
      await hire_worker({ task: next.task });
    }

    if (next.kind === "state") {
      await state(next.input);
    }

    const decision = await decide(run);

    if (decision.kind === "accept") return report("done", run);
    if (decision.kind === "block") return report("blocked", run);
    if (decision.kind === "fail") return report("failed", run);

    await updateSnapshots(run, decision);
  }

  return report("partial", run);
}
```

## 6. Coding isolation policy

For coding tasks:

- read-only workers may share the main workspace
- implementer workers should use isolated worktrees when the task is non-trivial
- parallel implementers must use isolated worktrees
- patches/diffs are artifacts, not the only execution environment
- the integrated result must be verified again

Worktree operations are performed through `run_command`.

Workspace-aware file and command tools may accept `cwd`.

## 7. Failure handling

On failure, record:

```txt
what was tried
what happened
why it failed
what changed in understanding
next recommended action
```

After repeated failures, change strategy. Do not loop the same move.

Recommended rule:

```txt
After two failed attempts in the same direction, reframe or hire a specialist.
```

## 8. Design constraint

Keep the runtime boring.

The intelligence should be in:

- task contracts
- worker prompts
- Markdown state
- evidence collection
- decision policy

Not in a complex framework.
