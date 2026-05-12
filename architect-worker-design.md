# Worker Architect: Automated Prompt Generation for Worker Dispatch

## Problem

When the 10x coder delegates to a worker, it currently:

1. Calls `load_worker_template("generic")` → template with `{{PLACEHOLDERS}}`
2. Fills all placeholders manually (paths, commands, facts, etc.)
3. Passes the filled prompt as `system_prompt` to `dispatch_worker` / `start_workers`

Steps 1–3 consume parent context tokens. The parent also carries ~30 lines of template instructions in SYSTEM_PROMPT.md. This is context pressure on the main agent for mechanical work.

## Design

### Two paths for worker prompt generation

```
Parent calls dispatch_worker / start_workers
    with: { task, template, context }
         │
         v
    Has built-in system prompt for this role?
    (e.g. prompts/workers/reviewer.md exists)
         │
    ┌────┴────┐
    YES       NO
    │         │
    Use it    Call architect LLM
    directly  to generate prompt
    │         │
    └────┬────┘
         v
    Spawn worker subprocess
```

**Path A — Built-in role prompts:** For roles with well-crafted built-in system prompts (reviewer, tester, validator, coder), use the prompt directly. No architect call. The parent just provides `task` and `context`.

**Path B — Architect LLM call:** For generic/custom roles, or when no built-in exists, the runtime calls the architect LLM to generate `system_prompt` and `task_prompt` from the template + context.

### Tool arguments

```rust
struct DispatchWorkerArgs {
    task: String,       // what the worker should do
    template: String,   // "generic", "tester", "reviewer", "validator"
    context: String,    // markdown string — parent structures into sections
}
```

`context` is a **plain markdown string**. The parent agent structures it naturally:

```markdown
## Project
- Working directory: .
- Tech stack: Rust, Cargo

## Files
- src/client.rs
- src/workers.rs

## Commands
- cargo test
- cargo check

## Known Facts
- client.rs owns HTTP streaming with SSE
- workers.rs spawns child processes

## Constraints
- Do not modify src/main.rs
```

No struct, no schema. The coder writes markdown sections, the architect reads them.

### Architect output format

The architect returns XML-tagged content (easy to regex, no brace-escaping issues):

```
<system_prompt>
Act as a specialist worker...
</system_prompt>

<task_prompt>
Review src/client.rs for...
</task_prompt>
```

Parsing:

```rust
fn parse_architect_output(text: &str) -> Result<(String, String)> {
    let sys = extract_tag(text, "system_prompt")?;
    let task = extract_tag(text, "task_prompt")?;
    Ok((sys, task))
}

fn extract_tag(text: &str, tag: &str) -> Result<String> {
    let re = regex::Regex::new(&format!(r"<{tag}>\s*([\s\S]*?)\s*</{tag}>"))?;
    let cap = re.captures(text).context(format!("missing <{tag}> in architect output"))?;
    Ok(cap[1].to_string())
}
```

### Architect client

Create a **separate `Client`** for architect calls. This:
- Can use the cheapest/fastest available profile
- Doesn't interfere with the parent's client or streaming state
- Uses `Client::chat_json` (non-streaming)

```rust
// In workers.rs or a new architect.rs
fn create_architect_client() -> Result<Client> {
    let profile = profiles::get_profile("ds-flash")
        .context("architect profile not found")?;
    providers::new_client(profile)
}
```

### Non-streaming `Client::chat_json`

New method on `Client` — sends `stream: false`, parses a single JSON response body:

```rust
impl Client {
    pub async fn chat_json(
        &self,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<ChatResponse, ClientError> {
        let mut req_body = (self.build_req)(messages, tools);
        req_body["stream"] = serde_json::Value::Bool(false);
        let resp = self.http
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .json(&req_body)
            .send()
            .await
            .map_err(ClientError::Http)?;
        let status = resp.status();
        let body = resp.text().await.map_err(ClientError::Http)?;
        if !status.is_success() {
            if status.as_u16() == 429 {
                return Err(ClientError::RateLimited { body });
            }
            return Err(ClientError::ApiError {
                status: status.as_u16(),
                body,
            });
        }
        // Parse OpenAI-compatible JSON response
        let v: serde_json::Value = serde_json::from_str(&body)
            .context("parse architect response")?;
        // Extract content from choices[0].message.content
        parse_json_chat_response(v)
    }
}
```

### Error handling

**Architect failure = dispatch failure.** If the architect LLM call fails (rate limit, parse error, network), the `dispatch_worker` / `start_workers` call returns an error to the parent agent. The parent sees the error and can retry or adjust.

No silent fallback to unfilled templates — that would produce broken workers.

### Built-in role prompts

Over time, craft high-quality system prompts for common roles:

```
prompts/
  workers/
    reviewer.md      # ready-to-use, no architect needed
    tester.md         # ready-to-use
    validator.md      # ready-to-use
    coder.md          # ready-to-use (implementation worker)
  templates/
    generic.md        # template for architect to fill
    tester.md         # template for architect (kept for custom variants)
    reviewer.md       # template for architect
    validator.md      # template for architect
  ARCHITECT_PROMPT.md # architect system prompt
  SYSTEM_PROMPT.md    # 10x coder system prompt
```

Decision logic:

```rust
async fn resolve_worker_prompts(
    client: &Client,
    template: &str,
    task: &str,
    context: &str,
) -> Result<(String, String)> {
    // Path A: built-in role prompt exists → use directly
    if let Some(builtin) = get_builtin_worker_prompt(template) {
        let system_prompt = format!("{builtin}\n\n## Context\n\n{context}");
        return Ok((system_prompt, task.to_string()));
    }
    // Path B: architect generates from template + context
    architect_worker_prompt(client, template, task, context).await
}
```

### SYSTEM_PROMPT.md changes

Remove lines 254–283 (Worker Prompt Templates section). Replace with:

```markdown
### Worker Prompt Templates

When delegating, provide `template` (generic/tester/reviewer/validator), `task`,
and `context` (markdown with project info, files, commands, constraints, known facts).
ogent generates the worker's system prompt automatically.
```

Remove `load_worker_template` tool from tool definitions.

### Impact

| Before | After |
|--------|-------|
| ~30 lines of template instructions in system prompt | ~3 lines |
| Parent loads template (~60 lines) into context | Template stays in runtime |
| Parent fills placeholders (reasoning tokens) | Architect LLM / built-in prompt |
| `load_worker_template` tool schema in every request | Removed |
| Parent crafts system_prompt string | Parent writes markdown context |

## Implementation Plan

1. Add `Client::chat_json` — non-streaming LLM call in `client.rs`
2. Create `prompts/ARCHITECT_PROMPT.md` — the architect system prompt
3. Add `ARCHITECT_PROMPT` const in `prompts.rs`; remove `load_worker_template` and template consts
4. Add `resolve_worker_prompts()` and `architect_worker_prompt()` in `workers.rs`
5. Create architect client factory (cheapest profile, `chat_json`)
6. Modify `dispatch_worker` and `start_workers` tool args and schemas in `tools.rs`
7. Add built-in role prompts in `prompts/workers/` (start with one, e.g. reviewer)
8. Simplify SYSTEM_PROMPT.md — replace template instructions with brief delegation note
9. Update tests
