# Contracts

Contracts are the unit of delegation.

Every worker dispatch should be contract-bound.

In v1, contracts are structural Markdown, not strict JSON.

## Task contract

Created once near the start of the run.

Path:

```txt
contracts/task.md
```

Recommended shape:

```md
# Task

Original user task, rewritten as an executable objective.

# Goal

What success means.

# Definition of done

- Concrete condition 1
- Concrete condition 2

# Constraints

- Hard rule 1
- Hard rule 2

# Non-goals

- What should not be done

# Required evidence

- What proof is needed before acceptance

# Open risks

- Known risks or uncertainties
```

## Worker contract

Created for each worker in `dispatch_workers`.

Recommended shape:

```md
# Task

What this worker should do.

# Goal

What success means for this worker.

# Owned scope

The files, sections, claims, modules, or artifacts this worker owns.

# Forbidden scope

What this worker must not touch.

# Inputs

Relevant state paths, files, diffs, previous worker outputs, shared contracts, or commands.

# Required output

The exact output expected.

# Failure conditions

When to stop and report blocked.
```

## Parallel contracts

When dispatching many workers, every worker must have a non-overlapping contract.

Bad:

```txt
Worker A: improve frontend
Worker B: improve frontend
```

Good:

```txt
Worker A: implement frontend API client
Worker B: implement loading/error states
Worker C: review visual consistency
```

## Shared contracts

If workers must coordinate, write a shared contract first.

Examples:

```txt
contracts/shared/api.md
contracts/shared/design-system.md
contracts/shared/paper-outline.md
contracts/shared/data-schema.md
```

A shared API contract might define:

```md
# Shared API contract

## Endpoint

POST /api/config

## Request

```json
{
  "path": "string"
}
```

## Response

```json
{
  "ok": true,
  "config": {}
}
```
```

## Good contract properties

A good contract is:

- narrow
- testable or reviewable
- explicit about constraints
- clear about output shape
- clear about what is forbidden
- clear about what counts as evidence
- clear about ownership

## Bad contract examples

Too broad:

```txt
Make the code better.
```

No output shape:

```txt
Review this.
```

No constraints:

```txt
Fix the repo however you want.
```

No evidence:

```txt
Tell me if it works.
```

## Good contract examples

```md
# Task

Review the patch against the original task contract.

# Goal

Find correctness issues, hidden hacks, unnecessary complexity, contract drift, and missing verification.

# Constraints

- Do not modify files.
- Do not approve if tests were weakened.
- Do not approve without evidence.

# Required output

Return:

1. Verdict: pass/fail
2. Blocking issues
3. Non-blocking issues
4. Missing evidence
5. Recommended next action
```

```md
# Task

Review this macro API for hygiene, ambiguity, diagnostics, and compile-time failure modes.

# Constraints

- Do not rewrite implementation.
- Do not comment on unrelated architecture.

# Required output

Return:

1. Blocking macro issues
2. Non-blocking improvements
3. Minimal suggested patch if needed
4. Verdict
```

## Contract drift

Contract drift happens when the system quietly changes the goal.

Examples:

- user asked to preserve behavior, system changes API
- user asked to fix tests, system deletes failing tests
- user asked for production-ready work, system adds workaround
- user asked for concise answer, system writes a full report
- user asked for implementation, system only gives a plan

The Director must detect and reject contract drift.
