# Reference

## CLI Flags

| Flag | Meaning |
| --- | --- |
| `--profile <name>` | Model/profile selection |
| `--autocompact <percent>` | Auto-compaction threshold (`-1` disables) |
| `--resume[=<session_id>]` | Resume existing session |
| `--fork[=<session_id>]` | Fork existing session into a child session |
| `--temp` | Ephemeral mode (no session persistence) |
| `--create-skill <name>` | Generate/update `.ogent/skills/<name>/SKILL.md` |
| `--serve <addr>` | WebSocket server mode (`ws://<addr>`) |

`--create-skill` cannot be combined with `--resume`, `--fork`, or `--serve`.
`--serve` cannot be combined with `--resume`, `--fork`, or an initial prompt.

## Required Environment

- `DEEPSEEK_API_KEY` must be set for provider access.
- `EXA_API_KEY` must be set at startup. If missing/empty, `ogent` exits immediately with an error.

## WebSocket Protocol (`--serve`)

`--serve` exposes a single-agent websocket control protocol for clients such as TUIs, editor integrations, or browser UIs.

Start a server:

```bash
ogent --serve 127.0.0.1:9876
```

Or during development:

```bash
cargo run -- --serve 127.0.0.1:9876
```

Each websocket connection starts unbound. The first valid event must be `start`, `fork`, or `resume`; setup creates exactly one Director agent for that connection.

### Client State Machine

```text
connect
  -> send start | fork | resume
  -> wait for session event
  -> send message/control events
  -> render status/message/error events until exit or close
```

Clients should treat outbound events as asynchronous. A `status` event can arrive before or after the setup `session` event.

### Inbound Events

Send each event as one websocket text frame containing JSON.

Start a new session:

```json
{"type":"start","repo":"/path/to/repo","temp":false,"profile":"ds-flash","autocompact":80}
```

Fork an existing session into a new child session:

```json
{"type":"fork","repo":"/path/to/repo","session":"<session_id>","profile":"ds-flash","autocompact":80}
```

Resume an existing session:

```json
{"type":"resume","repo":"/path/to/repo","session":"<session_id>","profile":"ds-flash","autocompact":80}
```

Send user input:

```json
{"type":"message","content":"Fix the failing tests"}
```

Control the active agent:

```json
{"type":"cancel"}
{"type":"new"}
{"type":"compact","focus":"optional focus task"}
{"type":"profile","profile":"ds-flash"}
{"type":"exit"}
```

Setup rules:

- `start`, `fork`, and `resume` require `repo`.
- `repo` is canonicalized and must exist as a directory.
- `temp` is valid only on `start`; `fork` and `resume` reject it.
- `profile` and `autocompact` are valid on `start`, `fork`, and `resume`.
- Omitted `profile`, `autocompact`, and `temp` values use the server startup defaults.
- Before setup, non-setup events return `error.code = "not_initialized"`.
- After setup, `start`, `fork`, and `resume` return `error.code = "already_initialized"`.
- `resume` returns `error.code = "session_active"` if that session is already active in this websocket server process.

Runtime control behavior:

- `message` while idle starts a turn.
- `message` during an in-flight turn cancels the current model request, preserves any partial assistant response already received, appends the new user message, and starts a new turn.
- `cancel` cancels only an in-flight turn. If idle, it is effectively a no-op.
- `profile` changes the model profile for later requests. Unknown profiles do not emit a websocket `error` or `status` change.
- `exit` ends the agent loop and closes the websocket after persistence.
- `new` starts a fresh child session inside the same connection.
- `compact` asks the model for a handoff, then starts a fresh child session seeded with that handoff.

Current tracking limitation:

- `new` and successful `compact` rotate the internal `session_id`, but the websocket layer does not currently emit a new `session` event for the replacement session.
- Active-session protection continues tracking the original setup session after `new` or `compact`, not the replacement session.
- Clients that must know or protect the active session ID should avoid these controls for now or discover sessions from `.ogent/sessions/` after the fact.

