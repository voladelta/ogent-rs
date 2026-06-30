# Lua Toolset Guide

This repository-only file is a developer and offline review reference. It is not embedded as a
runtime prompt artifact. Runtime prompt assembly injects `PROMPT_TOOLSET_CORE.md` by default;
the agent loads `PROMPT_TOOLSET_GIT.md`, `PROMPT_TOOLSET_WRITE.md`, and
`PROMPT_TOOLSET_SUBAGENT.md` on demand with `load_toolset(name)`.

You execute all workspace operations by writing Lua 5.5 code inside either the `exec` (stateless, one-off) or `eval` (stateful, persistent session) tools. You DO NOT call tools via JSON schema; instead, you write Lua scripts that call the registered global functions.

## Tool Selection: `exec` vs `eval`

* **Use `exec` (Stateless)** for simple, one-off, or self-contained operations that do not need to persist state between agent turns (e.g., executing a single build/test command, reading a specific file, performing a one-off search). This keeps the environment clean and avoids side effects.
* **Use `eval` (Stateful)** when you want to define helper functions, declare globals, or retain state for later turns. Prefer it for multi-step exploration of large files, structured git data, long shell output, or bulky context: load once, filter/map/reduce in Lua, keep intermediate tables in session state, and print or return only the compact result needed for the next decision. `eval` does not bypass the 32,768-character output cap; it helps you stay under it.

> **Important: `exec` and `eval` DO NOT share state.** They run in completely separate Lua VMs. Variables or functions set in one `eval` call persist for future `eval` calls, but an `exec` call can never see them (and vice versa). If you need state, use `eval` consistently.

## Retrieval Discipline

Treat retrieval as a sequence of evidence-narrowing moves, not as a single magic search.

Default ladder:

1. **Exact clue available**: If the user, compiler, test, stack trace, docs, or file gives you an exact symbol, error string, config key, path, command name, or phrase, start with `search_text` or a bounded shell `rg`. Exact search is usually the fastest path to source evidence.
2. **Intent clue only**: If you only know the behavior in natural language, start with `colgrep` via `shell`, then confirm candidates with exact tools (`search_text`, `outline`, `read_lines`, `git_changes`, tests).
3. **Candidate file found**: Use `outline` for supported source files, then read the smallest relevant range with `read_lines` or `read_hash_anchors`. Avoid reading whole files just because a search found them.
4. **Many matches or bulky output**: Use `eval` to keep result tables in session state, filter/rank there, and print only compact evidence: path, line, symbol, and the few lines needed for the next decision.
5. **No match**: Change one thing at a time: broaden the exact term, switch case sensitivity, search filenames with `glob`, then try semantic `colgrep` if the issue may be phrased differently.
6. **Before final claims**: Verify with the strongest practical evidence for the task: tests, type checks, build output, git diff, or direct file references.

Do not treat semantic search results as proof. They are candidate locators. Read the relevant source and verify before editing or answering.

## Sandbox Constraints & Rules

1. **Virtual Root / Workspace Path Restriction**: Inside the sandbox, paths must be relative to the workspace root (e.g. `'src/main.rs'`). Do not use host absolute paths (e.g. `/Users/mbp/...`).
2. **Library Restrictions**: The Lua sandbox only permits safe standard libraries: `table`, `string`, `math`, `utf8`, and `coroutine`. Unsafe modules like `os`, `io`, `debug`, and `package` are completely unavailable.
3. **Execution Limits**:
   - Memory is capped at **32MB**.
   - CPU is limited to **32,000 instructions**. Avoid infinite or highly nested loops.
    - Stdout printing / return value output is capped at **32,768 characters**.
4. **Data Return Channel**:
   - `print(...)` writes to the captured stdout buffer. Arguments are separated by tabs, and newlines separate print statements.
   - Any value returned by the script is serialized as JSON and output as the final return value.
   - Return values are returned to you as:
     ```text
     --- Stdout Output ---
     <captured print calls>

     --- Return Value ---
     <JSON representation of the returned value>
     ```

