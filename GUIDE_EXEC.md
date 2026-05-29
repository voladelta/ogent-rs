## secure_exec Cheatsheet

> **CRITICAL: Tool Output Limit (8k)**
> The returned output of this tool is strictly capped at **8,192 characters**.
> If your code prints more than that (e.g., reading a large file or printing huge logs), the output will be truncated.
> To read large files, you **must use file offsets** to read in chunks (see Filesystem section below).

**Write plain JS — no type annotations, no `interface`, no generics.** Any `type` / `interface` blocks below are illustrative only; the sandbox rejects TypeScript syntax at parse time.

### 5 rules you will break

> **Rule 1 — Always `console.log` results.**
> Every value-returning call (`readHashAnchors`, `colgrep`, `repoTree`, `webSearch`, `fs.readFile`, …) silently discards its return value unless you wrap it. `await colgrep('auth flow')` produces no output. Always: `console.log(await ...)`. **(Note: Overall output is capped at 8k; avoid printing large files.)**

> **Rule 2 — Never swap `colgrep`'s `query` and `pattern`.**
> `query` = natural language fed to the semantic ranker ("function that validates JWT tokens").
> `pattern` = regex/grep term fed to the pre-filter ("async fn").
> Passing a regex as `query` gives semantic gibberish. Passing NL as `pattern` matches nothing.

> **Rule 3 — `throughAnchor` deletes both endpoints.**
> `replace` or `delete` with `throughAnchor` removes the anchor line, the throughAnchor line, and everything between. If you want to keep line 48, point `throughAnchor` at line 47.

> **Rule 4 — Do file operations with `fs`, not shell utilities.**
> `cp`, `mv`, `find`, `bash`, and `sh` are not allowed commands. `exec` / `execSync` usually route through a shell and will fail. Copy, move, list, and remove paths with `node:fs/promises`; use `repoTree` for tree inspection.

> **Rule 5 — Parse structured files before rewriting them.**
> For JSON/YAML/etc., read and parse the existing file, mutate only the requested fields, then write it back with the required formatting. Do not construct a replacement object unless the task explicitly says to discard unknown fields.

### Sandbox basics
- **Virtual root.** Inside the sandbox, `/` maps to the host workspace root. Use workspace-relative paths (`'src/app.ts'`) — do **not** pass host absolute paths (`/Users/...`). `process.cwd()` returns `/` inside the sandbox; do not try to recover the host path from it.
- **Return channel.** The sandbox runs as an isolated V8 module; its execution result is discarded by the runtime. The only data channel back to your next turn is stdout — captured verbatim as the tool result. Think of it like a subprocess: values flow out via `console.log`, not return. `console.error` also works and is prefixed with `[stderr]` in the result.
- **Terminal truncation.** Only the first 5 lines of stdout are shown on the terminal display; you (the agent) receive the **full output** in the tool result. Don't let the truncation notice mislead you — the data is there.
- **Node builtins.** All `node:` builtins are importable (`node:fs/promises`, `node:child_process`, `node:path`, `node:crypto`, etc.) but they are **not globals**. Import `fs` before calling `fs.readFile`. Injected globals (`readHashAnchors`, `colgrep`, `repoTree`, etc.) are pre-declared — do not `import` them.
- **Allowed commands.** The constant `ALLOWED_COMMANDS` (`['node', 'uv', 'git']`) is injected as a global if you need it programmatically. Spawning anything outside that list throws. Shell helpers and common Unix utilities (`bash`, `sh`, `cp`, `mv`, `find`, `ls`) are intentionally unavailable.

### Quick reference
Edit cycle — **read → plan → batch-apply**:
```javascript
console.log(await readHashAnchors('src/app.ts'));
// output:
//   1:4a2f|import fs from 'node:fs/promises'
//   2:b31c|
//   3:9d1a|export function greet(name) {
//            ↑ anchor = "3:9d1a"  (everything after | is for your eyes only)
await applyAnchorEdits('src/app.ts', [
  { anchor: '3:9d1a', action: 'replace', newString: 'export function greet(name = "world") {' }
]); // all-or-nothing
```
Anchors go stale after any write — re-read before a second pass on the same file.

For non-trivial edits, declare `from`, `to`, `unchanged`, `verify` alongside `code` (optional but helps catch mistakes).

### Anchor format
An anchor is exactly `"line:hash"`, e.g. `"12:a430"`. **Do not include the `|content` suffix** that `readHashAnchors` prints — that suffix is for your reading, not for the API.

