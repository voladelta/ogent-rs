# Lua Toolset Guide

You execute all workspace operations by writing Lua 5.5 code inside either the `exec` (stateless, one-off) or `eval` (stateful, persistent session) tools. You do not call tools via JSON schema; instead, you write Lua scripts that call the registered global functions.

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

#### `read_file(path, offset, limit)`
Reads a file's contents from the workspace.
- **Parameters** (positional):
  - `path` (string): Relative path to the file.
  - `offset` (integer, optional): 0-indexed byte offset. Defaults to `0`.
  - `limit` (integer, optional): Max bytes to read. Defaults to the remaining file size.
- **Returns**: `(content_string, nil)` or `(nil, error)`
- **Example**:
  ```lua
  local content, err = read_file("Cargo.toml", 0, 500)
  if not content then error(err) end
  print(content)
  ```

#### `write_file{path=..., content=..., overwrite_existing=...}`
Writes content to a file. Creates parent directories if they do not exist.
- **Parameters** (table):
  - `path` (string): Relative path to write.
  - `content` (string): Complete file content.
  - `overwrite_existing` (boolean, optional): If `true`, overwrites existing files. If `false` or omitted, fails if the file already exists.
- **Returns**: `(success_msg, nil)` or `(nil, error)`
- **Example**:
  ```lua
  local res, err = write_file{path="scratch/test.txt", content="hello world\n", overwrite_existing=true}
  if not res then error(err) end
  ```

#### `read_hash_anchors(path, offset, limit)`
Reads a file with each line prefixed by its 1-indexed line number and 4-character FNV-1a hash (e.g. `15:af63|line content`). Use this to obtain anchors before editing.
- **Parameters** (positional): Same as `read_file`.
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
Displays the repository directory structure tree. Respects `.gitignore`.
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

#### `shell{command=..., timeout_seconds=...}`
Executes a command inside the workspace root.
- **Parameters** (table):
  - `command` (string): Shell command to execute (e.g. `"cargo test"`, `"git diff"`).
  - `timeout_seconds` (integer, optional): Bounded timeout (1-600 seconds). Defaults to `120`.
- **Returns**: `(stdout_stderr_combined, nil)` or `(nil, error)`
- **Rules**:
  - `cd` commands must target paths inside the workspace root or `/tmp`.
  - Direct filesystem modification utilities (like `cp`, `mv`, `rm`) are not allowed; use Lua `write_file` or `apply_anchor_edits` instead.
- **Example**:
  ```lua
  local output, err = shell{command = "cargo test"}
  if not output then error(err) end
  print(output)
  ```

---

### 3. Skills Discovery & Loading

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
  - `root` (string): Skill directory root (obtained via `list_skills`).
  - `path` (string): Relative path inside the skill.
- **Returns**: `(asset_content, nil)` or `(nil, error)`

---

### 4. Web Search & Integration

#### `web_search{query=..., num_results=..., type=...}`
Queries Exa search for highlights and excerpts.
- **Parameters** (table):
  - `query` (string): Natural language search terms.
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
Queries Exa for code snippets, library details, or API signatures.
- **Parameters** (table):
  - `query` (string): Coding pattern/API query.
- **Returns**: `(results_markdown, nil)` or `(nil, error)`

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
