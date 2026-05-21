---
name: ogent
description: Use ogent as an external coding co-worker for focused software engineering tasks. Invoke when you need an independent CLI agent to implement, debug, review, verify, research, summarize, design, or write within the current repository.
---

# ogent

Use `ogent` when an independent coding co-worker would help make concrete progress in the current repository.

`ogent` is best for focused, bounded work: implementing a scoped change, debugging a failure, reviewing a diff, gathering evidence, validating claims, summarizing run state, or asking a specialist role for design/writing/research judgment.

## When to Use

Use this skill when:
- the user explicitly asks to use `ogent`
- a focused task can be delegated with a clear contract
- a second pass would improve confidence: review, verification, debugging, or research
- the task benefits from a specialist role rather than general conversation

Do not use it for tiny one-shot answers, broad ambiguous goals without a contract, destructive operations, credential discovery, or work outside the current repository unless the user explicitly asks.

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
| `reviewer` | judging whether work satisfies a contract |
| `verifier` | running or designing evidence checks |
| `researcher` | gathering and organizing evidence |
| `system_architect` | module/API/system boundary decisions |
| `database_architect` | data model, schema, query, migration decisions |
| `visual_designer` | UI direction, layout, hierarchy, visual implementation notes |
| `writer` | drafting or rewriting prose |
| `qa_writer` | StackOverflow-style technical answers |
| `critic` | sharp critique before user-facing delivery |
| `summarizer` | compressing transcript or run history into continuation state |

## Task Contract

Give `ogent` a complete but compact contract. Include only what changes behavior:

```text
Goal:

Context:

Scope:

Constraints:

Forbidden moves (actions `ogent` must not take, such as editing tests, deleting files, or broad refactors):

Evidence required:

Expected output:
```

Good contracts name files, commands, acceptance criteria, risk boundaries, and whether edits are allowed. Prefer relative paths. Do not pass secrets.

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
Do not edit tests unless the failure is a test bug and you can prove it.

Forbidden moves:
Do not change parser public APIs.

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
- Keep delegation scoped; avoid asking `ogent` to refactor broadly unless the user requested it.
- Do not ask `ogent` to bypass tests, hide failures, weaken acceptance criteria, or perform destructive operations.
- Treat `.ogent/sessions/`, `.ogent/journal.md`, and build outputs as runtime artifacts; read them only when needed and do not edit them unless requested.
- Do not claim `ogent` ran checks or changed files unless you observed the output or inspected the repository state.
