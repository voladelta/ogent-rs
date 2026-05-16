# STATE.md

## Purpose

The Director tracks long-running goals through filesystem-like state.

The Director should not rely on chat history. It should resume from a task dossier made of Markdown snapshots, append-only logs, contracts, worker outputs, evidence, and final reports.

## Core model

Use two kinds of state:

```txt
1. Append-only logs = what happened
2. Current snapshots = what matters now
```

Snapshots answer:

```txt
Where are we now?
What matters?
What is next?
```

Logs answer:

```txt
What happened?
Why did we choose this?
What changed?
```

## Minimal layout

```txt
.director/
  direction.md
  goal.md
  workflow.md
  status.md
  next_action.md
  events.jsonl
  decisions.jsonl
```

## Recommended layout

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
    decision_packet/current.md
    worker_batch_summary/current.md
    evidence/current.md
    ownership_map/current.md

  logs/
    events.jsonl
    decisions.jsonl

  contracts/
    task.md
    shared/
    workers/

  workers/
    hired/
    outputs/

  artifacts/
    evidence/
    patches/
    reports/
```

## Direction

Long-lived north star.

Path:

```txt
snapshots/direction/current.md
```

Example:

```md
# Direction

Build a minimal Director agent runtime.

# Principles

- One user command: director "<task prompt>"
- Runtime primitives stay small
- State is filesystem-like
- Tasks are structural Markdown
- Workers are processes
- Director designs workflow and judges evidence
- Verification depends on discipline
- Avoid productized primitives too early
```

## Goal

The current outcome being pursued.

Path:

```txt
snapshots/goal/current.md
```

## Task

The current executable unit.

Path:

```txt
snapshots/task/current.md
```

## Workflow

The current plan.

Path:

```txt
snapshots/workflow/current.md
```

## Status

Current execution state.

Path:

```txt
snapshots/status/current.md
```

## Next action

The current frontier.

Path:

```txt
snapshots/next_action/current.md
```

This file is critical. A Director should always be able to answer:

> What should happen next, and why?

## Decision packet

The compact working set for the next Director decision.

Path:

```txt
snapshots/decision_packet/current.md
```

Purpose:

```txt
Give the Director enough context to decide without loading the full run history.
```

Recommended shape:

```md
# Decision Packet

## Goal

## Current contract

## Current workflow

## Active workers

## Latest worker output summary

## Evidence summary

## Open risks

## Pending decisions

## Next action candidates
```

The Director should load this first when resuming or deciding what to do next.

## Worker batch summary

A compact summary of the latest completed worker batch.

Path:

```txt
snapshots/worker_batch_summary/current.md
```

Use this after `wait_workers` to avoid repeatedly loading every raw worker output.

Recommended shape:

```md
# Worker Batch Summary

## Workers completed

## Outputs received

## Evidence added

## Conflicts or disagreements

## Follow-up needed
```

## Evidence summary

The current compact evidence view.

Path:

```txt
snapshots/evidence/current.md
```

Use this to summarize proofs, checks, sources, benchmarks, reviews, and known gaps. Raw evidence still lives under `artifacts/evidence/`.

## Ownership map

The current boundary map for parallel work.

Path:

```txt
snapshots/ownership_map/current.md
```

Use this before parallel implementation work. If the Director cannot write a clear ownership map, it should not parallelize implementation.

Recommended shape:

```md
# Ownership Map

## Worker A owns

## Worker A forbidden scope

## Worker B owns

## Worker B forbidden scope

## Shared contracts/interfaces

## Integrator-only files

## Integration risks
```

Rule:

```txt
No ownership map, no parallel implementation.
```

## Resume flow

On a new run, the Director reads the compact decision state first:

```txt
snapshots/decision_packet/current.md
snapshots/goal/current.md
snapshots/task/current.md
snapshots/workflow/current.md
snapshots/status/current.md
snapshots/next_action/current.md
snapshots/risks/current.md
snapshots/evidence/current.md
```

Then it reads raw state only when needed for the current decision:

```txt
logs/decisions.jsonl
logs/events.jsonl
workers/outputs/
artifacts/evidence/
contracts/
```

Resume flow:

```txt
Read decision packet
→ read current contract and workflow
→ read evidence and risk summaries
→ inspect pending workers if any
→ load raw outputs only when needed
→ choose next action
→ continue
```

## Context loading order

The Director should not load all run history by default.

Load state in this order:

```txt
1. snapshots/decision_packet/current.md
2. contracts/task.md
3. snapshots/workflow/current.md
4. snapshots/worker_batch_summary/current.md
5. snapshots/evidence/current.md
6. snapshots/risks/current.md
7. only the raw worker outputs, logs, diffs, or evidence needed for the current decision
```

## Compaction

Long-running tasks create too much history.

After every major decision or completed loop, the Director should update:

```txt
snapshots/status/current.md
snapshots/next_action/current.md
snapshots/risks/current.md
snapshots/decision_packet/current.md
```

After every completed worker batch, the Director should also update:

```txt
snapshots/worker_batch_summary/current.md
snapshots/evidence/current.md
```

Before parallel implementation work, the Director should update:

```txt
snapshots/ownership_map/current.md
```

If the goal changed, also update:

```txt
snapshots/goal/current.md
```

If the strategic direction changed, update:

```txt
snapshots/direction/current.md
```

Do not delete raw logs by default. Just stop loading them unless needed.

## Source of truth

Use Markdown + JSONL files as canonical state.

Optional search/index layers can be added later:

```txt
SQLite FTS = rebuildable lexical index
semantic index = rebuildable semantic index
```

Indexes should never own canonical state.

Rule:

```txt
The Director writes files.
Search systems index files.
No index owns the truth.
```
