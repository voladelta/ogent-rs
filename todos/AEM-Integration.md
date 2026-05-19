# Anchored Episodic Memory for ogent

Implementation reference for adding evidence-backed memory to ogent.

Goal: make useful past experience available without making memory authoritative, hidden, stale, or expensive.

## Invariants

1. Immutable raw episode snapshots are the source of truth.
2. Every atom, scenario, and lesson must trace to an immutable snapshot or explicit human input.
3. Memory is advisory. The agent must verify current repo state before acting.
4. Runtime memory access fails open. Missing or corrupt memory must not stop a
   task.
5. Incomplete or active sessions are skipped by default.
6. Stable operating memory may be injected at startup, under a strict size budget.
7. Task-specific recall is explicit through the `recall` tool.
8. Worker processes do not get `recall`; the parent passes relevant memory.
9. Memory writes happen only through ingestion, feedback, promotion, retirement, and trace logging.
10. Redaction runs before content enters SQLite, FTS, primer, or recall output.

## Memory Model

| Layer | Name     | Role                                                         |
| ----- | -------- | ------------------------------------------------------------ |
| L0    | Episode  | Immutable snapshot of one ogent session                      |
| L1    | Atom     | Evidence-backed fact, action, failure, fix, or result        |
| L2    | Scenario | Reusable software-task pattern distilled from evidence       |
| L3    | Lesson   | Stable user preference, repo rule, process rule, or bug rule |

Only active lessons of type `preference`, `repo_rule`, or `process_rule` are eligible for startup injection. Episodes, atoms, and scenarios are returned only through explicit recall or inspect commands.

Active-session compression remains part of ogent's existing compaction design and is not stored, ranked, or recalled through this memory DB.

## Storage

```text
.ogent/
  sessions/<session-id>/
    meta.json
    messages.jsonl
    states.json
    workers/<worker-id>/
      messages.jsonl
      states.json
  memory/
    memory.db
    traces.jsonl
    raw/<episode-id>/
      meta.json
      messages.jsonl
```

`memory.db` is the authoritative index. `traces.jsonl` is a readable append-only mirror of recall events. Raw snapshots under `.ogent/memory/raw/` are immutable evidence copies created during ingestion because resumable sessions can rewrite `.ogent/sessions/<id>/messages.jsonl`.

Required dependencies:

```toml
rusqlite = { version = "0.32", features = ["bundled"] }
sha2 = "0.10"
regex = "1"
```

SQLite setup:

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
```

Use SQLite FTS5 through bundled SQLite. Do not add embeddings or remote model calls to ingestion, primer rendering, or recall ranking.

## IDs

| Type     | Format          | Stable source                                           |
| -------- | --------------- | ------------------------------------------------------- |
| Episode  | `ep_<12 hex>`   | SHA-256 of normalized `meta.json` plus `messages.jsonl` |
| Atom     | `atom_<12 hex>` | SHA-256 of episode id, kind, content, evidence ref      |
| Scenario | `scn_<12 hex>`  | SHA-256 of title and evidence ids at creation           |
| Lesson   | `lsn_<12 hex>`  | SHA-256 of title, rule, scope, source at creation       |

Reingesting unchanged input must produce the same episode and atom IDs. Editing a scenario or lesson changes `updated_at` but not its ID.

## Schema

```sql
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS episodes (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    parent_session_id TEXT,
    kind TEXT NOT NULL,                 -- agent | steer | worker
    status TEXT NOT NULL,               -- active | superseded | pruned
    superseded_by_episode_id TEXT,
    profile TEXT,
    prompt TEXT,
    started_at INTEGER,
    ended_at INTEGER,
    ingested_at INTEGER NOT NULL,
    transcript_sha256 TEXT NOT NULL,
    meta_sha256 TEXT NOT NULL,
    raw_messages_path TEXT NOT NULL,
    raw_meta_path TEXT NOT NULL,
    source_messages_path TEXT NOT NULL,
    source_meta_path TEXT NOT NULL,
    outcome TEXT NOT NULL,              -- success | partial | failure | unknown
    outcome_confidence REAL NOT NULL,
    completion_summary TEXT,
    artifact_paths_json TEXT NOT NULL,
    tool_error_count INTEGER NOT NULL DEFAULT 0,
    command_failure_count INTEGER NOT NULL DEFAULT 0,
    pinned INTEGER NOT NULL DEFAULT 0,
    UNIQUE(session_id, transcript_sha256)
);

