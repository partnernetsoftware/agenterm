//! `agenterm cli script run FILE` must pick the engine from the file's
//! extension, and this test exercises the **product path** to say so.
//!
//! # Why the product path and not `from_entry_path`
//!
//! `ScriptBackend::from_entry_path` was correct, tested, and documented from
//! the day it was written, and had **zero callers in production code** until
//! 2026-08-28. One of the tests covering it is named
//! `lua_task_entry_backend_selection` and carries the comment "Verify
//! path-based backend selection" -- while verifying nothing but the pure
//! function. Meanwhile `agenterm cli script run t.qjs` answered with *rh's*
//! parse error for a JavaScript file, and `.lua` did the same.
//!
//! So the acceptance criterion written into PRD 02.36 §接下来 04 before the
//! repair started was explicit: **a test must assert the product path, or the
//! repair reproduces the defect it repairs.** This file is that criterion.
//! It spawns the real binary and reads what came out.
//!
//! # What actually broke
//!
//! Two places, and neither was a missing call:
//!
//! 1. `worker_supervisor::script_backend_environment` materialised `"rh"` into
//!    the worker's environment when the parent had none. An eagerly-set
//!    default is indistinguishable from a user's explicit choice, so the
//!    "explicit beats extension" rule matched every time and the extension was
//!    never consulted.
//! 2. Every engine's `check`/`execute` re-read the environment through
//!    `enabled()` after the dispatcher had already chosen. Once the dispatcher
//!    gained a second input, the two disagreed and the engine refused work it
//!    had just been handed.
//!
//! Both are the same shape: a decision with more than one home.

use std::process::Command;

const AGENTERM_BIN: &str = env!("CARGO_BIN_EXE_agenterm");

/// Run the CLI with `AGENTERM_SCRIPT_BACKEND` either removed or set, and
/// return `(stdout, stderr, exit code)`.
fn run_script(path: &std::path::Path, backend: Option<&str>) -> (String, String, i32) {
    let mut command = Command::new(AGENTERM_BIN);
    command.args(["cli", "script", "run"]).arg(path);
    match backend {
        Some(value) => command.env("AGENTERM_SCRIPT_BACKEND", value),
        None => command.env_remove("AGENTERM_SCRIPT_BACKEND"),
    };
    let output = command.output().expect("the CLI binary runs");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

fn write(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("fixture is writable");
    path
}

/// A `.qjs` file with no environment variable set runs on qjswasm.
///
/// The program is chosen to be unambiguous about *which* engine ran it: it
/// uses a template literal, an arrow function, `Array.prototype.map` and
/// `.length` -- four things rh's parser rejects outright. A wrong engine
/// cannot accidentally produce `sum=3`.
#[cfg(feature = "script-qjswasm")]
#[test]
fn a_qjs_entry_runs_on_qjswasm_without_being_told_to() {
    let dir = std::env::temp_dir().join(format!("agenterm-route-qjs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = write(
        &dir,
        "t.qjs",
        "const xs = [1,2,3];\nreturn `sum=${xs.map(x => x * 2).length}`;\n",
    );

    let (stdout, stderr, code) = run_script(&path, None);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("sum=3"),
        "a `.qjs` entry must reach qjswasm with no environment variable; \
         got stdout={stdout} stderr={stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An explicit backend beats the extension, in the direction that matters.
///
/// This is the half that keeps the repair from being the same defect pointed
/// the other way: someone who states a backend must get it, even for a file
/// whose extension says otherwise. rh cannot parse this program, so its
/// failure *is* the assertion -- a silent reroute to qjswasm would succeed.
#[test]
fn an_explicit_backend_beats_the_extension() {
    let dir = std::env::temp_dir().join(format!("agenterm-route-explicit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = write(&dir, "t.qjs", "const xs = [1,2,3];\nreturn xs.map(x => x);\n");

    let (stdout, stderr, code) = run_script(&path, Some("rh"));
    assert_ne!(
        code, 0,
        "rh cannot run this program, so an explicit rh must fail rather than \
         be quietly rerouted; stdout={stdout} stderr={stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("rh_backend") || combined.contains("rh parse error"),
        "the failure must come from rh, naming the engine the caller asked \
         for; got {combined}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The routing is general, not a `.qjs` special case.
#[cfg(feature = "script-lua")]
#[test]
fn a_lua_entry_runs_on_lua_without_being_told_to() {
    let dir = std::env::temp_dir().join(format!("agenterm-route-lua-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = write(&dir, "t.lua", "return 1+2\n");

    let (stdout, stderr, code) = run_script(&path, None);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains('3'),
        "a `.lua` entry must reach lua with no environment variable; \
         got stdout={stdout} stderr={stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