---

## Global Functions Reference

Most workspace functions return `(result, nil)` on success or `(nil, error_string)` on failure. Handle those errors cleanly. Exceptions are called out explicitly: `task_update` returns no result, while `agent` and `parallel` return their value directly or raise a Lua runtime error.

> **Note on structured data**: Functions that return structured data (e.g. `glob`, `git_status`, `git_diff`, `git_changes`, `git_log`, `file_info`) automatically decode JSON into native Lua tables. There is no `json_decode` global — this keeps the abstraction clean and prevents scripts from relying on raw JSON parsing.

### 1. Filesystem & Editing

#### `file_info(path)`
Returns metadata for a file without reading its contents.
- **Parameters** (positional):
  - `path` (string): Relative path to the file.
- **Returns**: `(table, nil)` with fields `path`, `size_bytes` (integer), `line_count` (integer), or `(nil, error)`
- **Use this before `read_file`, `read_lines`, or `read_hash_anchors` to check whether a file is within the 1MB read limit.**
- **Example**:
  ```lua
  local info, err = file_info("src/main.rs")
  if not info then error(err) end
  print(info.size_bytes, info.line_count)
  -- If size_bytes > 1048576, read_file, read_lines, and read_hash_anchors will refuse it.
  ```

#### `read_file(path, offset, limit)`
Reads a file's contents from the workspace.
- **Parameters** (positional):
  - `path` (string): Relative path to the file. (Max file size limit is 1MB / 1,048,576 bytes; attempting to read larger files returns an error).
  - `offset` (integer, optional): 0-indexed byte offset. Defaults to `0`.
  - `limit` (integer, optional): Max bytes to read. Defaults to the remaining file size.
- **Returns**: `(content_string, nil)` or `(nil, error)`
- **Note**: `read_file` refuses files larger than 1MB, even when `offset`/`limit` are provided. For larger files, use `file_info` to confirm size, then inspect with bounded shell commands such as `sed -n`, `head`, or `rg`.
- **Example**:
  ```lua
  local content, err = read_file("Cargo.toml", 0, 500)
  if not content then error(err) end
  print(content)
  ```

#### `read_lines(path, start_line, end_line)`
Reads a 1-indexed inclusive line range from a workspace file. Refuses files larger than 1 MB; for larger files use bounded shell commands.
- **Parameters**: `path` (string), `start_line` (integer, >= 1), `end_line` (integer, >= `start_line`)
- **Returns**: `(content_string, nil)` or `(nil, error)`
- **Note**: `read_lines` refuses files larger than 1MB. For larger files, use bounded shell commands such as `sed -n`, `head`, or `rg`.

#### `write_file{path=..., content=..., overwrite_existing=...}`
Writes content to a file. **Replaces the entire file.**
- **Parameters** (table):
  - `path` (string): Relative path to write. Automatically creates any missing parent directories.
  - `content` (string): Complete file content.
  - `overwrite_existing` (boolean, optional): If `true`, overwrites existing files. If `false` or omitted, fails if the file already exists.
- **Returns**: `(success_msg, nil)` or `(nil, error)`
- **Note**: This is a full-file replacement. For appending incremental content (logs, progress output), use `append_file` instead. For targeted edits to existing code, prefer `apply_anchor_edits`.
- **Example**:
  ```lua
  local res, err = write_file{path="scratch/test.txt", content="hello world\n", overwrite_existing=true}
  if not res then error(err) end
  ```

#### `append_file(path, content)`
Appends content to a file. Creates the file if it does not exist.
- **Parameters** (positional):
  - `path` (string): Relative path to the file.
  - `content` (string): Content to append.
- **Returns**: `(success_msg, nil)` or `(nil, error)`
- **Note**: Use this for logs, scratch notes, and intentional append-only artifacts. For source edits, prefer `apply_anchor_edits`.

