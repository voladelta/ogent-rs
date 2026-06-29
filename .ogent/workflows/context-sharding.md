---
name: context-sharding
description: Extract large files, docs, or codebase areas into compact source-backed context shards.
status: draft
important_if:
  - the useful context is too large to load directly
  - future agents will need the same repo knowledge
  - a task depends on architecture, invariants, ownership, or cross-file behavior
  - a large source file or document needs to be reduced without losing correctness
skip_if:
  - the task can be answered from one small file
  - the context is only useful once and can be summarized in the final answer
  - the user asked for implementation rather than context extraction
default_output_dir: .ogent/context
---

# Context Sharding Workflow

Goal: turn oversized context into compact, reusable, source-backed shards.

Success means: each shard helps a future agent decide what to load, what to preserve, and what not to assume.

Stop when: the shard is written or the missing information is classified as a blocker.

## Toolset Guidance

Load these toolsets only when the shard task needs them:
- `write`: before writing or updating shard files
- `git`: when git history, diffs, or changed files are source evidence
- `subagent`: only when delegating bounded extraction or review work

Use core read tools for source selection and extraction. Load no extra toolsets beyond the capability areas the task enters.

## 1. Define The Consumer

Before extracting, state who will use the shard and for what decision.

Return:
- consumer task
- decision the shard should support
- source files or docs likely involved
- non-goals for this shard

Non-goals should prevent broad summaries. Examples:
- Do not document every function.
- Do not rewrite architecture docs.
- Do not include implementation plans.
- Do not copy large source blocks.

## 2. Unknowns Before Extraction

List missing information that could materially change the shard.

Classify each item:
- blocker: cannot produce a truthful shard without this
- assumption: proceed, but label the assumption
- safe default: proceed using the default

Ask the user only for blockers. Use safe defaults where possible.

## 3. Select Sources

Choose the smallest source set that can support the shard.

Use source-backed inspection:
- use semantic search for intent
- use exact search for known symbols or text
- read entry points before leaf details
- inspect tests when behavior or invariants are unclear

Do not read the whole repo just to feel certain. Expand only when a concrete unknown requires it.

## 4. Extract Load-Bearing Facts

Extract only facts that change future work:
- boundaries and owners
- entry points
- data flow
- invariants
- failure modes
- extension points
- naming or style conventions
- verification commands
- traps and stale assumptions

Separate:
- facts directly supported by sources
- inferences from multiple sources
- assumptions that need future verification

## 5. Write The Shard

Write a shard under `.ogent/context/<name>.md`.

Use this template:

```md
---
name: short-kebab-name
description: One sentence describing when to load this shard.
status: draft
important_if:
  - condition that makes this shard relevant
sources:
  - path/to/source.rs
  - path/to/doc.md
---

# Purpose

What decision this shard helps with.

# Load When

- Specific trigger.
- Specific trigger.

# Facts

- Source-backed fact.
- Source-backed fact.

# Invariants

- Behavior or boundary to preserve.

# Entry Points

- `path`: why it matters.

# Failure Modes

- What future agents are likely to get wrong.

# Do Not Assume

- Assumption to avoid.

# Refresh Triggers

- When this shard should be regenerated.
```

## 6. Verify The Shard

Before finishing:
- re-open the shard
- check every strong claim against a listed source
- remove trivia that does not change future work
- label assumptions plainly
- confirm the shard is smaller than the source context it replaces

Return:
- shard path
- sources used
- blockers, assumptions, and safe defaults
- remaining uncertainty
