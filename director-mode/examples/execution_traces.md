# Example Execution Traces

These traces show how the Director behaves across different disciplines.

## Trace 1: Coding

User prompt:

```txt
fix the failing tests, don't overcomplicate
```

### Frame

```json
{
  "goal": "Make the failing tests pass with the smallest correct code change.",
  "definition_of_done": [
    "Identify failing tests",
    "Find root cause",
    "Apply minimal fix",
    "Run relevant tests",
    "Avoid broad refactor"
  ],
  "constraints": [
    "Do not overcomplicate",
    "Do not weaken tests",
    "No hacks"
  ]
}
```

### Workflow

```txt
Verifier → Debugger → Implementer → Reviewer → Verifier → Director decision
```

### Events

```json
{"event":"task.framed","output_summary":"Goal and definition of done created."}
{"event":"worker.dispatched","actor":"Verifier","step":"find failing tests"}
{"event":"worker.completed","actor":"Verifier","output_summary":"parseConfig.test.ts failing: timeout default undefined."}
{"event":"worker.dispatched","actor":"Debugger","step":"root cause"}
{"event":"worker.completed","actor":"Debugger","output_summary":"Default config merge was removed."}
{"event":"worker.dispatched","actor":"Implementer","step":"minimal patch"}
{"event":"worker.completed","actor":"Implementer","output_summary":"Restored default merge."}
{"event":"worker.dispatched","actor":"Reviewer","step":"patch review"}
{"event":"worker.completed","actor":"Reviewer","output_summary":"Pass. Minimal, no test weakening."}
{"event":"worker.dispatched","actor":"Verifier","step":"run tests"}
{"event":"worker.completed","actor":"Verifier","output_summary":"Relevant and full tests pass."}
{"event":"director.accepted","decision":"done"}
```

### Final report

```txt
Done.

Changed:
- src/config/parseConfig.ts

What changed:
- Restored default config merge so missing fields receive defaults.

Evidence:
- Initial failure reproduced
- Patch reviewed
- Relevant tests pass
- Full test suite passes
```

## Trace 2: Research

User prompt:

```txt
find me the best option for building a fast tui chat app in go, few deps
```

### Frame

```json
{
  "goal": "Recommend a Go TUI stack for a fast chat-style terminal UI with few dependencies.",
  "definition_of_done": [
    "Identify viable libraries",
    "Compare against speed, dependency count, chat UI fit, maintainability",
    "Recommend primary option and fallback"
  ],
  "constraints": [
    "Prefer few dependencies",
    "Must support chat UI patterns",
    "Avoid over-engineered framework"
  ]
}
```

### Workflow

```txt
Researcher → Go TUI contractor → Critic → Director decision
```

### Decision

```txt
Primary recommendation depends on meaning of fast:

- Fastest to build: Bubble Tea
- Fewest deps/control: tcell

Director final pick:
Use Bubble Tea if shipping the chat UI matters.
Use tcell if strict minimal deps and custom control matter more.
```

## Trace 3: Writing

User prompt:

```txt
write a sharp post about software as cached agents, make it sound smart but not cringe
```

### Workflow

```txt
Strategist → Writer → Critic → Editor → Director decision
```

### Draft after revision

```txt
Software is not the opposite of agents.

Software is what you get when agency becomes stable enough to cache.

A person repeats a workflow.
An agent performs it under instruction.
A contract forms around the steps.
Verification hardens the contract.
Eventually, the workflow stops needing judgment every time.

That cached workflow becomes software.

So the real question is not:
"Will agents replace apps?"

It is:
"Which messy workflows are finally stable enough to be compressed?"
```

### Decision

```txt
Accepted.

Evidence:
- Critic flagged abstraction/hype issues.
- Editor added contracts and verification.
- Director checked against no-cringe constraint.
```

## Trace 4: Design

User prompt:

```txt
make this poster deeper, more premium, same cute matcha vibe
```

### Workflow

```txt
Visual Analyst → Beverage Poster Art Director contractor → Designer → Critic → Director decision
```

### Output direction

```txt
Use editorial product-stage composition:
- diagonal depth from lower-left to upper-right
- cream 3D product platform
- mint/lavender background panels
- controlled graphic shadows
- fewer doodles, but keep 3–5 cute accents
- stronger headline hierarchy
- paper grain for print-premium texture
```

### Decision

```txt
Accepted with risks.

Risks:
- Too much restraint may remove charm.
- Realistic shadows may clash with flat cute style.

Constraint:
Use graphic shadows, not photorealistic shadows.
```

## Trace 5: Optimization with blocked result

User prompt:

```txt
make memory usage 10x lower
```

### Workflow

```txt
Verifier baseline → Memory profiling contractor → Implementer → Verifier → Debugger → Director decision
```

### Evidence

```txt
Baseline:
Peak RSS 1.2 GB

Streaming patch:
Peak RSS 180 MB
Reduction about 6.7x

Problem:
Output checksum differs because original implementation globally sorts records.
```

### Decision

```txt
Blocked / partial.

Reason:
Naive streaming improves memory but changes behavior.
10x reduction likely requires external chunked sort or changed input contract.

Director does not accept the patch as done.
```


---

# Parallel dispatch trace

## Prompt

```txt
add config file support to the app
```

## Director frames task

```md
# Goal

Add config file support while preserving existing CLI flags.

# Definition of done

- Config file can be loaded
- CLI flags override config values
- Frontend can display/edit config
- Backend/API supports config loading
- Docs are updated
- Integrated checks pass
```

## Director writes shared API contract

```txt
contracts/shared/config-api.md
```

## Director dispatches workers in parallel

```ts
dispatch_workers({
  workers: [
    { role: "implementer", task: "# Task
Backend config loading..." },
    { role: "implementer", task: "# Task
Frontend config UI..." },
    { role: "writer", task: "# Task
Docs update..." }
  ]
});
```

## Director waits

```ts
wait_workers({});
```

## Director integrates

```ts
dispatch_workers({
  sync: true,
  workers: [
    { role: "reviewer", task: "# Task
Review each worker output for scope violations." },
    { role: "implementer", task: "# Task
Integrate accepted branches/patches." },
    { role: "verifier", task: "# Task
Run final integrated checks." }
  ]
});
```

## Decision

Accept only if the integrated result satisfies the original task contract.
