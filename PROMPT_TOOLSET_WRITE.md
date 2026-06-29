# Lua Toolset Write

Use this shard only when editing files is allowed or likely. Preserve existing behavior unless
the task contract explicitly changes it.

## Write Discipline

- Inspect the relevant files before editing.
- Prefer targeted anchored edits over full-file rewrites.
- Avoid unrelated formatting churn.
- Do not edit tests, examples, snapshots, or benchmarks merely to pass checks.
- If an edit invalidates the plan, stop and revise before continuing.
- Verify after writing.

## File Mutation Tools

### `write_file{path=..., content=..., overwrite_existing=...}`

Writes complete file content.

- Parameters:
  - `path`: workspace-relative path.
  - `content`: full new file content.
  - `overwrite_existing`: optional bool. If false or omitted, existing files cause an error.
- Use for new files or intentional full-file replacement.
- Avoid for targeted source changes unless a full rewrite is truly the smallest correct move.

### `append_file(path, content)`

Appends content to a file, creating it if needed.

- Use for logs, scratch notes, or append-only artifacts.
- Do not use for source edits.

### `read_hash_anchors(path, offset, limit)`

Reads a file with each line prefixed as `line:hash|content`.

- Use before `apply_anchor_edits`.
- Same 1 MB file-size limit as `read_file`.
- Anchors are stale if the file changes; re-read before applying uncertain edits.

### `preview_anchor_edits(path, ops)`

Validates an anchored edit batch and returns a bounded unified diff without writing.

- Same `ops` shape as `apply_anchor_edits`.
- Use before risky or multi-edit changes.
- Long previews are truncated with a visible marker.

### `apply_anchor_edits(path, ops)`

Applies a batch of anchored edits to one file.

Each op has:

- `start_at`: required `"line:hash"` anchor, without `|content`.
- `end_at`: optional `"line:hash"` anchor for inclusive ranges.
- `action`: one of `"replace"`, `"delete"`, `"insert_before"`, `"insert_after"`.
- `content`: required for insert/replace, omitted for delete.

Rules:

- Range replace/delete removes both endpoints and everything between them.
- Ops in one batch must be non-overlapping.
- The batch is all-or-nothing for the file.
- If any anchor mismatches, no edit is applied.

Example:

```lua
local anchors, err = read_hash_anchors("src/main.rs")
if not anchors then error(err) end

local ops = {
  { start_at = "12:b5f2", action = "replace", content = "let value = 42;" },
  { start_at = "20:a1c3", action = "insert_after", content = "println!(\"done\");" }
}

local preview, err = preview_anchor_edits("src/main.rs", ops)
if not preview then error(err) end

local ok, err = apply_anchor_edits("src/main.rs", ops)
if not ok then error(err) end
```

## Edit Cycle

For targeted code edits:

1. Inspect worktree state if git tools are available.
2. Locate the relevant symbol or line range with search, outline, and reads.
3. Read anchors for the smallest useful region or file.
4. Build one non-overlapping ops batch.
5. Preview when useful.
6. Apply once.
7. Verify with tests, type checks, lint, build, or direct diff inspection.

If an anchor is stale, use the mismatch line only when the intended line is clearly unchanged.
Otherwise re-read anchors and re-plan.
