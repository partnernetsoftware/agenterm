//! The `agenterm.*` host door -- the complete world a wasm guest can see.
//!
//! ```text
//! print(ptr, len)                                        -> ()
//! fleet_call(op_ptr, op_len, params_ptr, params_len)     -> i32   // status
//! fleet_result_len()                                     -> i32
//! fleet_result(dst_ptr, dst_len)                         -> i32   // written, negative = too small
//! ```
//!
//! # Why the bridge answer arrives in two passes
//!
//! tinyvm's typed host callback is `Fn(&[Val], &mut [u8]) -> Result<Vec<Val>,
//! WasmError>` (`wasm.rs`, `Module::bind_import_typed`): the callback holds
//! `&mut` on the guest's linear memory for its whole duration, while re-entering
//! the guest needs `Instance::invoke_by_name(&mut self)`. In safe Rust those
//! cannot both hold, so **a host callback cannot call back into the guest** --
//! and therefore cannot ask the guest to allocate a landing buffer.
//!
//! So `fleet_call` returns only a status and parks the answer in a per-slot
//! pending buffer; the guest then asks its length, picks its own destination,
//! and has the host copy it in. The cost is two extra border crossings per
//! bridge call. What it buys: no re-entrancy, and no requirement that a guest
//! export an allocator.
//!
//! (`agenterm-wasmcore`'s six-argument single call is not portable here for
//! exactly that reason -- it depends on the host calling the guest's
//! `wasmcore_alloc`. The two ABIs keep the same status codes so guest authors
//! learn one set.)
//!
//! # Failure policy
//!
//! | what happened | door's answer | why |
//! |---|---|---|
//! | pointer/length outside linear memory | trap the slot | memory safety: continuing means inventing an answer |
//! | `op`/`params` are not UTF-8 | status 1 + diagnostic | the guest asked for something a `&str` cannot carry; recoverable |
//! | bridge said `Err` | status 1 + its message | an application error is a normal result, not a crash |
//! | no bridge installed | status 2 + fixed diagnostic | tells the guest the capability is absent, not broken |
//! | bridge answer over `max_bridge_result_bytes` | status 1 + diagnostic | refusal, never a prefix (see below) |
//! | `print` bytes over `max_stdout_bytes` | keep the prefix, set the flag | a cut the caller is told about beats a silent drop |
//!
//! Truncating a bridge answer and truncating stdout are deliberately opposite.
//! Stdout is a stream a reader can see is cut; a bridge answer is one value the
//! guest parses, and half a JSON document is indistinguishable from a whole one
//! until it corrupts something downstream.

use std::cell::RefCell;
use std::rc::Rc;

use tinyvm::{Val, WasmError};

use crate::{Budget, FleetBridgeFn, QjswasmError};

/// The one module name a guest may import from.
const DOOR: &str = "agenterm";

const STATUS_OK: i32 = 0;
const STATUS_ERR: i32 = 1;
const STATUS_NO_BRIDGE: i32 = 2;

/// Host-authored answers. These are the pending buffer's contents when the
/// door itself, rather than the bridge, produced the outcome. They are exempt
/// from `max_bridge_result_bytes`: that cap bounds what an outside bridge can
/// push into a slot, not the door's own bounded, constant strings.
const NO_BRIDGE: &str = "agenterm: no fleet bridge is installed in this slot";
const NOT_UTF8: &str = "agenterm: fleet_call op and params must be UTF-8 text";
const RESULT_TOO_LARGE: &str = "agenterm: fleet result exceeds the slot's max_bridge_result_bytes";

/// The exact shape of each door import: `(field, params, results)`. Every
/// parameter and result is `i32`, which `ImportDesc::i32_only` reports in one
/// flag, so this table is a complete signature check.
const SIGNATURES: [(&str, usize, usize); 4] = [
    ("print", 2, 0),
    ("fleet_call", 4, 1),
    ("fleet_result_len", 0, 1),
    ("fleet_result", 2, 1),
];

/// One slot's door state, shared by the four closures.
struct Pending {
    /// `agenterm.print` bytes, verbatim. Held as bytes rather than a `String`
    /// because the cap can land mid-code-point; decoding happens once, at
    /// read-back.
    stdout: Vec<u8>,
    stdout_truncated: bool,
    max_stdout: usize,
    /// The most recent `fleet_call` answer, awaiting collection. Survives
    /// collection so a guest may copy it twice; the next `fleet_call` replaces
    /// it.
    result: Vec<u8>,
}

