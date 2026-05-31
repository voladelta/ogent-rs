# Architecture

This document describes the high-level architecture of `ogent`.
If you want to familiarize yourself with the codebase, start here.

## Bird's Eye View

`ogent` is a CLI agent that runs a task inside a workspace and exits.
It is not a server, a daemon, or an interactive REPL — it does one job, then stops.

At the highest level, the process looks like this:

1. `main` parses the CLI, loads config, and constructs one `Agent`.
2. The `Agent` runs a loop: it sends the conversation to an LLM, receives a response,
   executes any tool calls, and appends results back to the conversation.
3. The loop ends when the model returns a message with no tool calls.
4. Unless the run is temporary, the conversation is persisted as a session transcript and the process exits.

The key design decision: **the LLM is given exactly two tools — `exec` and `eval` — both of
which run Lua scripts**. All real capabilities (file I/O, shell commands, web search, subagents)
live inside the Lua sandbox, not directly in the tool schema the model sees. This creates a
single, auditable boundary between the model and the system.

```
User prompt
  └─> Agent (turn loop)
        └─> LLM (exec / eval only)
              └─> Lua VM
                     ├─> read_file, read_lines, write_file, apply_anchor_edits
                     ├─> preview_anchor_edits, shell, glob, repo_map
                     ├─> search_text, outline
                     ├─> git_status, git_diff, git_changes, git_show, git_log
                     ├─> web_search, web_read
                     ├─> list_skills, load_skill
                     └─> agent{...}, parallel{...}   ← spawns subagents
```

## Code Map

### [`src/main.rs`](src/main.rs)

Entry point. Parses CLI args (`--profile`, `--verbose`, `--temp`, `--resume`, and the task prompt), checks that
`EXA_API_KEY` is set, loads config, builds the `Client`, and calls `run_agent_cli`.

`run_agent_cli` either constructs the initial message list via `prompts::build_initial_messages`
or loads a previous transcript via `session::load_session_in` when `--resume` is set. It
creates the `Agent`, attaches a `CliOutputSink`, runs the loop, and persists the session
unless `--temp` is set.

The `director` actor ID is assigned here — it is the root agent's identifier in all output.

### [`src/agent.rs`](src/agent.rs)

The agent turn loop and output pipeline.

**`Agent`** is the central struct. It holds:
- `workspace` — the sandbox root (immutable after construction)
- `client` — the LLM HTTP client
- `messages` — the full conversation history (`Vec<Message>`)
- `tools` — the two tool schemas sent to the LLM (`exec`, `eval`)
- `lua_session` — the persistent `eval` Lua VM (`Arc<Mutex<Option<Lua>>>`)
- `skill_store` — discovered skills
- `output_sink` — where rendered output goes (CLI or custom)
- `actor_id` — the `[director]` / `[role]` prefix shown in terminal output

**`run_loop`** is the main loop. Each iteration:
1. Opens a streaming channel to the sink.
2. Calls `client.chat(messages, tools, stream_tx)`.
3. Awaits the stream handle, then handles the response.
4. If the response has tool calls, dispatches `exec` and `eval` directly to
   `tools::exec()` and `tools::eval()`.
5. Appends all messages (assistant + tool results) to `self.messages`.
6. Breaks when no tool calls are present.

 **`AgentOutputSink`** is a trait with five hooks: `message`, `stream_event`, `tool_call`,
`tool_result`, and `task_update`. The CLI implementation (`CliOutputSink`) renders these
to stdout using `print_actor_text`, which uses a `Mutex<(last_actor, at_line_start)>` to
avoid interleaved output across concurrently streaming subagents.

**Architecture Invariant:** `run_loop` does not call `persist`. The caller (`run_agent_cli`)
is responsible for persistence, whether the loop succeeds or fails.

**Architecture Invariant:** subagents spawned inside `tools/lua.rs` also call `Agent::run_loop`,
but they never call `persist`. Only the root CLI agent persists a session.

### [`src/types.rs`](src/types.rs)

Shared data types. No logic. Everything else imports from here.

- **`Message`** — a conversation turn. Has `role` (`System`, `User`, `Assistant`, `Tool`),
  `content`, `origin` (`Human`, `Internal`, `Model`, `Tool`), optional `reasoning_content`,
  and optional `tool_calls`.
- **`ToolCall`** / **`FunctionCall`** — a model-requested tool invocation with `id`, `name`,
  and `arguments` (a JSON string).
- **`Tool`** / **`ToolFunction`** — the schema sent to the LLM. Only `exec` and `eval` are
  ever sent.
- **`ChatResponse`** — the raw response from the LLM: `content`, `reasoning_content`,
  `tool_calls`, `usage`.

