---
name: implement
description: Make non-trivial code or repo changes with explicit unknowns, non-goals, design, verification, and divergence analysis.
status: stable
important_if:
  - the user asks to implement, fix, refactor, wire, add, remove, migrate, or update code
  - the task changes source files, tests, docs, configuration, prompts, workflows, or runtime behavior
  - the task crosses a public API, data shape, filesystem, process, network, security, or persistence boundary
  - verification is needed to know whether the task is complete
skip_if:
  - the task is a direct answer
  - the task is trivial, reversible, and does not cross a behavior boundary
  - the user asks for discussion, review, or planning only
  - the user explicitly says not to edit files
---

# Implement Workflow

Goal: move the repo from current state to target state with the smallest correct, verified change.

Success means: the requested outcome is implemented, protected behavior is preserved, verification was run or honestly reported unavailable, and meaningful divergence from the design is surfaced.

Stop when: the target state is reached and verified, or the work is honestly PARTIAL or BLOCKED.

## Toolset Guidance

Load these toolsets before using the corresponding capability:
- `write`: before mutating files or planning anchored edits
- `git`: before inspecting worktree state, diffs, changed files, history, or commits
- `subagent`: only before delegating to subagents, running parallel Lua tasks, or sending task updates

Load no extra toolsets beyond the capability areas the task enters.

## 1. Unknowns Before Answers

Before drafting a spec, plan, or design, list missing information that could materially change the implementation. Apply the shared Unknowns And Scope categories: blocker, assumption, safe default.

Ask the user only for blockers. For assumptions and safe defaults, continue.

Use this shape:

```md
# Unknowns

- [blocker] ...
- [assumption] ...
- [safe default] ...
```

If there are no material unknowns, say so and continue.

## 2. Task Contract

Define the task before editing.

Include:
- goal
- current state
- target state
- success criteria
- non-goals
- protected invariants
- files or modules likely involved
- verification plan

Make non-goals explicit. Non-goals prevent helpful scope creep.

Examples:
- Do not add auth.
- Do not redesign unrelated UI.
- Do not introduce a new database table unless required.
- Do not change existing API behavior.
- Do not touch billing logic.

Use concrete boundaries, not vague warnings.

## 3. Program Design Before Code

Before editing files, produce a short design for non-trivial changes.

Include:
- files likely to change
- functions, types, commands, or docs likely to change
- data flow or control flow
- existing patterns to preserve
- tests or checks to run
- expected failure modes
- what evidence would invalidate the design

Do not implement during this phase unless the task is trivial.

If invalidating evidence appears while coding, stop and revise the design before continuing.

## 4. Abstraction Control

Prefer the smallest local change that satisfies the task contract.

Do not introduce new:
- abstractions
- frameworks
- global state
- configuration systems
- helper layers
- data stores
- public APIs

unless the design proves they are necessary.

A new abstraction is justified only when it:
- removes real duplication
- protects a real invariant
- clarifies an existing boundary
- matches an established local pattern

Optimize for boring, obvious code that a future agent or tired human can modify with local context.

## 5. Implementation

Implement one coherent unit at a time.

Rules:
- read the relevant files before editing
- trace the existing pattern before adding new code
- preserve existing behavior unless the contract explicitly changes it
- keep edits scoped to the task
- avoid unrelated formatting churn
- do not edit tests, examples, snapshots, or benchmarks merely to pass checks
- do not patch around a broken foundation when a root-cause fix is required

If the clean fix is larger than expected, say so before expanding scope.

## 6. Verification

Run the strongest practical verification available for the change.

Prefer:
- focused tests for the changed behavior
- type checks or compile checks
- lint checks when relevant
- integration or manual reproduction steps when behavior crosses boundaries

Map success criteria to evidence.

If verification fails, classify the failure using the shared Evidence And Verification policy before editing again.

Then repair, revise the contract, report PARTIAL, or report BLOCKED.

Do not claim verification that was not run.

## 7. Divergence Analysis

After implementation, compare the final result against the design.

Report only meaningful divergence:
- what changed from the design
- why it changed
- whether the deviation is safe
- what new risk it introduces
- what a reviewer should inspect first

Skip divergence analysis only when there was no meaningful design because the task was trivial.

## 8. Final Report

End as COMPLETE, PARTIAL, or BLOCKED.

For COMPLETE, report:
- files changed
- behavior changed
- verification run and result
- meaningful divergence, if any
- remaining uncertainty, if any

For PARTIAL, report:
- what was completed
- what remains
- why it remains
- smallest next step

For BLOCKED, report:
- blocker
- evidence
- what is needed
- what was not changed
