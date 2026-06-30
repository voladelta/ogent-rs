---
name: review
description: Review code, diffs, designs, plans, prompts, or agent-produced work for correctness and risk.
status: stable
important_if:
  - the user asks for a review, audit, critique, or PR assessment
  - evaluating a diff, design, implementation, workflow, prompt, or plan
  - checking agent-generated work before accepting it
skip_if:
  - the user asks to implement rather than review
  - the requested output is a direct answer
  - there is no artifact, claim, or design to evaluate
---

# Review Workflow

Goal: find material risks, bugs, regressions, missing verification, and maintainability problems.

Success means: findings are specific, evidence-backed, ordered by severity, and useful for the next decision.

Stop when: the review has reported all material findings or clearly states that no material findings were found.

A finding is valid only when it separates observed evidence from inferred risk. If the evidence is incomplete, state the uncertainty instead of smoothing it into a confident claim.

## Toolset Guidance

Load these toolsets only when the review needs them:
- `git`: before reviewing worktree state, diffs, changed files, or commit history
- `subagent`: only when delegating an isolated investigation or independent check
- `write`: only if the user changes the task from review to fixing

Review is read-first. Load no extra toolsets beyond the capability areas the task enters.

## 1. Scope And Unknowns

Before reviewing, state:
- artifact under review
- intended behavior or claim, if known
- protected invariants
- non-goals for the review

List missing information that could materially change the review. Apply the shared Unknowns And Scope categories: blocker, assumption, safe default.

Ask the user only for blockers.

## 2. Gather Evidence

Use the smallest evidence set that can support the review.

For code:
- inspect the diff or changed files
- inspect adjacent code when needed to understand contracts
- inspect tests when behavior or regressions are relevant
- run checks only when review scope and time justify it

For plans or prompts:
- compare goal, current state, target state, constraints, verification, and stop condition
- look for vague quality words that should become checkable criteria
- look for missing non-goals and hidden scope expansion

## 3. Review By Failure Mode

Prioritize:
- correctness bugs
- behavior regressions
- API or data contract breakage
- security or privilege mistakes
- race, retry, timeout, and resource-bound failures
- missing or weak verification
- abstraction inflation
- unclear ownership or source of truth
- prompt or workflow instructions that encourage fake completion

Do not spend review budget on style unless style creates real risk.

## 4. Findings First

Report findings first, ordered by severity.

Each finding must include:
- exact trigger: input, state, diff hunk, workflow step, or user action that exposes the issue
- evidence: source line, command output, reproduced behavior, or cited artifact text
- falsifier or uncertainty: what would disprove the claim, or what remains unverified

Use this format:

```md
# Findings

- [P1] Title
  File/line or source:
  Trigger:
  Evidence:
  Problem:
  Risk:
  Falsifier or uncertainty:
  Suggested fix:

- [P2] Title
  File/line or source:
  Trigger:
  Evidence:
  Problem:
  Risk:
  Falsifier or uncertainty:
  Suggested fix:

# Questions

- Only questions that block correctness.

# Verification Notes

- Checks run, if any.
- Checks not run and why.

# Summary

Brief secondary context only.
```

If there are no material findings, say that clearly and report remaining test gaps or residual risk.

## 5. Non-Goals

Unless the user asks otherwise:
- do not rewrite the artifact
- do not fix the code during review
- do not produce a generic summary before findings
- do not approve work without evidence
- do not nitpick harmless style differences