### EditOperation type
```typescript
type EditOperation =
  | { action: 'replace'; anchor: string; throughAnchor?: string; newString: string }
  | { action: 'insert_before' | 'insert_after'; anchor: string; newString: string }
  | { action: 'delete'; anchor: string; throughAnchor?: string };
```
- `replace` with no `throughAnchor` → replaces just the one anchor line.
- `replace` / `delete` with `throughAnchor` → removes **both endpoints and everything between**. `anchor:'5:..', throughAnchor:'8:..'` deletes lines 5, 6, 7, **and 8**. To keep line 8: point `throughAnchor` at line 7.
- `insert_before` / `insert_after` → insert without removing the anchor line. **Passing `throughAnchor` with an insert throws.**
- **Trailing `\n` in `newString` is stripped.** `'foo'` and `'foo\n'` are identical — `\n` is a line separator, not a terminator.

### Editing example (batch — preferred)
```javascript
console.log(await readHashAnchors('src/app.ts')); // surface anchors to your next turn
// inspect the output, plan all edits, then in the same or next call:
await applyAnchorEdits('src/app.ts', [
  { anchor: '5:a1b2', action: 'replace', newString: 'const x = 1;' },
  { anchor: '20:c3d4', action: 'insert_after', newString: 'console.log(x);' },
  { anchor: '30:d4e5', action: 'insert_before', newString: '// next: validate input' },
  { anchor: '38:f6g7', action: 'delete' },
  // range replace — lines 45 through 48 inclusive. \n separates lines in newString:
  {
    anchor: '45:e5f6',
    throughAnchor: '48:g7h8',
    action: 'replace',
    newString: `try {
  return doWork();
} catch (err) {
  return null;
}`,
  },
]);
```

`applyAnchorEdits` is **per-file**: validates every op before writing — all-or-nothing **for that file only** (see Error recovery for multi-file behaviour). It throws on:
- **stale anchor** — the line's hash no longer matches. The error includes the current hash and line content; use them to fix the retry.
- **overlapping edits** — two ops touch the same line range.
- **out-of-bounds line** or **invalid anchor format**.

### Error recovery

**Stale anchor** — error includes the new hash and current content; use them directly for a one-shot retry without re-reading the whole file:
```
anchor mismatch at line 12: expected a430, current b5f2
12:b5f2|const newName = 'updated';
```
```javascript
// Swap in the new anchor from the error message and retry:
await applyAnchorEdits('src/app.ts', [
  { anchor: '12:b5f2', action: 'replace', newString: 'const newName = "final";' },
]);
```
If multiple anchors are stale (e.g. a previous write shifted lines), re-read the whole file instead:
```javascript
const anchors = await readHashAnchors('src/app.ts');
console.log(anchors); // re-plan from current state, then apply a fresh batch
```

> **Multi-file partial failure** — file A is already written the moment `applyAnchorEdits` returns for it, regardless of what happens to file B afterward. The write is not contingent on subsequent calls succeeding. **Do not retry file B assuming file A is still at its pre-edit state.** Confirm A's actual state first:

```javascript
// File A's write already happened — read it to confirm what landed:
console.log(await fs.readFile('src/a.ts', 'utf8'));
// File B's write never happened — re-read its anchors and retry:
console.log(await readHashAnchors('src/b.ts'));
```

**Overlapping edits** — two ops in the same batch touch the same line range. Fix by merging them into a single op with a wider `throughAnchor`, or splitting them into sequential calls (re-reading anchors between each).

### New and empty files
Both functions throw `ENOENT` if the file doesn't exist, and any op on an empty file fails ("anchor line is outside file"). Always seed with `fs.writeFile` first:
```javascript
await fs.writeFile('src/new.ts', 'export const x = 1;\n');
```

### Editing limitations
- **Cannot change trailing-newline state.** `applyAnchorEdits` preserves whether the file already ends with `\n` but cannot change it: `await fs.writeFile(p, (await fs.readFile(p, 'utf8')).trimEnd() + '\n')`.
- **No atomic multi-file transactions.** Each `applyAnchorEdits` call is independent; a failure on file B does not roll back file A.

