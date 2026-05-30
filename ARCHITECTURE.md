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
  - CLI parsing, startup verification (e.g. EXA_API_KEY checks), allowed workspace root initialization (e.g. adding `~/.ogent/` to allowed roots), and agent process launch.
- [src/config.rs](file:///Users/mbp/Codehub/ogent-rs/src/config.rs)
  - `config.yaml` loader with repo-level (`{workspace}/.ogent/config.yaml`) then home (`~/.ogent/config.yaml`) fallback.
  - Holds configuration profiles, providers, and default profile options.
- [src/agent.rs](file:///Users/mbp/Codehub/ogent-rs/src/agent.rs)
  - Agent turn loop, event loop execution, and tool-call coordination.
  - Coordinates session persistence.
  - Exposes `AgentOutputSink`/`CliOutputSink` for normal vs. verbose outputs.
  - Implements thread-safe `print_actor_text` tag prefixing (`[director]`, `[<role>]`) to prevent concurrent streaming output interleaving.
- [src/workspace.rs](file:///Users/mbp/Codehub/ogent-rs/src/workspace.rs)
  - Workspace root abstraction, sandbox boundaries, and safe path resolution.
  - Prevents the agent from executing commands or accessing files outside the workspace root (with exception of global allowed roots such as `~/.ogent`).
- [src/client.rs](file:///Users/mbp/Codehub/ogent-rs/src/client.rs), [src/providers.rs](file:///Users/mbp/Codehub/ogent-rs/src/providers.rs), [src/sse.rs](file:///Users/mbp/Codehub/ogent-rs/src/sse.rs)
  - LLM client initialization, provider payload generation, Server-Sent Events (SSE) parsing, and partial JSON argument repair.
- [src/tools/mod.rs](file:///Users/mbp/Codehub/ogent-rs/src/tools/mod.rs)
  - Tool registry (`ToolDef`), dispatch, and schema collection. Submodules own each domain:
    - `lua.rs` — Sandboxed Lua 5.5 environment (`exec` and `eval` tools), dynamic tool wrappers, positional helper functions, and the subagent workflow functions: `agent`, `parallel`, `task_update`.
    - `fs.rs` — read_file, write_file, hash-anchor read/edit.
    - `shell.rs` — shell command execution and cd policy.
    - `repo.rs` — repo_map.
    - `web.rs` — Exa web API client.
    - `skills.rs` — load_skill, list_skills, load_skill_asset.
- [src/session.rs](file:///Users/mbp/Codehub/ogent-rs/src/session.rs)
  - Transcript persistence and workspace-scoped session state routing.
- [src/prompts.rs](file:///Users/mbp/Codehub/ogent-rs/src/prompts.rs)
  - Standard agent instructions and prompt assembly.
  - Dynamically resolves subagent role guidelines checking workspace `.ogent/`, workspace root, and global `~/.ogent/` paths, falling back to embedded `PROMPT_ROLE_GENERIC.md` if no file exists.
  - Assembles subagent message histories as separate system/user/human messages.
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

1. **Single CLI Agent Process**: CLI launches exactly one root agent process (`director`).
2. **Agent Prompt & Tool Scope**: The LLM agent loop is strictly limited to `exec` and `eval` tools. All workspace tools (filesystem, shell commands, skills, and web search) and subagent operations are exposed solely through the Lua execution sandbox.
3. **Immutable Workspace**: Every Agent owns a single, immutable `Workspace` root derived from the process's current directory at startup.
4. **Workspace Sandboxing**: Tool executions, shell command execution directories, and session file operations must resolve strictly within the active `Workspace` root or designated allowed roots (`~/.ogent`).
5. **Agent-Only Edits**: All agent file modifications must occur via the `write_file` or `apply_anchor_edits` tools.
6. **Graceful Exit**: An agent run terminates once the model emits a final text response with no pending tool calls.
7. **Dynamic Skills**: Skills are loaded dynamically via `list_skills` and `load_skill` functions in the Lua sandbox rather than injected at startup.
8. **Lua Sandbox Safety**: The Lua execution engine restricts scripts to safe libraries (no `os`, `io`, `debug`, or `package`), limits memory to **32MB**, and caps CPU cycles to **32,000 instructions**.
9. **Asynchronous Sandbox Timeout Hooks**: To ensure run-away Lua loops abort gracefully in an async context, scripts run inside an isolated `LuaThread` (coroutine) and standard instruction hooks are registered directly on the coroutine thread (`thread.into_async(())?.await`), preventing Tokio thread worker lockups.
10. **Skill Path Whitelisting**: Skills are searched for and loaded from exactly five whitelisted directories (under `cwd/` and `~/`), and skill assets loaded via `load_skill_asset` undergo strict path traversal verification.
11. **File Size Limits**: Files read via sandbox filesystem operations (`read_file`, `read_hash_anchors`) and skill assets (`load_skill_asset`) are subject to a **1MB** size limit to ensure runtime stability.
12. **Actor Tagging and Prefixing**: All stdout prints and streamed LLM responses are tagged with `[<actor_id>]` prefixes. Thread-safe buffering ensures tags and contents are printed cleanly without line corruption or inter-actor interleaving.
13. **Subagent VM State Isolation**: Subagents are spawned in independent, sandboxed Lua VMs with completely fresh global environments and isolated execution state to avoid thread contention, deadlocks, and cross-agent state leakage.

---

## Session Layout

Session transcripts are stored at the repository level:

```txt
{workspace_root}/.ogent/
  sessions/
    {session_id}.jsonl
```

- **Transcript Persistence**: Direct CLI invocations write the conversation transcript to `{session_id}.jsonl` on exit.
