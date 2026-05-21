# What feedback calibration would mean for ogent

The Director makes decomposition decisions blind every time: how many workers, what role, what scope boundary, serial or parallel. No learning from past outcomes. The calibration problem is: **can the Director get better at decomposition over multiple sessions in the same workspace?**

## Three layers

1. What to track (signal design)
After each worker batch completes, store a minimal calibration entry:
```
role: implementer
task_type: refactor | feature | bugfix | test
scope_size: 1-file | 2-5-files | 6+-files | cross-module
outcome: completed | partial | blocked | failed
retries: 0 | 1 | 2+
parallelized: yes | no
contract_quality: well_scoped | too_broad | ambiguous | wrong_role
```

Key design choice: `scope_size` and `contract_quality` are the Director's self-assessment, not objective measures. The Director judges its own decomposition after seeing the result. This is realistic — a manager learns "I gave them too much" by seeing the output.

2. Where to store it (persistence)
Two options:
- **Per-session state** (calibration_log in states.json) — dies with the session, only helps within one run
- **Cross-session artifact** (.ogent/calibration.jsonl) — persists across sessions, accumulates workspace knowledge

Cross-session is the real value. But it's a new persistence surface. The tradeoff: adding a new file vs getting calibration that's actually useful.

3. How the Director uses it (the loop)
**Post-batch (write side)**:
After integrating worker results, the Director appends a self-assessment: "Was this contract the right size? Did I parallelize correctly? Should I have split this differently?"

**Pre-dispatch (read side)**:
Before planning, the Director checks calibration history. Decision rules emerge naturally from patterns:
- "Last 3 `implementer` dispatches with `scope_size`: 6+-files all failed → split this one"
- "`researcher` + `implementer` parallel batch succeeded 8/10 times → safe to parallelize"
- "`debugger` with `contract_quality`: ambiguous always needed retry → invest more in the contract"

## Why this is hard in practice
**Noisy signal**. A worker's failure could be:
1. Bad decomposition (contract too big, wrong role) → calibration should catch this
2. Model capability limit (the model literally can't do it) → calibration can't fix this
3. Unusual edge case (one-off weirdness) → calibration would learn the wrong lesson

Distinguishing 1 from 2/3 is the core difficulty. The Director has to judge root cause, and it can be wrong.

**Cold start**. A new workspace has no calibration data. The first N sessions are blind anyway. This is acceptable — it mirrors how a human manager learns a team.
Context budget. Calibration data in the prompt costs tokens. A workspace with 50 prior sessions would have a large calibration log. Need compaction: aggregate by role + task_type, not preserve raw entries.

## Simplest implementable version
No Rust changes. Prompt-only. Add to SYSTEM_PROMPT.md:
1. A calibration_log state key with a compact format
2. A rule: after each worker batch integration, append one calibration entry
3. A rule: before dispatching, scan calibration for relevant patterns and adjust scope/parallelism if patterns suggest it

## Calibration
After integrating each worker batch, compact the outcome into `calibration_log`:
```
role: <role>
task_type: explore | design | implement | debug | review | verify
scope: <1-file | 2-5-files | 6+-files | cross-module>
outcome: completed | partial | blocked
retries: <count>
parallelized: yes | no
decomposition_quality: <good | too_broad | ambiguous | wrong_role>
```
Before dispatching, check `calibration_log` for patterns:
- 2+ failures on similar scope → split or narrow
- parallel conflicts on specific file areas → serialize
- chronic wrong-role failures → reconsider role mapping

The Director itself does the tracking and the learning. Zero infrastructure, pure protocol. If it proves valuable after real use, then invest in a proper cross-session calibration file.