impl Pending {
    /// Append up to the cap. Past it, keep the prefix and raise the flag --
    /// never drop silently, and never grow past the budget.
    fn write_stdout(&mut self, bytes: &[u8]) {
        let room = self.max_stdout.saturating_sub(self.stdout.len());
        if bytes.len() > room {
            self.stdout.extend_from_slice(&bytes[..room]);
            self.stdout_truncated = true;
        } else {
            self.stdout.extend_from_slice(bytes);
        }
    }
}

/// The caller's handle on one slot's door. A slot holds this next to its
/// instance; dropping it drops the pending buffer and the captured bridge.
pub(crate) struct HostState {
    pending: Rc<RefCell<Pending>>,
}

impl HostState {
    /// Bytes `agenterm.print` accumulated since the previous take, and whether
    /// [`Budget::max_stdout_bytes`] cut them.
    ///
    /// Draining rather than peeking is deliberate: stdout is reported per call,
    /// so one call's output must not reappear in the next call's `Outcome`.
    ///
    /// Invalid UTF-8 is replaced with U+FFFD instead of being rejected. Guest
    /// output is diagnostic text, and refusing to show it because one byte is
    /// malformed loses exactly the message that mattered; the cap can also cut
    /// a code point in half, so read-back has to be lossy regardless.
    pub(crate) fn take_stdout(&self) -> (String, bool) {
        let mut pending = self.pending.borrow_mut();
        let bytes = std::mem::take(&mut pending.stdout);
        let truncated = std::mem::replace(&mut pending.stdout_truncated, false);
        (String::from_utf8_lossy(&bytes).into_owned(), truncated)
    }
}

/// Bind the four door functions into `module` and return the state they share.
///
/// A guest need not import all four -- or any. Only the imports it actually
/// declares are bound; the rest are simply absent, which is why the loop below
/// consults `Module::imports()` before binding rather than treating tinyvm's
/// "no imported function named" as a failure.
pub(crate) fn install(
    module: &mut tinyvm::WasmModule,
    budget: &Budget,
    bridge: Option<FleetBridgeFn>,
) -> Result<HostState, QjswasmError> {
    check_declarations(module)?;

    let pending = Rc::new(RefCell::new(Pending {
        stdout: Vec::new(),
        stdout_truncated: false,
        max_stdout: budget.max_stdout_bytes,
        result: Vec::new(),
    }));

    let state = Rc::clone(&pending);
    bind(module, "print", move |args, memory| {
        let bytes = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        state.borrow_mut().write_stdout(bytes);
        Ok(Vec::new())
    })?;

    let state = Rc::clone(&pending);
    let max_result = budget.max_bridge_result_bytes;
    bind(module, "fleet_call", move |args, memory| {
        // Bounds-check both regions before anything else runs: an out-of-range
        // pointer must not reach the bridge, let alone read host memory.
        let op = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        let params = guest_slice(memory, arg(args, 2)?, arg(args, 3)?)?;

        let (status, payload) = match (str::from_utf8(op), str::from_utf8(params)) {
            (Err(_), _) | (_, Err(_)) => (STATUS_ERR, NOT_UTF8.as_bytes().to_vec()),
            (Ok(op), Ok(params)) => match &bridge {
                None => (STATUS_NO_BRIDGE, NO_BRIDGE.as_bytes().to_vec()),
                Some(bridge) => match bridge(op, params) {
                    Ok(answer) => (STATUS_OK, answer.into_bytes()),
                    Err(message) => (STATUS_ERR, message.into_bytes()),
                },
            },
        };
        // The cap applies to whatever the bridge produced, success or failure
        // message alike, and replaces it wholesale rather than cutting it.
        let (status, payload) = if payload.len() > max_result {
            (STATUS_ERR, RESULT_TOO_LARGE.as_bytes().to_vec())
        } else {
            (status, payload)
        };

        state.borrow_mut().result = payload;
        Ok(vec![Val::I32(status)])
    })?;

    let state = Rc::clone(&pending);
    bind(module, "fleet_result_len", move |_args, _memory| {
        Ok(vec![Val::I32(pending_len(&state)?)])
    })?;

    let state = Rc::clone(&pending);
    bind(module, "fleet_result", move |args, memory| {
        let dst_ptr = arg(args, 0)?;
        let dst_len = arg(args, 1)?;
        // Check the destination the guest *declared*, not the part that happens
        // to be used: a buffer outside linear memory is a broken guest whether
        // or not the pending bytes would have fitted in it.
        let dst = guest_slice_mut(memory, dst_ptr, dst_len)?;
        let needed = pending_len(&state)?;
        if (needed as usize) > dst.len() {
            // Nothing is written. The negated requirement tells the guest how
            // much room to find, which is the only useful thing to say here.
            return Ok(vec![Val::I32(-needed)]);
        }
        dst[..needed as usize].copy_from_slice(&state.borrow().result);
        Ok(vec![Val::I32(needed)])
    })?;

    Ok(HostState { pending })
}

