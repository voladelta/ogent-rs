use anyhow::{Context, Result};
use mlua::{HookTriggers, Lua, LuaSerdeExt, StdLib, Value};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::tools::{Handler, ToolContext, ToolDef, parse_args};

const MAX_AGENT_DEPTH: u32 = 3;

pub fn tools() -> Vec<ToolDef> {
  vec![
    ToolDef {
      name: "exec",
      description: "Execute a one-off stateless Lua 5.5 script. Captures stdout prints and the final return value.",
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
      handler: Handler::async_fn(|ctx, args| async move { exec_tool(ctx, &args).await }),
    },
    ToolDef {
      name: "eval",
      description: "Execute a stateful Lua 5.5 script within the persistent session. Captures stdout prints and the final return value. Retains global variables/functions between calls.",
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
      handler: Handler::async_fn(|ctx, args| async move { eval_tool(ctx, &args).await }),
    },
  ]
}

#[derive(Deserialize)]
struct LuaArgs {
  code: String,
  #[allow(dead_code)]
  reason: Option<String>,
}

fn create_sandboxed_vm() -> Result<Lua> {
  // Load safe libraries. Coroutine is required for async support in mlua.
  let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE;
  let lua = Lua::new_with(libs, mlua::LuaOptions::default())?;

  // Limit memory usage to 32MB
  lua.set_memory_limit(32 * 1024 * 1024)?;

  Ok(lua)
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
    crate::tools::configured_agent_tools(),
    crate::session::generate_session_id(),
    ctx.skill_store.clone(),
    role.to_string(),
    ctx.verbose,
    ctx.agent_depth + 1,
  );
  subagent.set_output_sink(ctx.output_sink.clone());

  let run_res = subagent.run_loop().await;
  match run_res {
    Ok(_) => {
      let last_msg = subagent
        .messages
        .iter()
        .rfind(|m| m.role == crate::types::Role::Assistant);
      Ok(last_msg.map(|m| m.content.clone()).unwrap_or_default())
    }
    Err(e) => Err(mlua::Error::RuntimeError(format!(
      "subagent run loop failed: {e}"
    ))),
  }
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

  // Helper to decode JSON strings into Lua tables
  let json_decode = lua.create_function(|lua, s: String| {
    let v: serde_json::Value = serde_json::from_str(&s)
      .map_err(|e| mlua::Error::RuntimeError(format!("json_decode: {e}")))?;
    lua.to_value(&v)
  })?;
  globals.set("json_decode", json_decode)?;

  // Track manually-registered names so the generic loop doesn't overwrite them
  let mut registered = std::collections::HashSet::new();
  registered.insert("task_update".to_string());
  registered.insert("agent".to_string());
  registered.insert("parallel".to_string());
  registered.insert("json_decode".to_string());
  // exec and eval must not be exposed inside the Lua sandbox: calling eval from
  // within an eval session would deadlock because eval_tool holds the
  // lua_session lock and parking_lot::Mutex is not reentrant.
  registered.insert("exec".to_string());
  registered.insert("eval".to_string());

  // Register generic handlers for every tool from all_tools().
  // `registered.insert()` returns false if the name was already present (manually
  // registered above), so we skip duplicates rather than overwriting them.
  for tool in crate::tools::all_tools() {
    if !registered.insert(tool.name.to_string()) {
      continue;
    }
    let tool_name = tool.name.to_string();
    let ctx_clone = ctx.clone();
    let lua_fn = lua.create_async_function(move |lua, args: Value| {
      let tool_name = tool_name.clone();
      let ctx = ctx_clone.clone();
      async move {
        let json_args = match args {
          Value::Nil => "{}".to_string(),
          _ => {
            let json_val: serde_json::Value = lua.from_value(args)?;
            json_val.to_string()
          }
        };
        let result = crate::tools::execute_tool(ctx, &tool_name, &json_args).await;
        match result {
          Ok(output) => Ok((Value::String(lua.create_string(output)?), Value::Nil)),
          Err(e) => Ok((Value::Nil, Value::String(lua.create_string(e.to_string())?))),
        }
      }
    })?;
    globals.set(tool.name, lua_fn)?;
  }

  // Inject thin Lua wrappers that convert positional arguments to tables
  // and delegate to the generic handlers registered above.
  let positional_wrappers = r#"
