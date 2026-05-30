# Lua Toolset Guide

You execute all workspace operations by writing Lua 5.5 code inside either the `exec` (stateless, one-off) or `eval` (stateful, persistent session) tools. You do not call tools via JSON schema; instead, you write Lua scripts that call the registered global functions.

## Tool Selection: `exec` vs `eval`

* **Use `exec` (Stateless)** for simple, one-off, or self-contained operations that do not need to persist state between agent turns (e.g., executing a single build/test command, reading a specific file, performing a one-off search). This keeps the environment clean and avoids side effects.
* **Use `eval` (Stateful)** when you want to define helper functions, declare global variables, or retain state that you will reuse or build upon in subsequent turns of the conversation (e.g., keeping track of a set of line anchors or caching files during a complex multi-step editing workflow).

## Sandbox Constraints & Rules

1. **Virtual Root / Workspace Path Restriction**: Inside the sandbox, paths must be relative to the workspace root (e.g. `'src/main.rs'`). Do not use host absolute paths (e.g. `/Users/mbp/...`).
2. **Library Restrictions**: The Lua sandbox only permits safe standard libraries: `table`, `string`, `math`, `utf8`, and `coroutine`. Unsafe modules like `os`, `io`, `debug`, and `package` are completely unavailable.
3. **Execution Limits**:
   - Memory is capped at **32MB**.
   - CPU is limited to **32,000 instructions**. Avoid infinite or highly nested loops.
   - Stdout printing / return value output is capped at **16,384 characters**.
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

All functions return `(result, nil)` on success or `(nil, error_string)` on failure. Always handle errors cleanly.

### 1. Filesystem & Editing

#### `file_info(path)`
Returns metadata for a file without reading its contents.
- **Parameters** (positional):
  - `path` (string): Relative path to the file.
- **Returns**: `(table, nil)` with fields `path`, `size_bytes` (integer), `line_count` (integer), or `(nil, error)`
- **Use this before `read_file` or `read_hash_anchors` to plan offset/limit when a file may be large.**
- **Example**:
  ```lua
  local info, err = file_info("src/main.rs")
  if not info then error(err) end
  print(info.size_bytes, info.line_count)
  -- If size_bytes > 1048576, read_file will refuse it; use read_hash_anchors with offset/limit instead
  ```

#### `read_file(path, offset, limit)`
Reads a file's contents from the workspace.
- **Parameters** (positional):
  - `path` (string): Relative path to the file. (Max file size limit is 1MB / 1,048,576 bytes; attempting to read larger files returns an error).
  - `offset` (integer, optional): 0-indexed byte offset. Defaults to `0`.
  - `limit` (integer, optional): Max bytes to read. Defaults to the remaining file size.
- **Returns**: `(content_string, nil)` or `(nil, error)`
- **Note**: For files larger than 1MB, first call `file_info` to get `size_bytes` and `line_count`, then page through using `offset`/`limit` or use `read_hash_anchors` with the same offsets.
- **Example**:
  ```lua
  local content, err = read_file("Cargo.toml", 0, 500)
  if not content then error(err) end
  print(content)
  ```

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
- **Use this for**: log files, progress tracking, writing output incrementally, or any case where you want to add to a file without reading its current contents first.
- **Example**:
  ```lua
  local res, err = append_file("scratch/log.txt", "step 1 done\n")
  if not res then error(err) end
  ```

#### `read_hash_anchors(path, offset, limit)`
Reads a file with each line prefixed by its 1-indexed line number and 4-character FNV-1a hash (e.g. `15:af63|line content`). Use this to obtain anchors before editing.
- **Parameters** (positional): Same as `read_file` (under the same 1MB size limit constraint).
- **Returns**: `(anchors_string, nil)` or `(nil, error)`
- **Side Effect**: Saves the file path in a global session variable so subsequent `apply_anchor_edits` calls do not require repeating the path.
- **Example**:
  ```lua
  local anchors, err = read_hash_anchors("src/main.rs", 0, 1000)
  if not anchors then error(err) end
  print(anchors)
  ```

