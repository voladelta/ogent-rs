# Lua Toolset Core

You execute workspace operations by writing Lua 5.5 code inside either `exec` or `eval`.
You do not call workspace tools through JSON directly; Lua scripts call registered globals.

This core shard covers execution, retrieval, read-only workspace inspection, prompt artifact
loading, shell, and web tools. Extra guide shards are first-class prompt artifacts:

- Load `write` before mutating ordinary workspace files or planning anchored edits.
- Load `git` before inspecting git status, diffs, history, or changed files.
- Load `subagent` before spawning agents, running parallel Lua tasks, or sending task updates.
- Runtime toolset artifacts are limited to `core`, `git`, `write`, and `subagent`.

If a workflow names extra toolsets, call `load_toolset(name)` for those guides before using the
corresponding tools.

## `exec` vs `eval`

- Use `exec` for one-off, stateless operations.
- Use `eval` when you need helper functions, retained tables, or multi-step exploration across turns.
- `exec` and `eval` do not share state. `eval` state persists only across future `eval` calls.
- Neither `exec` nor `eval` bypasses the 32,768-character tool output cap.

## Retrieval Discipline

Treat retrieval as evidence narrowing:

1. Start with exact search when you have a symbol, error string, path, command name, or phrase.
2. Use semantic `colgrep` via `shell` when only intent is known, then confirm with source reads.
3. Use `outline` before reading large supported source files.
4. Read the smallest useful range with `read_lines`.
5. Use `eval` to keep bulky intermediate tables in Lua and return only compact evidence.
6. Verify final claims with tests, type checks, build output, git diff, or direct file references.

Do not treat semantic search as proof. It locates candidates; source and verification decide.

## Sandbox Rules

- Paths must be workspace-relative unless a tool explicitly documents another form.
- The Lua sandbox includes safe libraries only: `table`, `string`, `math`, `utf8`, `coroutine`.
- `os`, `io`, `debug`, and `package` are unavailable.
- Memory is capped at 32 MB.
- CPU is limited by an instruction hook. Avoid unbounded loops.
- Tool output is capped at 32,768 characters.

Most globals return `(result, nil)` on success or `(nil, error_string)` on failure. Handle
errors explicitly:

```lua
local value, err = read_file("Cargo.toml", 0, 2000)
if not value then error(err) end
return value
```

Structured globals such as `glob`, `file_info`, and `outline` return native Lua tables.
There is no public `json_decode` global.

## Filesystem Read Tools

### `file_info(path)`

Returns file metadata without reading contents.

- Parameters: `path` string.
- Returns: table with `path`, `size_bytes`, `line_count`, or error.
- Use before reading when file size is uncertain.

### `read_file(path, offset, limit)`

Reads workspace file content.

- Parameters: `path`, optional byte `offset`, optional byte `limit`.
- Refuses files larger than 1 MB.
- For large files, use `file_info`, `read_lines`, bounded shell commands, or targeted search.

### `read_lines(path, start_line, end_line)`

Reads a 1-indexed inclusive line range.

- Refuses files larger than 1 MB.
- Prefer this over whole-file reads once you know the relevant span.

## Workspace Exploration

### `repo_map{path=..., levels=...}` or `repo_map()`

Returns a directory tree, respecting `.gitignore`.

- Defaults: `path="."`, `levels=3`.
- Use for quick orientation.

### `glob(pattern)`

Returns a sorted Lua array of matching workspace files, respecting `.gitignore`.

- Supports `*`, `**`, `?`, `[...]`, and `{a,b}`.
- Returns files only, not directories.

### `search_text{pattern=..., paths=..., regex=..., case_sensitive=..., context=..., max_matches=...}`

Searches text files by exact string or regex.

- Defaults: `paths={"."}`, `regex=false`, `case_sensitive=true`, `context=0`, `max_matches=100`.
- `context` is capped at 5; `max_matches` is capped at 500.
- Returns matches with `path`, `line`, `column`, `text`, `before`, `after`.
- Use for exact source evidence.

### `outline(path)`

Returns a best-effort tree-sitter outline for `.rs`, `.go`, and `.py` files.

- Returns entries with `name`, `kind`, `start_line`, optional `end_line`, and `signature`.
- Unsupported files return an error.
- This is navigation aid, not a compiler symbol table.

## Shell

### `shell{command=..., timeout_seconds=...}`

Runs a shell command inside the workspace root.

- Defaults: `timeout_seconds=120`, capped at 600.
- Use for project CLIs, build/test commands, `colgrep`, and one-off inspection.
- Prefer structured globals over shell pipelines when they exist.
- `cd` commands must stay inside the workspace or `/tmp`.
- Avoid shell redirection for source edits; use write tools from the write shard.

For semantic code search:

```lua
local out, err = shell{command = "colgrep 'error handling' src/"}
if not out then error(err) end
return out
```

## Prompt Artifacts

Prompt artifacts return complete content or error. They do not silently truncate; oversized
artifacts must be split.

### `list_skills()`

Lists skills from `.skills/`, `.agents/skills/`, `.ogent/skills/`, `~/.agents/skills/`, and
`~/.ogent/skills/`.

### `load_skill(name)`

Loads a skill prompt by name.

### `load_skill_asset(root, path)`

Reads an asset inside a loaded skill root. Use the root path returned by `list_skills()`.

### `list_workflows()`

Lists workflows from `.ogent/workflows/` and `~/.ogent/workflows/`.

### `load_workflow(name)`

Loads a workflow by frontmatter `name` or Markdown file stem.

### `list_context_shards()`

Lists available context shards.

### `load_context_shard(name)`

Loads a context shard by frontmatter `name` or Markdown file stem.

### `write_context_shard(name, content)`

Creates or updates one repo-scoped context shard by name. Use this instead of constructing
context-shard file paths. If the Markdown frontmatter includes `name`, it must match this
argument.

### `list_toolsets()`

Lists built-in toolset guides. The default prompt includes `core`; other guides are loaded on
demand.

### `load_toolset(name)`

Loads a built-in toolset guide by name. Supported names: `core`, `git`, `write`, `subagent`.
Filename-style names such as `PROMPT_TOOLSET_WRITE.md` also work.

## Web

### `web_search{query=..., num_results=..., type=...}`

Queries Exa search for highlights and excerpts.

- Required: `query`.
- Optional: `num_results`, `type="auto"` or `"deep-reasoning"`.

### `web_read{urls=..., mode=...}`

Reads key excerpts or full text from URLs.

- Required: `urls` array.
- Optional: `mode="highlights"` or `"text"`.

### `web_code_context{query=...}`

Queries Exa for code snippets, library details, or API signatures. Prefer for concrete API
or package usage questions.