/// Reject a guest whose door declarations do not match the ABI, at load time
/// rather than on first use.
///
/// Both cases here would otherwise surface much later and much less clearly: a
/// mistyped import as a `host argument type` trap mid-run, and an unknown
/// `agenterm.*` name as an unbound-import trap. Catching them here keeps the
/// crate's promise that load-time refusal and run-time trap are distinguishable
/// failure classes.
pub(crate) fn check_declarations(module: &tinyvm::WasmModule) -> Result<(), QjswasmError> {
    for desc in module.imports() {
        // An import from any other module namespace can never be bound: this
        // door is the whole world a guest gets, and PRD 36 forbids growing a
        // second OS-shaped surface (`wasi_snapshot_preview1`) beside it. It is
        // refused here, at load time, naming the import -- because the
        // alternative is what happened before: such a module validated clean
        // and then died on the core's `Trap("call to unbound imported
        // function")` at the first call, which names nothing and reads as the
        // guest's fault. "Rejected before it could run" and "trapped while
        // running" must be tellable apart.
        if desc.module != DOOR {
            return Err(QjswasmError::Door(format!(
                "guest imports `{}.{}`; `{DOOR}.*` is the only host module this engine \
                 offers, so nothing can bind it",
                desc.module, desc.field
            )));
        }
        let Some(&(_, params, results)) =
            SIGNATURES.iter().find(|(field, _, _)| *field == desc.field)
        else {
            return Err(QjswasmError::Door(format!(
                "guest imports unknown door function `{DOOR}.{}`",
                desc.field
            )));
        };
        if desc.n_params != params || desc.n_results != results || !desc.i32_only {
            return Err(QjswasmError::Door(format!(
                "guest declares `{DOOR}.{}` with the wrong signature: the door takes \
                 {params} i32 parameter(s) and returns {results}",
                desc.field
            )));
        }
    }
    Ok(())
}

/// Bind one door function, tolerating a guest that never imported it.
fn bind<F>(module: &mut tinyvm::WasmModule, field: &str, f: F) -> Result<(), QjswasmError>
where
    F: Fn(&[Val], &mut [u8]) -> Result<Vec<Val>, WasmError> + 'static,
{
    // `bind_import_typed` matches against the same `import_descs` that
    // `imports()` exposes and reports an absent name as an error, so asking
    // first is exact -- and avoids deciding absence by matching on a message.
    if !module
        .imports()
        .iter()
        .any(|desc| desc.module == DOOR && desc.field == field)
    {
        return Ok(());
    }
    module.bind_import_typed(DOOR, field, f).map_err(|error| {
        QjswasmError::Door(format!("binding `{DOOR}.{field}`: {}", error.message()))
    })
}

/// The pending answer's length as an `i32`.
///
/// `max_bridge_result_bytes` is a `usize`, so a host that sets it above 2 GiB
/// could park an answer this ABI cannot describe. That is a host
/// misconfiguration, not a guest fault, but the guest is the one who would see
/// a nonsense length, so it traps instead.
fn pending_len(state: &Rc<RefCell<Pending>>) -> Result<i32, WasmError> {
    i32::try_from(state.borrow().result.len())
        .map_err(|_| WasmError::Trap("agenterm door: pending result exceeds i32"))
}

/// One declared argument. The core already verified arity and types before
/// dispatch; this only unpacks.
fn arg(args: &[Val], index: usize) -> Result<i32, WasmError> {
    match args.get(index) {
        Some(Val::I32(value)) => Ok(*value),
        _ => Err(WasmError::Trap("agenterm door: argument")),
    }
}

/// `[ptr, ptr+len)` inside the guest's linear memory, or a trap.
///
/// Negative values fail the conversion rather than wrapping, so a guest cannot
/// reach behind the memory slice with a sign trick.
fn guest_slice(memory: &[u8], ptr: i32, len: i32) -> Result<&[u8], WasmError> {
    let range = guest_range(memory.len(), ptr, len)?;
    memory
        .get(range)
        .ok_or(WasmError::Trap("agenterm door: pointer out of bounds"))
}

