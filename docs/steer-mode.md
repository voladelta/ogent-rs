# Steer Mode

`--steer` starts an interactive terminal UI.

```bash
cargo run -- --steer --profile ds-pro "Write a small web server"
```

The TUI shows:

- a status bar with profile, model, turn, token count, and auto mode
- a scrollable log of reasoning summaries, assistant content, tool calls, and worker updates
- an input box for steering messages and commands

Supported commands:

| Input | Effect |
|---|---|
| `/auto` | Enable auto-continuation |
| `/stop` | Disable auto-continuation after the current turn |
| `/complete` | Ask the agent to summarize the session, call `complete`, save the journal entry, and exit |
| `/cancel` | Cancel the in-flight model request |
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