local _t = {}
_t.read_file = read_file
_t.append_file = append_file
_t.file_info = file_info
_t.read_hash_anchors = read_hash_anchors
_t.edit_hash_anchors = edit_hash_anchors
_t.load_skill = load_skill
_t.list_skills = list_skills
_t.load_skill_asset = load_skill_asset
_t.glob = glob

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
  local ok, err = _t.edit_hash_anchors({path=path, ops=ops})
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
"#;
  lua.load(positional_wrappers).exec()?;

  Ok(())
}

async fn run_lua_vm_async(lua: &Lua, code: &str) -> Result<String> {
  // 1. Capture stdout via print override
  let stdout_buffer = Arc::new(parking_lot::Mutex::new(String::new()));
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
    let end = final_response.floor_char_boundary(16384);
    final_response.truncate(end);
    final_response.push_str("\n... [Output truncated to 16k limit] ...");
  }

  Ok(final_response)
}

// exec is stateless: create a fresh VM, run the script, and discard it.
// With mlua's `send` feature, Lua is Send and the `thread.into_async()` future
// is also Send, so we can run it directly in async context without burning a
// blocking thread.
async fn exec_tool(ctx: ToolContext, args: &str) -> Result<String> {
  let args: LuaArgs = parse_args(args)?;
  let lua = create_sandboxed_vm()?;
  register_tools_in_lua(&lua, ctx)?;
  run_lua_vm_async(&lua, &args.code).await
}

