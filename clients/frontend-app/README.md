# frontend-app

A Vite + Svelte 5 websocket frontend for `ogent --serve`.

## Run

Install dependencies:

```bash
bun install
```

Start ogent:

```bash
ogent --serve 127.0.0.1:9876
```

Start the app:

```bash
bun run dev
```

Open the URL printed by Vite, usually `http://127.0.0.1:5173`.

## Model

- Top bar: groups for related work.
- Main rail: horizontally scrolling Director transcript panes.
- Setup pane: creates, resumes, or forks sessions.
- Transcript panes: final assistant content is shown fully; reasoning and tool events are collapsed rows.
- Pane footer: model profile and token count.

## Notes

This app is JavaScript-only Svelte, not TypeScript. It intentionally avoids a UI kit while the interaction model is still settling.