#### `read_hash_anchors(path, offset, limit)`
Reads a file with each line prefixed by its 1-indexed line number and 4-character FNV-1a hash (e.g. `15:af63|line content`). Use this to obtain anchors before editing.
  - **Parameters** (positional): Same as `read_file`. Refuses files larger than 1 MB even when `offset`/`limit` are provided.
- **Returns**: `(anchors_string, nil)` or `(nil, error)`
- **Note**: `read_hash_anchors` refuses files larger than 1MB, even when `offset`/`limit` are provided.
- **Example**:
  ```lua
  local anchors, err = read_hash_anchors("src/main.rs", 0, 1000)
  if not anchors then error(err) end
  print(anchors)
  ```

#### `apply_anchor_edits(path, ops)`
Applies a batch array of range-based edits (replacements, insertions, deletions) to a file.
- **Parameters**:
  - `path` (string): Relative path to the file.
  - `ops` (array of tables): Each table in the array represents an edit operation:
    - `start_at` (string): The start line anchor in `"line:hash"` format (e.g. `"12:b5f2"`). Do not include the `|content` part.
    - `end_at` (string, optional): The end line anchor in `"line:hash"` format for multi-line replacements or deletions.
    - `action` (string): One of `"replace"`, `"delete"`, `"insert_before"`, `"insert_after"`.
    - `content` (string, optional): New content to insert/replace.
- **Returns**: `(success_msg, nil)` or `(nil, error)`
- **Notes**:
  - `replace` or `delete` with `end_at` removes **both endpoints and everything in between**.
  - All operations in a batch must be non-overlapping.
  - If any anchor's line or hash does not match the current file state, the entire batch fails (all-or-nothing per file).
- **Example**:
  ```lua
  local ops = {
    { start_at = "12:b5f2", action = "replace", content = "local x = 42" },
    { start_at = "20:a1c3", action = "insert_after", content = "print(x)" }
  }
  local res, err = apply_anchor_edits("src/main.rs", ops)
  if not res then error(err) end
  ```

#### `preview_anchor_edits(path, ops)`
Validates the same anchored edit operations as `apply_anchor_edits` and returns a bounded unified diff preview without writing the file.
- **Parameters**: Same as `apply_anchor_edits`.
- **Returns**: `(diff_string, nil)` or `(nil, error)`. Long previews are truncated with a visible marker.

---

### 2. Workspace Exploration & Shell

#### `repo_map{path=..., levels=...}` or `repo_map()`
Displays the repository directory structure tree. Automatically respects `.gitignore` rules and ignores hidden files/directories (starting with `.`). File entries include a human-readable size (e.g. `main.rs  4.2 KB`).
- **Parameters** (table, optional):
  - `path` (string, optional): Directory relative to the workspace. Defaults to `"."`.
  - `levels` (integer, optional): Max depth. Defaults to `3`.
- **Returns**: `(tree_string, nil)` or `(nil, error)`
- **Example**:
  ```lua
  local tree, err = repo_map{levels = 2}
  if not tree then error(err) end
  print(tree)
  ```

#### `glob(pattern)`
Returns a Lua array of relative file paths matching a glob pattern. Automatically respects `.gitignore` rules and excludes hidden paths.
- **Parameters** (positional):
  - `pattern` (string): Glob pattern relative to workspace root. Supports `*`, `**`, `?`, `[...]`, and `{a,b}` brace expansions.
- **Returns**: `(array_of_strings, nil)` or `(nil, error_string)`
- **Notes**:
  - Results are sorted alphabetically.
  - Only files are returned — directories are never included.
  - Use `repo_map` to get a quick visual overview, and `glob` when you need a list of file paths to iterate over programmatically.
- **Example**:
  ```lua
  -- Find all Rust source files
  local files, err = glob("**/*.rs")
  if not files then error(err) end
  for _, path in ipairs(files) do print(path) end
  ```

