Create one ogent skill.

Output only the final `SKILL.md` file content. Do not wrap it in a code fence. Do not include explanations, analysis, or extra files.

Hard requirements:
- Start with YAML frontmatter containing exactly `name` and `description`.
- `name` must exactly match the requested artifact name.
- `description` must be one sentence that clearly states when the skill should be used.
- The body must be concise, procedural Markdown that tells an agent how to perform the reusable capability.
- Include examples, scripts, references, or assets sections only when they are useful for the objective.
- Do not include absolute local paths, credentials, secrets, destructive commands, or instructions that conflict with the user's repository safety rules.
