# Anchored Episodic Memory Integration for ogent

This document is the implementation reference for adding anchored episodic
memory to ogent.

The goal is not to make ogent remember everything. The goal is to make useful
past experience available without losing evidence, hiding state, or turning
memory into authority.

## Design Goal

ogent should remember:

- stable user and repository operating rules
- recurring task patterns
- prior failures and fixes
- successful implementation and verification paths
- enough evidence to inspect where every memory came from

ogent should not:

- treat memory as truth without checking the current repo
- inject long historical summaries into every request
- rely on opaque vector-only recall
- mutate old sessions
- ingest active or incomplete work by default
- let worker subprocesses independently pull hidden context

## Memory Model

ogent memory is layered:

| Layer | Name | Role |
|---|---|---|
| L0 | Raw episodes | Original `.ogent/sessions/<id>/messages.jsonl` evidence |
| L1 | Atoms | Evidence-backed facts, actions, failures, fixes, and results |
| L2 | Scenarios | Reusable software-task patterns distilled from evidence |
| L3 | Operating memory | Stable user preferences, repo rules, and process rules |

Active-session compression remains part of ogent's existing compaction design.
It is not stored, ranked, or recalled through this memory DB.

## Core Invariants

1. Immutable raw episode snapshots are the source of truth.
2. Every atom, scenario, and lesson must trace back to immutable raw episode
   evidence or explicit human input.
3. Runtime memory access must fail open. Missing or corrupt memory must not stop
   the agent.
4. Memory is advisory. The agent must verify current files, commands, and state
   before acting on a recalled fact.
5. Incomplete sessions are not ingested by default.
6. Stable operating memory may be injected at session start, but only under a
   strict size budget.
7. Task-specific recall is explicit through the `recall` tool.
8. Worker processes do not get the `recall` tool. The parent agent decides what
   memory is relevant and passes it in worker context.
9. Memory writes happen through ingestion, feedback, promotion, and retirement
   commands, not through arbitrary agent tool calls.
10. Redaction runs before content enters the memory DB or runtime recall output.

## Storage Layout

Memory lives under `.ogent/memory/`.

```text
.ogent/
  sessions/
    <session-id>/
      meta.json
      messages.jsonl
      workflow-state.json
  memory/
    memory.db
    traces.jsonl
    raw/
      <episode-id>/
        meta.json
        messages.jsonl
```

`memory.db` is the authoritative memory index. `traces.jsonl` is a readable
append-only mirror of recall events for humans. The trace file is redundant by
design; the DB remains the source of truth.

Raw episode snapshots under `.ogent/memory/raw/` are immutable evidence copies
created during ingestion. They are required because ogent sessions can be resumed
and persisted again, which can rewrite `.ogent/sessions/<id>/messages.jsonl`.

Required dependencies:

```toml
rusqlite = { version = "0.32", features = ["bundled"] }
sha2 = "0.10"
regex = "1"
```

Use SQLite FTS5 through bundled SQLite. Do not add embeddings or remote model
calls to the memory path.

Open the DB with:

- `PRAGMA foreign_keys = ON`
- `PRAGMA journal_mode = WAL`
- `PRAGMA busy_timeout = 5000`

This is required because `recall` writes trace rows while otherwise behaving as
a read-only agent tool, and multiple read-only tool calls may be batched.

## Identifier Rules

IDs are stable when they refer to derived evidence, and explicit when they refer
to human-authored abstractions.

| Type | Format | Source |
|---|---|---|
| Episode | `ep_<12 hex>` | SHA-256 of normalized `meta.json` plus `messages.jsonl` |
| Atom | `atom_<12 hex>` | SHA-256 of episode id, kind, content, and evidence ref |
| Scenario | `scn_<12 hex>` | SHA-256 of title and evidence ids at creation time |
| Lesson | `lsn_<12 hex>` | SHA-256 of title, rule, scope, and source at creation time |

Reingesting an unchanged session must produce the same episode and atom IDs.
Editing a lesson or scenario updates `updated_at` but does not change its ID.

## Memory Layers

### L0: Raw Episodes

An episode is one ingested ogent session.