#### `search_text{pattern=..., paths=..., regex=..., case_sensitive=..., context=..., max_matches=...}`
Searches workspace text files for matching lines by exact string or regex. Automatically respects `.gitignore`; skips unreadable, binary/non-UTF-8, and files larger than 1MB. This is not semantic search.
- **Parameters**:
  - `pattern` (string, required): Text or regex pattern.
  - `paths` (array of strings, optional): Relative files/directories to search. Defaults to `{ "." }`.
  - `regex` (boolean, optional): Treat `pattern` as a regex. Defaults to `false`.
  - `case_sensitive` (boolean, optional): Defaults to `true`.
  - `context` (integer, optional): Context lines before/after each match. Defaults to `0`, capped at `5`.
  - `max_matches` (integer, optional): Defaults to `100`, capped at `500`.
- **Returns**: `(array_of_matches, nil)` or `(nil, error_string)`. Each match is one matching line with `path`, `line`, `column` (1-indexed byte column of the first match on that line), `text`, `before`, and `after`.

#### `outline(path)`
Returns a lightweight best-effort tree-sitter navigation outline for `.rs`, `.go`, and `.py` files. This is for navigation, not a compiler symbol table; unsupported file types and files larger than 1MB return an error.
- **Parameters**: `path` (string): Relative Rust, Go, or Python file path.
- **Returns**: `(array_of_entries, nil)` or `(nil, error_string)`. Each entry has `name`, `kind` (such as `function`, `method`, `struct`, `enum`, `trait`, `impl`, `mod`, `type`, `interface`, `class`), `start_line`, optional `end_line`, and compact `signature`.

#### `shell{command=..., timeout_seconds=...}`
Executes a command inside the workspace root.
- **Parameters** (table):
  - `command` (string): Shell command to execute (e.g. `"cargo test"`, `"git diff"`).
  - `timeout_seconds` (integer, optional): Bounded timeout (1-600 seconds). Defaults to `120`.
- **Returns**: `(stdout_stderr_combined, nil)` or `(nil, error)`
- **Rules & Guidelines**:
  - `cd` commands must target paths inside the workspace root or `/tmp`.
  - **Guidelines**:
    - Prefer structured Lua globals over shell pipelines when they exist. Use tools like `search_text`, `outline`, `glob`, `git_status`, `git_diff`, `read_lines`, and `file_info` for inspection that needs filtering, counting, line mapping, or reuse in later Lua code.
    - For copying, moving/renaming, or deleting files/directories, run standard shell commands (such as `cp`, `mv`, `rm`) within the workspace.
    - For creating new files or editing existing files, prefer the built-in `write_file` and `apply_anchor_edits` functions over shell command redirects (e.g. `echo ... > file`) or shell-based text editors.
    - Use `shell` for build/test commands, project-specific CLIs, and one-off commands whose raw output is already the desired result.
    - Avoid `cmd | grep | awk | head` when a structured tool can return a bounded Lua table; structured results are easier to filter in `eval` and less likely to hit the 32,768-character output cap.
    - **For semantic code search, use `colgrep` via `shell`.** See the colgrep guide (injected separately) for full usage. Quick example: `shell{command = "colgrep 'error handling' src/"}`.
- **Example**:
  ```lua
  local output, err = shell{command = "cargo test"}
  if not output then error(err) end
  print(output)
  ```

---

### 3. Git Operations (Structured)

These functions return parsed, structured data instead of raw command output. Use them when you need to inspect changes, map line numbers, or compute edits directly from diff information.

#### `git_status{staged=..., paths=..., untracked=...}`
Returns a JSON-decoded Lua array of file change entries.
- **Parameters** (table, optional):
  - `staged` (boolean, optional): `true` = entries with staged changes; `false` = entries with worktree changes (including untracked); omit = all.
  - `paths` (array of strings, optional): Restrict to specific relative paths.
  - `untracked` (boolean, optional): Include untracked files. Defaults to `true`.
