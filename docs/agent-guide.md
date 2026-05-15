# Agent Guide

How the agent works internally: its phases, checkpoints, task tracking, skills, and coworker delegation.

## Agent

The agent works in **phases**, writing short in-session checkpoints and hiring specialist coworkers when needed.

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

Checkpoints help preserve working state across phase changes, delegation, and compaction. They are model-facing context notes only: runtime code does not parse them, save them as durable memory, or load them on future runs.

### Compaction

When context usage crosses the autocompact threshold (default 80%), or when the user runs `/compact [focus]`, the agent produces a handoff brief and spawns a new child session. The brief preserves:

- Goal, what was done, current state, relevant excerpts, next steps
- Full task tracker state (goal, phases, todos) so work resumes seamlessly
- A reference to the parent session transcript (`.ogent/sessions/<id>/messages.jsonl`)

The parent session is preserved on disk unchanged. The new child session starts fresh with the handoff brief as its first user message. Task tracker state is carried forward in memory.

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

Create a local skill with:

```bash
ogent --create-skill repo-audit "Review repositories for correctness, security, and maintainability risks"
```

Creator mode asks the selected profile for exactly one `SKILL.md`, validates required frontmatter and body content, and writes it to `.ogent/skills/<name>/SKILL.md`. Existing skills are included as context and improved in place after the replacement validates.

At startup, available skills are discovered and listed in the user message. The agent can call `load_skill` to inject a skill body into the next turn.

The `colgrep` skill is preloaded: if their `SKILL.md` files exist in a skill root, ogent auto-injects their full body into the initial user message after the skills list. This gives the agent semantic code search and repo context instructions without spending a turn on `load_skill`.

Skills do not define or activate workflows. Skills are reusable capability instructions. Workflows are optional session control policies loaded explicitly with `--workflow`.

### Workflows

Workflows are optional. Start a session with one active workflow when the task benefits from enforced steps, evidence checks, or bounded loops:

```bash
ogent --workflow common-sw "fix parser panic"
ogent --workflow auto-iteration "improve benchmark score"
ogent --workflow workflows/iteration.yaml --steer
```

If no workflow is supplied, ogent behaves normally.

Workflow state is persisted in `.ogent/sessions/<id>/workflow-state.json` and reloaded on resume/fork. The model sees only the current workflow context before each turn.

Workflow tools are included in the model tool schema only when a workflow is active:
- `workflow_status`
- `workflow_enter_step`
- `workflow_record_check`
- `workflow_run_check`

Workflow enforcement:
- first step must be the workflow `start`
- transitions must follow `next`
- gated transitions require a reason
- required checks must pass or be waived before leaving a step
- `max_visits` bounds loops
- `complete` requires a terminal workflow step

Workflow and goal tracking are separate:
- Goal/task tracker = objective and progress display
- Workflow = process control and evidence gates

When a workflow step is entered and a task tracker exists, ogent mirrors the step as the current tracker phase for visibility.

Built-in workflows live in `workflows/`:
- `common-sw` — general software work: intake, execute, verify, repair, review, done.
- `auto-iteration` — bounded measured optimization/research loop: frame, baseline, propose, implement, evaluate, fix, decide, report.

Create a local workflow with:

```bash
ogent --create-workflow release-check "Gate a release through build, tests, review, and final approval evidence"
```

Creator mode asks the selected profile for exactly one workflow YAML file, validates it with the runtime workflow schema, and writes it to `.ogent/workflows/<name>.yaml`. Local workflows can be loaded by name with `--workflow <name>`.

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

The agent uses `dispatch_worker` when:
- The task has parallel independent work streams
- A specialist perspective is needed (security review, docs, tests)
- The task is large enough that splitting context helps

**Golden rule:** Give the worker JUST ENOUGH context — but it must be the RIGHT context. A worker without file paths or commands will fail silently.

**Worker prompt templates** in `prompts/workers/` (`generic`, `coder`, `tester`, `reviewer`, `validator`) define the worker's role and behavior. Built-in specialist templates are used directly; custom roles are generated from the generic template via an architect LLM call. The concrete assignment goes in the separate `task` argument.

**Dispatch checklist:**
- [ ] You actually need a worker (prefer direct action for <3 turns of work)
- [ ] `template` selects the worker role and defines behavior/scope; built-in templates (generic, coder, tester, reviewer, validator) are used directly, custom roles are generated from the generic template
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