#### `apply_anchor_edits(ops)` or `apply_anchor_edits(path, ops)`
Applies a batch array of range-based edits (replacements, insertions, deletions) to a file.
- **Parameters**:
  - `path` (string, optional): Relative path. If omitted, uses the path from the last `read_hash_anchors` call.
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
- **Examples**:
  ```lua
  -- Find all Rust source files
  local files, err = glob("**/*.rs")
  if not files then error(err) end
  for _, path in ipairs(files) do print(path) end
  ```
  ```lua
  -- Find files in a specific directory
  local files, err = glob("src/tools/*.rs")
  if not files then error(err) end
  return files
  ```

#### `shell{command=..., timeout_seconds=...}`
Executes a command inside the workspace root.
- **Parameters** (table):
  - `command` (string): Shell command to execute (e.g. `"cargo test"`, `"git diff"`).
  - `timeout_seconds` (integer, optional): Bounded timeout (1-600 seconds). Defaults to `120`.
- **Returns**: `(stdout_stderr_combined, nil)` or `(nil, error)`
- **Rules & Guidelines**:
  - `cd` commands must target paths inside the workspace root or `/tmp`.
  - **Guidelines**:
    - For copying, moving/renaming, or deleting files/directories, run standard shell commands (such as `cp`, `mv`, `rm`) within the workspace.
    - For creating new files or editing existing files, prefer the built-in `write_file` and `apply_anchor_edits` functions over shell command redirects (e.g. `echo ... > file`) or shell-based text editors.
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
  - `staged` (boolean, optional): `true` = staged only; `false` = unstaged only; omit = all.
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
  - `context` (integer, optional): Context lines per hunk. Defaults to `3`.
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

#### `git_changes{paths=..., context=..., stat_only=...}`
Convenience function that returns **all** status entries (both staged and worktree) with diff fields attached for files that have content changes. Covers the 90 % use case of "what changed and how".
- **Parameters** (table, optional):
  - `paths` (array of strings, optional): Restrict to specific relative paths.
  - `context` (integer, optional): Context lines per hunk. Defaults to `3`.
  - `stat_only` (boolean, optional): If `true`, omit `hunks` and return only stat summary.
- **Returns**: `(array_of_entries, nil)` or `(nil, error)`
- **Note**: Each entry has the same fields as `git_status`, plus:
  - `diff` (object or nil): worktree changes (index vs worktree), same shape as `git_diff` deltas.
  - `staged_diff` (object or nil): staged changes (HEAD vs index), same shape.
- **Example**:
  ```lua
  local changes, err = git_changes()
  if not changes then error(err) end
  for _, e in ipairs(changes) do
    if e.diff then
      print(e.path, "worktree:", #e.diff.hunks, "hunks")
    end
    if e.staged_diff then
      print(e.path, "staged:", #e.staged_diff.hunks, "hunks")
    end
  end
  ```

#### `git_show{path=..., ref=...}`
Reads a file at a specific git ref without checking it out.
- **Parameters** (table):
  - `path` (string): Relative path to the file.
  - `ref` (string, optional): Git ref (e.g. `"HEAD"`, `"HEAD~1"`, `"abc123"`). Defaults to `HEAD`.
- **Returns**: `(file_content_string, nil)` or `(nil, error)`
- **Example**:
  ```lua
  local content, err = git_show{path="src/main.rs", ref="HEAD~1"}
  if not content then error(err) end
  print(content)
  ```

#### `git_log{paths=..., n=...}`
Returns brief commit history for a set of paths.
- **Parameters** (table, optional):
  - `paths` (array of strings, optional): Restrict to specific relative paths.
  - `n` (integer, optional): Max number of commits. Defaults to `10`.
- **Returns**: `(log_text, nil)` or `(nil, error)`
- **Note**: Returns plain text (one line per commit in `--oneline` format), not JSON.
- **Example**:
  ```lua
  local log, err = git_log{paths={"src/main.rs"}, n=5}
  if not log then error(err) end
  print(log)
  ```

---

### 4. Skills Discovery & Loading

#### `list_skills()`
Lists all available skill prompt templates from `.ogent/skills/` and `~/.ogent/skills/`.
- **Returns**: `(markdown_string, nil)` or `(nil, error)`

#### `load_skill(name)`
Loads a skill's prompt template.
- **Parameters** (positional):
  - `name` (string): The name of the skill.
