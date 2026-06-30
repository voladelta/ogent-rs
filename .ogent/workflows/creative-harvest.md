---
name: creative-harvest
description: Generate divergent raw ideas, harvest useful design vectors, then constrain them into usable options.
status: stable
important_if:
  - naming
  - UI or product design
  - workflow design
  - DSL or API shape
  - architecture alternatives
  - prompts, skills, or agent behavior
  - the user asks for novel, weird, creative, fresh, or non-obvious ideas
skip_if:
  - the task has one correct answer
  - the task is a bug fix with a clear root cause
  - the task is safety-critical and novelty would add risk
  - the user asked for direct implementation
---

# Creative Harvest Workflow

Goal: produce useful originality without letting novelty escape the task constraints.

Success means: raw ideas are generated, useful vectors are extracted, weak parts are discarded, and final options are constrained enough to act on.

Stop when: the user has a compact set of usable options or a clear next experiment.

## Toolset Guidance

Default to core only.

Load extra toolsets only when creativity work crosses into a capability area:
- `git`: when current diffs, history, or repo state constrain the options
- `write`: only if the user asks to write or update files
- `subagent`: only when delegating independent generation, critique, or synthesis

Load no extra toolsets beyond the capability areas the task enters.

## 1. Frame The Problem

State:
- goal
- audience or consumer
- constraints
- non-goals
- evaluation criteria

If constraints are missing, apply the shared Unknowns And Scope categories: blocker, assumption, safe default. Use safe defaults where possible and ask only for blockers.

## 2. Raw Generation

Generate raw material before judging it.

Rules:
- produce more options than needed
- include weird but plausible options
- avoid generic defaults unless they are a useful baseline
- do not choose yet
- do not polish yet

For most tasks, produce 8 to 12 raw options.

## 3. Harvest Vectors

Extract the strongest underlying design vectors from the raw options.

For each vector, state:
- core move
- why it is promising
- what to keep
- what to discard
- risk or constraint pressure

Do not treat the first idea list as the answer. Treat it as raw material.

## 4. Constrain And Recombine

Use the best vectors to produce tighter options.

Apply constraints explicitly:
- implementation cost
- user comprehension
- maintainability
- compatibility
- reversibility
- visual or conceptual distinctiveness, if relevant

Discard options that need scope the user did not grant.

## 5. Select Or Present

If the user asked for a recommendation, choose one option and explain why.

If the user asked for exploration, present a small set of distinct options.

Output format:

```md
# Frame

# Raw Ideas

# Harvested Vectors

# Constrained Options

# Recommendation Or Next Experiment
```

## 6. Non-Goals

Unless the user asks otherwise:
- do not implement the idea
- do not invent product requirements
- do not hide tradeoffs
- do not optimize for novelty over usefulness
- do not expand into a full roadmap
