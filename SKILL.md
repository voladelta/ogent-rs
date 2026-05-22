---
name: ogent
description: Use ogent as an external coding co-worker for focused software engineering tasks. Invoke when you need an independent CLI agent to implement, debug, review, verify, research, summarize, design, or write within the current repository.
---

# ogent

Use `ogent` when an independent coding co-worker would help make concrete progress in the current repository.

`ogent` is best for focused, bounded work: implementing a scoped change, debugging a failure, reviewing or critiquing an artifact, gathering evidence, validating claims, summarizing run state, or asking a specialist role for design/writing/research judgment.

## When to Use

Use this skill when:
- the user explicitly asks to use `ogent`
- a focused task can be delegated with a clear contract
- a second pass would improve confidence: review, verification, debugging, or research
- the task benefits from a specialist role rather than general conversation

Use direct conversation for tiny one-shot answers. Ask the user to narrow broad ambiguous goals before delegation. Keep destructive operations, credential discovery, and work outside the current repository out of scope unless the user explicitly asks.

## First Check

Before invoking `ogent`, verify it is available:

```bash
ogent --help
```

If unavailable, say so and continue without claiming delegation.

Run `ogent` from the repository root or the intended workspace. Its workspace is the process current directory.

## Role Selection

Default to `ogent` for general software engineering. Use a specialist role when the task has a clear shape:

| Role | Use for |
| --- | --- |
| `ogent` | general coding, planning, investigation, mixed tasks |
| `implementer` | scoped code or artifact changes |
| `debugger` | root-cause analysis and minimal fix path |
| `reviewer` | judging whether work satisfies a contract, including sharp critique before delivery |
| `verifier` | running or designing evidence checks |
| `researcher` | gathering and organizing evidence |
| `system_architect` | module/API/system boundary decisions |
| `database_architect` | data model, schema, query, migration decisions |
| `visual_designer` | UI direction, layout, hierarchy, visual implementation notes |
| `writer` | drafting, rewriting prose, and StackOverflow-style technical answers |
| `summarizer` | compressing transcript or run history into continuation state |

Specialist roles receive scoped tool groups. Choose `ogent` when the task genuinely needs the full worker toolset. Choose `writer` or `summarizer` when the task may create a requested file but does not need shell commands or anchored code edits.

## Task Contract

Give `ogent` a complete but compact contract. Include only what changes behavior:

```text
Goal:

Context:

Scope:

Constraints:

Boundaries (actions outside scope, such as editing tests, deleting files, or broad refactors):

Evidence required:

Expected output:
```

Good contracts name files, commands, acceptance criteria, risk boundaries, and whether edits are allowed. Prefer relative paths. Keep secrets out of task contracts.

## Invocation Patterns

General task:

```bash
ogent "<task contract>"
```

Specialist role:

```bash
ogent --role reviewer "<review contract>"
```

With a specific model/profile when the caller requires it:

```bash
ogent --role implementer --profile kimi "<implementation contract>"
```

For multiline contracts, use a heredoc so the task is readable and reproducible:

```bash
task_contract=$(cat <<'TASK'
Goal:
Find the root cause of the failing parser test.

Context:
The failure appears after changing token normalization.

Scope:
src/parser.rs, src/lexer.rs, parser tests.

Constraints:
Treat tests as intended-behavior evidence. Edit tests only if the failure is a test bug and you can prove it.

Boundaries:
Keep parser public APIs unchanged.

Evidence required:
Reproduction command, root-cause evidence, and minimal fix path.
TASK
)

ogent --role debugger "$task_contract"
```

## Handling Results

Treat `ogent` output as a co-worker report, not automatic truth.

After it finishes:
- read the status and evidence
- inspect changed files or cited files before relying on them
- run the relevant verification yourself when the result affects user-facing claims
- summarize only the useful result back to the user
- preserve uncertainty, blockers, and failed checks

Expected final sections from `ogent` are:

```text
# Status
# Summary
# Changed Files
# Verification
# Evidence
# Risks
# Question
# Next Action
```

If status is `question`, answer the missing question yourself if possible, then rerun `ogent` with the new context. If status is `partial` or `blocked`, decide whether to continue directly, rerun with a narrower contract, or ask the user.

## Safety and Repo Hygiene

- Check `git status` before and after delegated edit tasks.
- Keep delegation scoped; request broad refactors only when the user requested them.
- Ask `ogent` to preserve tests, surface failures, keep acceptance criteria stable, and leave destructive operations to explicit user requests.
- Treat `.ogent/sessions/`, `.ogent/journal.md`, and build outputs as runtime artifacts; read them only when needed and edit them only when requested.
- Claim `ogent` ran checks or changed files only after you observed the output or inspected the repository state.