// eval is stateful: it reuses the same Lua VM across calls via a session lock.
// parking_lot::MutexGuard is !Send, so we cannot hold the guard across an await
// in a Send future. We confine the locked operation to spawn_blocking so the
// guard never crosses an await boundary.
async fn eval_tool(ctx: ToolContext, args: &str) -> Result<String> {
  let args: LuaArgs = parse_args(args)?;
  let session = ctx.lua_session.clone();
  tokio::task::spawn_blocking(move || {
    let mut guard = session.lock();
    if guard.is_none() {
      let lua = create_sandboxed_vm()?;
      register_tools_in_lua(&lua, ctx.clone())?;
      *guard = Some(lua);
    }
    let lua = guard.as_ref().unwrap();
    let handle = tokio::runtime::Handle::current();
    handle.block_on(async { run_lua_vm_async(lua, &args.code).await })
  })
  .await
  .context("spawn_blocking panicked")?
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
      lua_session: Arc::new(parking_lot::Mutex::new(None)),
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
    let res = exec_tool(ctx.clone(), r#"{"code": "print('hello'); return 2 + 2"}"#)
      .await
      .unwrap();
    assert!(res.contains("Stdout Output"));
    assert!(res.contains("hello"));
    assert!(res.contains("Return Value"));
    assert!(res.contains("4"));

    // Verify it is stateless (variables don't persist)
    let _ = exec_tool(ctx.clone(), r#"{"code": "global_var = 42"}"#)
      .await
      .unwrap();
    let res2 = exec_tool(ctx, r#"{"code": "return global_var"}"#)
      .await
      .unwrap();
    assert!(res2.contains("Nil") || res2.contains("Success (no output or return value)."));
  }

  #[tokio::test]
  async fn test_eval_stateful() {
    let ctx = test_context();
    let res = eval_tool(
      ctx.clone(),
      r#"{"code": "persisted_var = 100; return persisted_var"}"#,
    )
    .await
    .unwrap();
    assert!(res.contains("100"));

    let res2 = eval_tool(ctx, r#"{"code": "return persisted_var + 50"}"#)
      .await
      .unwrap();
    assert!(res2.contains("150"));
  }

  #[tokio::test]
  async fn test_infinite_loop_aborts() {
    let ctx = test_context();
    let res = exec_tool(ctx, r#"{"code": "while true do end"}"#)
      .await
      .unwrap();
    assert!(res.contains("Runtime Error"));
    assert!(res.contains("limit exceeded"));
  }

  #[tokio::test]
  async fn test_sandbox_restricts_os() {
    let ctx = test_context();
    let res = exec_tool(ctx, r#"{"code": "return os"}"#).await.unwrap();
    assert!(res.contains("Nil") || res.contains("Success"));
  }

  #[tokio::test]
  async fn test_tool_calling_from_lua() {
    let ctx = test_context();
    let res = exec_tool(
      ctx,
      r#"{"code": "local content, err = read_file('Cargo.toml', 0, 100); return content:find('ogent') ~= nil"}"#,
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

    let res = exec_tool(ctx, &json!({ "code": lua_code }).to_string())
      .await
      .unwrap();
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
      lua_session: Arc::new(parking_lot::Mutex::new(None)),
      client,
      output_sink: None,
      verbose: false,
      actor_id: "director".to_string(),
      agent_depth: 0,
    };

    // Test list_skills()
    let list_res = exec_tool(
      ctx.clone(),
      r#"{"code": "local res, err = list_skills(); if not res then error(err) end; return res"}"#,
    )
    .await
    .unwrap();
    assert!(list_res.contains("my_test_skill"));
    assert!(list_res.contains("A test skill for Lua"));
    assert!(list_res.contains("my_test_skill"));

    // Test load_skill("my_test_skill")
    let load_res = exec_tool(ctx.clone(), r#"{"code": "local res, err = load_skill('my_test_skill'); if not res then error(err) end; return res"}"#).await.unwrap();
    assert!(load_res.contains("skill name="));
    assert!(load_res.contains("my_test_skill"));
    assert!(load_res.contains("Hello from skill body!"));

    // Test load_skill_asset(root, path)
    let load_asset_code = format!(
      r#"local res, err = load_skill_asset('{}', 'references/MANUAL.md'); if not res then error(err) end; return res"#,
      skill_dir.to_string_lossy()
    );
    let asset_res = exec_tool(ctx.clone(), &json!({ "code": load_asset_code }).to_string())
      .await
      .unwrap();
    assert!(asset_res.contains("This is MANUAL content."));

    // Test load_skill_asset directory traversal rejection
    let bad_asset_code = format!(
      r#"local res, err = load_skill_asset('{}', '../../Cargo.toml'); return err"#,
      skill_dir.to_string_lossy()
    );
    let bad_res = exec_tool(ctx.clone(), &json!({ "code": bad_asset_code }).to_string())
      .await
      .unwrap();
    assert!(bad_res.contains("outside the skill root directory"));

    // Test load_skill_asset non-whitelisted root rejection
    let bad_root_code = r#"local res, err = load_skill_asset('/tmp', 'foo'); return err"#;
    let bad_res2 = exec_tool(ctx.clone(), &json!({ "code": bad_root_code }).to_string())
      .await
      .unwrap();
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
    let res = exec_tool(ctx, &json!({ "code": code }).to_string())
      .await
      .unwrap();
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
    let res = exec_tool(ctx, &json!({ "code": code }).to_string())
      .await
      .unwrap();
    let _ = std::fs::remove_file(temp_file);

    assert!(res.contains("temp_test_glob.txt"), "Res was: {res}");
  }

  #[tokio::test]
  async fn test_base_functions_available() {
    // Regression: ensure BASE library functions like pairs/type/pcall are present.
    // Lua::new_with in mlua loads BASE implicitly even when not specified in StdLib flags,
    // but we keep this test to catch any future regression if the sandbox setup changes.
    let ctx = test_context();
    let res = exec_tool(
      ctx,
      r#"{"code": "for k,v in pairs({a=1}) do return k end"}"#,
    )
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
    // parking_lot::Mutex releases the lock on panic, so eval can recover.
    let session = Arc::new(parking_lot::Mutex::new(None::<Lua>));
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

    // eval_tool should create a fresh VM and succeed despite the previous panic.
    let res = eval_tool(ctx, r#"{"code": "return 42"}"#).await.unwrap();
    assert!(
      res.contains("42"),
      "eval should recover after panic. Got: {res}"
    );
  }
}
