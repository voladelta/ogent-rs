---
name: ogent
description: Use ogent as an external coding co-worker for focused software engineering tasks. Invoke when you need an independent CLI agent to implement, debug, review, verify, research, summarize, design, or write within the current repository.
---

# ogent

Use `ogent` when an independent coding co-worker would help make concrete progress in the current repository.

`ogent` is best for focused, bounded work: implementing a scoped change, debugging a failure, reviewing or critiquing an artifact, gathering evidence, validating claims, or summarizing run state.

Run `ogent` from the repository root or the intended workspace. Its workspace is the process current directory.

## When to Use

Use this skill when:
- the user explicitly asks to use `ogent`
- a focused task can be delegated with a clear contract
- a second pass would improve confidence: review, verification, debugging, or research

Use direct conversation for tiny one-shot answers. Ask the user to narrow broad ambiguous goals before delegation. Keep destructive operations, credential discovery, and work outside the current repository out of scope unless the user explicitly asks.

## Task Contract

Give `ogent` a complete, compact task prompt. Open with the destination, then give the worker the path to evidence and the stopping condition.

```text
Goal: <one sentence>

Success means:
- <observable result>
- <required evidence>
- <required verification or inspection>
- <required output format>

Context:
<only the facts needed for this run>

Scope:
<files, directories, commands, or topics in bounds>

Constraints:
<hard boundaries: edit permission, runtime limits, safety, secrets, destructive actions>

Stop when:
<exact condition that ends the run>

Evidence required:
<commands, files, traces, diffs, logs, or reasoning that must appear in the final answer>

Expected output:
Use exactly these top-level sections: # Status, # Summary, # Changed Files, # Verification, # Evidence, # Risks, # Question, # Next Action.

Claim standard:
For security, sandbox, parser, validation, or correctness claims, give one concrete input, trace the validation/check path, trace the runtime/effect path, and name the invariant satisfied or violated before classifying the issue.
```

Strong contracts name the target state, files, commands, acceptance criteria, risk boundaries, edit permission, and verification evidence. Use relative paths. Put tool workflow details in the repo system prompt; put task-specific outcomes and evidence rules in the contract.

## Invocation Patterns

General task:

```bash
ogent "<task contract>"
```

With a specific model/profile when the caller requires it:

```bash
ogent --profile kimi "<task contract>"
```

Available profiles: `glm`, `kimi`, `ds-flash`, `ds-flash-max`, `ds-pro`, `ds-pro-max`

For multiline contracts, use a heredoc so the task is readable and reproducible:

```bash
task_contract=$(cat <<'TASK'
Goal:
Find the root cause of the failing parser test.

Success means:
- The failing behavior is reproduced or the blocker is reported.
- The root cause is supported by source or test evidence.
- The smallest justified next step is identified.

Context:
The failure appears after changing token normalization.

Scope:
src/parser.rs, src/lexer.rs, parser tests.

Constraints:
Treat tests as intended-behavior evidence. Edit source only after identifying the root cause. Edit tests when the evidence shows the test encodes stale behavior.

Boundaries:
Keep parser public APIs unchanged.

Stop when:
The root cause and smallest justified next step are clear, or the run reaches a specific blocker.

Evidence required:
Reproduction command, root-cause evidence, and minimal fix path.

Expected output:
Use exactly these top-level sections: # Status, # Summary, # Changed Files, # Verification, # Evidence, # Risks, # Question, # Next Action.
TASK
)

ogent "$task_contract"
```

## Handling Results

Treat `ogent` output as a co-worker report. Verify its evidence before relying on its conclusions.

After it finishes:
- read the status and evidence
- inspect changed files or cited files before relying on them
- run the relevant verification yourself when the result affects user-facing claims
- summarize only the useful result back to the user
- preserve uncertainty, blockers, and failed checks
- grade the report against the original contract: outcome, scope, evidence, verification, format, and repo hygiene

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

The `# Status` body is one of: `completed`, `partial`, `blocked`, `question`.

If status is `question`, answer the missing question yourself if possible, then rerun `ogent` with the new context. If status is `partial` or `blocked`, decide whether to continue directly, rerun with a narrower contract, or ask the user.

## Safety and Repo Hygiene

- Check `git status` before and after delegated edit tasks.
- Keep delegation scoped; request broad refactors only when the user requested them.
- Ask `ogent` to preserve tests, surface failures, keep acceptance criteria stable, and leave destructive operations to explicit user requests.
- Treat `.ogent/sessions/` and build outputs as runtime artifacts; read them only when needed and edit them only when requested.
- Claim `ogent` ran checks or changed files only after you observed the output or inspected the repository state.
