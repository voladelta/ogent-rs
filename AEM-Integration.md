# AEM Integration Design for ogent

> Port the architectural concepts of Anchored Episodic Memory (AEM) into ogent to improve harness of the agent after sessions.

> Reference repo: ~/Codehub/aem

## Separation of Concerns

| Layer | When | What |
|---|---|---|
| **Runtime agent** | Every turn | Has a `recall` tool to query the local memory DB. Lightweight — just a SQLite query + prompt injection. |
| **Offline ingestion** | Post-hoc (human or cron) | Scans `.ogent/sessions/` for unvisited sessions, chunks them, and writes to `.ogent/memory.db`. |
| **Human traces** | Anytime | A plain-text or JSONL log of what was recalled, what the agent did with it, and whether the run succeeded. Humans read this to reject bad memories or promote lessons. |

## Memory Store

A single SQLite file at `.ogent/memory.db`. Four tables, ported from AEM:

```sql
-- episodes: one per ingested session
CREATE TABLE episodes (
    id TEXT PRIMARY KEY,              -- hash of session content
    session_id TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    domain TEXT,
    task_type TEXT,
    goal TEXT,
    input_summary TEXT,
    outcome TEXT,                     -- success | partial | failure
    score REAL,                       -- 0.0..1.0 heuristic
    summary TEXT,
    fix_summary TEXT,
    artifact_paths TEXT,              -- JSON array
    raw_path TEXT NOT NULL            -- path to original messages.jsonl
);

-- chunks: searchable slices
CREATE TABLE chunks (
    id TEXT PRIMARY KEY,
    episode_id TEXT NOT NULL REFERENCES episodes(id),
    kind TEXT NOT NULL,               -- goal | situation | action | result | failure | fix
    content TEXT NOT NULL
);

-- lessons: gated abstractions (sparse)
CREATE TABLE lessons (
    id TEXT PRIMARY KEY,
    domain TEXT,
    title TEXT NOT NULL,
    applies_when TEXT,
    do_not_use_when TEXT,
    rule TEXT NOT NULL,
    evidence_episode_ids TEXT,        -- JSON array
    eval_delta REAL,
    status TEXT NOT NULL              -- active | rejected | retired
);

-- uses: recall traces
CREATE TABLE uses (
    id INTEGER PRIMARY KEY,
    run_session_id TEXT NOT NULL,
    episode_id TEXT REFERENCES episodes(id),
    lesson_id TEXT REFERENCES lessons(id),
    recalled_at INTEGER,
    helpful INTEGER                   -- NULL = unknown; 1 = yes; 0 = no
);
```

**Key invariant:** `raw_path` always points to the original `.ogent/sessions/{id}/messages.jsonl`. Episodes are immutable. If the original session is deleted, the episode stays but loses raw traceability.

## Offline Ingestion (`ogent memory-ingest`)

A new CLI subcommand, not a tool the agent calls.

```bash
# Scan .ogent/sessions/ for sessions not yet in memory.db
ogent memory-ingest

# Dry-run: show what would be ingested
ogent memory-ingest --dry-run

# Ingest a specific session
ogent memory-ingest --session 1778216383-2028
```

### Ingestion logic

For each unvisited session directory:

1. **Read** `meta.json` and `messages.jsonl`
2. **Compute** `episode.id` = stable hash of `(meta + transcript)`
3. **Skip** if `id` already exists in `episodes`
4. **Determine outcome** heuristically:
   - Scan tool results in transcript for `ERROR:` or non-zero bash exits → `failure`
   - If `complete` was called with open tracked work → `partial`
   - Else `success`
5. **Extract goal** from `meta.prompt` or first user message
6. **Extract summary** from `complete` tool call in transcript, or fall back to `journal.md` entry
7. **Chunk** the transcript:
   - `goal`: first user message
   - `situation`: initial `repo_map` or `read_file` outputs
   - `action`: sequences of `edit_hash_anchors` + `bash` calls
   - `result`: final assistant message before `complete`
   - `failure`: tool errors or failing test outputs
   - `fix`: the corrective edit/bash that resolved the failure
8. **Insert** `episode` + `chunks`

### Where it lives