- **Returns**: `(array_of_entries, nil)` or `(nil, error)`
- **Entry fields**:
  - `path` (string): Current path (after rename).
  - `old_path` (string or nil): Old path if renamed/copied.
  - `status` (string): `"added"`, `"deleted"`, `"modified"`, `"renamed"`, `"copied"`, `"untracked"`, `"ignored"`, `"type_changed"`, or `"unmerged"`.
  - `staged` (boolean): `true` if the change is in the index.
  - `worktree` (boolean): `true` if the change is in the working tree.
  - `index_char` (string): Single-character index state (`" "`, `"A"`, `"D"`, `"M"`, `"R"`, `"C"`, `"T"`, `"U"`, etc.).
  - `worktree_char` (string): Single-character worktree state (same set as `index_char`).
  - `display` (string): Two-letter porcelain code (e.g. `" M"`, `"R "`).
  - `state_description` (string): Human-readable summary (e.g. `"Modified in worktree"`, `"Added in index, modified in worktree"`, `"Renamed in index"`).
- **Example**:
  ```lua
  local changes, err = git_status{untracked = true}
  if not changes then error(err) end
  for _, e in ipairs(changes) do
    print(e.display, e.path, e.status)
  end
  ```

#### `git_diff{staged=..., base=..., paths=..., context=..., stat_only=...}`
Returns a JSON-decoded Lua array of file deltas with hunks, line numbers, and change metadata.
- **Parameters** (table, optional):
  - `staged` (boolean, optional): `true` = diff `--cached` (index vs HEAD). Default `false` (worktree vs index).
  - `base` (string, optional): Diff against a specific ref (e.g. `"HEAD~1"`). Ignored when `staged` is `true`.
  - `paths` (array of strings, optional): Restrict to specific relative paths.
  - `context` (integer, optional): Context lines per hunk. Defaults to `3`, capped at `20`.
  - `stat_only` (boolean, optional): If `true`, omit `hunks` and return only `path`, `change_type`, `insertions`, and `deletions`.
- **Returns**: `(array_of_deltas, nil)` or `(nil, error)`
- **Delta fields**:
  - `path` (string): New/current path.
  - `old_path` (string): Old path (same as `path` unless renamed/copied).
  - `change_type` (string): `"added"`, `"deleted"`, `"modified"`, `"renamed"`, `"copied"`, or `"type_changed"`.
  - `is_binary` (boolean): `true` for binary files.
  - `old_mode` / `new_mode` (string or nil): File mode strings (e.g. `"100644"`).
  - `similarity` (integer or nil): 0–100 for renames/copies.
  - `insertions` / `deletions` (integer or nil): Line counts.
  - `hunks` (array or nil): Each hunk has:
    - `old_start`, `old_lines`, `new_start`, `new_lines` (integers)
    - `header` (string): The `@@` header line.
    - `lines` (array): Each line has `type` (`"context"`, `"deletion"`, `"addition"`), `text`, `old_line` (integer or nil), `new_line` (integer or nil).
- **Note**: Renames or copies with identical content (`similarity = 100`) may produce **zero hunks**. This is correct — there are no line-level differences to show.
- **Example**:
  ```lua
  local deltas, err = git_diff{paths = {"src/main.rs"}, context = 3}
  if not deltas then error(err) end
  for _, d in ipairs(deltas) do
    print(d.path, d.change_type, d.insertions, d.deletions)
    if d.hunks then
      for _, h in ipairs(d.hunks) do
        for _, l in ipairs(h.lines) do
          print(l.old_line or "-", l.new_line or "-", l.type, l.text)
        end
      end
    end
  end
  ```

