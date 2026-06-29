---
name: discuss
description: Explore unclear goals, architecture, strategy, product decisions, or task framing before implementation.
status: draft
important_if:
  - the user asks to discuss, think through, explore, compare, decide, or brainstorm
  - requirements are unclear
  - the right output may be a decision memo rather than a patch
  - implementation would be premature or risky
skip_if:
  - the user gives a clear implementation task
  - the task is trivial and answerable directly
  - the user explicitly asks not to discuss
---

# Discuss Workflow

Goal: turn ambiguity into a useful decision, sharper task contract, or smallest next experiment.

Success means: the user has a clearer problem statement, explicit assumptions, tradeoffs, non-goals, and a concrete next step.

Stop when: the next decision or action is clear, or a blocker is identified.

## Toolset Guidance

Default to core only.

Load extra toolsets only when the discussion needs that capability:
- `git`: when current diffs, history, or changed files would materially change the recommendation
- `write`: only if the task turns into drafting or editing repo files
- `subagent`: only when delegating an isolated investigation

Load no extra toolsets beyond the capability areas the task enters.

## 1. Unknowns Before Answers

Before proposing answers, list missing information that could materially change the recommendation.

Classify each item:
- blocker: cannot proceed honestly without this
- assumption: proceed, but report uncertainty
- safe default: proceed using the default

Ask the user only for blockers. For assumptions and safe defaults, continue.

## 2. Frame The Discussion

State:
- goal
- current state
- target state or decision to make
- constraints
- non-goals
- protected invariants
- likely failure modes

If the user's framing seems wrong or too narrow, say so and explain the smaller claim supported by evidence.

## 3. Generate Options When Useful

If there are multiple plausible paths, compare them.

For each option, include:
- what it optimizes for
- what it costs
- what it risks
- what evidence would make it wrong
- smallest experiment or reversible first step

Do not create a tradeoff table unless comparison is actually useful.

## 4. Decide Or Narrow

If evidence supports one path, recommend it.

If evidence is insufficient, identify the cheapest observation that would change the decision.

Useful endpoints:
- rejected premise
- clarified problem statement
- decision memo
- phased plan
- implementation contract
- small experiment
- blocked state with required input

## 5. Non-Goals

Unless the user asks otherwise:
- do not edit code
- do not invent requirements
- do not force a patch-shaped answer
- do not ask questions whose answers would not change the recommendation
- do not turn uncertainty into fake certainty

## 6. Output Shape

Use the smallest shape that helps.

Default:

```md
# Working Read

# Unknowns

# Options Or Recommendation

# Non-Goals

# Smallest Next Step
```
