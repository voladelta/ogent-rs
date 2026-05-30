use anyhow::{Context, Result};
use mlua::{HookTriggers, Lua, LuaSerdeExt, StdLib, Value};
use parking_lot::Mutex;

use serde_json::json;
use std::sync::Arc;

use crate::tools::ToolContext;

use crate::types::{Tool, ToolFunction};

const MAX_AGENT_DEPTH: u32 = 3;

pub fn agent_tools() -> Vec<Tool> {
  vec![
    Tool {
      kind: "function".to_string(),
      function: ToolFunction {
        name: "exec".to_string(),
        description: "Execute a one-off stateless Lua 5.5 script. Captures stdout prints and the final return value.".to_string(),
        parameters: json!({
          "type": "object",
          "properties": {
            "code": {
              "type": "string",
              "description": "Lua 5.5 script to execute"
            },
            "reason": {
              "type": "string",
              "description": "Optional explanation of why the script is being executed (for tracking/session logging)"
            }
          },
          "required": ["code"],
          "additionalProperties": false
        }),
      },
    },
    Tool {
      kind: "function".to_string(),
      function: ToolFunction {
        name: "eval".to_string(),
        description: "Execute a stateful Lua 5.5 script within the persistent session. Captures stdout prints and the final return value. Retains global variables/functions between calls.".to_string(),
        parameters: json!({
          "type": "object",
          "properties": {
            "code": {
              "type": "string",
              "description": "Lua 5.5 script to execute"
            },
            "reason": {
              "type": "string",
              "description": "Optional explanation of why the script is being executed (for tracking/session logging)"
            }
          },
          "required": ["code"],
          "additionalProperties": false
        }),
      },
    },
  ]
}

macro_rules! register_sync {
  ($lua:expr, $globals:expr, $ctx:expr, $name:expr, $func:expr) => {{
    let ctx_clone = $ctx.clone();
    let lua_fn = $lua.create_function(move |lua, args: mlua::Value| {
      let json_args = match args {
        mlua::Value::Nil => "{}".to_string(),
        _ => {
          let json_val: serde_json::Value = lua.from_value(args)?;
          json_val.to_string()
        }
      };
      let result = $func(ctx_clone.clone(), &json_args);
      match result {
        Ok(output) => Ok((
          mlua::Value::String(lua.create_string(&output)?),
          mlua::Value::Nil,
        )),
        Err(e) => Ok((
          mlua::Value::Nil,
          mlua::Value::String(lua.create_string(&e.to_string())?),
        )),
      }
    })?;
    $globals.set($name, lua_fn)?;
  }};
}

macro_rules! register_async {
  ($lua:expr, $globals:expr, $ctx:expr, $name:expr, $func:expr) => {{
    let ctx_clone = $ctx.clone();
    let lua_fn = $lua.create_async_function(move |lua, args: mlua::Value| {
      let ctx = ctx_clone.clone();
      async move {
        let json_args = match args {
          mlua::Value::Nil => "{}".to_string(),
          _ => {
            let json_val: serde_json::Value = lua.from_value(args)?;
            json_val.to_string()
          }
        };
        let result = $func(ctx, &json_args).await;
        match result {
          Ok(output) => Ok((
            mlua::Value::String(lua.create_string(&output)?),
            mlua::Value::Nil,
          )),
          Err(e) => Ok((
            mlua::Value::Nil,
            mlua::Value::String(lua.create_string(&e.to_string())?),
          )),
        }
      }
    })?;
    $globals.set($name, lua_fn)?;
  }};
}

async fn spawn_subagent(
  ctx: &ToolContext,
  role: &str,
  task: &str,
  profile_override: Option<&str>,
) -> Result<String, mlua::Error> {
  if ctx.agent_depth >= MAX_AGENT_DEPTH {
    return Err(mlua::Error::RuntimeError(format!(
      "max subagent depth ({MAX_AGENT_DEPTH}) exceeded"
    )));
  }

  let client = if let Some(p_name) = profile_override {
    let config = crate::config::load_config(ctx.workspace.root())
      .map_err(|e| mlua::Error::RuntimeError(format!("failed to load config: {e}")))?;
    let profile = config
      .get_profile(p_name)
      .ok_or_else(|| mlua::Error::RuntimeError(format!("unknown profile: {p_name}")))?;
    let provider = config.provider_for(profile).ok_or_else(|| {
      mlua::Error::RuntimeError(format!("missing provider config for profile: {p_name}"))
    })?;
    crate::providers::new_client(profile, provider)
      .map_err(|e| mlua::Error::RuntimeError(format!("failed to construct client: {e}")))?
  } else {
    ctx.client.clone()
  };

  let messages = crate::prompts::build_subagent_messages(&ctx.workspace, role, task.to_string());

  let mut subagent = crate::agent::Agent::new(
    ctx.workspace.clone(),
    client,
    messages,
    crate::tools::agent_tools(),
    crate::session::generate_session_id(),
    ctx.skill_store.clone(),
    role.to_string(),
    ctx.verbose,
    ctx.agent_depth + 1,
  );
  subagent.set_output_sink(ctx.output_sink.clone());

  subagent
    .run_loop()
    .await
    .map_err(|e| mlua::Error::RuntimeError(format!("subagent run loop failed: {e}")))?;
  let last_msg = subagent
    .messages
    .iter()
    .rfind(|m| m.role == crate::types::Role::Assistant);
  Ok(last_msg.map(|m| m.content.clone()).unwrap_or_default())
}