- **Returns**: `(skill_body_string, nil)` or `(nil, error)`

#### `load_skill_asset(root, path)`
Securely reads an asset file inside a skill's directory (e.g. reference manual).
- **Parameters** (positional):
  - `root` (string): Absolute or workspace-relative root directory of the skill (matching the **Root** path returned by `list_skills()`).
  - `path` (string): Relative path of the asset file inside the skill root directory.
- **Returns**: `(asset_content, nil)` or `(nil, error)`

---

### 5. Web Search & Integration

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

### 6. Subagent Workflows & DSL

#### `task_update(status, summary)`
Sends a task status or progress update message. In non-verbose mode, these updates are printed directly to standard output, allowing progress monitoring of complex orchestrations.
- **Parameters** (positional):
  - `status` (string): Current phase or state name (e.g. `'init'`, `'review'`, `'fixing'`).
  - `summary` (string): Human-readable progress description or update summary.
- **Returns**: `(nil, nil)`

#### `agent{role=..., task=..., profile=...}`
Spawns a subagent in a fresh, isolated Lua VM sandbox sharing the parent's general configuration and system prompt, augmented by the subagent's specific role.
- **Parameters** (table):
  - `task` (string): The description of the task for the subagent to perform.
  - `role` (string, optional): Soft-skill profile name, which dynamically loads custom instructions from `PROMPT_ROLE_<ROLE>.md` (e.g. `RUST_GURU`, `GO_GURU`). Defaults to `'subagent'`.
  - `profile` (string, optional): Overrides the model profile (e.g. `'kimi'`). Defaults to the parent's model profile.
- **Returns**: `(response_markdown_string, nil)` or `(nil, error)`

#### `parallel{func1, func2, ...}`
Runs multiple Lua functions concurrently inside the async executor, using cooperative multitasking, and waits for all of them to complete.
- **Parameters** (array/list of functions):
  - An array of anonymous functions or function names to execute in parallel.
- **Returns**: `(array_of_results, nil)` or `(nil, error)`.
- **Error behavior**: If **any** task fails, the entire batch aborts and returns that task's error. To tolerate partial failures and collect all results regardless, wrap individual task bodies with `pcall`:
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

## Edit Cycle: Read → Plan → Batch Apply

Edits to code files should always follow a precise flow:
1. **Read Hash Anchors**: Fetch the region of the file you want to edit.
2. **Formulate Edits**: Plan your changes using exact line anchors.
3. **Apply Batch**: Call `apply_anchor_edits` with the planned edits.

### Editing Example (Batch replacement)
Suppose we want to edit `src/main.rs`.

**Step 1: Read the anchors**
```lua
local anchors, err = read_hash_anchors("src/main.rs", 0, 500)
if not anchors then error(err) end
print(anchors)
```
Output:
```text
1:a430|fn main() {
2:5c82|    println!("hello");
3:f4a2|}
```

**Step 2: Apply the edits**
We want to change `println!("hello")` to print a custom message. We construct the edit table:
```lua
local ops = {
  {
    start_at = "2:5c82",
    action = "replace",
    content = "    println!(\"hello from Lua sandbox!\");"
  }
}
local res, err = apply_anchor_edits("src/main.rs", ops)
if not res then error(err) end
print(res)
```

### Range Replacements
To delete or replace multiple lines, specify `end_at`. **Both endpoints and all lines in between will be deleted/replaced.**
If you have:
```text
10:e31a|if x > 10 then
11:b21f|    print("too large")
12:f3d4|    x = 10
13:1a8b|end
```
To replace lines 10 to 13 inclusive:
```lua
local ops = {
  {
    start_at = "10:e31a",
    end_at = "13:1a8b",
    action = "replace",
    content = "x = math.min(x, 10)"
  }
}
```

### Stale Anchor Recovery
If you attempt to apply edits and get an error like:
```text
anchor mismatch at line 12: expected e31a, current b5f2
12:b5f2|local y = 10
```
This means the line changed or shifted.
- If it shifted but content is correct, grab the new hash/line number from the error message (`12:b5f2`) and retry the call with the updated anchor.
- If the file has changed significantly, run `read_hash_anchors` again to fetch current anchors, re-plan, and apply.