### Filesystem (async)
```javascript
import fs from 'node:fs/promises';
import path from 'node:path';

await fs.mkdir('scratch/out', { recursive: true });
await fs.writeFile('scratch/out/example.txt', 'hi\n');
const text = await fs.readFile('src/main.ts', 'utf8'); // Note: capped at 8k; do not console.log large file contents!
const exists = await fs.access('foo').then(() => true).catch(() => false);
const meta = await fs.stat('src/main.ts');
await fs.copyFile('src/a.txt', 'scratch/out/a.txt');
await fs.rename('a.txt', 'b.txt');
await fs.unlink('b.txt');
await fs.rmdir('scratch/out');

async function copyDir(src, dest) {
  await fs.mkdir(dest, { recursive: true });
  for (const entry of await fs.readdir(src, { withFileTypes: true })) {
    const from = path.join(src, entry.name);
    const to = path.join(dest, entry.name);
    if (entry.isDirectory()) await copyDir(from, to);
    else await fs.copyFile(from, to);
  }
}
```

For structured files, preserve unknown fields:
```javascript
const config = JSON.parse(await fs.readFile('config.json', 'utf8'));
config.enabled = true;
await fs.writeFile('config.json', JSON.stringify(config, null, 2) + '\n');
```

To read large files without triggering the 8k output limit truncation, open the file and read it in chunks:
```javascript
const handle = await fs.open('large_file.txt', 'r');
const buffer = Buffer.alloc(8000);
const { bytesRead } = await handle.read(buffer, 0, 8000, offset); // offset in bytes
console.log(buffer.toString('utf8', 0, bytesRead));
await handle.close();
```

### Parallel reads
Use `Promise.all` to fan out independent I/O:
```javascript
const [main, types] = await Promise.all([
  fs.readFile('src/main.ts', 'utf8'),
  fs.readFile('src/types.ts', 'utf8'),
]);
```

### Spawning (allowed: node, uv, git)
```javascript
import { spawnSync } from 'node:child_process';
const r = spawnSync('node', ['--version']);
console.log(r.stdout.toString());
console.log(r.stderr.toString());
console.log('exit code:', r.status);

// Custom cwd must be workspace-relative (or stay omitted for the workspace root).
const log = spawnSync('git', ['log', '-1', '--oneline'], { cwd: 'packages/server' });
console.log(log.stdout.toString());
```
A host-absolute `cwd` is reinterpreted as a sandbox path, not the host path you intended.

Prefer `spawnSync('node', ['script.mjs', 'arg'])` for verification commands. Avoid `exec`, `execSync`, and shell command strings; they typically require `sh` / `bash`, which are not allowed.

### Outbound HTTP
```javascript
const resp = await fetch('https://api.example.com/data');
console.log(await resp.json());
```

### Colgrep
`colgrep(query, options?)` — semantic search; returns `{ score, unit: { name, file, line, code } }[]`.

Options: `pattern` (regex pre-filter), `results` (default 15), `paths` (file or dir), `include` (glob), `codeOnly` (skip docs/yaml/json).

> **`query` = natural language. `pattern` = regex.** Don't swap them. Rule 2 above.
> **Empty query alone throws** — only valid when `pattern` is also set.

```javascript
console.log(await colgrep('function that parses CLI flags'));
console.log(await colgrep('retry logic', { paths: 'src/client.ts', results: 5 }));
console.log(await colgrep('', { pattern: 'async function', codeOnly: true }));
console.log(await colgrep('error handling', { pattern: 'throw', codeOnly: true }));
```

### Web helpers
All return a formatted markdown `string` — always `console.log` the result.
- `webSearch(query, numResults = 10) → string`
- `webRead(urls: string[], mode = 'highlights') → string`
- `webCodeContext(query) → string`

### repoTree
`repoTree(level = 3, path = '.') → string` — gitignore-respecting ASCII tree. Always `console.log` the result.
```javascript
console.log(await repoTree());         // top-level, depth 3
console.log(await repoTree(2, 'src')); // depth 2 inside src/
```

### Skills
Allows querying and loading local and home skill definitions.
- `listSkill() → Promise<{ name: string, description: string }[]>` — lists all discovered skills with their names and descriptions.
- `loadSkill(skillName: string) → Promise<{ name: string, root: string, body: string }>` — retrieves the skill name, root folder, and markdown body (without frontmatter).

Files and directories under `.skills`, `.ogent/skills`, `.agents/skills`, `~/.ogent/skills`, and `~/.agents/skills` are automatically whitelisted for read/write access and child process execution (`cwd`) inside the sandbox.

```javascript
console.log(await listSkill());
const skill = await loadSkill('colgrep');
console.log(skill.body);
```