The episode stores metadata and pointers to raw files. It does not copy the full
transcript into the DB. It does copy `meta.json` and `messages.jsonl` into an
immutable memory snapshot directory.

### L1: Atoms

An atom is an evidence-backed unit extracted from an episode.

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
- `workflow`

Atoms are the main searchable layer.

### L2: Scenarios

A scenario is a reusable task pattern distilled from atoms and episodes.

Examples:

- "Adding a new CLI subcommand in ogent"
- "Debugging TUI event-loop regressions"
- "Adding a workflow-gated behavior with tests"

Scenarios are sparse. They may be proposed by ingestion, but only active
scenarios are used in recall.

### L3: Operating Memory

Operating memory is a stable rule, preference, or process convention.

Examples:

- "Use `colgrep` as the primary code search tool."
- "For non-trivial design work, stress-test with hand-computed scenarios."
- "Do not add comments unless the code convention requires them."

Operating memory is the only layer eligible for automatic startup injection.

## SQLite Schema

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
    raw_messages_path TEXT NOT NULL,    -- immutable memory snapshot
    raw_meta_path TEXT NOT NULL,        -- immutable memory snapshot
    source_messages_path TEXT NOT NULL, -- original session path at ingest time
    source_meta_path TEXT NOT NULL,     -- original session path at ingest time
    outcome TEXT NOT NULL,              -- success | partial | failure | unknown
    outcome_confidence REAL NOT NULL,   -- 0.0..1.0
    completion_summary TEXT,
    artifact_paths_json TEXT NOT NULL,  -- JSON array
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
    artifact_paths_json TEXT NOT NULL,  -- JSON array
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

`evidence_ref_json` must be a JSON object with enough information to find the
source in the immutable raw snapshot.

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

The memory DB may store concise redacted content. The immutable raw snapshot
remains the recoverable source. `source_path` is diagnostic only and must not be
used as evidence after ingestion.

## Transcript Parsing Contract

`messages.jsonl` is parsed as ordered `Message` values from `src/types.rs`.

Message indexes in evidence refs are zero-based line indexes in the JSONL file.

Important fields:

- `role`
- `content`
- `reasoning_content`
- `tool_calls`
- `tool_call_id`

Completion summaries are read from the assistant `tool_calls` argument for
`complete` or `worker_complete`, then confirmed by the following tool result.
The tool result must indicate success, for example `Task marked complete.` or
`Worker marked complete.`.

Tool failures are detected from tool-result messages whose content starts with
`ERROR:` or from command output containing a non-zero exit status emitted by the
`bash` tool.

Reasoning content is never indexed directly. It can only influence memory if the
assistant later exposes the decision in visible content, a tool call, a tool
result, or a completion summary.

## Redaction

Before inserting atom, scenario, lesson, FTS, or recall output content:

1. Replace obvious secrets with `[REDACTED_SECRET]`.
2. Replace API keys, bearer tokens, private keys, and common `.env` values.
3. Do not index raw environment dumps.
4. Do not index files or tool output that match secret-like paths unless a human
   explicitly forces ingestion.

Source session files are not modified. Redaction only controls what memory
copies and returns.

## Offline Ingestion

Ingestion is a CLI operation. It is not an agent tool.

```bash
ogent memory ingest
ogent memory ingest --dry-run
ogent memory ingest --session 1778566263-15c66
ogent memory ingest --include-incomplete
ogent memory ingest --reingest 1778566263-15c66
```

Default behavior:

1. Scan `.ogent/sessions/*/meta.json`.
2. Ignore sessions with no `messages.jsonl`.
3. Ignore active or incomplete sessions unless `--include-incomplete` is set.
4. Ignore worker sessions as primary episodes unless `--include-workers` is set.
5. Read `meta.json` and `messages.jsonl`, hash both, copy both into
   `.ogent/memory/raw/<episode-id>/`, then rehash the copied snapshot.
6. Skip the session if source files changed during copy.
7. Skip if `(session_id, transcript_sha256)` already exists.
8. If another active episode exists for the same `session_id` with a different
   transcript hash, mark the old episode `superseded`.
9. Extract metadata, outcome, artifacts, and atoms from the immutable snapshot.
10. Insert episode, atoms, and FTS rows in one transaction.
11. Print a deterministic report.