### Outbound Events

Every outbound event is one websocket text frame containing JSON.

Setup success:

```json
{
  "type": "session",
  "status": "ok",
  "session_id": "<session_id>",
  "mode": "start",
  "profile": "ds-flash",
  "repo": "/canonical/path/to/repo"
}
```

`mode` is one of `start`, `fork`, or `resume`.

The optional `title` field is present when the session has a user-visible title. A later `session` event with `status: "updated"` may update this metadata after the Director calls `set_title`.

Session metadata update:

```json
{
  "type": "session",
  "status": "updated",
  "session_id": "<session_id>",
  "mode": "start",
  "profile": "ds-flash",
  "title": "Fix login button",
  "repo": "/canonical/path/to/repo"
}
```

Status update:

```json
{
  "type": "status",
  "state": "idle",
  "tokens": 12345,
  "profile": "ds-flash",
  "model": "deepseek-v4-flash"
}
```

`state` is one of:

- `idle`: waiting for input or between internal turns.
- `reasoning`: receiving reasoning stream chunks from the provider.
- `replying`: receiving assistant content stream chunks from the provider.
- `working`: the model is requesting or running tools.

Clients should keep only the latest `status` as current state.

Transcript message:

```json
{
  "type": "message",
  "source": "director",
  "role": "assistant",
  "content": "Done.",
  "reasoning_content": "",
  "tool_calls": [],
  "tool_call_id": ""
}
```

Fields:

- `source` is `director` or a worker id such as `worker-1`.
- `role` is the stored transcript role, commonly `assistant` or `tool`.
- `content` is the visible message or tool result content.
- `reasoning_content` may contain model reasoning text when the provider returns it.
- `tool_calls` contains assistant tool call requests.
- `tool_call_id` links a `tool` role message back to the tool call.

Important rendering behavior:

- The websocket protocol currently emits completed transcript messages, not token-level streaming chunks.
- Use `status.state` for live progress indicators while waiting for the next `message`.
- Assistant messages with non-empty `tool_calls` are intermediate; the Director will continue after tool results.
- A final assistant message with empty `tool_calls` means the current turn is complete and the agent is idle again.

Error:

```json
{
  "type": "error",
  "code": "not_initialized",
  "message": "connection is not initialized; send start, fork, or resume first"
}
```

Known error codes:

- `invalid_event`: the inbound text frame was not valid protocol JSON.
- `websocket_read_error`: the websocket read loop failed.
- `not_initialized`: a non-setup event was sent before setup.
- `already_initialized`: setup was attempted after this connection already had an agent.
- `setup_failed`: setup validation or agent launch failed.
- `session_active`: the requested resume session is already active in this server process.
- `agent_error`: the agent loop failed after launch.
- `serialization_failed`: outbound JSON serialization failed.

Most protocol errors do not close the connection. The client may correct the request and continue unless the socket closes.

### Browser Client Example

This minimal HTML/JS client starts a session, appends assistant/tool messages, and tracks status.

```html
<!doctype html>
<meta charset="utf-8" />
<input id="repo" value="/Users/me/project" />
<button id="connect">Connect</button>
<pre id="status">disconnected</pre>
<div id="log"></div>
<input id="input" placeholder="Ask ogent..." />
<button id="send">Send</button>

<script>
let ws;

const log = (text) => {
  const el = document.createElement("pre");
  el.textContent = text;
  document.querySelector("#log").appendChild(el);
};

document.querySelector("#connect").onclick = () => {
  ws = new WebSocket("ws://127.0.0.1:9876");

  ws.onopen = () => {
    ws.send(JSON.stringify({
      type: "start",
      repo: document.querySelector("#repo").value,
      profile: "ds-flash",
      autocompact: 80
    }));
  };

  ws.onmessage = (event) => {
    const msg = JSON.parse(event.data);
    if (msg.type === "session") {
      log(`session ${msg.session_id} (${msg.mode})`);
    } else if (msg.type === "status") {
      document.querySelector("#status").textContent =
        `${msg.state} | ${msg.tokens} tokens | ${msg.profile}`;
    } else if (msg.type === "message") {
      log(`[${msg.source}:${msg.role}] ${msg.content}`);
    } else if (msg.type === "error") {
      log(`[error:${msg.code}] ${msg.message}`);
    }
  };

  ws.onclose = () => {
    document.querySelector("#status").textContent = "closed";
  };
};

document.querySelector("#send").onclick = () => {
  const input = document.querySelector("#input");
  ws.send(JSON.stringify({ type: "message", content: input.value }));
  input.value = "";
};
</script>
```

