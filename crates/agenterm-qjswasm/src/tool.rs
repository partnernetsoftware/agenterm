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
//!
//! Every operation is one `host_ops` on the call's bill, charged at
//! `bind_metered` before it runs, together with the bytes of its string
//! arguments; what it parks is charged as `host_bytes` when parked. So the
//! bill counts both directions through the door, and `tool_result()` --
//! which only moves an answer already paid for -- adds operations but no
//! bytes.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tinyvm::{Val, WasmError};
use tinyvm_qjs::{HostFn, HostParam, HostResult};

use crate::host::{
    STATUS_ERR, STATUS_OK, arg, bind_metered, contain, guest_slice, guest_slice_mut,
};
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
pub(crate) const SIGNATURES: [(&str, usize, usize); 42] = [
    ("fs.exists", 2, 1),
    ("fs.read_to_string", 2, 1),
    ("fs.write", 4, 1),
    ("fs.append", 4, 1),
    ("fs.try_lock_exclusive", 2, 1),
    ("fs.unlock", 1, 1),
    ("fs.create_dir_all", 2, 1),
    ("fs.remove_file", 2, 1),
    ("fs.read_dir", 2, 1),
    ("fs.metadata", 2, 1),
    ("process.command", 2, 1),
    ("process.command_stdout", 2, 1),
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
    ("time.now_ms", 0, 1),
    ("time.sleep_ms", 1, 1),
    ("process.spawn", 2, 1),
    ("process.state", 1, 1),
    ("process.kill", 1, 1),
    ("process.wait", 2, 1),
    ("process.pid", 1, 1),
    ("process.read", 2, 1),
    ("process.platform_facts", 1, 1),
    ("process.window_key", 3, 1),
    ("process.window_pointer", 3, 1),
    ("process.window_message", 3, 1),
    ("process.window_resize", 3, 1),
    ("process.window_rect", 2, 1),
    ("process.window_control", 3, 1),
    ("image.inspect_png", 2, 1),
    ("fs.symlink_metadata", 2, 1),
    ("process.status", 2, 1),
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
        decl(
            "fs.append",
            vec![HostParam::StrPtrLen, HostParam::StrPtrLen],
            HostResult::I32,
        ),
        decl("fs.try_lock_exclusive", s(), HostResult::I32),
        decl("fs.unlock", vec![HostParam::I32], HostResult::I32),
        decl("fs.create_dir_all", s(), HostResult::I32),
        decl("fs.remove_file", s(), HostResult::I32),
        decl("fs.read_dir", s(), HostResult::I32),
        decl("fs.metadata", s(), HostResult::I32),
        decl("process.command", s(), HostResult::I32),
        decl("process.command_stdout", s(), HostResult::I32),
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
        decl(
            "crypto.sha256_file",
            vec![HostParam::StrPtrLen],
            HostResult::I32,
        ),
        // The three the shared `test_harness` and `artifact_files` libraries
        // reach for that the first cut of the door did not have. Same shape
        // as `fs.remove_file`: status in, diagnostic (if any) via
        // `tool_result`, nothing parked on success.
        decl(
            "fs.remove_dir_all",
            vec![HostParam::StrPtrLen],
            HostResult::I32,
        ),
        decl(
            "fs.rename",
            vec![HostParam::StrPtrLen, HostParam::StrPtrLen],
            HostResult::I32,
        ),
        decl(
            "fs.copy",
            vec![HostParam::StrPtrLen, HostParam::StrPtrLen],
            HostResult::I32,
        ),
        // Wall-clock milliseconds since the epoch, as decimal text through the
        // two-pass fetch: the value does not fit an i32 and this door carries
        // text, so `Number(tool_result())` is the script's spelling. The one
        // host fact `test_harness` needs that nothing else provided -- it
        // names run directories with it. Deliberately the only clock: a
        // sandbox script has none, and a tool script's receipt records that
        // it asked.
        decl("time.now_ms", Vec::new(), HostResult::I32),
        // Long-lived children. 29 of the 71 rh scripts start a server or a
        // GUI, poll it, and kill it at the end -- `process.command` runs to
        // completion and cannot express that. rh's model, kept exactly:
        // `spawn` takes the same JSON spec and answers a handle; `state` is
        // "running" | "exited" | "unknown" via try_wait; `kill` is
        // best-effort; `wait(h, timeout_ms)` drains both pipes and answers
        // the same JSON `command` does. Handles are per slot and every child
        // still running when the slot drops is killed -- a tool script does
        // not get to leave orphans, which is what rh's own gates assert.
        //
        // `kill` is the process, not its tree, exactly as rh's `kill` was (rh
        // had a separate `kill_tree`). A script that spawns `sh -c "…"` and
        // kills the handle kills the shell; what the shell started is the
        // script's to know about. Spawn the real program when the handle is
        // what you mean to own.
        decl("time.sleep_ms", vec![HostParam::I32], HostResult::I32),
        decl("process.spawn", vec![HostParam::StrPtrLen], HostResult::I32),
        decl("process.state", vec![HostParam::I32], HostResult::I32),
        decl("process.kill", vec![HostParam::I32], HostResult::I32),
        decl(
            "process.wait",
            vec![HostParam::I32, HostParam::I32],
            HostResult::I32,
        ),
        decl("process.pid", vec![HostParam::I32], HostResult::I32),
        decl(
            "process.platform_facts",
            vec![HostParam::I32],
            HostResult::I32,
        ),
        decl(
            "process.window_key",
            vec![HostParam::I32, HostParam::StrPtrLen],
            HostResult::I32,
        ),
        decl(
            "process.window_pointer",
            vec![HostParam::I32, HostParam::StrPtrLen],
            HostResult::I32,
        ),
        decl(
            "process.window_message",
            vec![HostParam::I32, HostParam::StrPtrLen],
            HostResult::I32,
        ),
        decl(
            "process.window_resize",
            vec![HostParam::I32, HostParam::StrPtrLen],
            HostResult::I32,
        ),
        decl(
            "process.window_rect",
            vec![HostParam::I32, HostParam::I32],
            HostResult::I32,
        ),
        decl(
            "process.window_control",
            vec![HostParam::I32, HostParam::StrPtrLen],
            HostResult::I32,
        ),
        decl(
            "image.inspect_png",
            vec![HostParam::StrPtrLen],
            HostResult::I32,
        ),
        decl(
            "process.read",
            vec![HostParam::I32, HostParam::I32],
            HostResult::I32,
        ),
        // `symlink_metadata` does not follow the link, so `is_symlink` is
        // answerable -- rh's gates use it to refuse a manifest that is a link.
        // `process.status` is `command` without capture: exit code only, for
        // the 15 call sites that run a tool for its side effect and would
        // otherwise pay to buffer output nobody reads.
        decl(
            "fs.symlink_metadata",
            vec![HostParam::StrPtrLen],
            HostResult::I32,
        ),
        decl(
            "process.status",
            vec![HostParam::StrPtrLen],
            HostResult::I32,
        ),
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

/// The raw parameter positions that carry a byte length -- the second word
/// of every `StrPtrLen` in `field`'s declaration. `bind_metered` sums these
/// into `host_bytes`, so the bill charges what a script *sends* through the
/// door as well as what it collects; `tool_result`'s landing buffer is not a
/// send and is not on the list.
pub(crate) fn argument_length_slots(field: &str) -> Vec<usize> {
    let Some(decl) = declarations().into_iter().find(|d| d.field == field) else {
        return Vec::new();
    };
    let mut raw = 0;
    let mut slots = Vec::new();
    for param in &decl.params {
        match param {
            HostParam::StrPtrLen => {
                slots.push(raw + 1);
                raw += 2;
            }
            HostParam::I32 | HostParam::F64 => raw += 1,
        }
    }
    slots
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
    /// Children started by `process.spawn`, by handle. `None` once waited.
    children: Vec<Handle>,
    /// Advisory locks held by handle index; released on `fs.unlock` or when
    /// the state drops. rh's `fs_try_lock_exclusive` backed the `.cargo-lock`
    /// pre-flight and the hold-every-`.lock`-while-removing protocol of
    /// prune-target-incremental (wave 3).
    locks: Vec<Option<std::fs::File>>,
    /// The call's host-side bill, shared with the `agenterm.*` door.
    meter: Rc<RefCell<crate::host::Meter>>,
    /// `Budget::cancel`, for the operations that wait.
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// One spawned child, by handle index. A waited child keeps its pid and
/// replays its first answer: rh scripts `wait_with_output` a child they
/// already reaped, and `complete` re-waits every owned handle -- wave 2 met
/// "handle N was already waited" in every group that waits its own server.
enum Handle {
    Running(Running),
    Done {
        pid: u32,
        answer: Result<String, String>,
    },
}

/// A child that has not been waited: its drains run from the moment it is
/// spawned, so `process.read` can hand out what has arrived so far and a
/// chatty long-lived server never blocks on a full pipe. `timeout_ms` from
/// the spec is a deadline `process.state`, `process.read` and `process.wait`
/// all enforce -- rh gave `.start()` children a 15/22 s cap and wave 2 found
/// the door ignored it.
struct Running {
    child: std::process::Child,
    drains: Drains,
    started: Instant,
    deadline: Option<Duration>,
    killed_by_deadline: bool,
    read_stdout: usize,
    read_stderr: usize,
}

impl Running {
    /// Kill the child once its deadline has passed; true if it did.
    fn enforce_deadline(&mut self) -> bool {
        if !self.killed_by_deadline && self.deadline.is_some_and(|d| self.started.elapsed() >= d) {
            let _ = self.child.kill();
            // Reap it now, so the very next `try_wait` sees the exit rather
            // than a SIGKILLed process the kernel has not yet collected.
            let _ = self.child.wait();
            self.killed_by_deadline = true;
        }
        self.killed_by_deadline
    }
}

/// The two capture buffers a child's pipes drain into, from spawn on.
struct Drains {
    stdout: Option<Arc<Mutex<Vec<u8>>>>,
    stderr: Option<Arc<Mutex<Vec<u8>>>>,
}

impl Drains {
    fn start(child: &mut std::process::Child, max_capture: usize) -> Self {
        fn drain<R: std::io::Read + Send + 'static>(
            pipe: Option<R>,
            max_capture: usize,
        ) -> Option<Arc<Mutex<Vec<u8>>>> {
            pipe.map(|mut pipe| {
                let buf = Arc::new(Mutex::new(Vec::new()));
                let sink = Arc::clone(&buf);
                std::thread::spawn(move || {
                    let mut chunk = [0u8; 4096];
                    let mut total = 0usize;
                    while let Ok(n) = pipe.read(&mut chunk) {
                        if n == 0 {
                            break;
                        }
                        let take = n.min(max_capture.saturating_sub(total));
                        if take > 0 {
                            if let Ok(mut b) = sink.lock() {
                                b.extend_from_slice(&chunk[..take]);
                            }
                            total += take;
                        }
                    }
                });
                buf
            })
        }
        Self {
            stdout: drain(child.stdout.take(), max_capture),
            stderr: drain(child.stderr.take(), max_capture),
        }
    }

    /// Bytes from `from` onward, as text; and where the buffer ends now.
    fn since(buf: &Option<Arc<Mutex<Vec<u8>>>>, from: usize) -> (String, usize) {
        let Some(buf) = buf else {
            return (String::new(), from);
        };
        let b = buf.lock().map(|b| b.clone()).unwrap_or_default();
        let end = b.len().max(from);
        (
            String::from_utf8_lossy(&b[from.min(b.len())..]).into_owned(),
            end,
        )
    }
}

impl Drop for ToolState {
    /// No orphans: a child the script never waited for is killed with the
    /// slot. rh's gates assert `orphan_free` in their cleanup manifest, and
    /// the door has to make that true rather than trust every script to.
    fn drop(&mut self) {
        for handle in &mut self.children {
            if let Handle::Running(r) = handle {
                let _ = r.child.kill();
                let _ = r.child.wait();
            }
        }
    }
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
    meter: Rc<RefCell<crate::host::Meter>>,
) -> Result<Rc<RefCell<ToolState>>, QjswasmError> {
    let shared = Rc::new(RefCell::new(ToolState {
        result: Vec::new(),
        calls: Vec::new(),
        fault: None,
        max_result: budget.max_bridge_result_bytes,
        args,
        children: Vec::new(),
        locks: Vec::new(),
        meter: Rc::clone(&meter),
        cancel: budget.cancel.clone(),
    }));

    // ---- fs ---------------------------------------------------------------

    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "fs.exists", move |args, memory| {
        let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        direct(&state, "fs.exists", || {
            Ok(i32::from(
                std::fs::exists(utf8(path)?).map_err(|e| format!("fs.exists: {e}"))?,
            ))
        })
    })?;

    let state = Rc::clone(&shared);
    let max_result = budget.max_bridge_result_bytes;
    bind_metered(
        module,
        &meter,
        DOOR,
        "fs.read_to_string",
        move |args, memory| {
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
                std::fs::read_to_string(path)
                    .map_err(|e| format!("fs.read_to_string `{path}`: {e}"))
            })
        },
    )?;

    // `fs.try_lock_exclusive(path) -> handle | -1`: an advisory exclusive lock
    // on the file (created if absent), or -1 when another holder has it.
    // `fs.unlock(handle)` releases it; dropping the state releases the rest.
    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "fs.try_lock_exclusive",
        move |args, memory| {
            let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
            direct(&state, "fs.try_lock_exclusive", || {
                let path = utf8(path)?;
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(path)
                    .map_err(|e| format!("fs.try_lock_exclusive `{path}`: {e}"))?;
                match file.try_lock() {
                    Ok(()) => {
                        let mut s = state.borrow_mut();
                        s.locks.push(Some(file));
                        i32::try_from(s.locks.len() - 1)
                            .map_err(|_| "fs.try_lock_exclusive: too many locks".to_string())
                    }
                    Err(std::fs::TryLockError::WouldBlock) => Ok(-1),
                    Err(std::fs::TryLockError::Error(e)) => {
                        Err(format!("fs.try_lock_exclusive `{path}`: {e}"))
                    }
                }
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "fs.unlock", move |args, _memory| {
        let h = arg(args, 0)?;
        direct(&state, "fs.unlock", || {
            let mut s = state.borrow_mut();
            match usize::try_from(h).ok().and_then(|i| s.locks.get_mut(i)) {
                Some(slot @ Some(_)) => {
                    // Dropping the file releases the lock.
                    *slot = None;
                    Ok(0)
                }
                Some(None) => Ok(0),
                None => Err(format!("fs.unlock: no lock with handle {h}")),
            }
        })
    })?;

    // `fs.append(path, text)`: the journal case. test_harness re-read and
    // rewrote its commands.jsonl on every record -- O(n^2) in string copies,
    // 15-20M steps of a journey's budget by the 35th record (wave 3).
    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "fs.append", move |args, memory| {
        let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        let text = guest_slice(memory, arg(args, 2)?, arg(args, 3)?)?;
        answer(&state, "fs.append", || {
            use std::io::Write as _;
            let path = utf8(path)?;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| format!("fs.append `{path}`: {e}"))?;
            file.write_all(text)
                .map_err(|e| format!("fs.append `{path}`: {e}"))?;
            Ok(String::new())
        })
    })?;

    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "fs.write", move |args, memory| {
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
    bind_metered(
        module,
        &meter,
        DOOR,
        "fs.create_dir_all",
        move |args, memory| {
            let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
            answer(&state, "fs.create_dir_all", || {
                let path = utf8(path)?;
                std::fs::create_dir_all(path)
                    .map_err(|e| format!("fs.create_dir_all `{path}`: {e}"))?;
                Ok(String::new())
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "fs.remove_file",
        move |args, memory| {
            let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
            answer(&state, "fs.remove_file", || {
                let path = utf8(path)?;
                std::fs::remove_file(path).map_err(|e| format!("fs.remove_file `{path}`: {e}"))?;
                Ok(String::new())
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "fs.remove_dir_all",
        move |args, memory| {
            let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
            answer(&state, "fs.remove_dir_all", || {
                let path = utf8(path)?;
                std::fs::remove_dir_all(path)
                    .map_err(|e| format!("fs.remove_dir_all `{path}`: {e}"))?;
                Ok(String::new())
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "fs.rename", move |args, memory| {
        let from = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        let to = guest_slice(memory, arg(args, 2)?, arg(args, 3)?)?;
        answer(&state, "fs.rename", || {
            let (from, to) = (utf8(from)?, utf8(to)?);
            std::fs::rename(from, to).map_err(|e| format!("fs.rename `{from}` -> `{to}`: {e}"))?;
            Ok(String::new())
        })
    })?;

    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "fs.copy", move |args, memory| {
        let from = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        let to = guest_slice(memory, arg(args, 2)?, arg(args, 3)?)?;
        answer(&state, "fs.copy", || {
            let (from, to) = (utf8(from)?, utf8(to)?);
            std::fs::copy(from, to).map_err(|e| format!("fs.copy `{from}` -> `{to}`: {e}"))?;
            Ok(String::new())
        })
    })?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "fs.symlink_metadata",
        move |args, memory| {
            let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
            answer(&state, "fs.symlink_metadata", || {
                let path = utf8(path)?;
                let meta = std::fs::symlink_metadata(path)
                    .map_err(|e| format!("fs.symlink_metadata `{path}`: {e}"))?;
                Ok(serde_json::json!({
                    "is_file": meta.is_file(),
                    "is_dir": meta.is_dir(),
                    "is_symlink": meta.file_type().is_symlink(),
                    "len": meta.len(),
                    // Milliseconds since the Unix epoch, or null where the
                    // filesystem has no modification time. `target-report`
                    // (oldest/newest write, age) was the one rh script the door
                    // could not carry without it.
                    "modified_ms": modified_ms(&meta),
                })
                .to_string())
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.status",
        move |args, memory| {
            let spec = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
            let cancel = state.borrow().cancel.clone();
            direct(&state, "process.status", || {
                let spec: CommandSpec = serde_json::from_str(utf8(spec)?)
                    .map_err(|e| format!("process.status: the spec is not valid: {e}"))?;
                let timeout = spec.timeout_ms.map(Duration::from_millis);
                // Not captured: spawned with null pipes, so a chatty child
                // neither blocks on a pipe nobody reads nor dies of SIGPIPE on
                // one that was dropped.
                let mut child = spawn_command(&spec, false)?;
                let started = Instant::now();
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) => return Ok(status.code().unwrap_or(-1)),
                        Ok(None) if timeout.is_some_and(|t| started.elapsed() >= t) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err("process.status: timed out".to_string());
                        }
                        Ok(None) => {
                            // A cancel kills the child too: no orphans.
                            if cancellable_sleep(&cancel, Duration::from_millis(5)).is_err() {
                                let _ = child.kill();
                                let _ = child.wait();
                                return Err(CANCELLED_MARK.to_string());
                            }
                        }
                        Err(e) => return Err(format!("process.status: {e}")),
                    }
                }
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "fs.read_dir", move |args, memory| {
        let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        answer(&state, "fs.read_dir", || read_dir(utf8(path)?))
    })?;

    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "fs.metadata", move |args, memory| {
        let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        answer(&state, "fs.metadata", || {
            let path = utf8(path)?;
            let meta = std::fs::metadata(path).map_err(|e| format!("fs.metadata `{path}`: {e}"))?;
            Ok(serde_json::json!({
                "is_file": meta.is_file(),
                "is_dir": meta.is_dir(),
                "len": meta.len(),
                "modified_ms": modified_ms(&meta),
            })
            .to_string())
        })
    })?;

    // ---- process ----------------------------------------------------------

    let state = Rc::clone(&shared);
    let max_capture = budget.max_bridge_result_bytes;
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.command",
        move |args, memory| {
            let spec = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
            let cancel = state.borrow().cancel.clone();
            answer(&state, "process.command", || {
                let spec: CommandSpec = serde_json::from_str(utf8(spec)?)
                    .map_err(|e| format!("process.command: the spec is not valid: {e}"))?;
                run_command(spec, max_capture, &cancel)
            })
        },
    )?;

    // `process.command_stdout(spec)`: the child's stdout as the parked bytes,
    // no envelope. A script that only wants the text paid ~81 steps a byte
    // to `JSON.parse` the answer around it (wave 3). Status 0 only when the
    // command succeeded; otherwise the parked bytes are the usual envelope
    // (`exit_code`, `stderr`, `timed_out`) so the failure is still legible.
    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.command_stdout",
        move |args, memory| {
            let spec = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
            let cancel = state.borrow().cancel.clone();
            answer(&state, "process.command_stdout", || {
                let spec: CommandSpec = serde_json::from_str(utf8(spec)?)
                    .map_err(|e| format!("process.command_stdout: the spec is not valid: {e}"))?;
                let envelope = run_command(spec, max_capture, &cancel)?;
                let parsed: serde_json::Value = serde_json::from_str(&envelope)
                    .map_err(|e| format!("process.command_stdout: {e}"))?;
                if parsed["success"].as_bool() == Some(true) {
                    Ok(parsed["stdout"].as_str().unwrap_or_default().to_string())
                } else {
                    Err(envelope)
                }
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "process.id", move |_args, _memory| {
        direct(&state, "process.id", || {
            i32::try_from(std::process::id())
                .map_err(|_| "process.id: the pid does not fit an i32".to_string())
        })
    })?;

    // ---- env --------------------------------------------------------------

    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "env.get", move |args, memory| {
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
    bind_metered(module, &meter, DOOR, "env.has", move |args, memory| {
        let name = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        direct(&state, "env.has", || {
            Ok(i32::from(std::env::var_os(utf8(name)?).is_some()))
        })
    })?;

    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "env.cwd", move |_args, _memory| {
        answer(&state, "env.cwd", || {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .map_err(|e| format!("env.cwd: {e}"))
        })
    })?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "time.sleep_ms",
        move |args, _memory| {
            let ms = arg(args, 0)?;
            let cancel = state.borrow().cancel.clone();
            direct(&state, "time.sleep_ms", || {
                let ms = u64::try_from(ms).map_err(|_| "time.sleep_ms: negative".to_string())?;
                cancellable_sleep(&cancel, Duration::from_millis(ms.min(60_000)))?;
                Ok(0)
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.spawn",
        move |args, memory| {
            let spec = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
            direct(&state, "process.spawn", || {
                let spec: CommandSpec = serde_json::from_str(utf8(spec)?)
                    .map_err(|e| format!("process.spawn: the spec is not valid: {e}"))?;
                let mut child = spawn_command(&spec, true)?;
                let drains = Drains::start(&mut child, max_capture);
                let mut s = state.borrow_mut();
                s.children.push(Handle::Running(Running {
                    child,
                    drains,
                    started: Instant::now(),
                    deadline: spec.timeout_ms.map(Duration::from_millis),
                    killed_by_deadline: false,
                    read_stdout: 0,
                    read_stderr: 0,
                }));
                i32::try_from(s.children.len() - 1)
                    .map_err(|_| "process.spawn: too many children".to_string())
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.state",
        move |args, _memory| {
            let h = arg(args, 0)?;
            answer(&state, "process.state", || {
                let mut s = state.borrow_mut();
                let slot = usize::try_from(h).ok().and_then(|i| s.children.get_mut(i));
                Ok(match slot {
                    Some(Handle::Running(r)) => {
                        r.enforce_deadline();
                        match r.child.try_wait() {
                            Ok(Some(_)) => "exited",
                            Ok(None) => "running",
                            Err(_) => "unknown",
                        }
                    }
                    Some(Handle::Done { .. }) => "exited",
                    None => return Err(format!("process.state: no child with handle {h}")),
                }
                .to_string())
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.kill",
        move |args, _memory| {
            let h = arg(args, 0)?;
            direct(&state, "process.kill", || {
                let mut s = state.borrow_mut();
                match usize::try_from(h).ok().and_then(|i| s.children.get_mut(i)) {
                    Some(Handle::Running(r)) => {
                        let _ = r.child.kill();
                        Ok(0)
                    }
                    Some(Handle::Done { .. }) => Ok(0),
                    None => Err(format!("process.kill: no child with handle {h}")),
                }
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.wait",
        move |args, _memory| {
            let h = arg(args, 0)?;
            let timeout_ms = arg(args, 1)?;
            let cancel = state.borrow().cancel.clone();
            answer(&state, "process.wait", || {
                let index = usize::try_from(h).ok();
                // Take the child out for the wait; the slot holds its pid and,
                // afterwards, its answer, so a second wait replays the first.
                let child = {
                    let mut s = state.borrow_mut();
                    let slot = index
                        .and_then(|i| s.children.get_mut(i))
                        .ok_or_else(|| format!("process.wait: no child with handle {h}"))?;
                    match slot {
                        Handle::Done { answer, .. } => return answer.clone(),
                        Handle::Running(r) => {
                            let pid = r.child.id();
                            let taken = std::mem::replace(
                                slot,
                                Handle::Done {
                                    pid,
                                    answer: Err(format!(
                                        "process.wait: handle {h} is being waited"
                                    )),
                                },
                            );
                            match taken {
                                Handle::Running(r) => r,
                                Handle::Done { .. } => unreachable!("just matched Running"),
                            }
                        }
                    }
                };
                // The wait's own timeout, capped by whatever is left of the
                // spawn deadline.
                let timeout = u64::try_from(timeout_ms).ok().map(Duration::from_millis);
                let remaining = child
                    .deadline
                    .map(|d| d.saturating_sub(child.started.elapsed()));
                let timeout = match (timeout, remaining) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                let answer = finish_child(child, timeout, &cancel);
                {
                    let mut s = state.borrow_mut();
                    if let Some(Handle::Done { answer: kept, .. }) =
                        index.and_then(|i| s.children.get_mut(i))
                    {
                        *kept = answer.clone();
                    }
                }
                answer
            })
        },
    )?;

    // `process.pid(handle)`: the child's OS pid, before and after the wait.
    // rh's `child.id` backed ~40 identity checks in the smoke scripts
    // (`protocol.pid == server.id`); a handle is a slot index, so without
    // this every port needed a `sh -c 'printf $$'` wrapper or `pgrep -f`.
    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "process.pid", move |args, _memory| {
        let h = arg(args, 0)?;
        direct(&state, "process.pid", || {
            let s = state.borrow();
            match usize::try_from(h).ok().and_then(|i| s.children.get(i)) {
                Some(Handle::Running(r)) => Ok(i32::try_from(r.child.id()).unwrap_or(i32::MAX)),
                Some(Handle::Done { pid, .. }) => Ok(i32::try_from(*pid).unwrap_or(i32::MAX)),
                None => Err(format!("process.pid: no child with handle {h}")),
            }
        })
    })?;

    // The process-window ops: rh's `child.platform_facts` and
    // `child.window_*`, which the GUI journeys (startup, unix-frontend,
    // workbench, theme) stop on without. The platform crate owns the
    // contract and the adapters; the door maps a handle to a pid and JSON to
    // the contract types, and answers the contract's typed error as
    // `<op>: <message> (<code>[, <cause>])`.
    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.platform_facts",
        move |args, _memory| {
            let h = arg(args, 0)?;
            answer(&state, "process.platform_facts", || {
                let pid = child_pid(&state.borrow(), h, "process.platform_facts")?;
                let f = agenterm_platform::process_window::facts(pid);
                Ok(serde_json::json!({
                    "top_level_window_supported": f.supported,
                    "top_level_window_present": f.present,
                    "top_level_window_id": f.window_id,
                    "top_level_window_title": f.title,
                    "foreground_window_id": f.foreground_window_id,
                    "top_level_window_is_foreground": f.is_foreground,
                })
                .to_string())
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.window_key",
        move |args, memory| {
            let h = arg(args, 0)?;
            let name = guest_slice(memory, arg(args, 1)?, arg(args, 2)?)?;
            direct(&state, "process.window_key", || {
                let pid = child_pid(&state.borrow(), h, "process.window_key")?;
                let key = window_key_named(utf8(name)?)?;
                agenterm_platform::process_window::key(pid, key)
                    .map_err(|e| window_error("process.window_key", e))?;
                Ok(STATUS_OK)
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.window_pointer",
        move |args, memory| {
            let h = arg(args, 0)?;
            let spec = guest_slice(memory, arg(args, 1)?, arg(args, 2)?)?;
            direct(&state, "process.window_pointer", || {
                let pid = child_pid(&state.borrow(), h, "process.window_pointer")?;
                let spec: PointerSpec = serde_json::from_str(utf8(spec)?)
                    .map_err(|e| format!("process.window_pointer: the spec is not valid: {e}"))?;
                let action = pointer_action_named(&spec.action)?;
                agenterm_platform::process_window::pointer(pid, action, spec.x, spec.y)
                    .map_err(|e| window_error("process.window_pointer", e))?;
                Ok(STATUS_OK)
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.window_message",
        move |args, memory| {
            let h = arg(args, 0)?;
            let spec = guest_slice(memory, arg(args, 1)?, arg(args, 2)?)?;
            answer(&state, "process.window_message", || {
                let pid = child_pid(&state.borrow(), h, "process.window_message")?;
                let spec: MessageSpec = serde_json::from_str(utf8(spec)?)
                    .map_err(|e| format!("process.window_message: the spec is not valid: {e}"))?;
                let message = agenterm_platform::contract::process_window::ProcessWindowMessage {
                    message: spec.message,
                    wparam: usize::try_from(spec.wparam)
                        .map_err(|_| "process.window_message: wparam is negative".to_string())?,
                    lparam: spec.lparam,
                };
                let result = agenterm_platform::process_window::message(pid, message)
                    .map_err(|e| window_error("process.window_message", e))?;
                Ok(serde_json::json!({ "result": result }).to_string())
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.window_resize",
        move |args, memory| {
            let h = arg(args, 0)?;
            let spec = guest_slice(memory, arg(args, 1)?, arg(args, 2)?)?;
            direct(&state, "process.window_resize", || {
                let pid = child_pid(&state.borrow(), h, "process.window_resize")?;
                let spec: ResizeSpec = serde_json::from_str(utf8(spec)?)
                    .map_err(|e| format!("process.window_resize: the spec is not valid: {e}"))?;
                agenterm_platform::process_window::resize(pid, spec.width, spec.height)
                    .map_err(|e| window_error("process.window_resize", e))?;
                Ok(STATUS_OK)
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.window_rect",
        move |args, _memory| {
            let h = arg(args, 0)?;
            let client = arg(args, 1)? != 0;
            answer(&state, "process.window_rect", || {
                let pid = child_pid(&state.borrow(), h, "process.window_rect")?;
                let r = agenterm_platform::process_window::rect(pid, client)
                    .map_err(|e| window_error("process.window_rect", e))?;
                Ok(serde_json::json!({
                    "left": r.left, "top": r.top, "right": r.right, "bottom": r.bottom,
                    "width": r.right - r.left, "height": r.bottom - r.top,
                })
                .to_string())
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.window_control",
        move |args, memory| {
            let h = arg(args, 0)?;
            let spec = guest_slice(memory, arg(args, 1)?, arg(args, 2)?)?;
            answer(&state, "process.window_control", || {
                let pid = child_pid(&state.borrow(), h, "process.window_control")?;
                let spec: ControlSpec = serde_json::from_str(utf8(spec)?)
                    .map_err(|e| format!("process.window_control: the spec is not valid: {e}"))?;
                use agenterm_platform::process_window as w;
                let err = |e| window_error("process.window_control", e);
                let answer = match spec.op.as_str() {
                    "exists" => {
                        w::control_exists(pid, spec.id).map_err(err)?;
                        serde_json::json!({})
                    }
                    "visible" => {
                        serde_json::json!({ "visible": w::control_visible(pid, spec.id).map_err(err)? })
                    }
                    "text" => {
                        serde_json::json!({ "text": w::control_text(pid, spec.id).map_err(err)? })
                    }
                    "set_text" => {
                        w::control_set_text(pid, spec.id, spec.text.as_deref().unwrap_or_default())
                            .map_err(err)?;
                        serde_json::json!({})
                    }
                    "click" => {
                        w::control_click(pid, spec.id).map_err(err)?;
                        serde_json::json!({})
                    }
                    other => {
                        return Err(format!(
                            "process.window_control: no op named `{other}`; exists, visible, text, set_text, click"
                        ));
                    }
                };
                Ok(answer.to_string())
            })
        },
    )?;

    // `image.inspect_png(path)`: rh's `rh::image::inspect_png`, which the GUI
    // journeys use to read the evidence screenshots they took back -- not the
    // pixels (binary bytes do not cross this door) but what a test asserts on:
    // `{width, height, samples, luminance}`, where `samples` is the number
    // of pixels read and `luminance` their mean Rec. 601 luma, 0..255.
    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "image.inspect_png",
        move |args, memory| {
            let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
            answer(&state, "image.inspect_png", || {
                let path = utf8(path)?;
                let file = std::fs::File::open(path)
                    .map_err(|e| format!("image.inspect_png: cannot open `{path}`: {e}"))?;
                let facts = inspect_png(std::io::BufReader::new(file))
                    .map_err(|e| format!("image.inspect_png: `{path}`: {e}"))?;
                Ok(serde_json::json!({
                    "width": facts.width, "height": facts.height,
                    "samples": facts.samples, "luminance": facts.luminance,
                })
                .to_string())
            })
        },
    )?;

    // `process.read(handle, max_bytes)`: what the child has written since
    // the last read, without waiting -- rh's `child.stdout.read(4096, 2s)`,
    // minus the blocking (a script polls with `time_sleep_ms`). The
    // answer is JSON `{stdout, stderr, state}`; `process.wait` still answers
    // the whole capture.
    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "process.read",
        move |args, _memory| {
            let h = arg(args, 0)?;
            let max_bytes = usize::try_from(arg(args, 1)?).unwrap_or(usize::MAX);
            answer(&state, "process.read", || {
                let mut s = state.borrow_mut();
                match usize::try_from(h).ok().and_then(|i| s.children.get_mut(i)) {
                    Some(Handle::Running(r)) => {
                        r.enforce_deadline();
                        let (mut out, end_out) = Drains::since(&r.drains.stdout, r.read_stdout);
                        let (mut err, end_err) = Drains::since(&r.drains.stderr, r.read_stderr);
                        // Hand out at most `max_bytes` of each, on char boundaries.
                        let cut = |t: &mut String, from: usize| -> usize {
                            if t.len() <= max_bytes {
                                return from + t.len();
                            }
                            let mut at = max_bytes;
                            while !t.is_char_boundary(at) {
                                at -= 1;
                            }
                            t.truncate(at);
                            from + at
                        };
                        r.read_stdout = cut(&mut out, r.read_stdout).min(end_out);
                        r.read_stderr = cut(&mut err, r.read_stderr).min(end_err);
                        let state_text = match r.child.try_wait() {
                            Ok(Some(_)) => "exited",
                            Ok(None) => "running",
                            Err(_) => "unknown",
                        };
                        Ok(
                        serde_json::json!({ "stdout": out, "stderr": err, "state": state_text })
                            .to_string(),
                    )
                    }
                    Some(Handle::Done { .. }) => Ok(
                        serde_json::json!({ "stdout": "", "stderr": "", "state": "exited" })
                            .to_string(),
                    ),
                    None => Err(format!("process.read: no child with handle {h}")),
                }
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "time.now_ms",
        move |_args, _memory| {
            answer(&state, "time.now_ms", || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis().to_string())
                    .map_err(|e| format!("time.now_ms: {e}"))
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(
        module,
        &meter,
        DOOR,
        "crypto.sha256_file",
        move |args, memory| {
            let path = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
            answer(&state, "crypto.sha256_file", || {
                use sha2::Digest as _;
                let path = utf8(path)?;
                let bytes =
                    std::fs::read(path).map_err(|e| format!("crypto.sha256_file `{path}`: {e}"))?;
                let digest = sha2::Sha256::digest(&bytes);
                Ok(digest
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>())
            })
        },
    )?;

    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "arg_count", move |_args, _memory| {
        let n = state.borrow().args.len();
        Ok(vec![Val::I32(n as i32)])
    })?;

    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "arg", move |args, _memory| {
        let index = arg(args, 0)?;
        answer(&state, "arg", || {
            let s = state.borrow();
            usize::try_from(index)
                .ok()
                .and_then(|i| s.args.get(i))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "arg: index {index} out of range; arg_count() is {}",
                        s.args.len()
                    )
                })
        })
    })?;

    // ---- the shared fetch, exactly the fleet door's two passes -------------

    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "result_len", move |_args, _memory| {
        Ok(vec![Val::I32(result_len(&state)?)])
    })?;

    let state = Rc::clone(&shared);
    bind_metered(module, &meter, DOOR, "result", move |args, memory| {
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
        Ok(Err(message)) if message == CANCELLED_MARK => {
            let meter = Rc::clone(&state.borrow().meter);
            let _ = meter.borrow_mut().check_cancel();
            return Err(WasmError::Trap(crate::host::CANCELLED));
        }
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
    // The operation and its arguments went on the bill in `bind_metered`;
    // what is parked here is the other direction, and it is billed as
    // parked -- the capped refusal, when that is what the script will read.
    s.meter.borrow_mut().answered(payload.len());
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
    // The operation and its arguments went on the bill in `bind_metered`;
    // what is counted here is the diagnostic if one is parked, and the wait
    // if it waited. The previous operation's parked answer is not re-billed
    // for still being there.
    let meter = Rc::clone(&state.borrow().meter);
    let started = Instant::now();
    let outcome = contain(
        &format!("the tool door panicked while serving `{DOOR}.{op}`"),
        run,
    );
    // Only the operations that *wait* put their wall clock on the bill:
    // a file read is compute the step budget cannot see, but it is not
    // the idle time A1.12 wants separated out.
    if WAITING_OPS.contains(&op) {
        meter.borrow_mut().waited(started.elapsed());
    }
    match outcome {
        Ok(Ok(value)) => Ok(vec![Val::I32(value)]),
        Ok(Err(message)) if message == CANCELLED_MARK => {
            // The wait saw the flag; record it the way `charge` would have.
            let _ = meter.borrow_mut().check_cancel();
            Err(WasmError::Trap(crate::host::CANCELLED))
        }
        Ok(Err(message)) => {
            meter.borrow_mut().answered(message.len());
            state.borrow_mut().result = message.into_bytes();
            Ok(vec![Val::I32(-1)])
        }
        Err(panic) => {
            state.borrow_mut().fault = Some(panic);
            Err(WasmError::Trap(TOOL_PANICKED))
        }
    }
}

/// What a waiting operation returns when [`Meter::check_cancel`] said stop
/// mid-wait: `direct` / `answer` turn it into the [`CANCELLED`] trap instead
/// of a parked error string, so the script cannot catch its way past a
/// cancel.
const CANCELLED_MARK: &str = "\0agenterm-cancelled";

/// Sleep at most `total`, in slices, and stop early when the embedder asks.
fn cancellable_sleep(
    cancel: &Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    total: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    while started.elapsed() < total {
        if cancel
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
        {
            return Err(CANCELLED_MARK.to_string());
        }
        let left = total - started.elapsed();
        std::thread::sleep(left.min(Duration::from_millis(25)));
    }
    Ok(())
}

/// The `tool.*` operations whose time is waiting rather than computing.
const WAITING_OPS: &[&str] = &[
    "time.sleep_ms",
    "process.wait",
    "process.command",
    "process.command_stdout",
];

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
/// What `image.inspect_png` answers.
struct PngFacts {
    width: u32,
    height: u32,
    samples: u64,
    luminance: f64,
}

/// Decode a PNG and reduce it to its dimensions, its pixel count and the
/// mean luma. Every colour type the `png` crate expands to 8-bit RGB/RGBA/
/// grey/grey-alpha is read; 16-bit samples are stripped to their high byte.
fn inspect_png(reader: impl std::io::Read) -> Result<PngFacts, String> {
    let mut decoder = png::Decoder::new(reader);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("not a PNG this engine can read: {e}"))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("cannot decode the image: {e}"))?;
    let bytes = &buf[..info.buffer_size()];
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => return Err(format!("unexpected colour type after expansion: {other:?}")),
    };
    let mut total = 0f64;
    let mut samples = 0u64;
    for px in bytes.chunks_exact(channels) {
        let luma = if channels >= 3 {
            0.299 * f64::from(px[0]) + 0.587 * f64::from(px[1]) + 0.114 * f64::from(px[2])
        } else {
            f64::from(px[0])
        };
        total += luma;
        samples += 1;
    }
    Ok(PngFacts {
        width: info.width,
        height: info.height,
        samples,
        luminance: if samples == 0 {
            0.0
        } else {
            total / samples as f64
        },
    })
}

/// The pid behind a handle, running or done: the window ops observe and
/// drive a child by its process id.
fn child_pid(s: &ToolState, h: i32, op: &str) -> Result<u32, String> {
    match usize::try_from(h).ok().and_then(|i| s.children.get(i)) {
        Some(Handle::Running(r)) => Ok(r.child.id()),
        Some(Handle::Done { pid, .. }) => Ok(*pid),
        None => Err(format!("{op}: no child with handle {h}")),
    }
}

fn window_error(
    op: &str,
    e: agenterm_platform::contract::process_window::ProcessWindowError,
) -> String {
    match e.cause {
        Some(cause) => format!("{op}: {} ({}, {cause})", e.message, e.code),
        None => format!("{op}: {} ({})", e.message, e.code),
    }
}

/// rh's key names, exactly as the journeys spell them.
fn window_key_named(
    name: &str,
) -> Result<agenterm_platform::contract::process_window::ProcessWindowKey, String> {
    use agenterm_platform::contract::process_window::ProcessWindowKey as K;
    Ok(match name {
        "Backspace" => K::Backspace,
        "Delete" => K::Delete,
        "Down" => K::Down,
        "End" => K::End,
        "Enter" => K::Enter,
        "Escape" => K::Escape,
        "F2" => K::F2,
        "Home" => K::Home,
        "Left" => K::Left,
        "Right" => K::Right,
        "Tab" => K::Tab,
        "Up" => K::Up,
        other => {
            return Err(format!(
                "process.window_key: no key named `{other}` (process_window_key_invalid)"
            ));
        }
    })
}

fn pointer_action_named(
    name: &str,
) -> Result<agenterm_platform::contract::process_window::ProcessWindowPointerAction, String> {
    use agenterm_platform::contract::process_window::ProcessWindowPointerAction as A;
    Ok(match name {
        "click" => A::Click,
        "down" => A::Down,
        "move" => A::Move,
        "move-held" => A::MoveHeld,
        "up" => A::Up,
        "capture-changed" => A::CaptureChanged,
        other => {
            return Err(format!(
                "process.window_pointer: no action named `{other}` (process_window_pointer_action_invalid)"
            ));
        }
    })
}

#[derive(serde::Deserialize)]
struct PointerSpec {
    action: String,
    x: i32,
    y: i32,
}

#[derive(serde::Deserialize)]
struct MessageSpec {
    message: u32,
    wparam: i64,
    lparam: isize,
}

#[derive(serde::Deserialize)]
struct ResizeSpec {
    width: i32,
    height: i32,
}

#[derive(serde::Deserialize)]
struct ControlSpec {
    id: i32,
    op: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandSpec {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    current_dir: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Variables to strip from the inherited environment. rh's
    /// `command.env_remove("HTTP_PROXY")`; the gate scripts use it to prove a
    /// launch ignores an ambient proxy, and setting the variable to "" is not
    /// the same test.
    #[serde(default)]
    env_remove: Vec<String>,
    timeout_ms: Option<u64>,
    stdin_text: Option<String>,
    /// Send the stream to this file instead of capturing it. The capture is
    /// bounded by `max_bridge_result_bytes` (1 MiB) and a 6 MB cargo log
    /// was a thrown refusal (wave 3); rh had `stdout_file`/`stderr_file`.
    /// The answer's `stdout`/`stderr` are empty for a redirected stream.
    #[serde(default)]
    stdout_path: Option<String>,
    #[serde(default)]
    stderr_path: Option<String>,
}

/// Spawn, feed stdin, capture both streams, wait bounded, answer as JSON:
/// `{"exit_code": n | null, "success": bool, "stdout", "stderr", "timed_out"}`.
///
/// `exit_code` is `null` when the child was killed -- by the timeout here or
/// by a signal -- and `success` is then false. Captured streams are UTF-8 with
/// U+FFFD for anything else, like `print`; binary output cannot cross a door
/// that carries text.
fn run_command(
    spec: CommandSpec,
    max_capture: usize,
    cancel: &Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<String, String> {
    let timeout = spec.timeout_ms.map(Duration::from_millis);
    let child = spawn_command(&spec, true)?;
    wait_child(child, timeout, max_capture, cancel)
}

/// Spawn per the spec with both streams piped and stdin fed on its own
/// thread. Shared by `process.command` (which waits at once) and
/// `process.spawn` (which hands back a handle).
/// `capture` is whether the caller will read the child's stdout and stderr.
/// `process.status` will not, and it used to spawn with pipes and drop them at
/// once -- so a child that printed anything died of SIGPIPE and the door
/// answered `-1` for `agenterm-cc --help` (wave-1 measured it). A caller that
/// does not read gets `Stdio::null()`: the child writes into nothing and
/// exits with its own status.
/// `fs.metadata`'s `modified_ms`: whole milliseconds since the Unix epoch,
/// `None` when the platform or filesystem has no answer (the JSON says null).
fn modified_ms(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn spawn_command(spec: &CommandSpec, capture: bool) -> Result<std::process::Child, String> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut command = Command::new(&spec.program);
    command.args(&spec.args);
    if let Some(dir) = &spec.current_dir {
        command.current_dir(dir);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    for key in &spec.env_remove {
        command.env_remove(key);
    }
    command
        .stdin(if spec.stdin_text.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(match &spec.stdout_path {
            Some(path) => Stdio::from(
                std::fs::File::create(path)
                    .map_err(|e| format!("process: creating stdout_path `{path}`: {e}"))?,
            ),
            None if capture => Stdio::piped(),
            None => Stdio::null(),
        })
        .stderr(match &spec.stderr_path {
            Some(path) => Stdio::from(
                std::fs::File::create(path)
                    .map_err(|e| format!("process: creating stderr_path `{path}`: {e}"))?,
            ),
            None if capture => Stdio::piped(),
            None => Stdio::null(),
        });

    let mut child = command
        .spawn()
        .map_err(|e| format!("process.command: spawning `{}`: {e}", spec.program))?;
    if let (Some(text), Some(mut stdin)) = (&spec.stdin_text, child.stdin.take()) {
        let text = text.clone();
        std::thread::spawn(move || {
            let _ = stdin.write_all(text.as_bytes());
        });
    }
    Ok(child)
}

/// Drain both streams on their own threads, wait bounded, answer as JSON:
/// `{"exit_code": n | null, "success": bool, "stdout", "stderr", "timed_out"}`.
///
/// `exit_code` is `null` when the child was killed -- by the timeout here or
/// by a signal -- and `success` is then false. Captured streams are UTF-8 with
/// U+FFFD for anything else, like `print`; binary output cannot cross a door
/// that carries text. Capture is capped at `max_capture` per stream.
fn wait_child(
    mut child: std::process::Child,
    timeout: Option<Duration>,
    max_capture: usize,
    cancel: &Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<String, String> {
    let drains = Drains::start(&mut child, max_capture);
    finish_child(
        Running {
            child,
            drains,
            started: Instant::now(),
            deadline: None,
            killed_by_deadline: false,
            read_stdout: 0,
            read_stderr: 0,
        },
        timeout,
        cancel,
    )
}

/// Wait for a running child (its drains already going), within `timeout`
/// of *now*; the whole capture comes back, whatever `process.read` handed
/// out before. A child the spawn deadline already killed reports
/// `timed_out`.
fn finish_child(
    mut running: Running,
    timeout: Option<Duration>,
    cancel: &Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<String, String> {
    let (stdout, stderr) = (running.drains.stdout.take(), running.drains.stderr.take());
    let mut child = running.child;
    let mut timed_out = running.killed_by_deadline;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if timeout.is_some_and(|t| started.elapsed() >= t) => {
                timed_out = true;
                let _ = child.kill();
                break child.wait().ok();
            }
            Ok(None) => {
                // A cancel kills the child too: no orphans.
                if cancellable_sleep(cancel, Duration::from_millis(5)).is_err() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(CANCELLED_MARK.to_string());
                }
            }
            Err(e) => return Err(format!("process.wait: {e}")),
        }
    };
    // Give the drains a short grace to reach EOF now that the child is gone;
    // past it, take what arrived. A held-open pipe is the grandchild's
    // business, not this call's.
    let grace = Instant::now() + Duration::from_millis(200);
    let collect = |buf: Option<Arc<Mutex<Vec<u8>>>>| -> Vec<u8> {
        let Some(buf) = buf else { return Vec::new() };
        while Arc::strong_count(&buf) > 1 && Instant::now() < grace {
            std::thread::sleep(Duration::from_millis(5));
        }
        buf.lock().map(|b| b.clone()).unwrap_or_default()
    };
    let (stdout, stderr) = (collect(stdout), collect(stderr));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The bill reads string lengths off the raw arguments by position, so
    /// the positions have to follow the declaration exactly: two words per
    /// string, one per number, the landing buffer of the fetch not counted.
    #[test]
    fn argument_length_slots_follow_the_declaration() {
        assert_eq!(argument_length_slots("fs.exists"), vec![1]);
        assert_eq!(argument_length_slots("fs.write"), vec![1, 3]);
        assert_eq!(argument_length_slots("process.window_key"), vec![2]);
        assert_eq!(argument_length_slots("time.sleep_ms"), Vec::<usize>::new());
        assert_eq!(argument_length_slots("result"), Vec::<usize>::new());
        assert_eq!(argument_length_slots("no.such"), Vec::<usize>::new());
    }

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

    #[cfg(unix)]
    #[test]
    fn a_child_past_its_timeout_is_killed_and_says_so() {
        let spec = CommandSpec {
            program: "sh".into(),
            args: vec!["-c".into(), "sleep 30".into()],
            current_dir: None,
            env: BTreeMap::new(),
            env_remove: Vec::new(),
            timeout_ms: Some(50),
            stdin_text: None,
            stdout_path: None,
            stderr_path: None,
        };
        let started = Instant::now();
        let json = run_command(spec, 1 << 20, &None).expect("the command ran");
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