### [`src/client.rs`](src/client.rs), [`src/providers.rs`](src/providers.rs), [`src/sse.rs`](src/sse.rs)

The LLM HTTP layer.

`providers.rs` translates a config profile into an HTTP request body (OpenAI-compatible JSON).
`client.rs` sends it and streams back `StreamEvent`s via an `mpsc` channel.
`sse.rs` parses the Server-Sent Events byte stream, handling partial JSON argument accumulation
(models sometimes split tool call arguments across multiple chunks).

**Architecture Invariant:** `client.rs` knows nothing about tools, Lua, or the workspace.
It sends a `Vec<Message>` and `Vec<Tool>`, and returns a `ChatResponse`.

### [`src/tools/mod.rs`](src/tools/mod.rs)

Tool context and helpers.

**`ToolContext`** is passed to every tool handler. It carries everything a tool might need:
`workspace`, `skill_store`, `lua_session`, `client`, `output_sink`, `verbose`, `actor_id`.

**`agent_tools()`** returns the two `Tool` schemas sent to the LLM: `exec` and `eval`.
These are defined explicitly in `tools/lua.rs`.

Each tool module (`fs`, `git`, `repo`, `shell`, `skills`, `web`) exports its functions as
`pub fn` or `pub async fn`. They are not registered in a central registry — instead,
`tools/lua.rs` registers them directly into the Lua sandbox as globals.

**Architecture Invariant:** the LLM only ever sees `exec` and `eval` in its tool list.
All other tools are internal — accessible only via Lua scripts.

### [`src/tools/lua.rs`](src/tools/lua.rs)

The Lua execution sandbox. This is the most important file in the codebase.

`exec_tool` creates a **fresh** sandboxed Lua VM per call (stateless).
`eval_tool` reuses the **session-persistent** Lua VM stored in `Agent.lua_session` (stateful).

Both go through `run_lua_vm_async`, which:
1. Overrides `print` to capture stdout into a buffer.
2. Wraps the script in a Lua coroutine (thread).
3. Registers an instruction hook on the coroutine (not the main thread) that aborts after
   32,000 instructions. This keeps CPU-bound loops from blocking Tokio worker threads.
4. Runs the coroutine via `thread.into_async(())?.await`.
5. Returns captured stdout + return value (or runtime error) as a formatted string.

`register_tools_in_lua` injects every capability as a Lua global function using the
`register_sync!` and `register_async!` macros. Tools with special calling conventions
(positional args, Lua table return) are wrapped manually (e.g. `read_file`,
`apply_anchor_edits`, `glob`).

The `agent{role, task, profile}` Lua function spawns a full `Agent` inline — same code path
as the root agent, with a fresh `lua_session` and a role-specific prompt.

The `parallel{f1, f2, ...}` Lua function collects async Lua functions and awaits them with
`futures_util::future::join_all`.

**Architecture Invariant:** each `exec` call gets a completely isolated Lua VM — no globals
carry over between separate `exec` calls. Only `eval` shares state across calls.

**Architecture Invariant:** the Lua sandbox does not load `os`, `io`, `debug`, or `package`.
Scripts cannot open arbitrary files, spawn processes, or load native modules directly.
All I/O goes through the registered Rust-backed global functions.

**Architecture Invariant:** tool output to the model is capped at 16 KB. Any output beyond
that is truncated with a visible marker, preventing runaway tool results from filling context.

### [`src/tools/fs.rs`](src/tools/fs.rs)

Filesystem tools: `read_file`, `read_lines`, `write_file`, `append_file`, `file_info`,
`read_hash_anchors`, `apply_anchor_edits`, `preview_anchor_edits`.

All path arguments go through `workspace.workspace_path()` or `workspace.readable_path()`
before any I/O occurs. No tool in this module ever constructs an absolute path independently.

Files read via `read_file`, `read_lines`, and `read_hash_anchors` are subject to a 1 MB
size limit. `preview_anchor_edits` validates edits through the same anchored edit engine as
`apply_anchor_edits`, but returns a bounded unified diff without writing.

### [`src/tools/shell.rs`](src/tools/shell.rs)

`shell` — runs an arbitrary command inside the workspace root with a configurable timeout
(max 600 s). The working directory is always `workspace.root()`, never arbitrary. The
command is passed to the OS shell as a string.

**Architecture Invariant:** the shell tool does not accept a `cwd` argument. The working
directory is always the workspace root. This prevents shell commands from escaping the sandbox.

### [`src/tools/repo.rs`](src/tools/repo.rs)

`repo_map` — prints the directory tree of the workspace, respecting `.gitignore` and
skipping hidden paths.

`glob` — searches for files matching a glob pattern and returns a Lua array of matching
relative paths. Respects `.gitignore` rules.

