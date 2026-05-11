# Agent Guide

How the 10x coder works internally: its phases, checkpoints, task tracking, skills, and coworker delegation.

## 10x Coder

The 10x coder works in **phases**, writing short in-session checkpoints and hiring specialist coworkers when needed.

### Checkpoints

At meaningful in-session boundaries, the agent may write a short `<checkpoint>` note for its own context management:

```xml
<checkpoint>
- Evidence: ...
- State: ...
- Decisions: ...
- Risks: ...
- Next: ...
</checkpoint>
```

Checkpoints help preserve working state across phase changes, delegation, compaction, and handoff. They are model-facing context notes only: runtime code does not parse them, save them as durable memory, or load them on future runs.

### Runtime task tracking

`ogent` now supports runtime-owned task tracking with a strict hierarchy:

```text
Goal -> Phases -> Todos
```

Todos are optional per phase.

Tracking is maintained through tools, not free-form prose:
- call `set_goal` once
- use `update_phase` and `update_todo` as upserts
- use `revise_goal` rarely; it records the prior goal and reason
- include concise success criteria on `set_goal` / `revise_goal` when they clarify completion

Status values: `pending`, `in_progress`, `completed`, `blocked`, `skipped`
Complexity values: `simple`, `medium`, `complex`

### Skills

Skills are loaded from:

- `.ogent/skills/<name>/SKILL.md`
- `.skills/<name>/SKILL.md`
- `~/.ogent/skills/<name>/SKILL.md`

At startup, available skills are discovered and listed in the user message. The agent can call `load_skill` to inject a skill body into the next turn.

The `colgrep` and `codectx` skills are preloaded: if their `SKILL.md` files exist in a skill root, ogent auto-injects their full body into the initial user message after the skills list. This gives the 10x coder semantic code search and repo context instructions without spending a turn on `load_skill`.

Skills may optionally define a **workflow** graph in YAML frontmatter. When loaded, ogent enforces the phase graph at runtime: transitions are validated, loops are bounded, and `complete` is gated to terminal phases only. See "Workflow Skills" below.

Install the search CLIs you want the agent to use for efficient codebase discovery:

```bash
# macOS
brew install ripgrep ast-grep

# Install colgrep separately if you use semantic repo search.
brew install lightonai/tap/colgrep

# Then add its skill file:
mkdir -p ~/.ogent/skills/colgrep
$EDITOR ~/.ogent/skills/colgrep/SKILL.md
```

Recommended search behavior:
- `colgrep` for intent-based code search, system exploration, and symbol discovery.
- `rg` for exact text and regex matching.
- `ast-grep` for syntax-aware structural search.

### Hiring coworkers

The 10x coder uses `dispatch_worker` when:
- The task has parallel independent work streams
- A specialist perspective is needed (security review, docs, tests)
- The task is large enough that splitting context helps

**Golden rule:** Give the worker JUST ENOUGH context — but it must be the RIGHT context. A worker without file paths or commands will fail silently.

**Worker prompt templates** in `prompts/templates/` (`generic`, `tester`, `reviewer`, `validator`) are starting points for the worker `system_prompt`. The 10x coder customizes one of them for the worker's role, scope, constraints, and summary format, then puts the concrete assignment in the separate `task` argument. All `{{PLACEHOLDERS}}` must be filled before dispatch.

**Dispatch checklist:**
- [ ] You actually need a worker (prefer direct action for <3 turns of work)
- [ ] `system_prompt` defines role, allowed tools/actions, read/write scope, constraints, commands, and summary format
- [ ] `task` states the exact assignment, expected output, success criteria, and immediate next step
- [ ] All file paths are exact relative paths
- [ ] Commands are exact and copied into the worker scope
- [ ] Invariants/constraints from the current checkpoint or task context are included

The worker runs in isolation with your prompt. When done, it calls `worker_complete` with a structured Markdown summary. That summary is returned to the parent coder. You decide what to do next.

## Creating skills

Skills are **domain knowledge packages** stored as `.ogent/skills/<name>/SKILL.md`:

```
.ogent/skills/
├── rust-refactor/
│   └── SKILL.md          # Rust refactoring procedures
├── string-utils/
│   └── SKILL.md          # Rust string utility patterns
└── ...                   # User-created skills
```

Each `SKILL.md` has YAML frontmatter (`name`, `description`) and Markdown instructions. The description helps the agent decide when to apply the skill. The full body is loaded only when triggered (progressive disclosure).

### Workflow Skills

Skills can optionally define a directed phase graph that ogent enforces at runtime. This keeps the agent bound to a workflow instead of improvising turn-by-turn.

Add a `workflow:` block to the skill frontmatter:

```yaml
---
name: my-flow
description: TDD-style implementation flow
workflow:
  phases:
    write_test:
      next: [implement]
    implement:
      next: [run_test]
    run_test:
      next: [done, refactor]
      gate: true          # requires explicit branch choice
    refactor:
      next: [run_test]
      max_visits: 3       # hard loop budget
    done:
      terminal: true      # only here can complete succeed
---
```

**Fields:**
- `phases`: map of phase IDs to `PhaseDef`
- `next`: list of allowed next phases
- `terminal`: if `true`, `complete` is allowed only from this phase
- `gate`: if `true`, the LLM sees a reminder to explicitly choose a branch
- `max_visits`: reject transitions after N visits (loop budget)

**Enforcement points:**
1. **`update_phase`** — when status is `in_progress`, the target phase must be in `next` of the current phase. Illegal transitions return a tool error.
2. **`complete`** — if the current phase is not terminal, `complete` is rejected with the allowed exit phases listed.
3. **System prompt injection** — before every LLM call, ogent appends `[Workflow] Phase: X. Visits: N. Next: [...].` to the system prompt so the agent is anchored.

```bash
mkdir -p .ogent/skills/my-skill
cat > .ogent/skills/my-skill/SKILL.md << 'EOF'
---
name: my-skill
description: What this skill does and when to use it.
---

## Brief
Compressed procedure.

## Context
What to assume.

## Constraints
Hard limits.

## Procedure
1. Step one
2. Step two

## Verification
How to confirm success.
EOF
```
