//! The `tool.*` host door -- the OS-facing surface a *tool* `.qjs` gets, and a
//! sandbox `.qjs` never does.
//!
//! ```text
//! fs.exists(ptr, len)                     -> i32   // 1 / 0, -1 = could not ask
//! fs.read_to_string(ptr, len)             -> i32   // status, text parked
//! fs.write(p_ptr, p_len, t_ptr, t_len)    -> i32   // status
//! fs.create_dir_all(ptr, len)             -> i32   // status
//! fs.remove_file(ptr, len)                -> i32   // status
//! fs.read_dir(ptr, len)                   -> i32   // status, JSON array parked
//! fs.metadata(ptr, len)                   -> i32   // status, JSON object parked
//! process.command(ptr, len)               -> i32   // status, JSON object parked; spec is JSON
//! process.id()                            -> i32   // this process's pid
//! env.get(ptr, len)                       -> i32   // status, value parked
//! env.has(ptr, len)                       -> i32   // 1 / 0, -1 = could not ask
//! env.cwd()                               -> i32   // status, path parked
//! result_len()                            -> i32
//! result(dst_ptr, dst_len)                -> i32   // written, negative = too small
//! ```
//!
//! # Why a second door and not a bigger first one
//!
//! PRD 36 (`prd/PRD_02_36_agenterm_qjswasm.md`, "A1.1 的答案") decided it: the
//! 71 `.rh` scripts this door exists to receive are CI gates and build tools,
//! and they were never sandboxed -- rh transpiled `std::fs::exists` straight to
//! Rust's. Giving *every* `.qjs` a filesystem would turn the sandbox into a
//! sandbox in name only, so there are two kinds of `.qjs` instead: a sandbox
//! script sees `agenterm.*` and nothing else, a tool script sees `agenterm.*`
//! **and** this. The difference is who can open it. A sandbox slot refuses a
//! `tool.*` import at load time, by name (see `host::check_declarations`), and
//! only an [`Engine`](crate::Engine) built with
//! [`with_tool_door`](crate::Engine::with_tool_door) -- or a module compiled
//! with [`compile_qjs_tool`](crate::compile_qjs_tool) -- ever has it.
//!
//! This is not a WASI `fd_*` surface. It is a short table of named functions,
//! each an ordinary `HostFn` declaration with an ordinary wasm signature, only
//! the ones a script mentions become imports, and every call is recorded by
//! name in [`Outcome::tool_calls`](crate::Outcome::tool_calls) so a receipt
//! can say exactly which host capabilities a run reached.
//!
//! # The same mechanism as the fleet door, deliberately
//!
//! A tool function that produces text does **not** return it. It returns a
//! status and parks the answer; `tool_result()` fetches it in the same two
//! passes `fleet_result` uses, for the same reason (`src/host.rs`: a host
//! callback cannot re-enter the guest to ask for a landing buffer). One shared
//! fetch rather than one per function because a `HostResult::Bytes` door
//! receives its declared parameters on *both* passes, so `read_to_string(path)
//! -> Bytes` would read the file twice and trap on a size race. The doing is
//! one declaration, the fetching is another, and the two buffers -- fleet's
//! and this door's -- are separate, so a `fleet_call` never clobbers a file a
//! script has not collected yet.
//!
//! Status codes are the fleet door's: `0` ok, `1` the operation answered no
//! (its diagnostic is parked). There is no `2`: an absent door is a load-time
//! refusal here, not a run-time status. The two boolean questions (`exists`,
//! `has`) answer `1`/`0` directly and `-1` when the question could not be
//! asked at all -- a path that is not UTF-8, or an OS error other than "not
//! found" -- with the diagnostic parked.
//!
//! # Budget and containment
//!
//! Every parked answer is bounded by [`Budget::max_bridge_result_bytes`], the
//! same cap the fleet door applies, and refused rather than cut for the same
//! reason: half a file is worse than a refusal, because the script cannot tell
//! it was cut. `process.command` bounds its captured stdout and stderr by the
//! same number and kills the child at `timeout_ms` (default
//! [`DEFAULT_COMMAND_TIMEOUT_MS`]), because the core's step budget does not
//! measure time spent in the host. A panic inside a tool operation is caught
//! and reported as [`QjswasmError::Door`], never dressed up as status `1` --
//! `src/host.rs` explains why those are different answers.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::rc::Rc;
use std::time::{Duration, Instant};