fn register_tools_in_lua(lua: &Lua, ctx: ToolContext) -> Result<()> {
  let globals = lua.globals();

  // task_update(status, summary)
  let ctx_clone = ctx.clone();
  let task_update_fn = lua.create_function(move |_lua, args: mlua::MultiValue| {
    let status: String = match args.front() {
      Some(Value::String(s)) => s.to_str()?.to_string(),
      _ => {
        return Err(mlua::Error::RuntimeError(
          "first argument status must be a string".to_string(),
        ));
      }
    };
    let summary: String = match args.get(1) {
      Some(Value::String(s)) => s.to_str()?.to_string(),
      _ => {
        return Err(mlua::Error::RuntimeError(
          "second argument summary must be a string".to_string(),
        ));
      }
    };
    if let Some(sink) = &ctx_clone.output_sink {
      sink.task_update(&ctx_clone.actor_id, &status, &summary);
    }
    Ok(())
  })?;
  globals.set("task_update", task_update_fn)?;

  // agent({role, task, profile?})
  let ctx_clone = ctx.clone();
  let agent_fn = lua.create_async_function(move |lua, args: Value| {
    let ctx = ctx_clone.clone();
    async move {
      let args_val: serde_json::Value = match lua.from_value(args) {
        Ok(v) => v,
        Err(e) => {
          return Err(mlua::Error::RuntimeError(format!(
            "invalid arguments to agent: {e}"
          )));
        }
      };
      let role = args_val
        .get("role")
        .and_then(|r| r.as_str())
        .unwrap_or("subagent")
        .to_string();
      let task = match args_val.get("task").and_then(|t| t.as_str()) {
        Some(t) => t.to_string(),
        None => {
          return Err(mlua::Error::RuntimeError(
            "missing 'task' parameter in agent call".to_string(),
          ));
        }
      };
      let profile_override = args_val
        .get("profile")
        .and_then(|p| p.as_str())
        .map(|s| s.to_string());
      spawn_subagent(&ctx, &role, &task, profile_override.as_deref()).await
    }
  })?;
  globals.set("agent", agent_fn)?;

  // parallel({func1, func2, ...})
  let parallel_fn = lua.create_async_function(move |lua, args: Value| async move {
    let tasks: Vec<mlua::Function> = match args {
      Value::Table(t) => {
        let mut list = Vec::new();
        let mut i = 1;
        while let Ok(v) = t.get::<Value>(i) {
          match v {
            Value::Function(f) => list.push(f),
            Value::Nil => break,
            _ => {
              return Err(mlua::Error::RuntimeError(format!(
                "expected function at index {i} in parallel list"
              )));
            }
          }
          i += 1;
        }
        list
      }
      _ => {
        return Err(mlua::Error::RuntimeError(
          "expected an array of functions to parallel".to_string(),
        ));
      }
    };

    let mut futures = Vec::new();
    for task in tasks {
      let fut = task.call_async::<Value>(());
      futures.push(fut);
    }
    let results = futures_util::future::join_all(futures).await;
    let out_table = lua.create_table()?;
    for (i, res) in results.into_iter().enumerate() {
      match res {
        Ok(val) => out_table.set(i + 1, val)?,
        Err(e) => {
          return Err(mlua::Error::RuntimeError(format!(
            "task {} in parallel failed: {e}",
            i + 1
          )));
        }
      }
    }
    Ok(Value::Table(out_table))
  })?;
  globals.set("parallel", parallel_fn)?;

  // Helper to decode JSON strings into Lua tables.
  // Intentionally NOT exposed as a global: structured tool outputs are
  // already decoded by the positional wrappers below. Lua scripts should
  // never need raw JSON parsing.
  let json_decode = lua.create_function(|lua, s: String| {
    let v: serde_json::Value = serde_json::from_str(&s)
      .map_err(|e| mlua::Error::RuntimeError(format!("json_decode: {e}")))?;
    lua.to_value(&v)
  })?;

  // Register fs tools
  register_sync!(lua, globals, ctx, "read_file", crate::tools::fs::read_file);
  register_sync!(
    lua,
    globals,
    ctx,
    "write_file",
    crate::tools::fs::write_file
  );
  register_sync!(
    lua,
    globals,
    ctx,
    "append_file",
    crate::tools::fs::append_file
  );
  register_sync!(lua, globals, ctx, "file_info", crate::tools::fs::file_info);
  register_sync!(
    lua,
    globals,
    ctx,
    "read_hash_anchors",
    crate::tools::fs::read_hash_anchors
  );
  register_sync!(
    lua,
    globals,
    ctx,
    "apply_anchor_edits",
    crate::tools::fs::apply_anchor_edits
  );

  // Register repo tools
  register_sync!(lua, globals, ctx, "repo_map", crate::tools::repo::repo_map);
  register_sync!(lua, globals, ctx, "glob", crate::tools::repo::glob);

  // Register git tools
  register_async!(
    lua,
    globals,
    ctx,
    "git_status",
    crate::tools::git::git_status
  );
  register_async!(lua, globals, ctx, "git_diff", crate::tools::git::git_diff);
  register_async!(
    lua,
    globals,
    ctx,
    "git_changes",
    crate::tools::git::git_changes
  );
  register_async!(lua, globals, ctx, "git_show", crate::tools::git::git_show);
  register_async!(lua, globals, ctx, "git_log", crate::tools::git::git_log);

  // Register shell tool
  register_async!(lua, globals, ctx, "shell", crate::tools::shell::shell);

  // Register skills tools
  register_sync!(
    lua,
    globals,
    ctx,
    "load_skill",
    crate::tools::skills::load_skill
  );
  register_sync!(
    lua,
    globals,
    ctx,
    "list_skills",
    crate::tools::skills::list_skills
  );
  register_sync!(
    lua,
    globals,
    ctx,
    "load_skill_asset",
    crate::tools::skills::load_skill_asset
  );

  // Register web tools
  register_async!(
    lua,
    globals,
    ctx,
    "web_search",
    crate::tools::web::web_search
  );
  register_async!(lua, globals, ctx, "web_read", crate::tools::web::web_read);
  register_async!(
    lua,
    globals,
    ctx,
    "web_code_context",
    crate::tools::web::web_code_context
  );

  // Inject thin Lua wrappers that convert positional arguments to tables
  // and delegate to the functions registered above.
  // json_decode is passed as a local so the wrappers can decode structured
  // outputs without exposing raw JSON parsing to Lua scripts.
  let positional_wrappers = r#"
local json_decode = ...
local _t = {}
_t.read_file = read_file
_t.append_file = append_file
_t.file_info = file_info
_t.read_hash_anchors = read_hash_anchors
_t.apply_anchor_edits = apply_anchor_edits
_t.load_skill = load_skill
_t.list_skills = list_skills
_t.load_skill_asset = load_skill_asset
_t.glob = glob
_t.git_status = git_status
_t.git_diff = git_diff
_t.git_changes = git_changes
_t.git_show = git_show
_t.git_log = git_log

function read_file(path, offset, limit)
  return _t.read_file({path=path, offset=offset, limit=limit})
end
function append_file(path, content)
  return _t.append_file({path=path, content=content})
end
function file_info(path)
  local ok, err = _t.file_info({path=path})
  if ok then ok = json_decode(ok) end
  return ok, err
end
function read_hash_anchors(path, offset, limit)
  local ok, err = _t.read_hash_anchors({path=path, offset=offset, limit=limit})
  if ok then _G._last_hash_anchor_path = path end
  return ok, err
end
function apply_anchor_edits(...)
  local n = select('#', ...)
  local path, ops
  if n == 1 then
    ops = ...
    path = _G._last_hash_anchor_path
  else
    path, ops = ...
  end
  local ok, err = _t.apply_anchor_edits({path=path, ops=ops})
  if ok then _G._last_hash_anchor_path = path end
  return ok, err
end
function load_skill(name)
  return _t.load_skill({name=name})
end
function list_skills()
  return _t.list_skills()
end
function load_skill_asset(root, path)
  return _t.load_skill_asset({root=root, path=path})
end
function glob(pattern)
  local ok, err = _t.glob({pattern=pattern})
  if ok then ok = json_decode(ok) end
  return ok, err
end
function git_status(opts)
  local ok, err = _t.git_status(opts or {})
  if ok then ok = json_decode(ok) end
  return ok, err
end
function git_diff(opts)
  local ok, err = _t.git_diff(opts or {})
  if ok then ok = json_decode(ok) end
  return ok, err
end
function git_changes(opts)
  local ok, err = _t.git_changes(opts or {})
  if ok then ok = json_decode(ok) end
  return ok, err
end
function git_show(opts_or_path, git_ref)
  local opts
  if type(opts_or_path) == "table" then
    opts = opts_or_path
  else
    opts = {path = opts_or_path, ref = git_ref}
  end
  local ok, err = _t.git_show(opts)
  return ok, err
end
function git_log(opts)
  local ok, err = _t.git_log(opts or {})
  if ok then ok = json_decode(ok) end
  return ok, err
end
"#;
  let wrappers_func = lua.load(positional_wrappers).into_function()?;
  wrappers_func.call::<()>(json_decode)?;

  Ok(())
}

