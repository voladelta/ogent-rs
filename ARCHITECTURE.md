# Architecture

## Runtime Shape

```text
main.rs
  -> config.rs
  -> agent.rs
    -> workspace.rs
    -> client.rs + providers.rs + sse.rs
    -> tools.rs + session.rs
      -> hashline.rs
      -> symbol_tree.rs
```

## Module Ownership

- [src/main.rs](file:///Users/mbp/Codehub/ogent-rs/src/main.rs)
  - CLI parsing, startup verification (e.g. EXA_API_KEY checks), and worker runtime launch.
- [src/config.rs](file:///Users/mbp/Codehub/ogent-rs/src/config.rs)
  - `config.yaml` loader with repo-level (`{workspace}/.ogent/config.yaml`) then home (`~/.ogent/config.yaml`) fallback.
  - Holds configuration profiles, providers, and default profile options.
- [src/agent.rs](file:///Users/mbp/Codehub/ogent-rs/src/agent.rs)
  - Worker turn loop, event loop execution, and tool-call coordination.
  - Owns the immutable `Workspace` and coordinates session persistence.
- [src/workspace.rs](file:///Users/mbp/Codehub/ogent-rs/src/workspace.rs)
  - Workspace root abstraction, sandbox boundaries, and safe path resolution.
  - Prevents the agent from executing commands or accessing files outside the workspace root.
- [src/client.rs](file:///Users/mbp/Codehub/ogent-rs/src/client.rs), [src/providers.rs](file:///Users/mbp/Codehub/ogent-rs/src/providers.rs), [src/sse.rs](file:///Users/mbp/Codehub/ogent-rs/src/sse.rs)
  - LLM client initialization, provider payload generation, Server-Sent Events (SSE) parsing, and partial JSON argument repair.
- [src/tools.rs](file:///Users/mbp/Codehub/ogent-rs/src/tools.rs)
  - Worker tool schemas, arguments parsing, and backend execution (filesystem, web, bash).
- [src/session.rs](file:///Users/mbp/Codehub/ogent-rs/src/session.rs)
  - Transcript persistence and workspace-scoped session state routing.
- [src/prompts.rs](file:///Users/mbp/Codehub/ogent-rs/src/prompts.rs)
  - Standard worker instructions, skill loading, and startup skill discovery injection.
- [src/hashline.rs](file:///Users/mbp/Codehub/ogent-rs/src/hashline.rs)
  - Implementation of safe editing via FNV-1a line hashing and validation.
- [src/symbol_tree.rs](file:///Users/mbp/Codehub/ogent-rs/src/symbol_tree.rs)
  - Tree-sitter powered AST symbol extraction for Rust and Go files (used by the `code_map` tool).

---

## File Routing Map

Use this map to locate source files for specific request areas:

| Request Area | Start Here | Also Check |
| --- | --- | --- |
| CLI flags and worker launch | [src/main.rs](file:///Users/mbp/Codehub/ogent-rs/src/main.rs) | [README.md](file:///Users/mbp/Codehub/ogent-rs/README.md) |
| Worker loop and execution | [src/agent.rs](file:///Users/mbp/Codehub/ogent-rs/src/agent.rs) | [SYSTEM_PROMPT.md](file:///Users/mbp/Codehub/ogent-rs/SYSTEM_PROMPT.md) |
| Tool schemas and behavior | [src/tools.rs](file:///Users/mbp/Codehub/ogent-rs/src/tools.rs) | [src/hashline.rs](file:///Users/mbp/Codehub/ogent-rs/src/hashline.rs) |
| System prompt and initial messages | [src/prompts.rs](file:///Users/mbp/Codehub/ogent-rs/src/prompts.rs) | [SYSTEM_PROMPT.md](file:///Users/mbp/Codehub/ogent-rs/SYSTEM_PROMPT.md) |
| Session routing | [src/session.rs](file:///Users/mbp/Codehub/ogent-rs/src/session.rs) | [src/workspace.rs](file:///Users/mbp/Codehub/ogent-rs/src/workspace.rs) |
| Workspace path validation | [src/workspace.rs](file:///Users/mbp/Codehub/ogent-rs/src/workspace.rs) | [src/tools.rs](file:///Users/mbp/Codehub/ogent-rs/src/tools.rs) |
| Anchored editing mechanics | [src/hashline.rs](file:///Users/mbp/Codehub/ogent-rs/src/hashline.rs) | [src/tools.rs](file:///Users/mbp/Codehub/ogent-rs/src/tools.rs) |
| Symbol mapping & AST parsing | [src/symbol_tree.rs](file:///Users/mbp/Codehub/ogent-rs/src/symbol_tree.rs) | [src/tools.rs](file:///Users/mbp/Codehub/ogent-rs/src/tools.rs) |

---

## Key Invariants

1. **Single CLI Worker**: CLI launches exactly one worker-mode agent.
2. **Worker Prompt & Tool Scope**: Worker runs use only worker prompts and the full worker toolset.
3. **Immutable Workspace**: Every Agent owns a single, immutable `Workspace` root derived from the process's current directory at startup.
4. **Workspace Sandboxing**: Tool executions, bash directories, and session file operations must resolve strictly within the active `Workspace` root.
5. **Worker-Only Edits**: All worker file modifications must occur via the `write_file` or `edit_hash_anchors` tools.
6. **Graceful Exit**: An agent run terminates once the model emits a final text response with no pending tool calls.
7. **Skill Injection**: Startup skill discovery and injection remain fully enabled.

---

## Session Layout

Session transcripts are stored at the repository level:

```txt
{workspace_root}/.ogent/
  sessions/
    {session_id}/
      messages.jsonl
```

- **Transcript Persistence**: Direct CLI invocations write the conversation transcript to `messages.jsonl` on exit.
