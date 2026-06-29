# Lua Toolset Git

Use structured git globals instead of raw shell git when you need status, diffs, changed-file
metadata, line numbers, or commit history.

All paths are workspace-relative and validated before use.

## `git_status{staged=..., paths=..., untracked=...}`

Returns a Lua array of file change entries.

- `staged=true`: staged changes.
- `staged=false`: worktree changes.
- omit `staged`: all changes.
- `paths`: optional array of path filters.
- `untracked`: optional bool, defaults true.

Each entry includes:

- `path`, optional `old_path`
- `status`
- `staged`, `worktree`
- `display`
- `state_description`

Use before editing files in a git workspace.

## `git_diff{staged=..., base=..., paths=..., context=..., stat_only=...}`

Returns parsed file deltas with optional hunks.

- `staged=true`: index vs HEAD.
- `base`: compare against a ref such as `"HEAD~1"` when not staged.
- `paths`: optional path filters.
- `context`: defaults 3, capped at 20.
- `stat_only=true`: omit hunks.

Delta fields include:

- `path`, `old_path`
- `change_type`
- `is_binary`
- `insertions`, `deletions`
- `hunks`

Hunk lines include:

- `type`: `"context"`, `"deletion"`, or `"addition"`
- `text`
- `old_line`, `new_line`

Use `stat_only=true` first for large changes.

## `git_changes{paths=..., context=..., stat_only=..., base=..., symbols=...}`

Convenience function for current staged and worktree changes.

- Returns status entries with `diff` and/or `staged_diff` attached.
- `symbols=true` attaches best-effort outline symbols for changed source hunks.
- Use `git_changes{symbols=true, context=0}` to route review or edits before reading full files.

Unsupported files, deleted files, binary files, or changes outside outline entries simply omit
`symbols`.

## `git_show{path=..., ref=...}` or `git_show(path, ref)`

Reads a file at a git ref without checking it out.

- `path`: workspace-relative file.
- `ref`: optional, defaults `HEAD`.
- Use `ref="staged"` to read the staged/index version.

## `git_log{paths=..., n=...}`

Returns commit history entries.

- `paths`: optional array of path filters.
- `n`: defaults 10, capped at 100.
- Returns empty array when no history exists.

Entries include `sha`, `subject`, `author`, and `date`.

## Git Review Pattern

```lua
local changes, err = git_changes{symbols = true, context = 0}
if not changes then error(err) end

local out = {}
for _, e in ipairs(changes) do
  table.insert(out, e.display .. " " .. e.path .. " " .. e.status)
  if e.symbols then
    for _, s in ipairs(e.symbols) do
      local span = s.start_line .. "-" .. (s.end_line or s.start_line)
      table.insert(out, "  " .. s.kind .. " " .. s.name .. " " .. span)
    end
  end
end
return table.concat(out, "\n")
```

For large diffs, scope first with `git_status` or `git_diff{stat_only=true}`, then inspect
specific files or hunks.
