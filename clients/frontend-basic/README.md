# frontend-basic

A zero-build HTML/CSS/JS websocket client for `ogent --serve`.

## Run

Start ogent:

```bash
ogent --serve 127.0.0.1:9876
```

Open the client directly in a browser:

```bash
open clients/frontend-basic/index.html
```

Or serve it as static files later:

```bash
python3 -m http.server 8080 --directory clients/frontend-basic
```

Then open `http://127.0.0.1:8080`.

## Notes

- No bundle step, package manager, or external assets are required.
- The default websocket URL is `ws://127.0.0.1:9876`.
- The client uses one websocket connection for one Director session.
- The session card displays server-provided title metadata when `set_title` emits a session update.
- `new` and `compact` are available, but the current server protocol does not emit the replacement session ID after either action.