#### `git_changes{paths=..., context=..., stat_only=..., base=...}`
Convenience function that returns **all** status entries (both staged and worktree) with diff fields attached for files that have content changes. Covers the 90 % use case of "what changed and how".
- **Parameters** (table, optional):
  - `paths` (array of strings, optional): Restrict to specific relative paths.
  - `context` (integer, optional): Context lines per hunk. Defaults to `3`, capped at `20`.
  - `stat_only` (boolean, optional): If `true`, omit `hunks` and return only stat summary.
  - `base` (string, optional): Compare against a specific ref (e.g. `"HEAD~3"`) instead of the default `HEAD`. Both `diff` (worktree vs base) and `staged_diff` (index vs base) use this ref.
  - `symbols` (boolean, optional): If `true`, attach best-effort current-file outline entries enclosing changed hunk lines. Supported for `.rs`, `.go`, and `.py`. Defaults to `false`.
- **Returns**: `(array_of_entries, nil)` or `(nil, error)`
- **Note**: Each entry has the same fields as `git_status`, plus:
  - `diff` (object or nil): worktree changes (worktree vs base, or index vs worktree if no `base`), same shape as `git_diff` deltas.
  - `staged_diff` (object or nil): staged changes (index vs base, or HEAD vs index if no `base`), same shape.
  - `symbols` (array or nil): present only when `symbols=true` and enclosing outline entries are found. Each symbol has `name`, `kind`, `start_line`, optional `end_line`, `signature`, `changed_ranges` (array of inclusive `[start_line, end_line]` ranges), and `changed_line_count`.
- **Symbols note**: `symbols=true` is a navigation aid, not a semantic diff. It maps changed hunk line numbers to the smallest current-file outline entry, so a method usually wins over an enclosing `impl`. Use `changed_line_count` and `changed_ranges` to distinguish a tiny change inside a large symbol from a broad rewrite. Unsupported files, deleted files, binary files, or changes outside an outline entry simply omit `symbols`.
- **Output size warning**: Full hunks on large changes can exceed the 32,768-character stdout cap. For large refactors, use `stat_only=true` first to scope the change, then call `git_diff` on specific files.
- **Usage note**: For changed source files, prefer `git_changes{symbols=true, context=0}` before reading whole files. Then inspect only the relevant region with `read_lines(path, symbol.start_line, symbol.end_line)`.
- **Example**:
  ```lua
  local changes, err = git_changes{symbols = true, context = 0}
  if not changes then error(err) end
  for _, e in ipairs(changes) do
    print(e.path, e.status)
    if e.symbols then
      for _, s in ipairs(e.symbols) do
        local span = s.start_line .. "-" .. (s.end_line or s.start_line)
        print("  ", s.kind, s.name, span, s.changed_line_count .. " changed lines")
      end
    end
  end
  ```

#### `git_show{path=..., ref=...}` or `git_show(path, ref)`
Reads a file at a specific git ref without checking it out. Supports both table and positional calling conventions.
- **Parameters**:
  - `path` (string): Relative path to the file.
  - `ref` (string, optional): Git ref (e.g. `"HEAD"`, `"HEAD~1"`, `"abc123"`). Defaults to `HEAD`. Use `"staged"` to read the index (staged) version of the file.
- **Returns**: `(file_content_string, nil)` or `(nil, error)`
- **Example**:
  ```lua
  local content, err = git_show{path="src/main.rs", ref="HEAD~1"}
  if not content then error(err) end
  print(content)
  ```

#### `git_log{paths=..., n=...}`
Returns structured commit history for a set of paths.
- **Parameters** (table, optional):
  - `paths` (array of strings, optional): Restrict to specific relative paths.
  - `n` (integer, optional): Max number of commits. Defaults to `10`, capped at `100`.
- **Returns**: `(array_of_entries, nil)` or `(nil, error)`
- **Entry fields**:
  - `sha` (string): Commit hash.
  - `subject` (string): Commit subject line.
  - `author` (string): Author name.
  - `date` (string): Author date.
