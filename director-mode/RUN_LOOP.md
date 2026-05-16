# Director Run Loop

## Core loop

```txt
Frame → Dispatch → Evaluate → Decide → Compress → Loop
```

Expanded:

```txt
1. Receive messy prompt
2. Frame task contract
3. Design initial workflow
4. Decompose work if useful
5. Choose next move
6. Dispatch workers or hire a temporary worker
7. Wait for worker output
8. Review, verify, or integrate
9. Decide:
   - accept
   - revise
   - hire specialist
   - ask user
   - block
   - fail
10. Update snapshots/logs
11. Continue or report
```

## State transitions

```txt
RECEIVED
  ↓
FRAMED
  ↓
WORKFLOW_DESIGNED
  ↓
IN_PROGRESS
  ↓
NEEDS_REVIEW
  ↓
NEEDS_VERIFICATION
  ↓
DECIDING
  ↓
DONE | REVISING | BLOCKED | FAILED | PARTIAL
```

## Worker dispatch pattern

Use one primitive:

```ts
dispatch_workers({
  workers: [
    { role: "verifier", task: "# Task
..." },
    { role: "reviewer", task: "# Task
..." }
  ]
})
```

Default is async parallel dispatch.

Use `sync: true` for sequential dispatch:

```ts
dispatch_workers({
  sync: true,
  workers: [
    { role: "debugger", task: "# Task
Find the root cause." },
    { role: "implementer", task: "# Task
Fix using debugger output." },
    { role: "verifier", task: "# Task
Verify the fix." }
  ]
})
```

Sequential mode means:

```txt
worker[0] completes
  ↓
worker[1] starts
  ↓
worker[2] starts
```

Use sequential dispatch when workers depend on earlier outputs.

Use parallel dispatch when workers have non-overlapping ownership.

## Decision rules

### Accept

Accept only when:

- definition of done is satisfied
- required evidence exists
- open risks are acceptable and reported
- no contract drift occurred

### Revise

Revise when:

- output is close but incomplete
- review found fixable issues
- verification failed but root cause is actionable
- a better workflow is now clear

### Hire worker

Hire when:

- generic workers are too broad
- domain-specific judgment is needed
- repeated attempts failed
- review requires specialist criteria
- task is niche or high-risk

### Ask user

Ask only when:

- decision is product-level ambiguous
- action is destructive or irreversible
- credentials/secrets are required
- task constraints conflict
- user preference is genuinely needed

### Block

Block when:

- task cannot be completed under constraints
- verification cannot be obtained
- required information is unavailable
- continuing would be unsafe or dishonest

### Fail

Fail when:

- the system errored unrecoverably
- state is corrupted
- required tools are unavailable
- the run cannot continue safely

## Evidence policy

The Director must ask:

```txt
What evidence proves this task is done?
```

Examples:

```txt
Coding: tests, build, typecheck, benchmark, diff review
Research: sources, claim extraction, contradiction check
Writing: rubric fit, critic review, editing pass
Design: style analysis, hierarchy critique, before/after rationale
Optimization: baseline, after measurement, behavior preservation
```

## Parallel work policy

Parallel dispatch is useful, but only with ownership boundaries.

Safe parallelism requires:

```txt
non-overlapping scopes
shared interface contracts
worker-owned outputs
integration step
final verification
```

For coding, parallel implementers should work in isolated worktrees.

## Contract preservation

Do not silently mutate:

- goal
- definition of done
- constraints
- non-goals
- required evidence

If the contract must change, report it as a blocked/user-decision condition.

## Loop guardrails

- Keep steps small.
- Prefer reversible actions.
- Record failed attempts.
- Do not repeat the same failed strategy.
- Do not confuse activity with progress.
- Do not accept output without evidence.
- Do not let a worker redefine the task.
- Stop honestly when blocked.
