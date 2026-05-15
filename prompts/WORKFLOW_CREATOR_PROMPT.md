Create or improve one ogent workflow.

Output only the final workflow YAML. Do not wrap it in a code fence. Do not include explanations, analysis, or extra files.

Hard requirements:
- The YAML must deserialize into ogent's `Workflow` schema.
- `id` must exactly match the requested artifact name.
- `name` must be a human-readable title.
- `version` must be `1`.
- `start` must name an existing step.
- Define at least one terminal step.
- Every non-terminal step must have at least one `next` step.
- Every `next` step must exist.
- Use gates and required checks when evidence or review must be enforced.
- For command checks, use only commands explicitly named by the user or broadly standard for the implied stack, such as `cargo test` or `cargo build --release` for Rust. Do not invent project-specific script paths.
- Keep the workflow small enough to be followed during real work.