### [`src/tools/search.rs`](src/tools/search.rs)

Structured workspace inspection tools.

`search_text` — exact or regex text search over workspace files. It walks with `ignore` so
`.gitignore` is respected, skips unreadable/non-UTF-8/large files, and returns bounded
structured match rows for Lua-side filtering instead of raw shell pipeline output.

`outline` — lightweight best-effort tree-sitter navigation outline for Rust, Go, and Python
files. It is for agent navigation, not a compiler symbol table; unsupported file types return an
error.

### [`src/tools/web.rs`](src/tools/web.rs)

`web_search`, `web_read`, `web_code_context` — Exa API client. Requires `EXA_API_KEY`.

### [`src/tools/skills.rs`](src/tools/skills.rs)

`list_skills`, `load_skill`, `load_skill_asset` — delegates to `src/skills.rs`.

### [`src/workspace.rs`](src/workspace.rs)

**`Workspace`** holds:
- `root` — the canonical absolute path derived from `cwd` at startup. Never changes.
- `allowed_roots` — additional paths readable but not writable (e.g. `~/.ogent`).

`workspace_path(path)` — resolves `path` relative to `root`; rejects anything outside `root`.
Used for file writes, shell cwd, and session paths.

`readable_path(path)` — same as above but also accepts paths under `allowed_roots`.
Used for reading skills and loading role prompt files.

Path normalization (`.` and `..` resolution) happens at the boundary inside `normalize()`,
before any comparison. This blocks `../../../etc/passwd`-style traversal.

**Security note on symlinks:** Before checking the boundary, the path is canonicalized by
walking up to the deepest existing ancestor, resolving symlinks via `fs::canonicalize`, and
reconstructing the full real path. This prevents a symlink inside the workspace from
pointing outside it (e.g. `workspace/evil_link -> /etc`) and being followed to escape the
sandbox.

**Architecture Invariant:** `workspace_path` and `readable_path` are the only two path
resolution entry points. No tool resolves paths independently. If you add a new tool that
touches the filesystem, it must go through one of these two functions.

### [`src/hashline.rs`](src/hashline.rs)

Anchored file editing. The core problem it solves: LLMs generate edits against a
snapshot of a file, but by the time the edit is applied the file may have changed.
Anchors make edits position-independent.

`render_hashlines(source)` prefixes each line with `line_no:hash|`, where `hash` is a
4-hex-digit FNV-1a checksum of that line's content. Example: `42:a4f2|fn main() {`.

`apply_anchor_edits(source, ops)` applies a batch of `EditOp`s. Each op references a
`start_at` and optional `end_at` as `"line:hash"` strings. Before applying, the engine
validates that the hash at that line still matches — if the file has shifted or the line
changed, the edit is rejected with a clear error. Edits are applied in reverse line order
(high to low) so earlier edits do not shift the indices of later ones.

**`EditOp`** fields: `start_at`, `end_at` (optional), `action` (`replace`, `delete`,
`insert_before`, `insert_after`), `content`.

**Architecture Invariant:** `apply_anchor_edits` is an all-or-nothing batch operation.
All ops are resolved and validated before any mutation is applied. If any anchor mismatches,
the entire batch is rejected and the file is left unchanged.

### [`src/session.rs`](src/session.rs)

`persist_session_in(&workspace, &messages, &session_id)` serializes `messages` to JSONL at
`{workspace_root}/.ogent/sessions/{session_id}.jsonl`. Each line is one `Message`.

`load_session_in(&workspace, &session_id)` reads the same JSONL format for `--resume`. Resume
loads the prior transcript directly and does not rebuild the initial system/tool messages.

Session IDs are timestamped (`generate_session_id`) to avoid collisions.

Session IDs are restricted to ASCII letters, digits, and `-` before read or write path
construction.

**Architecture Invariant:** session files are written only by the root CLI agent, never by
subagents. Temporary root runs and subagent conversations exist only in memory for the duration
of the run.

### [`src/skills.rs`](src/skills.rs)

**`SkillStore`** discovers and loads skill prompt files from a fixed set of directories:

 ```
 {cwd}/.skills/
 {cwd}/.agents/skills/
 {cwd}/.ogent/skills/
 ~/.agents/skills/
 ~/.ogent/skills/
 ```

Skills are Markdown files. `list_skills()` returns a formatted directory of all discovered
skills. `load_skill(name)` reads and returns the file content. Skills are lazy — nothing
is loaded or injected at startup.

### [`src/prompts.rs`](src/prompts.rs)

Assembles message lists for agents.

`build_initial_messages(task)` returns the root agent's message list:
`[system: PROMPT_SYSTEM, user: PROMPT_TOOLSET, user: PROMPT_COLGREP, user: task]`.

