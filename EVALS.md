# Evals

This repo currently includes a small regression harness for checking agent
retrieval discipline across model profiles.

## Retrieval Harness

The harness prompt lives at:

```bash
evals/retrieval_harness.md
```

It asks an agent to answer eight architecture questions using workspace tools.
The goal is not benchmark breadth; it is to catch regressions in how the agent
searches:

- exact evidence before semantic search when literal clues exist
- `colgrep` as candidate discovery, not proof
- bounded source reads instead of broad file dumps
- direct evidence for claims
- explicit self-grading against a required rubric

The rubric intentionally checks failure-prone details such as symlink escape
protection, root-only session persistence, and the distinction between
model-visible tools and Lua globals.

## Running

Run the default profiles (`ds-flash`, `ds-pro`, `kimi`):

```bash
scripts/run_retrieval_harness.sh
```

Run a custom profile set:

```bash
scripts/run_retrieval_harness.sh ds-flash ds-pro kimi glm mimo-pro
```

The runner writes stdout, stderr, and a summary TSV under:

```bash
eval-results/retrieval-harness/<timestamp>/
```

`eval-results/` is git-ignored because outputs contain model transcripts and
provider-specific traces.

## Reading Results

Start with:

```bash
cat eval-results/retrieval-harness/<timestamp>/summary.tsv
```

Each row reports:

- `profile`
- process `exit_code`
- parsed `self_grade`
- path to saved stdout
- path to saved stderr

A clean run should have exit code `0` and `self_grade` of `pass` for each
profile. Treat `self_grade` as a first-pass signal, not a substitute for
reviewing the stdout when behavior changes.

## When To Run

Run this harness after changes to:

- `PROMPT_SYSTEM.md`
- `PROMPT_TOOLSET_CORE.md`
- `PROMPT_TOOLSET_GIT.md`
- `PROMPT_TOOLSET_WRITE.md`
- `PROMPT_TOOLSET_SUBAGENT.md`
- `PROMPT_COLGREP.md`
- tool result formatting
- search, outline, git, shell, or Lua tool behavior
- profile/provider defaults that may affect tool use

It is also useful before comparing model profiles, since the prompt and rubric
hold the task constant while profile behavior varies.
