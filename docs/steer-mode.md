# Steer Mode

`--steer` starts an interactive terminal UI.

```bash
cargo run -- --steer --profile ds-pro "Write a small web server"
```

The TUI shows:

- a status bar with profile, model, token count, and autocompact threshold/usage
- a scrollable log of reasoning summaries, assistant content, tool calls, and worker updates
- an input box for steering messages and commands

Supported commands:

| Input | Effect |
|---|---|
| `/complete` | Ask the agent to summarize the session, call `complete`, save the journal entry, and exit |
| `/cancel` | Cancel the in-flight model request |
| `/compact` | Compact the session: ask the model for a handoff brief, then spawn a new child session with the summary. The parent session is preserved on disk |
| `/compact <focus>` | Compact and focus the new session on a specific task |
| `/new` | Restart the session from scratch: clear history (except system prompt), reset turns/tokens/workers, and wait for input |
| `/q`, `/quit`, `quit`, `exit`, `Esc`, `Ctrl-C` | Exit steer mode |
| any other text | Abort the in-flight model request, append the text as a new user message, and re-prompt |

Navigation:

- `Up` / `Down`: scroll one line
- `PageUp` / `PageDown`: scroll one page
- `Home` / `End`: jump to top or follow bottom
- mouse wheel: scroll log

If you run steer mode without an initial prompt, the TUI waits for your first message:

```bash
cargo run -- --steer
```

When a steering message arrives during an LLM stream, the agent cancels the in-flight request, preserves any partial assistant content/tool calls already accumulated, appends your message, and starts the next turn.

## Compaction

`/compact` asks the model to produce a handoff brief (goal, what was done, current state, relevant excerpts, next steps). The response becomes the first user message in a new child session. The parent session is preserved on disk — the new session can read `.ogent/sessions/<parent-id>/messages.jsonl` if it needs details lost in the summary.

If a task tracker is active (goal, phases, todos), the full tracker state is included in the handoff request so the new session can resume without losing track.

`--autocompact <percent>` (default: 80) enables automatic compaction. When token usage crosses the threshold, the agent is nudged to complete current work, then a handoff brief is requested automatically.

The status bar shows `compact@80% [48% used]` when autocompact is enabled.