### TUI Client Shape

A TUI should split websocket reading from user input:

```text
main
  connect ws://127.0.0.1:9876
  send setup event
  spawn reader task:
    for each outbound event:
      session -> store session_id and repo
      status  -> update status bar
      message -> append transcript row
      error   -> append error row
  input loop:
    Enter      -> send {"type":"message","content":buffer}
    Ctrl-C     -> send {"type":"cancel"}
    Ctrl-N     -> send {"type":"new"} only if session-id tracking is not required
    Ctrl-X     -> send {"type":"exit"} and close after server closes
```

Do not block the websocket reader while waiting for keyboard input. The agent can emit status and messages while the user is idle.

### Session Protection

- The server keeps an in-process active session registry for websocket runs.
- `start` and `fork` create and register fresh session IDs.
- `resume` registers the target session only if it is not already active.
- IDs are unregistered when the connection or agent ends.

This only protects sessions inside one websocket server process. CLI `--resume` uses a separate workspace lock file described below.

### Disconnect Behavior

- Client disconnect is treated as `exit`.
- Dirty session data is persisted unless the session is temporary.
- An in-flight request is cancelled through the steer-loop exit path.

### Security Notes

- The websocket server has no authentication or origin checks.
- Prefer binding to `127.0.0.1:<port>` for local clients.
- Do not bind to a public interface unless you put an authenticated, trusted boundary in front of it.

### Known Limitations

- Tool execution, workers, state, and session files are scoped to the setup `repo`; skill discovery and custom system prompt discovery still use the server startup cwd and home config.
- The protocol does not currently expose token-level stream chunks. It exposes status changes plus completed transcript messages.
- `new` and `compact` do not currently send a replacement `session` event after rotating sessions.
- Active-session protection is not updated to the replacement session after `new` or `compact`.

## Resume Locking

`--resume` acquires `{workspace_root}/.ogent/sessions/{session_id}/active.lock` for the process lifetime. A second resume attempt for the same active session fails fast. If a stale lock file exists from a dead process, `ogent` now removes it automatically and continues.

## Director Tools

- `repo_map`
- `bash` (allowlisted to `colgrep` and `rg`)
- `load_skill`
- `state`
- `set_title`
- `dispatch_workers`
- `wait_workers`
- `inspect_worker`
- `cancel_workers`

## Worker Tools

- `read_file`
- `write_file`
- `read_hash_anchors`
- `edit_hash_anchors`
- `repo_map`
- `bash` (normal behavior)
- `web_search`
- `web_read`
- `web_code_context`
- `load_skill`
- `state`

Workers do **not** get `set_title`, `dispatch_workers`, or `wait_workers`.

## `set_title` Tool

Input:

```json
{
  "title": "Fix login button on mobile"
}
```

Rules:

- Director-only.
- Trims surrounding whitespace.
- Rejects empty, multi-line/control-character, or over-80-character titles.
- Stores the title in `{workspace_root}/.ogent/sessions/{session_id}/meta.json`.
- In WebSocket mode, emits a `session` event with `status: "updated"` and the new `title`.