async fn run_lua_vm_async(lua: &Lua, code: &str) -> Result<String> {
  // 1. Capture stdout via print override
  let stdout_buffer = Arc::new(Mutex::new(String::new()));
  let buffer_clone = stdout_buffer.clone();

  let print_fn = lua.create_function(move |_, args: mlua::MultiValue| {
    let mut buffer = buffer_clone.lock();
    let parts: Vec<String> = args
      .iter()
      .map(|v| v.to_string().unwrap_or_else(|_| "nil".to_string()))
      .collect();
    if !buffer.is_empty() {
      buffer.push('\n');
    }
    buffer.push_str(&parts.join("\t"));
    Ok(())
  })?;
  lua.globals().set("print", print_fn)?;

  // 2. Wrap the chunk in a function and run it inside a thread (coroutine)
  let func = lua.load(code).into_function()?;
  let thread = lua.create_thread(func)?;

  // Set instruction hook on the thread to abort infinite loops (abort after 32,000 instructions)
  thread.set_hook(
    HookTriggers::new().every_nth_instruction(32000),
    |_lua, _debug| {
      Err(mlua::Error::RuntimeError(
        "Execution timeout: instruction limit exceeded".to_string(),
      ))
    },
  )?;

  // 3. Execute the thread asynchronously
  let execution_result: Result<Value, _> = thread.into_async(())?.await;

  // 4. Format the final output
  let mut final_response = String::new();
  let stdout = stdout_buffer.lock().clone();
  if !stdout.is_empty() {
    final_response.push_str("--- Stdout Output ---\n");
    final_response.push_str(&stdout);
    final_response.push_str("\n\n");
  }

  match execution_result {
    Ok(Value::Nil) => {
      if final_response.is_empty() {
        final_response.push_str("Success (no output or return value).");
      }
    }
    Ok(val) => {
      final_response.push_str("--- Return Value ---\n");
      let val_str = match lua.from_value::<serde_json::Value>(val.clone()) {
        Ok(json_val) => {
          serde_json::to_string_pretty(&json_val).unwrap_or_else(|_| format!("{:?}", val))
        }
        Err(_) => format!("{:?}", val),
      };
      final_response.push_str(&val_str);
    }
    Err(err) => {
      final_response.push_str("--- Runtime Error ---\n");
      final_response.push_str(&err.to_string());
    }
  }

  if final_response.len() > 16384 {
    const MAX_LEN: usize = 16384;
    const TRUNCATE_MSG: &str = "\n... [truncated] ...\n";
    let half_budget = (MAX_LEN - TRUNCATE_MSG.len()) / 2;
    let head_end = final_response.floor_char_boundary(half_budget);
    let tail_start =
      final_response.floor_char_boundary(final_response.len() - half_budget);
    if tail_start > head_end {
      let mut truncated = String::with_capacity(MAX_LEN);
      truncated.push_str(&final_response[..head_end]);
      truncated.push_str(TRUNCATE_MSG);
      truncated.push_str(&final_response[tail_start..]);
      final_response = truncated;
    } else {
      let end = final_response.floor_char_boundary(MAX_LEN);
      final_response.truncate(end);
      final_response.push_str("\n... [Output truncated to 16k limit] ...");
    }
  }

  Ok(final_response)
}

