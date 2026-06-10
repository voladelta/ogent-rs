For semantic or intent-based code search, use the `colgrep` CLI tool via `shell`. For exact string or regex search inside Lua, prefer `search_text`; use `colgrep -e` when hybrid semantic + exact filtering is useful.

Use `colgrep` to find candidates, not to prove claims. After it points at likely files or symbols, confirm with exact tools such as `search_text`, `outline`, `read_lines`, `git_changes`, tests, or build output.

Default search policy:

1. If you have an exact error, symbol, command, field, config key, path, or phrase, start with exact search (`search_text` or bounded `rg`).
2. If you only have behavioral intent, start with `colgrep -k 20` or `colgrep -l`, then inspect the best candidates directly.
3. If the first query is noisy, add file filters, path filters, or a hybrid `-e` prefilter before increasing result volume.
4. If output is bulky, keep it in `eval` state and print compact summaries rather than dumping full search results.

## colgrep

> [!IMPORTANT]
> `colgrep` is a CLI tool, not a built-in Lua function. You must execute it using the `shell` function inside the `exec` or `eval` tool.
>
> - **Correct**: `shell{command = "colgrep 'auth flow' -k 10"}`
> - **Incorrect**: `colgrep("auth flow")`

### Quick Reference

```bash
# Basic semantic search
colgrep "<natural language query>" --results 10   # Basic search
colgrep "<query>" -k 25                           # Exploration (more results)
colgrep "<query>" ./src/parser                    # Search in specific folder
colgrep "<query>" ./src/main.rs                   # Search in specific file
colgrep "<query>" ./src/main.rs ./src/lib.rs      # Search in multiple files
colgrep "<query>" ./crate-a ./crate-b             # Search multiple directories

# File filtering
colgrep --include="*.rs" "<query>"                # Include only .rs files
colgrep --include="src/**/*.rs" "<query>"         # Recursive glob pattern
colgrep --include="*.{rs,md}" "<query>"           # Multiple file types (brace expansion)
colgrep --exclude="*.test.ts" "<query>"           # Exclude test files
colgrep --exclude-dir=vendor "<query>"            # Exclude vendor directory

# Pattern-only search (no semantic query needed)
colgrep -e "<pattern>"                            # Search by pattern only
colgrep -e "async fn" --include="*.rs"            # Pattern search with file filter

# Hybrid search (text + semantic)
colgrep -e "<text>" "<semantic query>"            # Hybrid: text + semantic
colgrep -e "<regex>" -E "<semantic query>"        # Hybrid with extended regex (ERE)
colgrep -e "<literal>" -F "<semantic query>"      # Hybrid with fixed string (no regex)
colgrep -e "<word>" -w "<semantic query>"         # Hybrid with whole word match

# Output options
colgrep -l "<query>"                              # List files only
colgrep -c "<query>"                              # Show full function content (50 lines max)
colgrep -n 10 "<query>"                           # Show 10 context lines (default: 6)
```

### Grep-Compatible Flags

| Flag            | Description                                 | Example                                      |
| --------------- | ------------------------------------------- | -------------------------------------------- |
| `-e <PATTERN>`  | Text pattern pre-filter                     | `colgrep -e "async" "concurrency"`           |
| `-E`            | Extended regex (ERE) for `-e`               | `colgrep -e "async\|await" -E "concurrency"` |
| `-F`            | Fixed string (no regex) for `-e`            | `colgrep -e "foo[bar]" -F "query"`           |
| `-w`            | Whole word match for `-e`                   | `colgrep -e "test" -w "testing"`             |
| `-k, --results` | Number of results to return                 | `colgrep --results 20 "query"`               |
| `-l`            | List files only                             | `colgrep -l "authentication"`                |
| `-r`            | Recursive (default, for compatibility)      | `colgrep -r "query"`                         |
| `--include`     | Include files matching pattern (repeatable) | `colgrep --include="*.py" "query"`           |
| `--exclude`     | Exclude files matching pattern              | `colgrep --exclude="*.min.js" "query"`       |
| `--exclude-dir` | Exclude directories                         | `colgrep --exclude-dir=node_modules "query"` |

**Notes:**

- `-F` takes precedence over `-E` (like grep)
- Default exclusions always apply: `.git`, `node_modules`, `target`, `.venv`, `__pycache__`
- When running from a subdirectory, results are restricted to that subdirectory. To search the full project, specify `.` or `..` as the path
- Multiple `--include` patterns use OR logic (matches if file matches any pattern)
- Brace expansion is supported: `*.{rs,md,py}` expands to match all three types

### Key Rules

1. **Confirm candidates with source evidence** before editing or answering.
2. **Increase `--results`** (or `-k`) when exploring (20-30 results).
3. **Use `-e`** for hybrid text+semantic filtering.
4. **Use `-E`** with `-e` for extended regex (alternation `|`, quantifiers `+?`, grouping `()`).
5. **Use `-F`** with `-e` when pattern contains regex special characters you want literal.
6. **Use `-w`** with `-e` to avoid partial matches (e.g., "test" won't match "testing").
7. **Use `--exclude`/`--exclude-dir`** to filter out noise (tests, vendors, generated code).
8. **Use brace expansion** for multiple file types (e.g., `--include="*.{rs,md,py}"`).