### Active Session Detection

A session is active when any of these are true:

- no successful completion tool result is present
- `meta.end_ts` is absent
- `meta.json` or `messages.jsonl` changes while ingestion is reading it
- the session directory contains an implementation-created lock file

The lock file is optional for compatibility with old sessions, but the ingestion
copy-and-rehash check is mandatory.

### Completion Detection

A session is complete when the transcript contains a successful `complete` tool
result, or a worker session contains a successful `worker_complete` tool result.

If no completion tool is found:

- default: skip
- with `--include-incomplete`: ingest with `outcome = unknown`

### Outcome Detection

Outcome is inferred from transcript evidence:

- `failure`: completion summary states failure, or final forced stop includes
  unresolved blockers, or command failures are unresolved
- `partial`: completion happened with explicit limitation/intent or open tracked
  work warning was forced
- `success`: completion happened, no unresolved command/tool failure is visible,
  and final summary claims the task was completed
- `unknown`: incomplete session, malformed transcript, or insufficient evidence

`outcome_confidence` must reflect inference quality. Do not store guessed
outcomes as high confidence.

Unresolved command failure rule:

1. A failed command creates an unresolved failure keyed by normalized command
   family and nearby artifact paths.
2. A later successful command from the same family, or a completion summary that
   explicitly names the failure as resolved, can resolve it.
3. If unresolved failures remain at completion, outcome is `partial` or
   `failure` depending on the completion summary.

### Atom Extraction Rules

Extraction must be deterministic and local. No LLM calls.

Sources:

- user messages
- assistant tool calls
- tool results
- completion summary arguments
- worker report summaries
- workflow state when present

Rules:

1. The first substantive user request becomes a `goal` atom.
2. User constraints and preferences become `constraint` or `preference` atoms.
3. `read_file`, `repo_map`, and `read_hash_anchors` outputs may produce
   `context` and `file` atoms, but large file content must be summarized by path,
   symbol, and line range rather than copied.
4. `edit_hash_anchors` and `write_file` calls produce `edit` atoms and artifact
   paths.
5. `bash` calls produce `command`, `failure`, and `verification` atoms.
6. A non-zero command creates a `failure` atom. A later passing related command
   creates a `fix` or `verification` atom.
7. The final `complete` summary produces a `result` atom.
8. Worker summaries produce `worker_report` atoms in the parent episode when
   visible in parent tool results.

Do not infer a fix unless there is evidence of a failing step followed by a
corrective edit or command and then a passing verification.

## Promotion to Scenarios and Lessons

Scenarios and lessons are sparse, higher-authority abstractions.

They can be created by:

- human CLI command
- curated file import
- deterministic promotion command that starts in `draft`

They are used in recall only when `status = active`.

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
- at least one evidence reference, unless `source = human`
- confidence and priority set explicitly or by default policy

## Startup Memory Primer

At the start of a non-worker session, ogent may inject a bounded memory primer
containing only active operating memory.

This fixes the failure mode where stable preferences are useful but the agent has
no reason to call `recall`.

Primer eligibility:

- `lessons.status = active`
- `lesson_type IN ('preference', 'repo_rule', 'process_rule')`
- scope matches the current workspace or is global
- priority is high enough for the size budget

Primer constraints:

- maximum 12 lessons
- maximum 2,000 characters after rendering
- never includes raw episodes or atoms
- each item includes `lesson_id`, rule, applies/avoid condition, confidence
- fail open if DB is absent or broken

Primer rendering:

```text
<memory_primer>
Memory is advisory. Verify current repo state before acting on it.
- lsn_...: Use colgrep as primary code search. Applies when searching code.
- lsn_...: Do not add comments unless convention requires them.
</memory_primer>
```

The primer belongs in the initial user-side context, not as a hidden override to
the system prompt.

## Runtime Recall Tool

`recall` is added to coder tools only. It is excluded from worker tools.

