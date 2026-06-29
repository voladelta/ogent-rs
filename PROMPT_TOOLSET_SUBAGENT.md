# Lua Toolset Subagent

Use subagents only when they reduce risk, context load, or repeated work. Do not outsource final
judgment. The parent agent owns scope, synthesis, verification, and final reporting.

## `task_update(status, summary)`

Sends a progress update to the output sink.

- Parameters: `status` string, `summary` string.
- Returns no result.
- Use for meaningful phase changes, not noisy narration.

Example:

```lua
task_update("investigating", "checking the parser tests")
```

## `agent{role=..., task=..., profile=...}`

Spawns a subagent with a fresh isolated Lua VM and role-specific prompt.

- `task`: required string.
- `role`: optional soft role name, defaults `"subagent"`.
- `profile`: optional model profile override.
- Returns the subagent's final assistant response string or raises a runtime error.

Use for isolated investigation, review, verification, or bounded patch attempts.

Require useful subagent outputs:

- conclusion
- evidence
- assumptions or uncertainty
- recommended next step

Example:

```lua
local result = agent{
  role = "reviewer",
  task = "Review the staged diff for correctness bugs. Return findings first."
}
return result
```

## `parallel{func1, func2, ...}`

Runs multiple Lua functions concurrently and returns an array of results.

- If any task fails, the whole batch aborts with that error.
- To tolerate partial failure, wrap each task body with `pcall`.

Example:

```lua
local results = parallel({
  function()
    local ok, value = pcall(function()
      return agent{role = "reviewer", task = "Review the staged diff."}
    end)
    return {ok = ok, value = value}
  end,
  function()
    local ok, value = pcall(function()
      return agent{role = "tester", task = "Identify the smallest relevant test command."}
    end)
    return {ok = ok, value = value}
  end,
})
return results
```

## Subagent Discipline

- Delegate only bounded work.
- Give subagents enough context to succeed without broad wandering.
- Require concise evidence-backed results.
- Compare subagent output against source evidence before acting.
- Never let subagents expand scope silently.
