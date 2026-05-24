---
name: ogent
description: Use ogent as an external coding agent for focused software engineering tasks. Invoke when you need an independent CLI agent to implement, debug, review, verify, research, summarize, design, or write within the current repository.
---

# ogent

Use `ogent` to delegate one focused repository task to an external coding agent. You remain responsible for framing, scope, verification, and final judgment.

## Task Contract

Give ogent a concrete contract, not a loose request. Put these fields in order:

```text
Goal:
Success means:
Context:
Scope:
Constraints:
Reasoning allocation:
Stop when:
Evidence required:
Expected output:
Claim standard:
```

Field guidance:
- `Goal`: one outcome, stated as behavior or artifact.
- `Success means`: observable acceptance criteria.
- `Context`: facts the agent should rely on before exploring.
- `Scope`: allowed files, commands, topics, and artifacts.
- `Constraints`: what must be preserved or avoided.
- `Reasoning allocation`: where to think deeply, and where to act directly.
- `Stop when`: completed, partial, blocked, or question condition.
- `Evidence required`: tests, commands, diffs, examples, or traces needed for a claim.
- `Expected output`: concise report shape.
- `Claim standard`: no success claim without matching evidence.

## Contract Nudges

Use these nudges in contracts when they fit the task:

- For implementation: make the smallest correct version work, verify it, make it right, then stop.
- For optimization or broad cleanup: defer until the requested behavior works and is correct.
- For code shape: after discovery identifies the implementation shape, use at most three short bullets: behavior to cover, file to edit, invariant to preserve. Then write the smallest covering test or edit the file. Do not draft planned functions, modules, helper names, extraction logic, pseudocode, or code-like snippets in reasoning unless choosing between materially different designs.
- For stale anchors: plan all same-file edits from one fresh snapshot when possible; after editing a file, re-read anchors before another edit round.
- For reasoning: spend thought on invariants, failure modes, validation paths, public behavior, and irreversible choices; act directly on obvious reversible steps.
- For failed checks: read the exact error, inspect implicated code, make one targeted edit, rerun the focused check, then reassess.
- For reviews: report only findings tied to the requested goal; put adjacent concerns under risks or next action.

For security, sandbox, parser, validation, execution, or correctness claims, require a concrete trace: one input through the check path and runtime/effect path, the protected invariant, and the classification.

## Optional Precision Blocks

Add these only when they reduce ambiguity:

```text
Role:
Procedure:
Candidate filter:
Finding template:
Classification labels:
Validation note:
Edge-case check:
```

Useful review labels:
- `must-fix-now`: breaks the requested behavior, invariant, security boundary, data contract, or maintainability threshold.
- `can-let-slip`: real but not blocking for this task.
- `missing-precursor`: cannot judge without named evidence.
- `reject`: change should not land.

## Invocation

Run from the repository root:

```bash
ogent --profile kimi "$(cat <<'TASK'
Goal:

Success means:

Context:

Scope:

Constraints:

Reasoning allocation:

Stop when:

Evidence required:

Expected output:
- Status: completed | partial | blocked | question
- Summary
- Changed files or reviewed files
- Verification
- Risks or uncertainty
- Next action, only if useful

Claim standard:
- Claim only what was observed.
- Include failing output when it affects the result.
TASK
)"
```

Use `--profile kimi` when the user asks for the kimi profile. Otherwise choose the profile that matches the task or the user's request.

Keep contracts short. Include repo-specific commands and file paths only when they are relevant evidence or scope boundaries.

## Handling Results

After ogent returns:
1. Read its report and inspect any changed files or claimed evidence.
2. Verify important claims yourself with the strongest practical check.
3. Classify the outcome as completed, partial, blocked, or question.
4. Summarize what you accept, what remains uncertain, and the next useful step.

For long sessions, inspect the `.ogent/sessions/*.jsonl` trace when behavior quality matters. Look for scope drift, excessive narration, stale-anchor edits, missing verification, or claims without evidence; then tighten the next task contract.

## Safety

Do not delegate secrets, credentials, destructive git operations, deployment, or broad rewrites unless explicitly requested and scoped.

Do not let ogent's conclusion replace your judgment. Treat it as an independent worker whose output needs synthesis and verification.