CREATE TABLE IF NOT EXISTS atoms (
    id TEXT PRIMARY KEY,
    episode_id TEXT NOT NULL REFERENCES episodes(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    content TEXT NOT NULL,
    evidence_ref_json TEXT NOT NULL,
    artifact_paths_json TEXT NOT NULL,
    importance REAL NOT NULL DEFAULT 0.5,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS scenarios (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    domain TEXT,
    task_type TEXT,
    applies_when TEXT NOT NULL,
    do_not_use_when TEXT,
    pattern TEXT NOT NULL,
    failure_modes TEXT,
    evidence_episode_ids_json TEXT NOT NULL,
    evidence_atom_ids_json TEXT NOT NULL,
    status TEXT NOT NULL,               -- draft | active | rejected | retired
    confidence REAL NOT NULL DEFAULT 0.5,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS lessons (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    lesson_type TEXT NOT NULL,          -- preference | repo_rule | process_rule | bug_rule
    scope TEXT NOT NULL,                -- workspace | repo | global
    applies_when TEXT NOT NULL,
    do_not_use_when TEXT,
    rule TEXT NOT NULL,
    evidence_episode_ids_json TEXT NOT NULL,
    evidence_atom_ids_json TEXT NOT NULL,
    status TEXT NOT NULL,               -- draft | active | rejected | retired
    confidence REAL NOT NULL DEFAULT 0.5,
    priority INTEGER NOT NULL DEFAULT 50,
    source TEXT NOT NULL,               -- human | promoted | ingested
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS recall_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_session_id TEXT NOT NULL,
    query TEXT NOT NULL,
    domain TEXT,
    limit_n INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    returned_text TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS recall_results (
    recall_event_id INTEGER NOT NULL REFERENCES recall_events(id) ON DELETE CASCADE,
    item_type TEXT NOT NULL,            -- atom | episode | scenario | lesson
    item_id TEXT NOT NULL,
    rank INTEGER NOT NULL,
    score REAL NOT NULL,
    helpful INTEGER,                    -- NULL unknown, 1 helpful, 0 unhelpful
    PRIMARY KEY (recall_event_id, item_type, item_id)
);

CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
    item_type,
    item_id UNINDEXED,
    title,
    body,
    tokenize = 'unicode61'
);
```

## Evidence References

Atoms store evidence as JSON:

```json
{
  "session_id": "1778566263-15c66",
  "raw_path": ".ogent/memory/raw/ep_abc123/messages.jsonl",
  "source_path": ".ogent/sessions/1778566263-15c66/messages.jsonl",
  "message_indexes": [12, 13, 14],
  "tool_call_ids": ["call_01"],
  "source": "tool_result",
  "quote_sha256": "..."
}
```

`raw_path` is evidence. `source_path` is diagnostic only and must not be used as evidence after ingestion.

Message indexes are zero-based JSONL line indexes. Parse rows as `Message` from `src/types.rs`. Never index `reasoning_content` directly; it can influence memory only if later exposed in visible content, a tool call, a tool result, or a completion summary.

## Redaction

Before storing or rendering copied content:

- replace obvious secrets with `[REDACTED_SECRET]`
- redact API keys, bearer tokens, private keys, and common `.env` values
- do not index raw environment dumps
- do not index secret-like files or tool output unless forced by a human command

Source session files and immutable raw snapshots are not modified.

## Ingestion

CLI:

```bash
ogent memory ingest
ogent memory ingest --dry-run
ogent memory ingest --session <session-id>
ogent memory ingest --include-incomplete
ogent memory ingest --include-workers
ogent memory ingest --reingest <session-id>
```

Default flow:

1. Scan `.ogent/sessions/*/meta.json`.
2. Ignore sessions without `messages.jsonl`.
3. Ignore worker sessions unless `--include-workers`.
4. Ignore active or incomplete sessions unless `--include-incomplete`.
5. Read, hash, copy, and rehash `meta.json` and `messages.jsonl` into
   `.ogent/memory/raw/<episode-id>/`.
6. Skip if source files changed during copy.
7. Skip if `(session_id, transcript_sha256)` already exists.
8. If the same `session_id` has another active episode with a different
   transcript hash, mark the old episode `superseded`.
9. Extract outcome, artifacts, and atoms from the immutable snapshot.
10. Insert episode, atoms, and FTS rows in one transaction.

Active or incomplete session if any condition is true:

- no terminal Director result can be inferred from the final assistant message or `states.json` status
- `meta.end_ts` is absent
- source files change while ingestion reads them
- session directory contains an implementation-created lock file

Completion:

- Director session complete: final assistant output exists and the run ended normally, or `states.json` contains terminal `status`
- worker session complete: worker finished successfully and produced output

Outcome:

| Outcome   | Rule                                                                                       |
| --------- | ------------------------------------------------------------------------------------------ |
| `success` | completion exists, no unresolved command/tool failure, final summary claims completion     |
| `partial` | completion includes limitation/intent, forced open work, or unresolved non-fatal failure   |
| `failure` | summary states failure, final blockers remain, or unresolved fatal command failure remains |
| `unknown` | incomplete, malformed, or insufficient evidence                                            |

Failed commands create unresolved failures keyed by normalized command family and nearby artifact paths. A later successful related command or completion summary that explicitly names the failure as resolved may resolve it.

Atom kinds:

- `goal`
- `context`
- `constraint`
- `preference`
- `file`
- `command`
- `edit`
- `decision`
- `failure`
- `fix`
- `verification`
- `result`
- `worker_report`
- `director_state`

Extraction rules:

- first substantive user request -> `goal`
- user constraints/preferences -> `constraint` or `preference`
- `read_file`, `repo_map`, `read_hash_anchors` -> bounded `context` / `file`
- large file output -> summarize by path, symbol, line range; do not copy full text
- `edit_hash_anchors`, `write_file` -> `edit` plus artifact paths
- `bash` -> `command`, `failure`, `verification`
- non-zero command -> `failure`
- later related pass after failure -> `fix` or `verification`
- final assistant output or terminal `status` -> `result`
- visible worker summaries in parent transcript -> `worker_report`

Do not infer a fix without failure, corrective action, and passing verification.

## Scenarios and Lessons

Scenarios and lessons are used in recall only when `status = active`.

CLI:

```bash
ogent memory promote scenario --from-episode <episode-id>
ogent memory promote lesson --from-atom <atom-id>
ogent memory lesson add --title ... --rule ... --applies-when ...
ogent memory scenario activate <scenario-id>
ogent memory lesson activate <lesson-id>
ogent memory lesson retire <lesson-id>
ogent memory scenario retire <scenario-id>
```

Activation requires:

- non-empty `applies_when`
- non-empty `do_not_use_when` for broad rules
- evidence reference unless `source = human`
- explicit or default confidence and priority

## Startup Primer

At non-worker session start, append a bounded primer to initial user context.

Eligibility:

- `lessons.status = active`
- `lesson_type IN ('preference', 'repo_rule', 'process_rule')`
- scope matches current workspace or is global
- priority fits the size budget

Limits:

- max 12 lessons
- max 2,000 rendered characters
- no episodes, atoms, scenarios, or raw evidence
- fail open if DB is missing or broken

Format:

```text
<memory_primer>
Memory is advisory. Verify current repo state before acting on it.
- lsn_...: <rule> Applies: <condition>. Avoid: <condition>. Confidence: <n>.
</memory_primer>
```

## Recall Tool

Add `recall` to Director tools only. Exclude it from worker tools.

```json
{
  "name": "recall",
  "description": "Search prior ogent memory. Memory is advisory; verify current repo state before acting.",
  "parameters": {
    "type": "object",
    "properties": {
      "query": { "type": "string" },
      "domain": { "type": "string" },
      "limit": { "type": "integer", "description": "Default 5, max 10." }
    },
    "required": ["query"],
    "additionalProperties": false
  }
}
```

Behavior:

1. Validate and trim query.
2. If DB is missing or broken, return a short fail-open message.
3. Search active lessons, active scenarios, atoms from active episodes, and active episodes.
4. Rank deterministically.
5. Render bounded text output.
6. Insert `recall_events` and `recall_results`.
7. Append `.ogent/memory/traces.jsonl`.

`recall` is read-only at the agent/repo level even though it writes trace rows. WAL and `busy_timeout` are required so batched recall calls do not normally conflict.

Ranking inputs:

- FTS/BM25 over title/body
- exact file path match boost
- exact tool/error token match boost
- active lesson/scenario boost
- success/partial outcome boost
- failure boost only for debug/error/test queries
- helpful feedback boost
- unhelpful feedback penalty
- low-confidence outcome penalty
- worker episode penalty unless query asks for worker/review/test
- superseded episode exclusion

Output limits:

- max 10 memory groups
- max 3 atoms per episode
- max 6,000 characters total
- max 700 characters per atom
- always preserve IDs and evidence refs

Output shape:

```text
Memory recall for: <query>
Memory is advisory. Verify current repo state before acting.

Lessons:
- <id> [confidence <n>] <title>
  Rule: ...
  Applies: ...
  Avoid: ...

Scenarios:
- <id> <title>
  Applies: ...
  Pattern: ...
  Failure modes: ...

Episodes and atoms:
- <episode id> <outcome> <goal>
  Artifacts: ...
  Relevant atoms:
  - <atom id> <kind>: ...
    Evidence: <raw path>, messages <indexes>

Trace:
- recall_event_id: <id>
```

## Feedback and Inspectability

```bash
ogent memory feedback --event <event-id> --item <item-id> --helpful true
ogent memory feedback --event <event-id> --item <item-id> --helpful false
ogent memory search "add CLI subcommand" --limit 5
ogent memory show episode <episode-id>
ogent memory show atom <atom-id>
ogent memory show scenario <scenario-id>
ogent memory show lesson <lesson-id>
ogent memory stats
```

Feedback updates `recall_results.helpful`; it never deletes evidence.

`show atom` must include evidence JSON and immutable raw snapshot path.

## Pruning

```bash
ogent memory prune --older-than-days 90
```

Rules:

- never prune pinned episodes
- never prune episodes referenced by active scenarios or active lessons
- never prune episodes with helpful recall feedback
- prune active episodes only when superseded and unreferenced
- delete atoms and FTS rows in the DB transaction
- delete raw snapshot files only after DB transaction succeeds
- require preview unless `--yes`

## Integration Points

| File                     | Required change                                          |
| ------------------------ | -------------------------------------------------------- |
| `Cargo.toml`             | Add `rusqlite`, `sha2`, `regex`                          |
| `src/main.rs`            | Add `memory` subcommands and primer injection            |
| `src/memory/mod.rs`      | Public memory API                                        |
| `src/memory/db.rs`       | SQLite connection, schema, migrations, transactions      |
| `src/memory/snapshot.rs` | Immutable raw episode snapshot creation and verification |
| `src/memory/redact.rs`   | Redaction                                                |
| `src/memory/ingest.rs`   | Session scan, outcome detection, atom extraction         |
| `src/memory/recall.rs`   | Search, ranking, trace logging                           |
| `src/memory/render.rs`   | Primer and recall rendering                              |
| `src/memory/feedback.rs` | Feedback, activate, retire                               |
| `src/tools.rs`           | Add Director-only `recall`; mark read-only               |
| `src/prompts.rs`         | Append startup primer to initial user context            |
| `src/session.rs`         | Helpers for listing session dirs and raw paths           |

## Acceptance Cases

Use these as regression tests and implementation checks.

| Case               | Required behavior                                                                         |
| ------------------ | ----------------------------------------------------------------------------------------- |
| Similar feature    | Recall returns scenario/episode/atoms, but agent still reads current files before editing |
| Failure then fix   | Recall groups failure, corrective action, and verification; no fix without all three      |
| Stable preference  | Primer exposes active operating lessons without explicit recall                           |
| Continue old work  | Recall returns immutable snapshot paths; resume/fork remains exact continuation           |
| Incomplete session | Default ingest skips; forced ingest stores `outcome = unknown`                            |
| Worker delegation  | Parent transcript stores worker report atom; worker tools exclude recall                  |
| Secret exposure    | Redacted DB/FTS/output; raw snapshot unchanged                                            |
| Bad memory         | Item-level feedback changes ranking; retirement disables broad bad rules                  |
| Resumed session    | New transcript creates new episode; old active episode becomes `superseded`               |
| Parallel recall    | Concurrent recalls both log traces or fail open cleanly under WAL/busy timeout            |

## Required Tests

Unit tests:

- schema migration creates tables and FTS
- missing DB recall fails open
- redaction removes representative secrets
- incomplete sessions skipped by default
- source mutation during ingest skips session
- completed session creates episode, goal atom, result atom
- raw snapshot remains valid after source session changes
- superseded episodes excluded from default recall
- failed command followed by pass creates failure and verification atoms
- exact file path boosts recall ranking
- unhelpful feedback lowers ranking
- primer includes only active operating lessons and respects budget
- worker tools exclude `recall`
- concurrent recall calls log traces or fail open cleanly

Integration tests:

- ingest fixture session and search it
- recall writes DB event rows and JSONL trace
- `show atom` returns evidence refs
- prune refuses active lesson evidence
- reingest does not duplicate unchanged session

Manual verification:

```bash
cargo test
cargo run -- memory ingest --dry-run
cargo run -- memory ingest --session <fixture-session>
cargo run -- memory search "add CLI subcommand"
cargo run -- "Use memory to see whether we have done a similar CLI change before"
```

Memory is implemented correctly when it can explain every returned claim, improve reuse without hiding current-state checks, expose stable preferences within budget, contain bad/stale/secret data, and remain inspectable by humans.