Add `src/memory/ingest.rs` and wire a subcommand in `main.rs` (next to `--steer`, `--continue`, etc.) or as a standalone binary under `src/bin/ingest.rs`. Given ogent already links SQLite (not yet, but we'd add `rusqlite`), a subcommand is simpler.

## Runtime `recall` Tool

Add to `tools.rs` / `build_coder_tools`:

```json
{
  "name": "recall",
  "description": "Search prior task episodes and lessons for relevant experience.",
  "parameters": {
    "type": "object",
    "properties": {
      "query": { "type": "string", "description": "The task or problem to find prior experience for." },
      "domain": { "type": "string", "description": "Optional domain filter, e.g. 'coding', 'docs'." },
      "limit": { "type": "integer", "default": 5 }
    },
    "required": ["query"]
  }
}
```

Implementation (`src/memory/recall.rs`):

```rust
pub fn recall(query: &str, domain: Option<&str>, limit: usize) -> Result<MemoryPack> {
    // 1. Tokenize query
    // 2. Load candidate episodes (filter by domain if given)
    // 3. Score by lexical overlap + Jaccard on chunks
    // 4. Load active lessons linked to top episodes
    // 5. Log use to `uses` table (helpful = NULL)
    // 6. Return MemoryPack
}
```

`MemoryPack` JSON returned to the agent:

```json
{
  "episodes": [
    {
      "id": "ep_abc123",
      "goal": "Fix stale cache after settings update",
      "outcome": "success",
      "summary": "Added invalidation after config writes...",
      "chunks": [
        {"kind": "action", "content": "Edited cache.ts to call invalidate()..."},
        {"kind": "result", "content": "Tests pass."}
      ],
      "artifacts": ["src/cache.ts"]
    }
  ],
  "lessons": [
    {
      "id": "lsn_def456",
      "title": "Invalidate cache after durable config writes",
      "rule": "After a committed config write, invalidate or refresh the cache.",
      "applies_when": "...",
      "evidence_count": 4
    }
  ]
}
```

The agent can read this and decide what to do. No automatic injection into the system prompt — the agent explicitly chooses to call `recall` when it thinks prior context would help.

### Prompt discipline

Add a short note to the system prompt:

> If a task feels familiar or you are unsure about an approach, call `recall` to check for prior episodes and lessons.

## Traces for Human Improvement

After `recall` runs, ogent writes a trace entry:

```jsonl
// .ogent/memory/traces.jsonl
{"run_session_id":"1778216383-2028","recalled_at":1715420000,"query":"fix cache bug","episode_ids":["ep_abc"],"lesson_ids":["lsn_def"],"helpful":null}
```

Humans (or a future cron job) read `traces.jsonl` to see:
- What was the query?
- What came back?
- Did the run succeed? (cross-reference with `episodes.outcome`)

If a recalled episode was misleading, a human can run:

```bash
# Mark an episode as unhelpful in a specific run
ogent memory-feedback --run 1778216383-2028 --episode ep_abc --helpful false

# Or retire a bad lesson
ogent memory-retire --lesson lsn_def
```

This feedback updates the `uses.helpful` column and eventually informs ranking adjustments (or manual lesson retirement).

## Retention and Pruning

Old episodes that are not referenced by lessons and have no recent recall traces can be pruned to keep the DB bounded.

```bash
# Prune episodes with no recall in the last <days> and no linked lessons
ogent memory-prune 30
ogent memory-prune 90
```

Safe defaults:
- Never prune episodes linked to active lessons.
- Never prune pinned episodes (future `ogent memory-pin <episode-id>`).
- Only prune episodes where `last_recalled_at` is older than the threshold and no `uses` row exists with `helpful=1`.

This can be run manually or scheduled via cron.

## Code Map

| File / New module | Role |
|---|---|
| `src/memory/db.rs` | SQLite schema, connection, migrations |
| `src/memory/ingest.rs` | Offline session → episode + chunk conversion |
| `src/memory/recall.rs` | Query ranking, `MemoryPack` assembly |
| `src/memory/render.rs` | Format `MemoryPack` into readable text for the agent |
| `src/memory/mod.rs` | Public API: `ingest`, `recall`, `feedback` |
| `src/tools.rs` | Add `"recall"` arm + schema in tool builder |
| `src/main.rs` | Add `memory-ingest`, `memory-feedback`, `memory-retire`, `memory-prune` CLI subcommands |

## Why This Fits

- **No runtime bloat:** The agent only pays for a SQLite query when it calls `recall`. No background indexing. No mandatory startup cost.
- **Immutable evidence:** Original `messages.jsonl` is never touched. The memory DB only points to it.
- **Human gate:** Ingestion is offline, so a human can review what goes in. Traces give humans visibility to correct bad recalls.
- **Fail-open:** If `.ogent/memory.db` is missing, `recall` returns "No memory found" and the agent continues normally.
- **No LLM in memory loop:** Ranking is lexical/Jaccard. No API calls. Fast and deterministic.

## Open Questions

1. **Chunking depth** — Do we extract chunks greedily at ingest time, or lazily at recall time? Eager is simpler and matches AEM.
2. **Lesson creation** — Should the agent ever propose a lesson via a tool (e.g., after `complete` it notices a pattern), or should lessons be purely human-authored? I lean toward: agent can propose, human ingests/promotes offline.