## `state` Tool

Arguments:

```json
{
  "action": "read | write | append | list",
  "path": "string",
  "content": "string"
}
```

Rules:

- `read`, `write`, `append` require non-empty `path`.
- `write` and `append` require `content`.
- `list` allows empty `path`; empty means list all keys.
- Workers use state key `progress/current` for concise current-phase progress. `wait_workers` reads this key for running workers.
- State storage:
  - Director: `{workspace_root}/.ogent/sessions/{session_id}/states.json`
  - Worker: `{workspace_root}/.ogent/sessions/{parent_session_id}/workers/{worker_id}/states.json`

## `dispatch_workers` Tool

Input:

```json
{
  "workers": [
    { "role": "implementer", "task": "..." },
    { "role": "verifier", "task": "..." }
  ]
}
```

Behavior:

- Spawns the full batch.
- Returns worker IDs immediately.
- Does not wait for worker completion.
- Each running worker status includes `progress`, initially `Starting`.
- `completed` is usually empty; it contains only dispatch-time failures such as invalid worker arguments or prompt-resolution errors.
- Call `wait_workers` to collect completed worker results.

Output shape:

```json
{
  "message": "Workers dispatched successfully. Their results are not available yet. Next action: call `wait_workers`.",
  "batch_id": "batch-1",
  "workers": [
    {
      "batch_id": "batch-1",
      "index": 0,
      "role": "implementer",
      "worker_id": "worker-1",
      "status": "running",
      "progress": "Starting"
    }
  ],
  "completed": [
    {
      "batch_id": "batch-1",
      "index": 0,
      "role": "implementer",
      "worker_id": "worker-1",
      "status": "failed",
      "output": "",
      "error": "dispatch-time error"
    }
  ]
}
```

## `wait_workers` Tool

Input:

```json
{}
```

Behavior:

- Returns immediately if any unseen worker result is available.
- Otherwise waits about 15 seconds.
- If no worker finishes during that wait, returns the still-running workers.
- Running worker statuses include `progress`.
- `progress` is read from the worker state key `progress/current`.
- If that key is missing, empty, unreadable, or malformed, `progress` is `Starting`.
- Repeat until the workers needed for the Director decision have completed.

Output shape:

```json
{
  "message": "Completed workers are available. Some workers are still running; call `wait_workers` again before depending on unfinished workers.",
  "completed": [
    {
      "batch_id": "batch-1",
      "index": 0,
      "role": "implementer",
      "worker_id": "worker-1",
      "status": "completed | failed",
      "output": "last assistant message",
      "error": null
    }
  ],
  "running": [
    {
      "batch_id": "batch-1",
      "index": 1,
      "role": "verifier",
      "worker_id": "worker-2",
      "status": "running",
      "progress": "Reading subscription schemas"
    }
  ]
}
```

## `inspect_worker` Tool

Input:

```json
{
  "id": "worker-1"
}
```

Behavior:

- Director-only.
- Reads the worker's persisted `states.json` from disk.
- Returns the raw JSON state map.
- Use to check `progress/current`, partial results, or errors before deciding to cancel or wait.
- Works for running, completed, failed, or cancelled workers.

## `cancel_workers` Tool

Input:

```json
{
  "ids": ["worker-1", "worker-2"]
}
```

Behavior:

- Director-only.
- Aborts in-flight workers immediately.
- Returns cancelled and not-found ids.
- Prefer waiting for workers that have already modified files, to avoid leaving partial or inconsistent changes.
- Consider canceling workers that are stuck, off-track, or have not yet produced durable changes.
- Aborted workers stop at their next await point; partial transcript remains in `messages.jsonl`.

Output shape:

```json
{
  "cancelled": ["worker-1"],
  "not_found": ["worker-2"]
}
```

## Terminal Completion

A run ends when the Director sends a final assistant message (no tool calls). The final user-facing output is the Director’s last assistant message.
