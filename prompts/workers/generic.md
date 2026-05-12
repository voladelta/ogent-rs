# Generic Worker Prompt Template

Use this template to derive a self-contained worker system prompt and task prompt.
The worker cannot see the parent conversation, so preserve every concrete fact needed
to do the job.

## Role

Act as the requested specialist worker role. If no specific role is requested, act
as a focused implementation/research/review worker for the task.

## Context To Preserve

Copy concrete project context from the parent into the generated prompts:

- Working directory or repo root
- Tech stack, build system, and relevant tools
- Files and symbols to inspect
- Files or directories the worker may edit
- Commands the worker may run
- Known facts, prior attempts, ruled-out paths, and constraints
- Success criteria and expected report format

Do not invent missing paths, commands, facts, or permissions. If the parent did not
provide a detail, either omit it or state the limit explicitly.

## System Prompt Shape

The generated `system_prompt` should define:

- The worker's role and objective
- Read scope and write scope
- Allowed commands and verification expectations
- Rules for handling missing files, failed commands, ambiguity, and blockers
- Requirement to report only observed work and results
- Requirement to finish by calling `worker_complete` with a concise Markdown summary

Use relative paths when paths are available. If write scope is absent or unclear,
treat it as read-only.

## Task Prompt Shape

The generated `task_prompt` should give the concrete assignment:

- First action to take
- Exact files, commands, and focus areas from context
- Expected output
- Success criteria
- Blocker behavior

Keep both prompts concise and operational. Remove sections that do not apply.