`build_subagent_messages(workspace, role, task)` returns a subagent's message list:
`[system: PROMPT_SYSTEM, user: role_prompt, user: PROMPT_TOOLSET, user: PROMPT_COLGREP, user: task]`.

`load_subagent_role(workspace, role)` resolves the role prompt file from `.ogent/`, workspace
root, or `~/.ogent/`, in that order. Falls back to the embedded `PROMPT_ROLE_GENERIC.md`.

`PROMPT_SYSTEM`, `PROMPT_TOOLSET`, `PROMPT_COLGREP`, and `PROMPT_ROLE_GENERIC` are embedded
at compile time via `include_str!`.

### [`src/config.rs`](src/config.rs)

Loads `config.yaml` from `{workspace}/.ogent/config.yaml` or `~/.ogent/config.yaml`.
Holds profiles (model name, temperature, etc.) and providers (base URL, API key env var).

---

## Cross-Cutting Concerns

### The Two-Tool Boundary

The model's tool schema is locked to `exec` and `eval`. This means:
- Any new workspace capability goes into a Lua global, not a new top-level tool.
- The model cannot do anything the Lua sandbox doesn't allow.
- Adding a capability requires only changing `register_tools_in_lua`.

### Subagent Spawning

When the model calls `agent{role, task, profile?}` via Lua, `tools/lua.rs` constructs a fresh `Agent`
inline and calls `run_loop` on it directly (not in a separate thread or process). Subagents
are concurrent via Tokio tasks but share the same Tokio runtime. Each subagent gets:
- its own `lua_session` (isolated VM state)
- its own `actor_id` (for output tagging)
- the same `workspace`, `skill_store`, `client`, and `output_sink` as the parent

`Agent` carries an `agent_depth` counter (0 for the root agent). Each `agent{}` call checks
this counter against `MAX_AGENT_DEPTH` (3) and rejects the spawn if the limit is reached.
The counter is threaded through `ToolContext` so every tool dispatch knows the current depth.

### Output Tagging

All terminal output is prefixed with `[actor_id]`. The `print_actor_text` function uses a
`Mutex<(last_actor, at_line_start)>` singleton to detect when the active actor changes and
insert a newline, preventing two actors from interleaving on the same line.

### Path Security

Two resolution modes exist: **Write mode** (`workspace_path`) restricts to `workspace.root`; **Read mode** (`readable_path`) also allows `allowed_roots`. Paths are canonicalized (symlinks resolved to real paths) before the boundary check to prevent escape via symlinks (e.g. `workspace/evil_link -> /etc`). `allowed_roots` is fixed at startup (`~/.ogent` and skill roots); no runtime additions are permitted.

---

## File Routing Map

| Request area | Start here | Also check |
|---|---|---|
| CLI flags and agent startup | [src/main.rs](src/main.rs) | [README.md](README.md) |
| Agent turn loop | [src/agent.rs](src/agent.rs) | [src/types.rs](src/types.rs) |
| Tool dispatch and registry | [src/tools/mod.rs](src/tools/mod.rs) | — |
| Lua sandbox and subagent DSL | [src/tools/lua.rs](src/tools/lua.rs) | [src/tools/mod.rs](src/tools/mod.rs) |
| Filesystem and editing tools | [src/tools/fs.rs](src/tools/fs.rs) | [src/hashline.rs](src/hashline.rs) |
| Anchored edit mechanics | [src/hashline.rs](src/hashline.rs) | [src/tools/fs.rs](src/tools/fs.rs) |
| Shell execution | [src/tools/shell.rs](src/tools/shell.rs) | [src/workspace.rs](src/workspace.rs) |
| Git operations | [src/tools/git.rs](src/tools/git.rs) | [src/tools/mod.rs](src/tools/mod.rs) |
| Workspace path validation | [src/workspace.rs](src/workspace.rs) | [src/tools/fs.rs](src/tools/fs.rs) |
| System prompt and messages | [src/prompts.rs](src/prompts.rs) | PROMPT_SYSTEM.md |
| Skills discovery and loading | [src/skills.rs](src/skills.rs) | [src/tools/skills.rs](src/tools/skills.rs) |
| Session persistence | [src/session.rs](src/session.rs) | [src/workspace.rs](src/workspace.rs) |
| LLM HTTP client | [src/client.rs](src/client.rs) | [src/providers.rs](src/providers.rs), [src/sse.rs](src/sse.rs) |
| Config loading | [src/config.rs](src/config.rs) | — |

---

## Session Layout

```
{workspace_root}/.ogent/
  config.yaml
  sessions/
    {session_id}.jsonl      ← one Message per line (JSONL)
  skills/
    {skill_name}.md
```