```json
{
  "name": "recall",
  "description": "Search prior ogent memory for relevant episodes, atoms, scenarios, and operating lessons. Memory is advisory; verify current repo state before acting.",
  "parameters": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "Task, problem, file, error, or decision to search memory for."
      },
      "domain": {
        "type": "string",
        "description": "Optional filter such as coding, docs, tui, workflow, testing."
      },
      "limit": {
        "type": "integer",
        "description": "Maximum returned memory groups. Default 5, max 10."
      }
    },
    "required": ["query"],
    "additionalProperties": false
  }
}
```

Behavior:

1. Validate and trim query.
2. If memory DB is missing, return `No memory database found.`
3. Search active lessons, active scenarios, atoms from active episodes, and
   active episodes.
4. Rank with deterministic local scoring.
5. Render a compact `MemoryPack`.
6. Insert `recall_events` and `recall_results`.
7. Append a JSONL line to `.ogent/memory/traces.jsonl`.

## Ranking

Ranking is deterministic and local.

Candidate sources:

- active lessons
- active scenarios
- atoms
- episodes via their goal, summary, and artifact paths

Score components:

- FTS/BM25 score over title/body
- exact file path match boost
- exact tool/error token match boost
- active lesson/scenario boost
- successful or partial outcome boost
- failure outcome boost only when query includes failure, bug, error, test, or
  debug terms
- recent helpful feedback boost
- unhelpful feedback penalty
- low-confidence outcome penalty
- worker episode penalty unless query explicitly asks for worker/review/test
- superseded episode exclusion

Lessons and scenarios should outrank raw atoms when they match well, because they
are curated. Raw atoms should outrank broad lessons when the query contains exact
file paths, error text, or command output.

## MemoryPack Output

The tool returns text, not raw JSON, because the agent consumes it directly.

Required structure:

```text
Memory recall for: <query>
Memory is advisory. Verify current repo state before acting.

Lessons:
- <id> [confidence <n>] <title>
  Rule: ...
  Applies: ...
  Avoid when: ...
  Evidence: <episode count> episodes

Scenarios:
- <id> <title>
  Applies: ...
  Pattern: ...
  Failure modes: ...
  Evidence: <episode ids>

Episodes and atoms:
- <episode id> <outcome> <goal>
  Summary: ...
  Artifacts: ...
  Relevant atoms:
  - <atom id> <kind>: ...
    Evidence: <raw path>, messages <indexes>

Trace:
- recall_event_id: <id>
```

Hard limits:

- max 10 memory groups
- max 3 atoms per episode
- max 6,000 characters total
- truncate individual atom content at 700 characters
- always preserve IDs and evidence refs when truncating

## Feedback

Feedback updates `recall_results.helpful`.

```bash
ogent memory feedback --event <event-id> --item <item-id> --helpful true
ogent memory feedback --event <event-id> --item <item-id> --helpful false
```

Feedback affects ranking but never deletes evidence.

Bad lessons or scenarios are retired explicitly:

```bash
ogent memory lesson retire <lesson-id>
ogent memory scenario retire <scenario-id>
```

## Inspectability CLI

Memory must be inspectable without running the agent.

```bash
ogent memory search "add CLI subcommand" --limit 5
ogent memory show episode <episode-id>
ogent memory show atom <atom-id>
ogent memory show scenario <scenario-id>
ogent memory show lesson <lesson-id>
ogent memory stats
```

`show atom` must include the evidence reference and immutable raw snapshot path.

## Pruning

```bash
ogent memory prune --older-than-days 90
```

Pruning rules:

- never prune pinned episodes
- never prune episodes referenced by active scenarios or active lessons
- never prune episodes with helpful recall feedback
- never prune active episodes unless superseded by a newer episode from the same
  session and unreferenced by active scenarios or lessons
- delete atoms through foreign-key cascade
- delete FTS rows in the same transaction
- delete immutable raw snapshot files only after the DB transaction succeeds
- print what would be deleted before deleting unless `--yes` is passed

## Integration Points