- **Note**: Returns an empty array `[]` (not an error) when a file has no commit history.
- **Example**:
  ```lua
  local log, err = git_log{paths={"src/main.rs"}, n=5}
  if not log then error(err) end
  for _, e in ipairs(log) do
    print(e.sha, e.subject, e.author, e.date)
  end
  ```

---

### 4. Skills Discovery & Loading

#### `list_skills()`
Lists all available skill prompt templates from configured repo and home skill roots (`.skills/`, `.agents/skills/`, `.ogent/skills/`, `~/.agents/skills/`, and `~/.ogent/skills/`).
- **Returns**: `(markdown_string, nil)` or `(nil, error)`. Lists return complete content or error; they are not silently truncated.

#### `load_skill(name)`
Loads a skill's prompt template.
- **Parameters** (positional):
  - `name` (string): The name of the skill.
- **Returns**: `(skill_body_string, nil)` or `(nil, error)`. Loaded prompt artifacts return complete content or error; they are not silently truncated.

#### `load_skill_asset(root, path)`
Securely reads an asset file inside a skill's directory (e.g. reference manual).
- **Parameters** (positional):
  - `root` (string): Absolute or workspace-relative root directory of the skill (matching the **Root** path returned by `list_skills()`).
  - `path` (string): Relative path of the asset file inside the skill root directory.
- **Returns**: `(asset_content, nil)` or `(nil, error)`

---

### 5. Toolset Guide Loading

The default prompt includes only the core toolset guide. Load extra guides on demand before
using capability areas that are not documented in core.

#### `list_toolsets()`
Lists built-in toolset guides.
- **Returns**: `(markdown_string, nil)` or `(nil, error)`. Lists return complete content or error; they are not silently truncated.

#### `load_toolset(name)`
Loads a built-in toolset guide by name.
- **Parameters** (positional):
  - `name` (string): One of `core`, `git`, `write`, or `subagent`. Filename-style names such as `PROMPT_TOOLSET_WRITE.md` also work.
- **Returns**: `(toolset_string, nil)` or `(nil, error)`. Loaded toolsets return complete content or error; oversized guides must be split.

---

### 6. Workflow & Context Loading

#### `list_workflows()`
Lists workflow documents from repo and home workflow roots (`.ogent/workflows/` and `~/.ogent/workflows/`).
- **Returns**: `(markdown_string, nil)` or `(nil, error)`. Lists return complete content or error; they are not silently truncated.

#### `load_workflow(name)`
Loads a workflow document by name.
- **Parameters** (positional):
  - `name` (string): The workflow name, from frontmatter `name` or the file stem.
- **Returns**: `(workflow_string, nil)` or `(nil, error)`. Loaded workflows return complete content or error; oversized workflows must be split.

#### `list_context_shards()`
Lists available context shard documents.
- **Returns**: `(markdown_string, nil)` or `(nil, error)`. Lists return complete content or error; they are not silently truncated.

#### `load_context_shard(name)`
Loads a context shard by name.
- **Parameters** (positional):
  - `name` (string): The context shard name, from frontmatter `name` or the file stem.
- **Returns**: `(context_shard_string, nil)` or `(nil, error)`. Loaded context shards return complete content or error; oversized shards must be split.

#### `write_context_shard(name, content)`
Creates or updates one repo-scoped context shard by name.
- **Parameters** (positional):
  - `name` (string): The context shard name, using a safe file-stem style identifier.
  - `content` (string): Complete Markdown shard content. If frontmatter includes `name`, it must match the `name` argument.
- **Returns**: `(message, nil)` or `(nil, error)`. Use this instead of constructing context-shard paths.

---

### 7. Web Search & Integration

#### `web_search{query=..., num_results=..., type=...}`
Queries Exa search for highlights and excerpts.
- **Parameters** (table):
  - `query` (string): Natural language search terms. Works equally well for general information and coding/API queries.
  - `num_results` (integer, optional): Default is `10`.
  - `type` (string, optional): `"auto"` or `"deep-reasoning"`. Defaults to `"auto"`.
