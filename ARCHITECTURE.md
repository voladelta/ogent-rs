# Architecture

## Runtime Shape

```text
main.rs
  -> config.rs
  -> agent.rs
    -> workspace.rs
    -> client.rs + providers.rs + sse.rs
    -> tools/ + session.rs
      -> lua.rs (exec/eval VM sandbox)
      -> hashline.rs
```

## Module Ownership

- [src/main.rs](file:///Users/mbp/Codehub/ogent-rs/src/main.rs)
  - CLI parsing, startup verification (e.g. EXA_API_KEY checks), and agent process launch.
- [src/config.rs](file:///Users/mbp/Codehub/ogent-rs/src/config.rs)
  - `config.yaml` loader with repo-level (`{workspace}/.ogent/config.yaml`) then home (`~/.ogent/config.yaml`) fallback.
  - Holds configuration profiles, providers, and default profile options.
- [src/agent.rs](file:///Users/mbp/Codehub/ogent-rs/src/agent.rs)
  - Agent turn loop, event loop execution, and tool-call coordination.
  - Owns the immutable `Workspace` and coordinates session persistence.
- [src/workspace.rs](file:///Users/mbp/Codehub/ogent-rs/src/workspace.rs)
  - Workspace root abstraction, sandbox boundaries, and safe path resolution.
  - Prevents the agent from executing commands or accessing files outside the workspace root.
- [src/client.rs](file:///Users/mbp/Codehub/ogent-rs/src/client.rs), [src/providers.rs](file:///Users/mbp/Codehub/ogent-rs/src/providers.rs), [src/sse.rs](file:///Users/mbp/Codehub/ogent-rs/src/sse.rs)
  - LLM client initialization, provider payload generation, Server-Sent Events (SSE) parsing, and partial JSON argument repair.
- [src/tools/mod.rs](file:///Users/mbp/Codehub/ogent-rs/src/tools/mod.rs)
  - Tool registry (`ToolDef`), dispatch, and schema collection. Submodules own each domain:
    - `lua.rs` — Sandboxed Lua 5.5 environment (`exec` and `eval` tools), dynamic tool wrappers, and positional helper functions.
    - `fs.rs` — read_file, write_file, hash-anchor read/edit.
    - `shell.rs` — shell command execution and cd policy.
    - `repo.rs` — repo_map.
    - `web.rs` — Exa web API client.
    - `skills.rs` — load_skill, list_skills, load_skill_asset.
- [src/session.rs](file:///Users/mbp/Codehub/ogent-rs/src/session.rs)
  - Transcript persistence and workspace-scoped session state routing.
- [src/prompts.rs](file:///Users/mbp/Codehub/ogent-rs/src/prompts.rs)
  - Standard agent instructions, skill loading, and startup skill discovery injection.
- [src/hashline.rs](file:///Users/mbp/Codehub/ogent-rs/src/hashline.rs)
  - Implementation of safe editing via FNV-1a line hashing and validation.

---

## File Routing Map

Use this map to locate source files for specific request areas:

| Request Area | Start Here | Also Check |
| --- | --- | --- |
| CLI flags and agent process launch | [src/main.rs](file:///Users/mbp/Codehub/ogent-rs/src/main.rs) | [README.md](file:///Users/mbp/Codehub/ogent-rs/README.md) |
| Agent loop and execution | [src/agent.rs](file:///Users/mbp/Codehub/ogent-rs/src/agent.rs) | [PROMPT_SYSTEM.md](file:///Users/mbp/Codehub/ogent-rs/PROMPT_SYSTEM.md) |
| Tool schemas and behavior | [src/tools/mod.rs](file:///Users/mbp/Codehub/ogent-rs/src/tools/mod.rs) | [src/hashline.rs](file:///Users/mbp/Codehub/ogent-rs/src/hashline.rs) |
| Lua VM scripting sandbox | [src/tools/lua.rs](file:///Users/mbp/Codehub/ogent-rs/src/tools/lua.rs) | [src/tools/mod.rs](file:///Users/mbp/Codehub/ogent-rs/src/tools/mod.rs) |
| System prompt and initial messages | [src/prompts.rs](file:///Users/mbp/Codehub/ogent-rs/src/prompts.rs) | [PROMPT_SYSTEM.md](file:///Users/mbp/Codehub/ogent-rs/PROMPT_SYSTEM.md) |
| Session routing | [src/session.rs](file:///Users/mbp/Codehub/ogent-rs/src/session.rs) | [src/workspace.rs](file:///Users/mbp/Codehub/ogent-rs/src/workspace.rs) |
| Workspace path validation | [src/workspace.rs](file:///Users/mbp/Codehub/ogent-rs/src/workspace.rs) | [src/tools/fs.rs](file:///Users/mbp/Codehub/ogent-rs/src/tools/fs.rs) |
| Anchored editing mechanics | [src/hashline.rs](file:///Users/mbp/Codehub/ogent-rs/src/hashline.rs) | [src/tools/fs.rs](file:///Users/mbp/Codehub/ogent-rs/src/tools/fs.rs) |
| Skills discovery and loading | [src/skills.rs](file:///Users/mbp/Codehub/ogent-rs/src/skills.rs) | [src/tools/skills.rs](file:///Users/mbp/Codehub/ogent-rs/src/tools/skills.rs) |

---

## Key Invariants

1. **Single CLI Agent Process**: CLI launches exactly one agent process.
2. **Agent Prompt & Tool Scope**: The LLM agent loop is strictly limited to `exec` and `eval` tools. All workspace tools (filesystem, shell commands, skills, and web search) are exposed solely through the Lua execution sandbox via positional or table-argument functions.
3. **Immutable Workspace**: Every Agent owns a single, immutable `Workspace` root derived from the process's current directory at startup.
4. **Workspace Sandboxing**: Tool executions, shell command execution directories, and session file operations must resolve strictly within the active `Workspace` root.
5. **Agent-Only Edits**: All agent file modifications must occur via the `write_file` or `apply_anchor_edits` tools.
6. **Graceful Exit**: An agent run terminates once the model emits a final text response with no pending tool calls.
7. **Skill Injection**: Startup skill discovery and injection remain fully enabled.
8. **Lua Sandbox Safety**: The Lua execution engine restricts scripts to safe libraries (no `os`, `io`, `debug`, or `package`), limits memory to **32MB**, and caps CPU cycles to **32,000 instructions**.
9. **Skill Path Whitelisting**: Skills are searched for and loaded from exactly five whitelisted directories (under `cwd/` and `~/`), and skill assets loaded via `load_skill_asset` undergo strict path traversal verification to prevent arbitrary file read access.
10. **File Size Limits**: Files read via sandbox filesystem operations (`read_file`, `read_hash_anchors`) and skill assets (`load_skill_asset`) are subject to a **1MB** size limit to ensure runtime stability and prevent unbounded memory usage.

---

## Session Layout

Session transcripts are stored at the repository level:

```txt
{workspace_root}/.ogent/
  sessions/
    {session_id}.jsonl
```

- **Transcript Persistence**: Direct CLI invocations write the conversation transcript to `{session_id}.jsonl` on exit.