// exec is stateless: create a fresh VM, run the script, and discard it.
// With mlua's `send` feature, Lua is Send and the `thread.into_async()` future
// is also Send, so we can run it directly in async context without burning a
// blocking thread.
pub async fn exec(ctx: ToolContext, code: &str) -> Result<String> {
  let lua = create_sandboxed_vm()?;
  register_tools_in_lua(&lua, ctx)?;
  run_lua_vm_async(&lua, code).await
}

// eval is stateful: it reuses the same Lua VM across calls via a session lock.
// MutexGuard is !Send, so we cannot hold the guard across an await
// in a Send future. We confine the locked operation to spawn_blocking so the
// guard never crosses an await boundary.
pub async fn eval(ctx: ToolContext, code: &str) -> Result<String> {
  let code = code.to_string();
  let session = ctx.lua_session.clone();
  tokio::task::spawn_blocking(move || {
    let mut guard = session.lock();
    if guard.is_none() {
      let lua = create_sandboxed_vm()?;
      register_tools_in_lua(&lua, ctx)?;
      *guard = Some(lua);
    }
    let lua = guard.as_ref().unwrap();
    let handle = tokio::runtime::Handle::current();
    handle.block_on(run_lua_vm_async(lua, &code))
  })
  .await
  .context("spawn_blocking panicked")?
}

