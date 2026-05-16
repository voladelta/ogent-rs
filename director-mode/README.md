# Director Agent Kit

A language-neutral specification and prompt kit for building a **Director**: an agentic CLI that turns messy task prompts into completed work through contracts, workers, just-in-time workers, review, verification, and evidence-based decisions.

Target user experience:

```bash
director "<task prompt>" --model=gpt-5.5
```

The user should not have to manually run `review`, `verify`, `fix`, or `resume` during normal use. The Director designs and oversees the workflow.

## Core idea

A Director is not a worker with many tools.

A Director is the control layer that:

1. frames messy intent into a task contract
2. designs a workflow
3. decomposes work into non-overlapping contracts
4. dispatches existing workers in parallel or sequence
5. hires temporary workers when needed
6. demands evidence
7. preserves the original contract
8. integrates outputs
9. accepts, revises, blocks, or reports honestly

The shortest definition:

> Director: the agent that decides what should happen next, who should do it, under what contract, and what evidence proves it worked.

## Mental model

```txt
Messy user prompt
  ↓
Task contract
  ↓
Workflow design
  ↓
Worker dispatch / hired worker creation
  ↓
Output
  ↓
Review / verification
  ↓
Integration
  ↓
Director decision
  ↓
Outcome / revise / blocked
```

## Primitive runtime

The runtime stays small:

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

Everything else is protocol encoded in Markdown, paths, logs, and worker contracts.

## Included files

```txt
README.md
SPEC.md
ARCHITECTURE.md
RUN_LOOP.md
CONTRACTS.md
DIRECTOR_TOOLS.md
STATE.md
PARALLEL_WORK.md
SYSTEM_PROMPT_DIRECTOR.md
ROADMAP.md

prompts/
  CONTRACTOR_FACTORY.md
  workers/
    implementer.md
    reviewer.md
    verifier.md
    researcher.md
    writer.md
    critic.md
    designer.md
    debugger.md
    summarizer.md

schemas/
  state.schema.json
  contract.schema.json
  event.schema.json

examples/
  execution_traces.md
```

## Recommended MVP

Expose one command:

```bash
director "<task prompt>" [--model=gpt-5.5] [--cwd=.] [--max-iterations=5] [--budget=medium]
```

Internally support:

- task framing
- workflow planning
- filesystem Markdown state
- batch worker dispatch
- temporary worker creation
- isolated worktrees for coding implementers
- review
- verification
- final reporting

## What this is not

This is not a swarm framework.

This is not a generic automation engine.

This is not a fixed pipeline.

The Director should be a **contract-preserving workflow designer**. It should adapt the workflow to the task while keeping the goal, constraints, and evidence requirements stable.