use tinyvm::{Val, WasmError};
use tinyvm_qjs::{HostFn, HostParam, HostResult};

use crate::host::{STATUS_ERR, STATUS_OK, arg, bind, contain, guest_slice, guest_slice_mut};
use crate::{Budget, QjswasmError};

/// The module name of this door. `agenterm` is the other.
pub(crate) const DOOR: &str = "tool";

/// How long `process.command` waits for a child that names no `timeout_ms`.
/// A child that never exits would otherwise hold the slot forever, and no
/// core `Limits` field can see it: steps are not spent while the host waits.
pub const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 60_000;

/// Host-authored answers, exempt from the cap like the fleet door's.
const RESULT_TOO_LARGE: &str = "tool: result exceeds the slot's max_bridge_result_bytes";
const NOT_UTF8: &str = "tool: arguments must be UTF-8 text";
const TOOL_PANICKED: &str = "tool door: an operation panicked";

/// The exact raw shape of each import: `(field, params, results)`, all `i32`.
/// The other half of [`declarations`]; a unit test derives one from the other.
pub(crate) const SIGNATURES: [(&str, usize, usize); 20] = [
    ("fs.exists", 2, 1),
    ("fs.read_to_string", 2, 1),
    ("fs.write", 4, 1),
    ("fs.create_dir_all", 2, 1),
    ("fs.remove_file", 2, 1),
    ("fs.read_dir", 2, 1),
    ("fs.metadata", 2, 1),
    ("process.command", 2, 1),
    ("process.id", 0, 1),
    ("env.get", 2, 1),
    ("env.has", 2, 1),
    ("env.cwd", 0, 1),
    ("arg_count", 0, 1),
    ("arg", 1, 1),
    ("crypto.sha256_file", 2, 1),
    ("fs.remove_dir_all", 2, 1),
    ("fs.rename", 4, 1),
    ("fs.copy", 4, 1),
    ("result_len", 0, 1),
    ("result", 2, 1),
];

/// The door as the `.qjs` compiler sees it.
///
/// # Names
///
/// A script writes the field with `.` as `_` -- `fs_exists`, `env_get` -- and
/// `tool_result` for the shared fetch. `HostFn::name` is matched as a bare
/// identifier upstream, so `tool.fs.exists(...)` cannot be a door call; the
/// namespace lives in the wasm import (`tool` / `fs.exists`) and, later, in a
/// `.qjs` wrapper library that gives scripts the `fs.exists(p)` spelling. The
/// raw names stay the raw names underneath it, as they do for the fleet door.
///
/// `result_len` is not a name a script may write: it is the length pass of
/// `tool_result`, and the second pass is the compiler's business.
pub(crate) fn declarations() -> Vec<HostFn> {
    fn decl(field: &str, params: Vec<HostParam>, result: HostResult) -> HostFn {
        HostFn {
            name: field.replace('.', "_"),
            module: DOOR.to_string(),
            field: field.to_string(),
            params,
            result,
        }
    }
    let s = || vec![HostParam::StrPtrLen];
    vec![
        decl("fs.exists", s(), HostResult::I32),
        decl("fs.read_to_string", s(), HostResult::I32),
        decl(
            "fs.write",
            vec![HostParam::StrPtrLen, HostParam::StrPtrLen],
            HostResult::I32,
        ),
        decl("fs.create_dir_all", s(), HostResult::I32),
        decl("fs.remove_file", s(), HostResult::I32),
        decl("fs.read_dir", s(), HostResult::I32),
        decl("fs.metadata", s(), HostResult::I32),
        decl("process.command", s(), HostResult::I32),
        decl("process.id", Vec::new(), HostResult::I32),
        decl("env.get", s(), HostResult::I32),
        decl("env.has", s(), HostResult::I32),
        decl("env.cwd", Vec::new(), HostResult::I32),
        // The invocation's own arguments, through the same two-pass fetch.
        // A tool script is a task entry -- `validate-artifact-manifest.qjs
        // -- scripts/artifacts.json` -- and the engine face cannot carry a
        // string *into* a guest (it has no door onto the guest allocator),
        // so argv arrives the way every other host string does: `arg(n)`
        // parks it, `tool_result()` collects it. `arg_count()` first, so a
        // script can refuse a bad count by name instead of reading `$2` of
        // two and finding `undefined`.
        decl("arg_count", Vec::new(), HostResult::I32),
        decl("arg", vec![HostParam::I32], HostResult::I32),
        // rh's `rh::crypto::sha256_file`, which the build-identity library
        // needs to fingerprint `Cargo.lock` and the artifact manifest. Lower
        // hex, 64 chars, through the two-pass fetch like every other string.
        decl("crypto.sha256_file", vec![HostParam::StrPtrLen], HostResult::I32),
        // The three the shared `test_harness` and `artifact_files` libraries
        // reach for that the first cut of the door did not have. Same shape
        // as `fs.remove_file`: status in, diagnostic (if any) via
        // `tool_result`, nothing parked on success.
        decl("fs.remove_dir_all", vec![HostParam::StrPtrLen], HostResult::I32),
        decl("fs.rename", vec![HostParam::StrPtrLen, HostParam::StrPtrLen], HostResult::I32),
        decl("fs.copy", vec![HostParam::StrPtrLen, HostParam::StrPtrLen], HostResult::I32),
        HostFn {
            name: "tool_result".to_string(),
            module: DOOR.to_string(),
            field: "result".to_string(),
            params: Vec::new(),
            result: HostResult::Bytes {
                length: "result_len".to_string(),
            },
        },
    ]
}