- **Returns**: `(results_markdown, nil)` or `(nil, error)`

#### `web_read{urls=..., mode=...}`
Reads key excerpts or full text from specified URLs.
- **Parameters** (table):
  - `urls` (array of strings): URLs to extract from.
  - `mode` (string, optional): `"highlights"` or `"text"`. Defaults to `"highlights"`.
- **Returns**: `(results_markdown, nil)` or `(nil, error)`

#### `web_code_context{query=...}`
Queries Exa specifically for code snippets, library details, or API signatures. Prefer this over `web_search` when looking for concrete code examples or crate/package documentation.
- **Parameters** (table):
  - `query` (string): Coding pattern/API query.
- **Returns**: `(results_markdown, nil)` or `(nil, error)`

---

### 8. Subagent Workflows & DSL

#### `task_update(status, summary)`
Sends a task status or progress update message. In non-verbose mode, these updates are printed directly to standard output, allowing progress monitoring of complex orchestrations.
- **Parameters** (positional):
  - `status` (string): Current phase or state name (e.g. `'init'`, `'review'`, `'fixing'`).
  - `summary` (string): Human-readable progress description or update summary.
- **Returns**: no result.

#### `agent{role=..., task=..., profile=...}`
Spawns a subagent in a fresh, isolated Lua VM sandbox sharing the parent's general configuration and system prompt, augmented by the subagent's specific role.
- **Parameters** (table):
  - `task` (string): The description of the task for the subagent to perform.
  - `role` (string, optional): Soft-skill profile name, which dynamically loads custom instructions from `PROMPT_ROLE_<ROLE>.md` (e.g. `RUST_GURU`, `GO_GURU`). Defaults to `'subagent'`.
  - `profile` (string, optional): Overrides the model profile (e.g. `'kimi'`). Defaults to the parent's model profile.
- **Returns**: `response_markdown_string` directly, or raises a Lua runtime error.
- **Example**:
  ```lua
  local response = agent{role = "reviewer", task = "Review the staged diff"}
  print(response)
  ```

#### `parallel{func1, func2, ...}`
Runs multiple Lua functions concurrently inside the async executor, using cooperative multitasking, and waits for all of them to complete.
- **Parameters** (array/list of functions):
  - An array of anonymous functions or function names to execute in parallel.
- **Returns**: `array_of_results` directly, or raises a Lua runtime error.
- **Error behavior**: If **any** task fails, the entire batch aborts with that task's error. To tolerate partial failures and collect all results regardless, wrap individual task bodies with `pcall`:
  ```lua
  local results = parallel({
    function()
      local ok, val = pcall(function() return agent{task="..."} end)
      return {ok=ok, value=val}
    end,
    function()
      local ok, val = pcall(function() return agent{task="..."} end)
      return {ok=ok, value=val}
    end,
  })
  ```

---

## Edit Cycle: Inspect → Read → Plan → Batch Apply

For targeted code edits:
1. Inspect the worktree/index state for files you may touch.
2. For changed source files, use `git_changes{symbols=true, context=0}` to locate the changed symbol, then read the smallest useful region with `read_hash_anchors` or `read_lines`.
3. Build one non-overlapping `ops` batch using exact `"line:hash"` anchors.
4. Apply once with `apply_anchor_edits`, then verify the resulting file or diff.

Example (`end_at` is inclusive for range replacements/deletions):
```lua
local ops = {
  { start_at = "12:b5f2", action = "replace", content = "local x = 42" },
  { start_at = "20:a1c3", end_at = "23:d9e0", action = "replace", content = "print(x)" }
}
local res, err = apply_anchor_edits("src/main.rs", ops)
if not res then error(err) end
```

If an anchor is stale, use the mismatch line from the error only when the intended line is clearly unchanged; otherwise re-read anchors and re-plan before applying.