fn create_sandboxed_vm() -> Result<Lua> {
  // Load safe libraries. Coroutine is required for async support in mlua.
  let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE;
  let lua = Lua::new_with(libs, mlua::LuaOptions::default())?;

  // Limit memory usage to 32MB
  lua.set_memory_limit(32 * 1024 * 1024)?;

  Ok(lua)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::workspace::Workspace;
  use std::sync::Arc;

  fn test_context() -> ToolContext {
    let workspace = Workspace::from_current_dir();
    let skill_store = Arc::new(crate::skills::SkillStore::new(workspace.root()));
    let client = crate::client::Client::new(
      "http://localhost",
      "dummy".into(),
      |_, _| Ok(serde_json::Value::Null),
      30,
    )
    .unwrap();
    ToolContext {
      workspace,
      skill_store,
      lua_session: Arc::new(Mutex::new(None)),
      client,
      output_sink: None,
      verbose: false,
      actor_id: "director".to_string(),
      agent_depth: 0,
    }
  }

  #[tokio::test]
  async fn test_exec_stateless() {
    let ctx = test_context();
    let res = exec(ctx.clone(), "print('hello'); return 2 + 2")
      .await
      .unwrap();
    assert!(res.contains("Stdout Output"));
    assert!(res.contains("hello"));
    assert!(res.contains("Return Value"));
    assert!(res.contains("4"));

    // Verify it is stateless (variables don't persist)
    exec(ctx.clone(), "global_var = 42").await.unwrap();
    let res2 = exec(ctx, "return global_var").await.unwrap();
    assert!(res2.contains("Nil") || res2.contains("Success (no output or return value)."));
  }

  #[tokio::test]
  async fn test_eval_stateful() {
    let ctx = test_context();
    let res = eval(ctx.clone(), "persisted_var = 100; return persisted_var")
      .await
      .unwrap();
    assert!(res.contains("100"));

    let res2 = eval(ctx, "return persisted_var + 50").await.unwrap();
    assert!(res2.contains("150"));
  }

  #[tokio::test]
  async fn test_infinite_loop_aborts() {
    let ctx = test_context();
    let res = exec(ctx, "while true do end").await.unwrap();
    assert!(res.contains("Runtime Error"));
    assert!(res.contains("limit exceeded"));
  }

  #[tokio::test]
  async fn test_sandbox_restricts_os() {
    let ctx = test_context();
    let res = exec(ctx, "return os").await.unwrap();
    assert!(res.contains("Nil") || res.contains("Success"));
  }

  #[tokio::test]
  async fn test_tool_calling_from_lua() {
    let ctx = test_context();
    let res = exec(
      ctx,
      "local content, err = read_file('Cargo.toml', 0, 100); return content:find('ogent') ~= nil",
    )
    .await
    .unwrap();
    assert!(res.contains("true"), "Result was: {}", res);
  }

  #[tokio::test]
  async fn test_hash_anchors_and_batch_edits_from_lua() {
    let ctx = test_context();
    let path = "temp_test_anchors.txt";
    let full_path = ctx.workspace.workspace_path(path).unwrap();
    std::fs::write(&full_path, "line 1\nline 2\nline 3\n").unwrap();

    let lua_code = r#"
      local anchors, err = read_hash_anchors('temp_test_anchors.txt')
      if not anchors then error(err) end
      local hash2 = anchors:match("2:(%w+)|line 2")
      local hash3 = anchors:match("3:(%w+)|line 3")
      local ops = {
        { start_at = "2:" .. hash2, action = "delete" },
        { start_at = "3:" .. hash3, action = "replace", content = "new line 3" }
      }
      -- Test 1-arg signature (uses path from read_hash_anchors)
      local res, err = apply_anchor_edits(ops)
      if not res then error(err) end

      -- Read again to get new anchors
      local anchors2, err = read_hash_anchors('temp_test_anchors.txt')
      if not anchors2 then error(err) end
      local hash3_new = anchors2:match("2:(%w+)|new line 3") -- line 3 became line 2
      local ops2 = {
        { start_at = "2:" .. hash3_new, action = "replace", content = "new line 3 updated" }
      }
      -- Test 2-arg signature
      local res2, err = apply_anchor_edits('temp_test_anchors.txt', ops2)
      if not res2 then error(err) end

      return read_file('temp_test_anchors.txt')
    "#;

    let res = exec(ctx, lua_code).await.unwrap();
    let _ = std::fs::remove_file(full_path);

    assert!(res.contains("line 1"), "Result was: {}", res);
    assert!(!res.contains("line 2"), "Result was: {}", res);
    assert!(res.contains("new line 3 updated"), "Result was: {}", res);
  }

  #[tokio::test]
  async fn test_skills_from_lua() {
    let temp = std::env::temp_dir().join(format!(
      "ogent-lua-skills-test-{}",
      crate::session::timestamp_ms()
    ));
    let skill_dir = temp.join(".ogent/skills/my_test_skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
      skill_dir.join("SKILL.md"),
      "---\nname: my_test_skill\ndescription: A test skill for Lua\n---\nHello from skill body!",
    )
    .unwrap();

    let asset_dir = skill_dir.join("references");
    std::fs::create_dir_all(&asset_dir).unwrap();
    std::fs::write(asset_dir.join("MANUAL.md"), "This is MANUAL content.").unwrap();

    let workspace = Workspace::from_root(temp.clone());
    let skill_store = Arc::new(crate::skills::SkillStore::new(workspace.root()));
    let client = crate::client::Client::new(
      "http://localhost",
      "dummy".into(),
      |_, _| Ok(serde_json::Value::Null),
      30,
    )
    .unwrap();
    let ctx = ToolContext {
      workspace,
      skill_store,
      lua_session: Arc::new(Mutex::new(None)),
      client,
      output_sink: None,
      verbose: false,
      actor_id: "director".to_string(),
      agent_depth: 0,
    };

    // Test list_skills()
    let list_res = exec(
      ctx.clone(),
      "local res, err = list_skills(); if not res then error(err) end; return res",
    )
    .await
    .unwrap();
    assert!(list_res.contains("my_test_skill"));
    assert!(list_res.contains("A test skill for Lua"));

    // Test load_skill("my_test_skill")
    let load_res = exec(
      ctx.clone(),
      "local res, err = load_skill('my_test_skill'); if not res then error(err) end; return res",
    )
    .await
    .unwrap();
    assert!(load_res.contains("skill name="));
    assert!(load_res.contains("my_test_skill"));
    assert!(load_res.contains("Hello from skill body!"));

    // Test load_skill_asset(root, path)
    let load_asset_code = format!(
      r#"local res, err = load_skill_asset('{}', 'references/MANUAL.md'); if not res then error(err) end; return res"#,
      skill_dir.to_string_lossy()
    );
    let asset_res = exec(ctx.clone(), &load_asset_code).await.unwrap();
    assert!(asset_res.contains("This is MANUAL content."));

    // Test load_skill_asset directory traversal rejection
    let bad_asset_code = format!(
      r#"local res, err = load_skill_asset('{}', '../../Cargo.toml'); return err"#,
      skill_dir.to_string_lossy()
    );
    let bad_res = exec(ctx.clone(), &bad_asset_code).await.unwrap();
    assert!(bad_res.contains("outside the skill root directory"));

    // Test load_skill_asset non-whitelisted root rejection
    let bad_root_code = r#"local res, err = load_skill_asset('/tmp', 'foo'); return err"#;
    let bad_res2 = exec(ctx.clone(), bad_root_code).await.unwrap();
    assert!(bad_res2.contains("not inside a whitelisted"));

    let _ = std::fs::remove_dir_all(temp);
  }

  #[tokio::test]
  async fn test_parallel_and_task_update_from_lua() {
    let ctx = test_context();
    let code = r#"
      task_update("testing", "running parallel test")
      local results = parallel({
        function() return 10 + 20 end,
        function() return 30 + 40 end
      })
      return results
    "#;
    let res = exec(ctx, code).await.unwrap();
    assert!(res.contains("30"), "Res was: {res}");
    assert!(res.contains("70"), "Res was: {res}");
  }

  #[tokio::test]
  async fn test_glob_from_lua() {
    let ctx = test_context();
    let temp_file = ctx.workspace.workspace_path("temp_test_glob.txt").unwrap();
    std::fs::write(&temp_file, "temp").unwrap();

    let code = r#"
      local files, err = glob("temp_test_glob.*")
      if not files then error(err) end
      return files
    "#;
    let res = exec(ctx, code).await.unwrap();
    let _ = std::fs::remove_file(temp_file);

    assert!(res.contains("temp_test_glob.txt"), "Res was: {res}");
  }

  #[tokio::test]
  async fn test_base_functions_available() {
    // Regression: ensure BASE library functions like pairs/type/pcall are present.
    // Lua::new_with in mlua loads BASE implicitly even when not specified in StdLib flags,
    // but we keep this test to catch any future regression if the sandbox setup changes.
    let ctx = test_context();
    let res = exec(ctx, "for k,v in pairs({a=1}) do return k end")
      .await
      .unwrap();
    assert!(
      res.contains("\"a\""),
      "Expected pairs() to work (BASE lib loaded). Got: {res}"
    );
  }

  #[tokio::test]
  async fn test_eval_survives_panic_while_holding_lock() {
    // Regression: with std::sync::Mutex, a panic while holding the session lock
    // would poison the mutex permanently, killing all future eval calls.
    // Mutex releases the lock on panic, so eval can recover.
    let session = Arc::new(Mutex::new(None::<Lua>));
    let session2 = session.clone();

    let t = std::thread::spawn(move || {
      let _g = session2.lock();
      panic!("intentional panic while holding lock");
    });
    assert!(t.join().is_err(), "thread must have panicked");

    let workspace = Workspace::from_current_dir();
    let skill_store = Arc::new(crate::skills::SkillStore::new(workspace.root()));
    let client = crate::client::Client::new(
      "http://localhost",
      "dummy".into(),
      |_, _| Ok(serde_json::Value::Null),
      30,
    )
    .unwrap();

    let ctx = ToolContext {
      workspace,
      skill_store,
      lua_session: session,
      client,
      output_sink: None,
      verbose: false,
      actor_id: "director".to_string(),
      agent_depth: 0,
    };

    // eval should create a fresh VM and succeed despite the previous panic.
    let res = eval(ctx, "return 42").await.unwrap();
    assert!(
      res.contains("42"),
      "eval should recover after panic. Got: {res}"
    );
  }

  #[tokio::test]
  async fn test_git_tools_from_lua() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // init git repo and make an initial commit
    std::process::Command::new("git")
      .arg("-C")
      .arg(root)
      .arg("init")
      .output()
      .unwrap();
    std::process::Command::new("git")
      .arg("-C")
      .arg(root)
      .args(["config", "user.email", "test@test.com"])
      .output()
      .unwrap();
    std::process::Command::new("git")
      .arg("-C")
      .arg(root)
      .args(["config", "user.name", "Test"])
      .output()
      .unwrap();

    std::fs::write(root.join("test.txt"), "hello\n").unwrap();
    std::process::Command::new("git")
      .arg("-C")
      .arg(root)
      .args(["add", "test.txt"])
      .output()
      .unwrap();
    std::process::Command::new("git")
      .arg("-C")
      .arg(root)
      .args(["commit", "-m", "init"])
      .output()
      .unwrap();

    // modify the file in the worktree
    std::fs::write(root.join("test.txt"), "hello world\n").unwrap();

    // create a fully staged file (no worktree changes)
    std::fs::write(root.join("staged.txt"), "staged content\n").unwrap();
    std::process::Command::new("git")
      .arg("-C")
      .arg(root)
      .args(["add", "staged.txt"])
      .output()
      .unwrap();

    let workspace = Workspace::from_root(root.to_path_buf());
    let skill_store = Arc::new(crate::skills::SkillStore::new(workspace.root()));
    let client = crate::client::Client::new(
      "http://localhost",
      "dummy".into(),
      |_, _| Ok(serde_json::Value::Null),
      30,
    )
    .unwrap();
    let ctx = ToolContext {
      workspace,
      skill_store,
      lua_session: Arc::new(Mutex::new(None)),
      client,
      output_sink: None,
      verbose: false,
      actor_id: "director".to_string(),
      agent_depth: 0,
    };

    // Test git_status
    let res = exec(
      ctx.clone(),
      "local status, err = git_status(); if not status then error(err) end; local out = {}; for _, e in ipairs(status) do table.insert(out, e.path .. ':' .. e.status) end; return table.concat(out, ',')",
    )
    .await
    .unwrap();
    assert!(
      res.contains("test.txt:modified"),
      "git_status result: {res}"
    );
    assert!(res.contains("staged.txt:added"), "git_status result: {res}");

    // Test git_diff
    let res = exec(
      ctx.clone(),
      "local diff, err = git_diff(); if not diff then error(err) end; return diff[1].path .. ':' .. diff[1].change_type .. ':' .. #diff[1].hunks",
    )
    .await
    .unwrap();
    assert!(
      res.contains("test.txt:modified:1"),
      "git_diff result: {res}"
    );

    // Test git_changes — worktree diff on test.txt, staged_diff on staged.txt
    let res = exec(
      ctx.clone(),
      "local changes, err = git_changes(); if not changes then error(err) end; local out = {}; for _, e in ipairs(changes) do if e.diff then table.insert(out, e.path .. ':diff:' .. #e.diff.hunks) end if e.staged_diff then table.insert(out, e.path .. ':staged_diff:' .. #e.staged_diff.hunks) end end; return table.concat(out, ',')",
    )
    .await
    .unwrap();
    assert!(
      res.contains("test.txt:diff:1"),
      "git_changes expected worktree diff on test.txt: {res}"
    );
    assert!(
      res.contains("staged.txt:staged_diff:1"),
      "git_changes expected staged diff on staged.txt: {res}"
    );

    // Test git_changes with base=HEAD (should still show diffs)
    let res = exec(
      ctx.clone(),
      "local changes, err = git_changes{base='HEAD'}; if not changes then error(err) end; local out = {}; for _, e in ipairs(changes) do if e.diff then table.insert(out, e.path .. ':diff:' .. #e.diff.hunks) end if e.staged_diff then table.insert(out, e.path .. ':staged_diff:' .. #e.staged_diff.hunks) end end; return table.concat(out, ',')",
    )
    .await
    .unwrap();
    assert!(
      res.contains("test.txt:diff:1"),
      "git_changes base=HEAD expected worktree diff on test.txt: {res}"
    );
    assert!(
      res.contains("staged.txt:staged_diff:1"),
      "git_changes base=HEAD expected staged diff on staged.txt: {res}"
    );

    // Test git_show (positional syntax)
    let res = exec(
      ctx.clone(),
      "local content, err = git_show('test.txt', 'HEAD'); if not content then error(err) end; return content:find('hello') ~= nil",
    )
    .await
    .unwrap();
    assert!(res.contains("true"), "git_show positional result: {res}");

    // Test git_show (table syntax)
    let res = exec(
      ctx.clone(),
      "local content, err = git_show{path='test.txt', ref='HEAD'}; if not content then error(err) end; return content:find('hello') ~= nil",
    )
    .await
    .unwrap();
    assert!(res.contains("true"), "git_show table syntax result: {res}");

    // Test git_show on staged file using ref='staged'
    let res = exec(
      ctx.clone(),
      "local content, err = git_show{path='staged.txt', ref='staged'}; if not content then error(err) end; return content:find('staged content') ~= nil",
    )
    .await
    .unwrap();
    assert!(res.contains("true"), "git_show staged result: {res}");

    // Test git_show error when file not found at ref
    let res = exec(
      ctx.clone(),
      "local content, err = git_show{path='nonexistent.txt', ref='HEAD'}; if content then error('expected error') end; return err",
    )
    .await
    .unwrap();
    assert!(
      res.contains("not found at ref"),
      "git_show missing file error result: {res}"
    );

    // Make a second commit so HEAD~1 exists for testing non-default ref
    std::fs::write(root.join("test.txt"), "hello world commit2\n").unwrap();
    std::process::Command::new("git")
      .arg("-C")
      .arg(root)
      .args(["add", "test.txt"])
      .output()
      .unwrap();
    std::process::Command::new("git")
      .arg("-C")
      .arg(root)
      .args(["commit", "-m", "second"])
      .output()
      .unwrap();

    // Test git_show at HEAD~1 (should show 'hello' from first commit)
    let res = exec(
      ctx.clone(),
      "local content, err = git_show{path='test.txt', ref='HEAD~1'}; if not content then error(err) end; return content:find('hello world commit2') == nil and content:find('hello') ~= nil",
    )
    .await
    .unwrap();
    assert!(res.contains("true"), "git_show HEAD~1 result: {res}");

    // Test git_log
    let res = exec(
      ctx,
      "local log, err = git_log({n=5}); if not log then error(err) end; for _, e in ipairs(log) do if e.subject:find('init') then return true end end; return false",
    )
    .await
    .unwrap();
    assert!(res.contains("true"), "git_log result: {res}");
  }
}