fn guest_slice_mut(memory: &mut [u8], ptr: i32, len: i32) -> Result<&mut [u8], WasmError> {
    let range = guest_range(memory.len(), ptr, len)?;
    memory
        .get_mut(range)
        .ok_or(WasmError::Trap("agenterm door: pointer out of bounds"))
}

fn guest_range(memory_len: usize, ptr: i32, len: i32) -> Result<std::ops::Range<usize>, WasmError> {
    let start =
        usize::try_from(ptr).map_err(|_| WasmError::Trap("agenterm door: negative pointer"))?;
    let count =
        usize::try_from(len).map_err(|_| WasmError::Trap("agenterm door: negative length"))?;
    let end = start
        .checked_add(count)
        .ok_or(WasmError::Trap("agenterm door: pointer overflow"))?;
    if end > memory_len {
        return Err(WasmError::Trap("agenterm door: pointer out of bounds"));
    }
    Ok(start..end)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// The four imports, written out once. Every guest below declares all four
    /// so the tests exercise one door shape; the partial-import cases get their
    /// own modules.
    const DOOR_IMPORTS: &str = r#"
        (import "agenterm" "print" (func $print (param i32 i32)))
        (import "agenterm" "fleet_call"
            (func $fleet_call (param i32 i32 i32 i32) (result i32)))
        (import "agenterm" "fleet_result_len" (func $fleet_result_len (result i32)))
        (import "agenterm" "fleet_result" (func $fleet_result (param i32 i32) (result i32)))
    "#;

    /// Call the bridge, then hand the whole pending buffer back to the host as
    /// stdout. Shared by the status tests, which differ only in the bridge.
    const RETRIEVE_INTO_STDOUT: &str = r#"
        (memory 1)
        (data (i32.const 0) "fleet.ping")
        (func (export "main") (result i32)
            (local $status i32) (local $need i32)
            (local.set $status (call $fleet_call
                (i32.const 0) (i32.const 10) (i32.const 0) (i32.const 0)))
            (local.set $need (call $fleet_result_len))
            (call $print (i32.const 256)
                (call $fleet_result (i32.const 256) (local.get $need)))
            (local.get $status))
    "#;

    fn guest(rest: &str) -> Vec<u8> {
        wat::parse_str(format!("(module {DOOR_IMPORTS} {rest})")).expect("test guest is valid wat")
    }

    /// A bridge that answers every call the same way, recording what it saw so
    /// a test can prove the door refused *before* reaching it.
    struct Recorder {
        bridge: FleetBridgeFn,
        calls: Arc<AtomicUsize>,
        seen: Arc<Mutex<Vec<(String, String)>>>,
    }

    fn recorder(answer: Result<String, String>) -> Recorder {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let hits = Arc::clone(&calls);
        let log = Arc::clone(&seen);
        let bridge: FleetBridgeFn = Arc::new(move |op: &str, params: &str| {
            hits.fetch_add(1, Ordering::SeqCst);
            log.lock().unwrap().push((op.to_owned(), params.to_owned()));
            answer.clone()
        });
        Recorder {
            bridge,
            calls,
            seen,
        }
    }

    /// tinyvm is fmt-free by design, so `WasmError` has no `Debug` and
    /// `Result::unwrap` is unavailable on anything it returns.
    fn load(wasm: &[u8], budget: &Budget) -> tinyvm::WasmModule {
        match tinyvm::WasmModule::from_bytes_with(wasm, budget.limits) {
            Ok(module) => module,
            Err(error) => panic!("test guest failed the load gate: {}", error.message()),
        }
    }

    /// Load, bind the door, instantiate, invoke -- the exact sequence a slot
    /// performs, minus the slot.
    fn run(
        wasm: &[u8],
        budget: &Budget,
        bridge: Option<FleetBridgeFn>,
    ) -> (Result<Vec<Val>, WasmError>, HostState) {
        let mut module = load(wasm, budget);
        let state = match install(&mut module, budget, bridge) {
            Ok(state) => state,
            Err(error) => panic!("door failed to install: {error}"),
        };
        let mut instance = match module.instantiate() {
            Ok(instance) => instance,
            Err(error) => panic!("test guest failed to instantiate: {}", error.message()),
        };
        let outcome = instance.invoke_by_name("main", &[]);
        (outcome, state)
    }

    fn returned(values: &Result<Vec<Val>, WasmError>) -> i32 {
        match values {
            Ok(values) => match values.as_slice() {
                [Val::I32(value)] => *value,
                other => panic!("expected one i32 result, got {}", other.len()),
            },
            Err(error) => panic!("guest trapped: {}", error.message()),
        }
    }

    fn install_error(wasm: &[u8]) -> QjswasmError {
        let budget = Budget::default();
        let mut module = load(wasm, &budget);
        match install(&mut module, &budget, None) {
            Ok(_) => panic!("expected the door to refuse this guest"),
            Err(error) => error,
        }
    }

    /// The full round trip: call the bridge, ask the length, pick a
    /// destination, copy in, print it back.
    #[test]
    fn status_ok_round_trips_through_two_passes() {
        let wasm = guest(
            r#"
            (memory 1)
            (data (i32.const 0) "fleet.ping")
            (data (i32.const 16) "{\"n\":1}")
            (func (export "main") (result i32)
                (local $status i32) (local $need i32) (local $written i32)
                (local.set $status
                    (call $fleet_call
                        (i32.const 0) (i32.const 10) (i32.const 16) (i32.const 7)))
                (local.set $need (call $fleet_result_len))
                (local.set $written (call $fleet_result (i32.const 256) (local.get $need)))
                (call $print (i32.const 256) (local.get $written))
                (i32.add
                    (i32.mul (local.get $status) (i32.const 1000))
                    (local.get $written)))
            "#,
        );
        let recorder = recorder(Ok("pong".to_owned()));
        let (outcome, state) = run(&wasm, &Budget::default(), Some(recorder.bridge));
        assert_eq!(returned(&outcome), 4, "status 0, then 4 bytes written");
        assert_eq!(recorder.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            recorder.seen.lock().unwrap().as_slice(),
            [("fleet.ping".to_owned(), "{\"n\":1}".to_owned())],
            "the bridge sees exactly the two guest regions"
        );
        assert_eq!(state.take_stdout(), ("pong".to_owned(), false));
    }

    /// A second call replaces the pending buffer rather than appending to it.
    #[test]
    fn a_second_call_replaces_the_pending_answer() {
        let wasm = guest(
            r#"
            (memory 1)
            (data (i32.const 0) "fleet.ping")
            (func (export "main") (result i32)
                (drop (call $fleet_call
                    (i32.const 0) (i32.const 10) (i32.const 0) (i32.const 0)))
                (drop (call $fleet_call
                    (i32.const 0) (i32.const 10) (i32.const 0) (i32.const 0)))
                (call $print (i32.const 256)
                    (call $fleet_result (i32.const 256) (call $fleet_result_len)))
                (call $fleet_result_len))
            "#,
        );
        let recorder = recorder(Ok("pong".to_owned()));
        let (outcome, state) = run(&wasm, &Budget::default(), Some(recorder.bridge));
        assert_eq!(returned(&outcome), 4);
        assert_eq!(state.take_stdout(), ("pong".to_owned(), false));
    }

    #[test]
    fn bridge_error_is_status_1_with_a_readable_message() {
        let wasm = guest(RETRIEVE_INTO_STDOUT);
        let recorder = recorder(Err("no such op: fleet.ping".to_owned()));
        let (outcome, state) = run(&wasm, &Budget::default(), Some(recorder.bridge));
        assert_eq!(returned(&outcome), STATUS_ERR);
        assert_eq!(
            state.take_stdout(),
            ("no such op: fleet.ping".to_owned(), false)
        );
    }

    #[test]
    fn absent_bridge_is_status_2_with_a_fixed_diagnostic() {
        let wasm = guest(RETRIEVE_INTO_STDOUT);
        let (outcome, state) = run(&wasm, &Budget::default(), None);
        assert_eq!(returned(&outcome), STATUS_NO_BRIDGE);
        assert_eq!(state.take_stdout(), (NO_BRIDGE.to_owned(), false));
    }

    /// A destination smaller than the pending bytes reports negative and leaves
    /// the guest's buffer exactly as it was.
    #[test]
    fn small_destination_returns_negative_and_writes_nothing() {
        let wasm = guest(
            r#"
            (memory 1)
            (data (i32.const 0) "op")
            (data (i32.const 256) "................")
            (func (export "main") (result i32)
                (local $written i32)
                (drop (call $fleet_call
                    (i32.const 0) (i32.const 2) (i32.const 0) (i32.const 0)))
                (local.set $written (call $fleet_result (i32.const 256) (i32.const 3)))
                (call $print (i32.const 256) (i32.const 16))
                (local.get $written))
            "#,
        );
        let recorder = recorder(Ok("0123456789".to_owned()));
        let (outcome, state) = run(&wasm, &Budget::default(), Some(recorder.bridge));
        assert_eq!(
            returned(&outcome),
            -10,
            "negative, and it names the room needed"
        );
        assert_eq!(
            state.take_stdout(),
            ("................".to_owned(), false),
            "the guest's buffer is untouched"
        );
    }

    #[test]
    fn retrieval_before_any_call_is_empty() {
        let wasm = guest(
            r#"
            (memory 1)
            (data (i32.const 256) "................")
            (func (export "main") (result i32)
                (local $written i32)
                (local.set $written (call $fleet_result (i32.const 256) (i32.const 16)))
                (call $print (i32.const 256) (i32.const 16))
                (i32.add
                    (i32.mul (call $fleet_result_len) (i32.const 1000))
                    (local.get $written)))
            "#,
        );
        let (outcome, state) = run(&wasm, &Budget::default(), None);
        assert_eq!(returned(&outcome), 0, "length 0, and 0 bytes written");
        assert_eq!(state.take_stdout(), ("................".to_owned(), false));
    }

    #[test]
    fn out_of_range_op_pointer_traps_before_the_bridge_runs() {
        let wasm = guest(
            r#"
            (memory 1)
            (func (export "main") (result i32)
                (call $fleet_call
                    (i32.const 1000000) (i32.const 4) (i32.const 0) (i32.const 0)))
            "#,
        );
        let recorder = recorder(Ok(String::new()));
        let (outcome, _state) = run(&wasm, &Budget::default(), Some(recorder.bridge));
        assert!(
            outcome.is_err(),
            "an out-of-range op pointer traps the slot"
        );
        assert_eq!(
            recorder.calls.load(Ordering::SeqCst),
            0,
            "the door refused before reading anything"
        );
    }

    /// In range at the start, past the end at the finish -- the case a
    /// start-only check would wave through.
    #[test]
    fn op_running_off_the_end_traps_before_the_bridge_runs() {
        let wasm = guest(
            r#"
            (memory 1)
            (func (export "main") (result i32)
                (call $fleet_call
                    (i32.const 65530) (i32.const 32) (i32.const 0) (i32.const 0)))
            "#,
        );
        let recorder = recorder(Ok(String::new()));
        let (outcome, _state) = run(&wasm, &Budget::default(), Some(recorder.bridge));
        assert!(outcome.is_err());
        assert_eq!(recorder.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn out_of_range_params_pointer_traps_before_the_bridge_runs() {
        let wasm = guest(
            r#"
            (memory 1)
            (data (i32.const 0) "op")
            (func (export "main") (result i32)
                (call $fleet_call
                    (i32.const 0) (i32.const 2) (i32.const 65535) (i32.const 8)))
            "#,
        );
        let recorder = recorder(Ok(String::new()));
        let (outcome, _state) = run(&wasm, &Budget::default(), Some(recorder.bridge));
        assert!(outcome.is_err());
        assert_eq!(recorder.calls.load(Ordering::SeqCst), 0);
    }

    /// A negative pointer must fail the conversion, not wrap into a host-side
    /// index.
    #[test]
    fn negative_pointer_traps_before_the_bridge_runs() {
        let wasm = guest(
            r#"
            (memory 1)
            (func (export "main") (result i32)
                (call $fleet_call
                    (i32.const -1) (i32.const 2) (i32.const 0) (i32.const 0)))
            "#,
        );
        let recorder = recorder(Ok(String::new()));
        let (outcome, _state) = run(&wasm, &Budget::default(), Some(recorder.bridge));
        assert!(outcome.is_err());
        assert_eq!(recorder.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn out_of_range_print_pointer_traps() {
        let wasm = guest(
            r#"
            (memory 1)
            (func (export "main") (result i32)
                (call $print (i32.const 65000) (i32.const 4096))
                (i32.const 0))
            "#,
        );
        let (outcome, _state) = run(&wasm, &Budget::default(), None);
        assert!(outcome.is_err());
    }

    /// The declared destination is checked even when the pending answer is
    /// short enough to have fitted.
    #[test]
    fn out_of_range_destination_traps() {
        let wasm = guest(
            r#"
            (memory 1)
            (func (export "main") (result i32)
                (call $fleet_result (i32.const 65530) (i32.const 64)))
            "#,
        );
        let (outcome, _state) = run(&wasm, &Budget::default(), None);
        assert!(outcome.is_err());
    }

    #[test]
    fn stdout_over_budget_is_truncated_and_flagged() {
        let wasm = guest(
            r#"
            (memory 1)
            (data (i32.const 0) "0123456789abcdef")
            (func (export "main") (result i32)
                (call $print (i32.const 0) (i32.const 16))
                (call $print (i32.const 0) (i32.const 16))
                (i32.const 0))
            "#,
        );
        let budget = Budget {
            max_stdout_bytes: 10,
            ..Budget::default()
        };
        let (outcome, state) = run(&wasm, &budget, None);
        assert_eq!(returned(&outcome), 0, "the guest is not trapped for it");
        assert_eq!(state.take_stdout(), ("0123456789".to_owned(), true));
    }

    /// Over-budget bridge answers are refused, not cut: the guest must never
    /// receive a prefix it cannot tell apart from a complete answer.
    #[test]
    fn bridge_result_over_budget_is_an_error_not_a_truncation() {
        let wasm = guest(RETRIEVE_INTO_STDOUT);
        let recorder = recorder(Ok("0123456789abcdef".to_owned()));
        let budget = Budget {
            max_bridge_result_bytes: 4,
            ..Budget::default()
        };
        let (outcome, state) = run(&wasm, &budget, Some(recorder.bridge));
        assert_eq!(returned(&outcome), STATUS_ERR, "over budget is status 1");
        let (stdout, _) = state.take_stdout();
        assert_eq!(stdout, RESULT_TOO_LARGE);
        assert!(
            !"0123456789abcdef".starts_with(&stdout),
            "a refusal, not a prefix of the payload"
        );
    }

    /// Non-UTF-8 `op` bytes are an application-level rejection, not a trap:
    /// nothing unsafe happened, the guest simply asked for something the door
    /// cannot carry as a `&str`.
    #[test]
    fn non_utf8_op_is_status_1_and_never_reaches_the_bridge() {
        let wasm = guest(
            r#"
            (memory 1)
            (data (i32.const 0) "\ff\fe")
            (func (export "main") (result i32)
                (local $status i32) (local $need i32)
                (local.set $status (call $fleet_call
                    (i32.const 0) (i32.const 2) (i32.const 0) (i32.const 0)))
                (local.set $need (call $fleet_result_len))
                (call $print (i32.const 256)
                    (call $fleet_result (i32.const 256) (local.get $need)))
                (local.get $status))
            "#,
        );
        let recorder = recorder(Ok(String::new()));
        let (outcome, state) = run(&wasm, &Budget::default(), Some(recorder.bridge));
        assert_eq!(returned(&outcome), STATUS_ERR);
        assert_eq!(recorder.calls.load(Ordering::SeqCst), 0);
        assert_eq!(state.take_stdout(), (NOT_UTF8.to_owned(), false));
    }

    /// Invalid UTF-8 on `print` is kept verbatim and replaced at read-back, so
    /// one broken byte never costs the guest its diagnostic output.
    #[test]
    fn non_utf8_print_is_replaced_not_rejected() {
        let wasm = guest(
            r#"
            (memory 1)
            (data (i32.const 0) "a\ffb")
            (func (export "main") (result i32)
                (call $print (i32.const 0) (i32.const 3))
                (i32.const 0))
            "#,
        );
        let (outcome, state) = run(&wasm, &Budget::default(), None);
        assert_eq!(returned(&outcome), 0);
        assert_eq!(state.take_stdout(), ("a\u{fffd}b".to_owned(), false));
    }

    /// A cut that lands mid-code-point is still readable text, not a panic.
    #[test]
    fn stdout_cut_inside_a_code_point_does_not_panic() {
        let wasm = guest(
            r#"
            (memory 1)
            (data (i32.const 0) "\e4\b8\ad")
            (func (export "main") (result i32)
                (call $print (i32.const 0) (i32.const 3))
                (i32.const 0))
            "#,
        );
        let budget = Budget {
            max_stdout_bytes: 2,
            ..Budget::default()
        };
        let (outcome, state) = run(&wasm, &budget, None);
        assert_eq!(returned(&outcome), 0);
        let (stdout, truncated) = state.take_stdout();
        assert!(truncated);
        assert_eq!(stdout, "\u{fffd}");
    }

    /// A guest that never talks to the fleet still loads: binding an import the
    /// guest did not declare is not an error.
    #[test]
    fn a_guest_importing_only_print_still_loads() {
        let wasm = wat::parse_str(
            r#"(module
                (import "agenterm" "print" (func $print (param i32 i32)))
                (memory 1)
                (data (i32.const 0) "hi")
                (func (export "main") (result i32)
                    (call $print (i32.const 0) (i32.const 2))
                    (i32.const 7)))"#,
        )
        .expect("valid wat");
        let (outcome, state) = run(&wasm, &Budget::default(), None);
        assert_eq!(returned(&outcome), 7);
        assert_eq!(state.take_stdout(), ("hi".to_owned(), false));
    }

    /// A guest importing nothing at all -- and declaring no memory -- is
    /// equally fine.
    #[test]
    fn a_guest_importing_nothing_still_loads() {
        let wasm = wat::parse_str(r#"(module (func (export "main") (result i32) (i32.const 1)))"#)
            .expect("valid wat");
        let (outcome, state) = run(&wasm, &Budget::default(), None);
        assert_eq!(returned(&outcome), 1);
        assert_eq!(state.take_stdout(), (String::new(), false));
    }

    /// A door function declared with the wrong shape is caught at install, so
    /// the caller gets a `Door` diagnostic instead of a trap on first use.
    #[test]
    fn a_mistyped_door_import_is_refused_at_install() {
        let wasm = wat::parse_str(
            r#"(module
                (import "agenterm" "print" (func $print (param i64 i32)))
                (memory 1)
                (func (export "main")))"#,
        )
        .expect("valid wat");
        let error = install_error(&wasm);
        assert!(
            matches!(&error, QjswasmError::Door(message) if message.contains("print")),
            "expected a Door diagnostic naming the import, got {error:?}"
        );
    }

    #[test]
    fn an_unknown_door_name_is_refused_at_install() {
        let wasm = wat::parse_str(
            r#"(module
                (import "agenterm" "exec" (func $exec (param i32) (result i32)))
                (memory 1)
                (func (export "main")))"#,
        )
        .expect("valid wat");
        let error = install_error(&wasm);
        assert!(
            matches!(&error, QjswasmError::Door(message) if message.contains("exec")),
            "expected a Door diagnostic naming the import, got {error:?}"
        );
    }

    /// An import from another module namespace is refused at load time, naming
    /// it.
    ///
    /// # This reverses an earlier decision, deliberately
    ///
    /// This test used to assert the opposite -- "imports from another module
    /// name are none of the door's business" -- and let such a module load.
    /// Measured, that is what it bought (a `wasi_snapshot_preview1.fd_write`
    /// guest, aarch64-apple-darwin, upstream rev `df8decd`):
    ///
    /// ```text
    /// validate_wasm = Ok(())
    /// spawn         = Ok
    /// call          = Err(Trap("call to unbound imported function"))
    /// ```
    ///
    /// Three things are wrong with that row, in order of seriousness. The
    /// `check` path passed a guest the `execute` path cannot run, which is the
    /// worst shape a gate can have. The failure arrived as a `Trap`, blaming a
    /// guest that was built correctly against a different host. And the trap
    /// names no import, because tinyvm is `no_std` and its messages are static
    /// prefixes, so nothing in the output says *which* import went unbound.
    ///
    /// Nothing was gained in exchange. `agenterm.*` is the entire world a guest
    /// gets -- PRD 36 makes that a discipline, not a default -- so an import
    /// from any other namespace can never be bound by anyone, by this host or a
    /// later one. Refusing it at load time, by name, is the same answer given
    /// earlier and legibly.
    ///
    /// The door's other leniency is untouched, and is a different thing: a
    /// guest may import *some or none* of the four door functions, which
    /// [`bind`] handles by consulting `Module::imports()` first. An absent
    /// import is not an unbindable one.
    #[test]
    fn a_foreign_import_is_refused_at_install() {
        let wasm = wat::parse_str(
            r#"(module
                (import "somewhere" "else" (func $else (param i32)))
                (memory 1)
                (func (export "main") (result i32) (i32.const 3)))"#,
        )
        .expect("valid wat");
        let error = install_error(&wasm);
        assert!(
            matches!(&error, QjswasmError::Door(message)
                if message.contains("somewhere") && message.contains("else")),
            "expected a Door diagnostic naming the import, got {error:?}"
        );
    }

    /// `take_stdout` drains, so the next call on the same slot starts clean.
    #[test]
    fn take_stdout_drains() {
        let wasm = guest(
            r#"
            (memory 1)
            (data (i32.const 0) "abc")
            (func (export "main") (result i32)
                (call $print (i32.const 0) (i32.const 3))
                (i32.const 0))
            "#,
        );
        let (_outcome, state) = run(&wasm, &Budget::default(), None);
        assert_eq!(state.take_stdout(), ("abc".to_owned(), false));
        assert_eq!(state.take_stdout(), (String::new(), false));
    }
}
