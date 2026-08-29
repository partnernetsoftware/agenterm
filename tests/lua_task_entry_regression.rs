//! Whole-file gate: exercises the lua engine, which defaults off.
#![cfg(feature = "script-lua")]

//! Regression test: Lua task entry execution through the script backend.
//!
//! Verifies that `.lua` entries in the task manifest are recognized,
//! that the `Lua` backend is selectable, and that lua evaluation works.
//!
//! History: originally written against `try_execute_lua_invocation` +
//! `LuaInvocationOptions`; those were folded into `LuaEngineBackend`
//! (Trait-M4, see `plan/design-script-engine-trait.md` §4), so the same
//! scenarios now run through the `ScriptEngineBackend` trait. The
//! `ScriptBackend::Rhai` variant assertions were dropped when that backend
//! was retired, and the `ScriptBackend::Rh` ones when that engine left the
//! repository on 2026-08-29 (`partnernetsoftware/rh`).

use agenterm::script_backend::ScriptBackend;
use agenterm::script_engine::{LuaEngineBackend, ScriptEngineBackend, ScriptInvocationOptions};
use serde_json::json;
use std::sync::Mutex;

/// Serializes the env-var-touching tests in this file (integration tests
/// within one target share a process; `AGENTERM_SCRIPT_BACKEND` is global).
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_lua_backend<T>(body: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prior = std::env::var("AGENTERM_SCRIPT_BACKEND").ok();
    unsafe {
        std::env::set_var("AGENTERM_SCRIPT_BACKEND", "lua");
    }
    let result = body();
    match prior {
        Some(value) => unsafe {
            std::env::set_var("AGENTERM_SCRIPT_BACKEND", value);
        },
        None => unsafe {
            std::env::remove_var("AGENTERM_SCRIPT_BACKEND");
        },
    }
    result
}

#[test]
fn lua_task_entry_backend_selection() {
    // Verify path-based backend selection.
    assert_eq!(
        ScriptBackend::from_entry_path("scripts/lua/test.lua"),
        Some(ScriptBackend::Lua)
    );
    // `.rh`/`.rhai` and unknown extensions route nowhere: there is no
    // default engine, and `resolve` turns `None` into a refusal by name.
    assert_eq!(ScriptBackend::from_entry_path("test.rh"), None);
    assert_eq!(ScriptBackend::from_entry_path("test.rhai"), None);
    assert_eq!(ScriptBackend::from_entry_path("test.txt"), None);
}

#[test]
fn lua_backend_str() {
    assert_eq!(ScriptBackend::Lua.as_str(), "lua");
    // `Qjs` was a third row here until that engine was archived. `qjs` is
    // still a name the CLI accepts -- it reaches `qjswasm` now -- but it is no
    // longer a backend of its own, so there is nothing to spell.
    #[cfg(feature = "script-qjswasm")]
    assert_eq!(ScriptBackend::Qjswasm.as_str(), "qjswasm");
}

#[test]
fn lua_invocation_eval_returns_value() {
    with_lua_backend(|| {
        let result = LuaEngineBackend
            .execute("return 42", &ScriptInvocationOptions::default(), None)
            .expect("lua eval");
        assert_eq!(result.value, Some(json!(42)));
    });
}

#[test]
fn lua_invocation_check_valid_source() {
    with_lua_backend(|| {
        LuaEngineBackend
            .check("return 0", &ScriptInvocationOptions::default())
            .expect("lua check");
    });
}

#[test]
fn lua_invocation_eval_with_print_captures_stdout() {
    with_lua_backend(|| {
        let result = LuaEngineBackend
            .execute(
                "print('hello') return 7",
                &ScriptInvocationOptions::default(),
                None,
            )
            .expect("lua eval");
        assert_eq!(result.value, Some(json!(7)));
        assert!(result.stdout.contains("hello"), "stdout: {}", result.stdout);
    });
}

#[test]
fn lua_backend_not_enabled_without_env() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prior = std::env::var("AGENTERM_SCRIPT_BACKEND").ok();
    unsafe {
        std::env::remove_var("AGENTERM_SCRIPT_BACKEND");
    }

    // Under the trait contract, "not selected" is an `enabled()` query, not
    // an `Ok(None)` return (the old try_execute_lua_invocation shape).
    assert!(!LuaEngineBackend.enabled());

    match prior {
        Some(value) => unsafe {
            std::env::set_var("AGENTERM_SCRIPT_BACKEND", value);
        },
        None => unsafe {
            std::env::remove_var("AGENTERM_SCRIPT_BACKEND");
        },
    }
}