/// One slot's tool-door state, shared by its closures.
pub(crate) struct ToolState {
    /// The most recent parked answer. Survives collection, replaced by the
    /// next parking operation -- the fleet door's rule.
    result: Vec<u8>,
    /// Every operation reached, fully qualified (`tool.fs.read_to_string`),
    /// in call order. Recorded *before* the operation runs, so an operation
    /// that panics is still on the receipt. Drained per call.
    calls: Vec<String>,
    /// A contained panic's message, for `slot.rs` to report as `Door`.
    fault: Option<String>,
    max_result: usize,
    /// The invocation's arguments, set by the embedder before the script
    /// runs. Read through `arg_count` / `arg`; never written by the guest.
    args: Vec<String>,
}

impl ToolState {
    pub(crate) fn take_calls(&mut self) -> Vec<String> {
        std::mem::take(&mut self.calls)
    }

    pub(crate) fn take_fault(&mut self) -> Option<String> {
        self.fault.take()
    }
}

/// Bind whichever `tool.*` imports the guest declares.
pub(crate) fn install(
    module: &mut tinyvm::WasmModule,
    budget: &Budget,
    args: Vec<String>,
) -> Result<Rc<RefCell<ToolState>>, QjswasmError> {
    let shared = Rc::new(RefCell::new(ToolState {
        result: Vec::new(),
        calls: Vec::new(),
        fault: None,
        max_result: budget.max_bridge_result_bytes,
        args,
    }));

    // ---- fs ---------------------------------------------------------------

    let state = Rc::clone(&shared);
    bind(module, DOOR, "fs.exists", move |args, memory| {
        let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        direct(&state, "fs.exists", || {
            Ok(i32::from(
                std::fs::exists(utf8(path)?).map_err(|e| format!("fs.exists: {e}"))?,
            ))
        })
    })?;

    let state = Rc::clone(&shared);
    let max_result = budget.max_bridge_result_bytes;
    bind(module, DOOR, "fs.read_to_string", move |args, memory| {
        let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        answer(&state, "fs.read_to_string", || {
            let path = utf8(path)?;
            // Refuse before allocating, not after: the cap is checked again on
            // the way out, but a multi-gigabyte file should not be read into
            // host memory to discover that it does not fit.
            let len = std::fs::metadata(path)
                .map_err(|e| format!("fs.read_to_string `{path}`: {e}"))?
                .len();
            if len > max_result as u64 {
                return Err(RESULT_TOO_LARGE.to_string());
            }
            std::fs::read_to_string(path).map_err(|e| format!("fs.read_to_string `{path}`: {e}"))
        })
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "fs.write", move |args, memory| {
        let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        let text = guest_slice(memory, arg(args, 2)?, arg(args, 3)?)?;
        answer(&state, "fs.write", || {
            let path = utf8(path)?;
            // The text is carried as bytes: it came from a JS String and so is
            // UTF-8 already, and re-validating would only add a way to fail.
            std::fs::write(path, text).map_err(|e| format!("fs.write `{path}`: {e}"))?;
            Ok(String::new())
        })
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "fs.create_dir_all", move |args, memory| {
        let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        answer(&state, "fs.create_dir_all", || {
            let path = utf8(path)?;
            std::fs::create_dir_all(path)
                .map_err(|e| format!("fs.create_dir_all `{path}`: {e}"))?;
            Ok(String::new())
        })
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "fs.remove_file", move |args, memory| {
        let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        answer(&state, "fs.remove_file", || {
            let path = utf8(path)?;
            std::fs::remove_file(path).map_err(|e| format!("fs.remove_file `{path}`: {e}"))?;
            Ok(String::new())
        })
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "fs.remove_dir_all", move |args, memory| {
        let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        answer(&state, "fs.remove_dir_all", || {
            let path = utf8(path)?;
            std::fs::remove_dir_all(path).map_err(|e| format!("fs.remove_dir_all `{path}`: {e}"))?;
            Ok(String::new())
        })
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "fs.rename", move |args, memory| {
        let from = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        let to = guest_slice(memory, arg(args, 2)?, arg(args, 3)?)?;
        answer(&state, "fs.rename", || {
            let (from, to) = (utf8(from)?, utf8(to)?);
            std::fs::rename(from, to).map_err(|e| format!("fs.rename `{from}` -> `{to}`: {e}"))?;
            Ok(String::new())
        })
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "fs.copy", move |args, memory| {
        let from = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        let to = guest_slice(memory, arg(args, 2)?, arg(args, 3)?)?;
        answer(&state, "fs.copy", || {
            let (from, to) = (utf8(from)?, utf8(to)?);
            std::fs::copy(from, to).map_err(|e| format!("fs.copy `{from}` -> `{to}`: {e}"))?;
            Ok(String::new())
        })
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "fs.read_dir", move |args, memory| {
        let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        answer(&state, "fs.read_dir", || read_dir(utf8(path)?))
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "fs.metadata", move |args, memory| {
        let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        answer(&state, "fs.metadata", || {
            let path = utf8(path)?;
            let meta = std::fs::metadata(path).map_err(|e| format!("fs.metadata `{path}`: {e}"))?;
            Ok(serde_json::json!({
                "is_file": meta.is_file(),
                "is_dir": meta.is_dir(),
                "len": meta.len(),
            })
            .to_string())
        })
    })?;

    // ---- process ----------------------------------------------------------

    let state = Rc::clone(&shared);
    let max_capture = budget.max_bridge_result_bytes;
    bind(module, DOOR, "process.command", move |args, memory| {
        let spec = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        answer(&state, "process.command", || {
            let spec: CommandSpec = serde_json::from_str(utf8(spec)?)
                .map_err(|e| format!("process.command: the spec is not valid: {e}"))?;
            run_command(spec, max_capture)
        })
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "process.id", move |_args, _memory| {
        direct(&state, "process.id", || {
            i32::try_from(std::process::id())
                .map_err(|_| "process.id: the pid does not fit an i32".to_string())
        })
    })?;

    // ---- env --------------------------------------------------------------

    let state = Rc::clone(&shared);
    bind(module, DOOR, "env.get", move |args, memory| {
        let name = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        answer(&state, "env.get", || {
            let name = utf8(name)?;
            match std::env::var(name) {
                Ok(value) => Ok(value),
                // Unset and unreadable are both "no value", and each says
                // which: a script that wants "" for unset has `env_has`.
                Err(std::env::VarError::NotPresent) => Err(format!("env.get: `{name}` is not set")),
                Err(std::env::VarError::NotUnicode(_)) => {
                    Err(format!("env.get: `{name}` is set but is not UTF-8"))
                }
            }
        })
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "env.has", move |args, memory| {
        let name = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        direct(&state, "env.has", || {
            Ok(i32::from(std::env::var_os(utf8(name)?).is_some()))
        })
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "env.cwd", move |_args, _memory| {
        answer(&state, "env.cwd", || {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .map_err(|e| format!("env.cwd: {e}"))
        })
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "crypto.sha256_file", move |args, memory| {
        let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        answer(&state, "crypto.sha256_file", || {
            use sha2::Digest as _;
            let path = utf8(path)?;
            let bytes = std::fs::read(path)
                .map_err(|e| format!("crypto.sha256_file `{path}`: {e}"))?;
            let digest = sha2::Sha256::digest(&bytes);
            Ok(digest.iter().map(|b| format!("{b:02x}")).collect::<String>())
        })
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "arg_count", move |_args, _memory| {
        let n = state.borrow().args.len();
        Ok(vec![Val::I32(n as i32)])
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "arg", move |args, _memory| {
        let index = arg(args, 0)?;
        answer(&state, "arg", || {
            let s = state.borrow();
            usize::try_from(index)
                .ok()
                .and_then(|i| s.args.get(i))
                .cloned()
                .ok_or_else(|| format!("arg: index {index} out of range; arg_count() is {}", s.args.len()))
        })
    })?;

    // ---- the shared fetch, exactly the fleet door's two passes -------------

    let state = Rc::clone(&shared);
    bind(module, DOOR, "result_len", move |_args, _memory| {
        Ok(vec![Val::I32(result_len(&state)?)])
    })?;

    let state = Rc::clone(&shared);
    bind(module, DOOR, "result", move |args, memory| {
        let dst = guest_slice_mut(memory, arg(args, 0)?, arg(args, 1)?)?;
        let needed = result_len(&state)?;
        if (needed as usize) > dst.len() {
            return Ok(vec![Val::I32(-needed)]);
        }
        dst[..needed as usize].copy_from_slice(&state.borrow().result);
        Ok(vec![Val::I32(needed)])
    })?;

    Ok(shared)
}

/// One text-producing operation: record it, run it contained, apply the cap,
/// park the answer, return the status.
fn answer(
    state: &Rc<RefCell<ToolState>>,
    op: &'static str,
    run: impl FnOnce() -> Result<String, String>,
) -> Result<Vec<Val>, WasmError> {
    state.borrow_mut().calls.push(format!("{DOOR}.{op}"));
    let (status, payload) = match contain(
        &format!("the tool door panicked while serving `{DOOR}.{op}`"),
        run,
    ) {
        Ok(Ok(text)) => (STATUS_OK, text.into_bytes()),
        Ok(Err(message)) => (STATUS_ERR, message.into_bytes()),
        Err(panic) => {
            state.borrow_mut().fault = Some(panic);
            return Err(WasmError::Trap(TOOL_PANICKED));
        }
    };
    let mut s = state.borrow_mut();
    // The cap applies to whatever the operation produced, answer or
    // diagnostic alike, and replaces it wholesale rather than cutting it.
    let (status, payload) = if payload.len() > s.max_result {
        (STATUS_ERR, RESULT_TOO_LARGE.as_bytes().to_vec())
    } else {
        (status, payload)
    };
    s.result = payload;
    Ok(vec![Val::I32(status)])
}

/// One operation whose whole answer is an `i32`: record it, run it contained,
/// return the number. On `Err` the answer is `-1` and the diagnostic is parked
/// where `tool_result()` finds it.
fn direct(
    state: &Rc<RefCell<ToolState>>,
    op: &'static str,
    run: impl FnOnce() -> Result<i32, String>,
) -> Result<Vec<Val>, WasmError> {
    state.borrow_mut().calls.push(format!("{DOOR}.{op}"));
    match contain(
        &format!("the tool door panicked while serving `{DOOR}.{op}`"),
        run,
    ) {
        Ok(Ok(value)) => Ok(vec![Val::I32(value)]),
        Ok(Err(message)) => {
            state.borrow_mut().result = message.into_bytes();
            Ok(vec![Val::I32(-1)])
        }
        Err(panic) => {
            state.borrow_mut().fault = Some(panic);
            Err(WasmError::Trap(TOOL_PANICKED))
        }
    }
}

fn utf8(bytes: &[u8]) -> Result<&str, String> {
    str::from_utf8(bytes).map_err(|_| NOT_UTF8.to_string())
}

fn result_len(state: &Rc<RefCell<ToolState>>) -> Result<i32, WasmError> {
    i32::try_from(state.borrow().result.len())
        .map_err(|_| WasmError::Trap("tool door: pending result exceeds i32"))
}

/// `fs.read_dir` as a JSON array sorted by name, so two runs over the same
/// directory answer the same bytes whatever order the OS hands entries out.
fn read_dir(path: &str) -> Result<String, String> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| format!("fs.read_dir `{path}`: {e}"))? {
        let entry = entry.map_err(|e| format!("fs.read_dir `{path}`: {e}"))?;
        let kind = entry
            .file_type()
            .map_err(|e| format!("fs.read_dir `{path}`: {e}"))?;
        entries.push((
            entry.file_name().to_string_lossy().into_owned(),
            entry.path().to_string_lossy().into_owned(),
            kind,
        ));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let items: Vec<serde_json::Value> = entries
        .into_iter()
        .map(|(name, path, kind)| {
            serde_json::json!({
                "name": name,
                "path": path,
                "is_file": kind.is_file(),
                "is_dir": kind.is_dir(),
                "is_symlink": kind.is_symlink(),
            })
        })
        .collect();
    Ok(serde_json::Value::Array(items).to_string())
}

/// What `process.command` takes, as JSON. Unknown fields are refused rather
/// than ignored: a script that writes `timeout` for `timeout_ms` should learn
/// so from the status, not from a child that ran unbounded.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandSpec {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    current_dir: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    timeout_ms: Option<u64>,
    stdin_text: Option<String>,
}

/// Spawn, feed stdin, capture both streams, wait bounded, answer as JSON:
/// `{"exit_code": n | null, "success": bool, "stdout", "stderr", "timed_out"}`.
///
/// `exit_code` is `null` when the child was killed -- by the timeout here or
/// by a signal -- and `success` is then false. Captured streams are UTF-8 with
/// U+FFFD for anything else, like `print`; binary output cannot cross a door
/// that carries text.
fn run_command(spec: CommandSpec, max_capture: usize) -> Result<String, String> {
    use std::process::{Command, Stdio};

    let mut command = Command::new(&spec.program);
    command.args(&spec.args);
    if let Some(dir) = &spec.current_dir {
        command.current_dir(dir);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    command
        .stdin(if spec.stdin_text.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|e| format!("process.command: spawning `{}`: {e}", spec.program))?;

    // Both streams are drained on their own threads so a child that fills one
    // pipe while the host reads the other cannot deadlock, and so the wait
    // below can be bounded without losing what was written.
    let stdin_writer = spec.stdin_text.and_then(|text| {
        child.stdin.take().map(|mut stdin| {
            std::thread::spawn(move || {
                // A child that exits without reading its stdin closes the
                // pipe; that is not the script's problem.
                let _ = stdin.write_all(text.as_bytes());
            })
        })
    });
    let stdout = child
        .stdout
        .take()
        .map(|r| std::thread::spawn(move || drain(r, max_capture)));
    let stderr = child
        .stderr
        .take()
        .map(|r| std::thread::spawn(move || drain(r, max_capture)));

    let timeout = Duration::from_millis(spec.timeout_ms.unwrap_or(DEFAULT_COMMAND_TIMEOUT_MS));
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                timed_out = true;
                let _ = child.kill();
                break child.wait().ok();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => {
                return Err(format!(
                    "process.command: waiting for `{}`: {e}",
                    spec.program
                ));
            }
        }
    };

    if let Some(writer) = stdin_writer {
        let _ = writer.join();
    }
    let collect = |reader: Option<std::thread::JoinHandle<Vec<u8>>>| {
        reader
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
    };
    let stdout = collect(stdout);
    let stderr = collect(stderr);

    let exit_code = if timed_out {
        None
    } else {
        status.and_then(|s| s.code())
    };
    let success = !timed_out && status.is_some_and(|s| s.success());
    Ok(serde_json::json!({
        "exit_code": exit_code,
        "success": success,
        "stdout": String::from_utf8_lossy(&stdout),
        "stderr": String::from_utf8_lossy(&stderr),
        "timed_out": timed_out,
    })
    .to_string())
}

/// Read a pipe to EOF, keeping at most `keep + 1` bytes. The whole stream is
/// consumed so the child never blocks on a full pipe; one byte past the cap
/// is kept so the caller's cap check sees "over", never a prefix that fits.
fn drain(mut reader: impl Read, keep: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => return kept,
            Ok(n) => {
                let room = (keep + 1).saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..n.min(room)]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The declarations and the raw signatures describe one door -- the same
    /// derivation `host.rs` performs for the fleet door.
    #[test]
    fn the_declarations_and_the_raw_signatures_are_one_door() {
        let width = |p: &HostParam| match p {
            HostParam::StrPtrLen => 2,
            HostParam::I32 | HostParam::F64 => 1,
        };
        let mut derived: Vec<(String, usize, usize)> = Vec::new();
        for decl in declarations() {
            assert_eq!(decl.module, DOOR, "`{}` is not on this door", decl.name);
            let raw: usize = decl.params.iter().map(width).sum();
            match &decl.result {
                HostResult::Void => derived.push((decl.field.clone(), raw, 0)),
                HostResult::I32 | HostResult::F64 => derived.push((decl.field.clone(), raw, 1)),
                HostResult::Bytes { length } => {
                    derived.push((length.clone(), raw, 1));
                    derived.push((decl.field.clone(), raw + 2, 1));
                }
            }
        }
        let mut expected: Vec<(String, usize, usize)> = SIGNATURES
            .iter()
            .map(|(field, p, r)| ((*field).to_string(), *p, *r))
            .collect();
        derived.sort();
        expected.sort();
        assert_eq!(derived, expected);
    }

    /// Script names are identifiers, distinct from each other and from the
    /// fleet door's, so the two tables can be declared together.
    #[test]
    fn script_names_are_distinct_identifiers_and_do_not_shadow_the_fleet_door() {
        let mut seen = std::collections::BTreeSet::new();
        for decl in declarations() {
            assert!(
                decl.name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "`{}` is not an identifier",
                decl.name
            );
            assert!(
                seen.insert(decl.name.clone()),
                "`{}` declared twice",
                decl.name
            );
        }
        for fleet in crate::host::declarations() {
            assert!(
                !seen.contains(&fleet.name),
                "`{}` shadows the fleet door",
                fleet.name
            );
        }
    }

    #[test]
    fn an_unknown_spec_field_is_refused() {
        let err = serde_json::from_str::<CommandSpec>(r#"{"program":"x","timeout":5}"#)
            .expect_err("`timeout` is not a field");
        assert!(err.to_string().contains("timeout"), "{err}");
    }

    #[test]
    fn drain_keeps_one_byte_past_the_cap_and_consumes_the_rest() {
        let data = vec![b'x'; 100_000];
        let kept = drain(&data[..], 10);
        assert_eq!(kept.len(), 11);
    }

    #[cfg(unix)]
    #[test]
    fn a_child_past_its_timeout_is_killed_and_says_so() {
        let spec = CommandSpec {
            program: "sh".into(),
            args: vec!["-c".into(), "sleep 30".into()],
            current_dir: None,
            env: BTreeMap::new(),
            timeout_ms: Some(50),
            stdin_text: None,
        };
        let started = Instant::now();
        let json = run_command(spec, 1 << 20).expect("the command ran");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the timeout was honoured"
        );
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["timed_out"], true);
        assert_eq!(v["success"], false);
        assert!(v["exit_code"].is_null());
    }
}
