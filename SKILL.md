---
name: ogent
description: Use ogent as an external coding co-worker for focused software engineering tasks. Invoke when you need an independent CLI agent to implement, debug, review, verify, research, summarize, design, or write within the current repository.
---

# ogent

Use `ogent` when an independent coding co-worker would help make concrete progress in the current repository.

Run `ogent` from the repository root or the intended workspace. Its workspace is the process current directory.

## Task Contract

Write the prompt sent to `ogent` in this field order. Fill every field with task-specific content. Keep the contract compact, concrete, and self-contained.

```text
Goal:
<one sentence naming the desired outcome for ogent>

Success means:
- <observable result>
- <acceptance criteria>
- <required evidence>
- <required verification or inspection>

Context:
<facts already known; include user intent, prior findings, relevant files, and branch state when useful>

Scope:
<files, directories, commands, topics, and allowed change area>

Constraints:
<edit permission, public API boundaries, safety boundaries, secrets/destructive-action limits, and runtime limits>

Stop when:
<exact condition that ends ogent's run>

Evidence required:
<commands, files, diffs, logs, traces, or reasoning ogent must report>

Expected output:
Use exactly these top-level sections: # Status, # Summary, # Changed Files, # Verification, # Evidence, # Risks, # Question, # Next Action.

Claim standard:
For security, sandbox, parser, validation, or correctness claims, give one concrete input, trace the validation/check path, trace the runtime/effect path, and name the invariant satisfied or violated before classifying the issue.
```

Strong contracts name the target state, acceptance criteria, relevant paths, permitted changes, constraints, evidence, and stop condition. Use relative paths. Keep the `task` argument focused on task-specific outcomes and evidence requirements.

Before invoking `ogent`, check the contract:
- Goal names one outcome.
- Success criteria are observable.
- Context gives enough facts for an independent run.
- Scope names the relevant files, directories, commands, or topics.
- Constraints state edit permission and hard boundaries.
- Stop condition tells `ogent` when to finish.
- Evidence requirements are inspectable by you after the run.

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
- The failing behavior is reproduced or a specific blocker is reported.
- The root cause is supported by source or test evidence.
- The smallest justified next step is identified.
- Verification evidence or the reason verification is unavailable is reported.

Context:
The failure appears after changing token normalization.

Scope:
src/parser.rs, src/lexer.rs, parser tests, and commands needed to reproduce the parser failure.

Constraints:
Treat tests as intended-behavior evidence. Edit source only after identifying the root cause. Edit tests when evidence shows the test encodes stale behavior. Keep parser public APIs unchanged.

Stop when:
The root cause and smallest justified next step are clear, or the run reaches a specific blocker.

Evidence required:
Reproduction command, relevant failing output, source/test trace, root-cause evidence, and minimal fix path.

Expected output:
Use exactly these top-level sections: # Status, # Summary, # Changed Files, # Verification, # Evidence, # Risks, # Question, # Next Action.

Claim standard:
For security, sandbox, parser, validation, or correctness claims, give one concrete input, trace the validation/check path, trace the runtime/effect path, and name the invariant satisfied or violated before classifying the issue.
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
