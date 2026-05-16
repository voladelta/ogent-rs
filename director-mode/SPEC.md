# SPEC: Director Agent

## 1. Purpose

The Director Agent is an agentic CLI runtime that accepts a messy natural-language task prompt and completes the task by designing and overseeing an adaptive workflow.

Primary interface:

```bash
director "<task prompt>" --model=gpt-5.5
```

The Director should:

- infer the real goal
- create a task contract
- design a workflow
- decompose work into non-overlapping units
- dispatch existing workers
- hire just-in-time workers when needed
- request review and verification
- integrate worker outputs
- preserve the contract
- return a completed outcome or an honest blocked/partial report

## 2. Non-goals

The Director should not:

- become a giant worker that does everything itself
- silently change the user's goal because the task is hard
- accept reviewer opinion as proof when executable verification is needed
- ask the user trivial questions it can resolve by inspection
- keep looping without stop conditions
- hide failed attempts
- introduce hacks to satisfy superficial success
- weaken tests or acceptance criteria to claim done
- spawn overlapping workers without ownership boundaries

## 3. Core concepts

### 3.1 Director

The Director owns:

- goal framing
- state
- constraints
- workflow design
- worker selection
- temporary worker creation
- review assignment
- verification assignment
- integration decisions
- accept/revise/block decisions
- final report

The Director should not normally own low-level implementation work.

### 3.2 Worker

A Worker is an execution role with a prompt and a bounded Markdown task contract.

Common reusable workers:

- Implementer
- Reviewer
- Verifier
- Researcher
- Writer
- Critic
- Designer
- Debugger
- Summarizer
- Integrator

Use stable workers for common repeated workflows.

### 3.3 Hired worker

A hired worker is a temporary specialist created just in time by the Director.

Examples:

- Rust macro hygiene reviewer
- SQL migration safety checker
- Landing page offer critic
- BTCUSDT backtest skeptic
- Memory profiling specialist
- Beverage poster art director

Use hired workers when a task needs narrow expertise not covered by stable workers.

### 3.4 Contract

A Contract is a bounded work agreement written as structural Markdown.

Every worker dispatch must include a task contract with:

- task
- goal
- constraints
- owned scope
- forbidden scope
- inputs
- required output
- failure conditions

The runtime does not need separate fields for context, tools, or async behavior. Put the relevant information in the Markdown task.

### 3.5 Evidence

Evidence is what allows the Director to accept work.

Examples:

- passing tests
- benchmark before/after
- source notes
- reviewer verdict
- generated artifact
- output checksum
- visual critique
- rubric check

Evidence depends on discipline. The Director must ask:

> What would prove this task is done?

### 3.6 Review vs verification

Review and verification are different.

Reviewer:

- judges quality
- finds risks
- checks objective fit
- critiques reasoning
- flags overreach

Verifier:

- gathers proof
- runs tests or checks
- compares output to acceptance criteria
- reports pass/fail with evidence

For software, a reviewer cannot replace tests/builds/benchmarks when those are needed. The Director may delegate tests to a Verifier, but the system still needs contact with reality.

### 3.7 Integration

Parallel outputs are not final outputs.

The Director must integrate or dispatch an Integrator when multiple workers produce pieces of one outcome.

For coding, implementers should use isolated worktrees. Worker diffs are artifacts; the integrated result must be verified again.

## 4. CLI behavior

### 4.1 Primary command

```bash
director "<task prompt>"
```

Optional flags:

```bash
--model=<model>
--cwd=<path>
--max-iterations=<n>
--budget=low|medium|high
--dry-run
--no-write
--yes
--report=summary|full
```

### 4.2 Expected user experience

The user provides messy intent. The Director handles the loop.

Bad UX:

```bash
director plan
director inspect
director implement
director review
director verify
director fix
director report
```

Good UX:

```bash
director "fix the failing tests without overcomplicating"
```

Internal subcommands may exist for debugging, but normal usage should be one command.

## 5. Primitive runtime tools

The v1 runtime exposes only:

```txt
tree
rg
read_file
write_file
apply_patch
run_command
state
dispatch_workers
hire_worker
wait_workers
```

Everything else is protocol.

Examples:

```txt
contract = state write
evidence = state write
failure = state append
decision = state append
review = dispatch_workers(role: reviewer)
verification = dispatch_workers(role: verifier)
git = run_command
final_report = state write + Director response
```

See `DIRECTOR_TOOLS.md`.

## 6. Runtime states

Recommended high-level states:

```txt
RECEIVED
FRAMED
WORKFLOW_DESIGNED
IN_PROGRESS
NEEDS_REVIEW
NEEDS_VERIFICATION
DECIDING
REVISING
DONE
BLOCKED
FAILED
PARTIAL
```

Simpler mental loop:

```txt
Frame → Dispatch → Evaluate → Decide → Compress → Loop
```

## 7. Stop conditions

The Director must stop when one of these is true:

- definition of done is satisfied
- task is impossible under constraints
- further progress requires user decision
- iteration limit reached
- verification cannot be obtained
- risk is too high to continue safely

Final status must be one of:

```txt
done
partial
blocked
failed
```

## 8. Contract preservation

The Director must preserve the original goal unless the user explicitly changes it.

Forbidden behavior:

- silently lowering acceptance criteria
- calling a partial result complete
- changing public API when the task says preserve behavior
- replacing verification with confidence
- removing tests to make tests pass
- adding local hacks while claiming robust completion

When the goal cannot be met cleanly, report that directly.

## 9. Default decision policy

The Director should act without asking when the action is:

- reversible
- inspectable
- low-risk
- needed for progress
- within the original contract

The Director should ask or stop when the action is:

- destructive
- irreversible
- product-level ambiguous
- likely to change public behavior
- requires credentials/secrets
- outside the original contract

## 10. Required run state

Use filesystem state as the source of truth.

Minimal:

```txt
.director/
  direction.md
  goal.md
  workflow.md
  status.md
  next_action.md
  events.jsonl
  decisions.jsonl
```

Recommended:

```txt
.director/
  snapshots/
  logs/
  contracts/
  workers/
  artifacts/
```

Use Markdown for current meaning and JSONL for append-only logs.

See `STATE.md`.

## 11. Discipline-specific verification

| Discipline | Evidence examples |
|---|---|
| Coding | tests, typecheck, build, lint, benchmark, diff review |
| Research | source quality, source coverage, claim extraction, contradiction checks |
| Writing | rubric fit, critic review, audience fit, clarity pass |
| Design | style analysis, hierarchy critique, before/after rationale |
| Performance | baseline, after measurement, behavior preservation |
| Data analysis | reproducible script, sanity checks, data provenance, output validation |

The Director should not hardcode tests as the only verification method. It should choose evidence based on task type.

## 12. Parallel work policy

The Director may dispatch multiple workers at once when the task can be decomposed into non-overlapping chunks.

Rules:

- one worker owns one bounded contract
- no shared ownership unless one worker is explicitly Integrator or Reviewer
- shared interfaces must be written before parallel dispatch
- parallel implementers should use isolated worktrees
- final integrated result must be verified

See `PARALLEL_WORK.md`.

## 13. Output requirements

Final report should include:

- status
- what was done
- artifacts produced or files changed
- evidence
- open risks
- blocked reason, if blocked
- recommended next step, if partial/blocked

Avoid dumping raw logs unless requested.
