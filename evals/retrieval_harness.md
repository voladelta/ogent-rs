# Retrieval Harness Regression

You are evaluating ogent's retrieval discipline. Do not edit files.

Answer the questions below using workspace tools. Prefer the retrieval ladder:

1. Use exact search when a literal clue is present.
2. Use `colgrep` only for intent-level discovery.
3. Confirm every answer by reading the source or docs directly.
4. Keep tool output compact: cite path and line numbers or tight source ranges.

Return a short report with:

- `profile_observed`: the model/profile you are running under if available, otherwise `unknown`
- `search_trace`: the compact sequence of search/read tools you used
- `answers`: one evidence-backed answer for each question
- `misses_or_uncertainty`: anything you could not verify
- `rubric_check`: a table with `Q`, `required facts present`, and `pass/fail`
- `self_grade`: `pass` only if every rubric item below is satisfied by direct evidence

Questions:

1. Where are the initial root-agent messages assembled, and which prompt files are included?
2. Where is the invariant enforced that only the root CLI agent persists sessions?
3. Where is Lua tool output capped, and what is the cap?
4. Which workspace path functions must filesystem tools use before I/O, and what escape case do they protect against?
5. Where is shell command working-directory behavior constrained?
6. Where does `git_changes` attach enclosing source symbols, and what limitation does the docs mention about those symbols?
7. Where are `exec` and `eval` registered as the only model-visible tools?
8. What is the intended difference between `exec` and `eval` state?

Required rubric:

1. Q1 must name `src/prompts.rs`, `build_initial_messages`, and the injected prompt constants: `PROMPT_SYSTEM`, `PROMPT_TOOLSET_CORE`, and `PROMPT_COLGREP`. It must also mention that `PROMPT_TOOLSET_GIT`, `PROMPT_TOOLSET_WRITE`, and `PROMPT_TOOLSET_SUBAGENT` are loadable on demand through `load_toolset(name)`.
2. Q2 must name the root persistence call site in `src/main.rs`, must state that subagents in `src/tools/lua.rs` call `run_loop` but do not call `persist`, and must not describe `--temp` as the root-only invariant.
3. Q3 must name `src/tools/lua.rs`, `run_lua_vm_async`, and the exact cap `32768`.
4. Q4 must name both `workspace_path` and `readable_path`, must say filesystem tools call them before I/O, and must explicitly identify **symlink escape outside the workspace** as the escape case. Answers that mention only absolute paths or `..` traversal fail Q4.
5. Q5 must name `src/tools/shell.rs`, must mention the actual process starts with `.current_dir(ctx.workspace.root())`, and must mention the preflight `cd` validation allowing only workspace paths or `/tmp`.
6. Q6 must name `attach_changed_symbols` in `src/tools/git.rs`, must mention mapping changed hunk lines to the smallest enclosing outline entry, and must include the docs limitation that `symbols=true` is a navigation aid, not a semantic diff. Answers that name a nonexistent `attach_outline_symbols` fail Q6.
7. Q7 must name `agent_tools()` in `src/tools/lua.rs`, must say the returned model-visible tools are exactly `exec` and `eval`, and must distinguish these from Lua globals registered inside the VM.
8. Q8 must state that `exec` creates a fresh Lua VM per call and `eval` reuses the session-persistent Lua VM; it must also state that `exec` and `eval` do not share state.

Scoring expectation: a good run should find all eight answers without broad file dumps, should not rely on semantic search alone, and should end with direct file evidence. If any required rubric item is missing, set `self_grade` to `fail` or `partial`, not `pass`.