| File | Required change |
|---|---|
| `Cargo.toml` | Add `rusqlite`, `sha2`, `regex` |
| `src/main.rs` | Add `memory` subcommands and primer injection during non-worker session setup |
| `src/memory/mod.rs` | Public memory API |
| `src/memory/db.rs` | SQLite connection, schema, migrations, transactions |
| `src/memory/redact.rs` | Redaction helpers |
| `src/memory/ingest.rs` | Session scanning and atom extraction |
| `src/memory/recall.rs` | Search, ranking, recall event logging |
| `src/memory/render.rs` | Primer and MemoryPack rendering |
| `src/memory/feedback.rs` | Helpful/unhelpful feedback, retire/activate |
| `src/memory/snapshot.rs` | Immutable raw episode snapshot creation and verification |
| `src/tools.rs` | Add coder-only `recall` tool and mark it read-only |
| `src/prompts.rs` | Append startup memory primer to initial user context |
| `src/session.rs` | Add helpers for listing session dirs and raw paths |

`recall` is read-only from the agent perspective even though it writes a recall
trace. It can be included in read-only batching because trace insertion does not
change repository files or agent-visible state. The SQLite connection must use
WAL and a busy timeout so concurrent recall calls do not fail under normal
read-only batching.

## Hand-Computed Scenario Checks

These checks were used to shape the final design. They are part of the spec
because they explain constraints that are easy to miss.

### Scenario 1: Similar Feature Implementation

State:

- Prior session added a CLI subcommand.
- New user asks to add another subcommand.
- Memory contains an active scenario and atoms from the prior session.

Trace:

1. Agent receives request.
2. Startup primer may include stable process rules only.
3. Agent calls `recall("add ogent CLI subcommand")`.
4. Recall returns the scenario, prior episode, edited files, and evidence refs.
5. Agent still reads current `src/main.rs` before editing.

Flaw found:

- A flat chunk store can return old edits without explaining when they apply.

Design fix:

- Add scenarios with `applies_when` and `do_not_use_when`.
- Require recall output to say memory is advisory.

### Scenario 2: Prior Failure and Fix

State:

- Prior task failed `cargo test` after a TUI change.
- Later edit fixed the failure and tests passed.
- New user asks to debug a similar TUI regression.

Trace:

1. Query includes `TUI`, `test`, and error terms.
2. Ranking boosts failure atoms because the query is a debug query.
3. Recall returns failure -> fix -> verification atoms together.

Flaw found:

- Failure atoms alone can mislead the agent into repeating a broken path.

Design fix:

- Recall groups relevant atoms by episode.
- A fix is emitted only when failure, corrective action, and verification are
  all present.

### Scenario 3: Stable User Preference

State:

- User has repeatedly asked for `colgrep` as primary search.
- New task is ordinary implementation.
- Agent may not know to call recall.

Trace:

1. Session starts.
2. Primer injects only active operating memory under a strict size budget.
3. Agent sees the search preference before choosing tools.

Flaw found:

- Explicit-only recall fails for preferences, because the agent has no trigger
  to search for them.

Design fix:

- Add bounded startup memory primer for L3 operating memory only.
- Keep episodes and atoms out of automatic injection.

### Scenario 4: Continuing Old Work

State:

- User says "continue the memory design work" without a session id.
- There are old memory-related episodes.

Trace:

1. Agent calls recall with the task phrase.
2. Recall returns matching episodes and immutable raw snapshot paths.
3. Agent can decide whether to inspect the raw snapshot or ask the user to resume
   a specific session.

Flaw found:

- Memory can be confused with session resume.

Design fix:

- Memory returns paths and summaries, but does not reconstruct agent state.
- Resume/fork remains the authority for continuing exact context.

### Scenario 5: Incomplete or Aborted Session

State:

- A session contains half-finished edits and no `complete` call.
- Ingest scans sessions.

Trace:

1. Ingest sees no successful `complete` call.
2. Default ingest skips the session.
3. Human can force `--include-incomplete`, which stores outcome `unknown`.

Flaw found:

- Ingesting every session by default pollutes memory with abandoned attempts.

Design fix:

- Skip incomplete sessions by default.
- Track low outcome confidence when forced.

### Scenario 6: Worker Delegation

State:

- Parent task dispatches a reviewer worker.
- Worker finds an issue and reports it.

Trace:

1. Parent transcript contains worker report as a tool result.
2. Ingest creates `worker_report` atoms in the parent episode.
3. Worker sessions are not primary recall targets by default.

Flaw found:

- Letting workers call recall independently creates hidden context and weakens
  parent integration.

Design fix:

- Exclude `recall` from worker tools.
- Parent passes relevant memory to workers explicitly.

### Scenario 7: Secret Exposure

State:

- A tool output contains an API key or `.env` dump.
- Ingest extracts atoms.

Trace:

1. Redaction runs before DB insertion and FTS indexing.
2. Redacted content is stored.
3. The immutable raw snapshot remains unchanged but is not copied into memory
   output.

Flaw found:

- Memory DB can make existing raw evidence exposure easier to search.

Design fix:

- Redact before indexing.
- Avoid indexing raw environment dumps and secret-like paths.

### Scenario 8: Bad Memory

State:

- A recalled lesson is obsolete after architecture changes.
- Agent follows it and user marks it unhelpful.

Trace:

1. Recall event has result rows.
2. Feedback marks the item unhelpful.
3. Later ranking penalizes it.
4. Human retires the lesson if it is broadly wrong.

Flaw found:

- Trace rows without item-level feedback are not enough.

Design fix:

- Split `recall_events` and `recall_results`.
- Feedback is attached to each returned item.

### Scenario 9: Resumed Session Rewrites Transcript

State:

- Session `S1` is ingested after completion.
- Later, the user resumes `S1`; ogent persists a longer `messages.jsonl` at the
  same source path.

Trace:

1. First ingest copies `S1` into `.ogent/memory/raw/ep_old/`.
2. Episode `ep_old.raw_messages_path` points at the immutable snapshot, not the
   mutable session file.
3. Resume rewrites `.ogent/sessions/S1/messages.jsonl`.
4. Second ingest computes a different transcript hash and creates `ep_new`.
5. The old episode is marked `superseded`.
6. Recall searches only active episodes unless the user explicitly inspects
   `ep_old`.

Flaw found:

- Pointing evidence at mutable session files breaks traceability after resume.

Design fix:

- Ingestion creates immutable raw snapshots.
- Episodes have `status` and `superseded_by_episode_id`.

### Scenario 10: Parallel Recall Calls

State:

- The model emits several read-only tool calls, including two `recall` calls.
- `recall` is batched with read-only tools.

Trace:

1. Both recall calls search memory.
2. Both try to insert `recall_events` and `recall_results`.
3. SQLite serializes writers under WAL with `busy_timeout = 5000`.
4. Both calls return normally unless the DB is actually unavailable.

Flaw found:

- Treating recall as read-only at the agent layer hides that it writes traces.

Design fix:

- DB connections must use WAL and busy timeout.
- The read-only classification is about repository and agent-visible state, not
  whether SQLite trace tables are written.

## Test Requirements

Unit tests:

- schema migration creates all tables and FTS table
- missing DB recall fails open
- redaction removes representative secrets
- incomplete sessions are skipped by default
- active sessions are skipped when source files change during ingestion
- completed sessions produce an episode and goal/result atoms
- raw episode snapshots remain valid after the source session file changes
- superseded episodes are excluded from default recall
- failed command followed by pass creates failure and verification atoms
- recall ranking boosts exact file path matches
- unhelpful feedback lowers rank
- primer includes only active operating lessons and respects size budget
- worker tools exclude `recall`
- concurrent recall calls both log trace rows or fail open cleanly

Integration tests:

- ingest a fixture session and search it
- recall logs event rows and JSONL trace
- show atom returns evidence refs
- prune refuses active lesson evidence
- reingest does not duplicate an unchanged session

Manual verification:

```bash
cargo test
cargo run -- memory ingest --dry-run
cargo run -- memory ingest --session <fixture-session>
cargo run -- memory search "add CLI subcommand"
cargo run -- "Use memory to see whether we have done a similar CLI change before"
```

## Final Implementation Contract

Memory is correct when:

1. It can explain where every returned claim came from.
2. It helps the agent avoid repeated work without hiding current-state checks.
3. Stable preferences are available without relying on the agent to remember to
   ask for them.
4. Incomplete, secret, stale, or misleading data has clear containment paths.
5. Humans can inspect, correct, promote, retire, and prune memory without reading
   SQLite internals.
