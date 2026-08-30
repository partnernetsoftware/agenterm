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
//! | bridge panicked | fail the call, `QjswasmError::Door` | not one of the two answers a bridge may give; see below |
//! | no bridge installed | status 2 + fixed diagnostic | tells the guest the capability is absent, not broken |
//! | bridge answer over `max_bridge_result_bytes` | status 1 + diagnostic | refusal, never a prefix (see below) |
//! | `print` bytes over `max_stdout_bytes` | keep the prefix, set the flag | a cut the caller is told about beats a silent drop |
//!
//! # A panicking bridge is contained, and is nobody's status code
//!
//! An embedder's bridge closure is host code this crate calls on a guest's
//! behalf, and the guest picks the `op` string that selects which of its paths
//! runs. Left alone, a panic there unwinds through the interpreter and out of
//! `Engine::call` -- so a script could steer a bridge into its panicking path
//! and, under a `panic = "abort"` profile, take the process with it. The door
//! therefore catches it.
//!
//! What it must *not* do is dress it up as `Err`: status 1 means "the
//! capability answered, and the answer is no", and a script that cannot tell
//! that from "the capability is broken" will carry on parsing a diagnostic as
//! data. So the call fails: the panic's own message is recorded beside the
//! slot and surfaces as [`QjswasmError::Door`], which is exactly the class for
//! "a contract at the host boundary was violated" -- here by the host's own
//! side of it. The slot itself is untouched and remains callable; nothing in
//! the guest's memory was mid-write, because the bridge is called before the
//! pending buffer is borrowed.
//!
//! Truncating a bridge answer and truncating stdout are deliberately opposite.
//! Stdout is a stream a reader can see is cut; a bridge answer is one value the
//! guest parses, and half a JSON document is indistinguishable from a whole one
//! until it corrupts something downstream.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The trap a door raises when [`Budget::max_host_ops`] is spent. Read back
/// through [`HostState::take_budget_refusal`], which is what turns the
/// core's `&'static str` into the budget's name.
pub(crate) const HOST_OPS_EXHAUSTED: &str = "agenterm: host operation budget exhausted";

/// The trap a door raises when [`Budget::cancel`] was set; read back through
/// [`HostState::take_cancelled`].
pub(crate) const CANCELLED: &str = "agenterm: cancelled by the host";

/// One call's host-side bill, shared by the `agenterm.*` and `tool.*` doors.
///
/// Three counters and a cap. Charged at the door *before* the operation
/// runs, like `ToolState::calls`, so an operation that panics is still on
/// the bill.
pub(crate) struct Meter {
    ops: u64,
    bytes: u64,
    waited: Duration,
    max_ops: usize,
    refused: Option<&'static str>,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    cancelled: bool,
}

impl Meter {
    pub(crate) fn new(
        max_ops: usize,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Self {
        Self {
            ops: 0,
            bytes: 0,
            waited: Duration::ZERO,
            max_ops,
            refused: None,
            cancel,
            cancelled: false,
        }
    }

    /// `Err` once the embedder has asked for the call to end; the caller
    /// traps with [`CANCELLED`] and `slot.rs` reads the reason back.
    pub(crate) fn check_cancel(&mut self) -> Result<(), &'static str> {
        if self
            .cancel
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
        {
            self.cancelled = true;
            return Err(CANCELLED);
        }
        Ok(())
    }

    pub(crate) fn take_cancelled(&mut self) -> bool {
        std::mem::take(&mut self.cancelled)
    }

    /// One more operation carrying `bytes` of arguments. `Err` when the cap
    /// is already spent: the operation must not run, and the caller traps
    /// with [`HOST_OPS_EXHAUSTED`].
    pub(crate) fn charge(&mut self, bytes: usize) -> Result<(), &'static str> {
        self.check_cancel()?;
        if self.ops >= self.max_ops as u64 {
            self.refused = Some("max_host_ops");
            return Err(HOST_OPS_EXHAUSTED);
        }
        self.ops += 1;
        self.bytes += bytes as u64;
        Ok(())
    }

    pub(crate) fn answered(&mut self, bytes: usize) {
        self.bytes += bytes as u64;
    }

    pub(crate) fn waited(&mut self, for_: Duration) {
        self.waited += for_;
    }

    /// `(ops, bytes, waited_ms)`, reset for the next call.
    pub(crate) fn take(&mut self) -> (u64, u64, u64) {
        let bill = (self.ops, self.bytes, self.waited.as_millis() as u64);
        self.ops = 0;
        self.bytes = 0;
        self.waited = Duration::ZERO;
        bill
    }

    pub(crate) fn take_refusal(&mut self) -> Option<&'static str> {
        self.refused.take()
    }
}

use tinyvm::{Val, WasmError};
use tinyvm_qjs::{HostFn, HostParam, HostResult};

use crate::tool;
use crate::{Budget, FleetBridgeFn, QjswasmError};

/// The module name every guest may import from. `tool::DOOR` is the other,
/// and only a slot that opened it may import from that one.
const DOOR: &str = "agenterm";

pub(crate) const STATUS_OK: i32 = 0;
pub(crate) const STATUS_ERR: i32 = 1;
const STATUS_NO_BRIDGE: i32 = 2;

/// Host-authored answers. These are the pending buffer's contents when the
/// door itself, rather than the bridge, produced the outcome. They are exempt
/// from `max_bridge_result_bytes`: that cap bounds what an outside bridge can
/// push into a slot, not the door's own bounded, constant strings.
const NO_BRIDGE: &str = "agenterm: no fleet bridge is installed in this slot";
const NOT_UTF8: &str = "agenterm: fleet_call op and params must be UTF-8 text";
const RESULT_TOO_LARGE: &str = "agenterm: fleet result exceeds the slot's max_bridge_result_bytes";

/// The `&'static str` the core carries when a bridge panicked. The useful text
/// -- which op, and what the panic said -- does not fit in a
/// `tinyvm::WasmError`, so it travels beside it in [`Pending::fault`] and
/// `slot.rs` reports that instead. This literal is the fallback nobody should
/// see.
const BRIDGE_PANICKED: &str = "agenterm door: the fleet bridge panicked";

/// The exact shape of each door import: `(field, params, results)`. Every
/// parameter and result is `i32`, which `ImportDesc::i32_only` reports in one
/// flag, so this table is a complete signature check.
const SIGNATURES: [(&str, usize, usize); 4] = [
    ("print", 2, 0),
    ("fleet_call", 4, 1),
    ("fleet_result_len", 0, 1),
    ("fleet_result", 2, 1),
];

/// The same door, said in the vocabulary the `.qjs` compiler needs: which host
/// functions a script may call, and how a JavaScript value is unwrapped onto
/// each raw parameter above.
///
/// # The direction of the unwrapping is the whole design
///
/// The door does **not** learn about JavaScript values. It keeps the raw i32
/// signatures a hand-written `.wasm` guest already stands behind -- nine tests
/// in this crate lock them -- and the compiler unwraps a JS String into the
/// `(ptr, len)` pair the door takes, then rewraps the two-pass byte answer into
/// a JS String. Teaching the door the engine's `(tag: i32, payload: i64)` pair
/// would break every hand-written guest and would leak one language's value
/// representation into a boundary meant to serve any guest. Recorded in
/// `plan/design-agenterm-qjswasm.md` 6.5 as the cross-repo contract; upstream
/// carries the mechanism (`Names::Declared`) and no `agenterm` vocabulary.
///
/// # Why three declarations for four imports
///
/// `fleet_result` is a [`HostResult::Bytes`] door: a wasm function cannot
/// return a slice, so the compiler asks `fleet_result_len` how many bytes are
/// waiting, bump-allocates a string of exactly that size on the guest's own
/// heap, then has `fleet_result` fill it -- and traps unless the copy wrote
/// exactly what the length promised. That is the two-pass retrieval this door
/// was built around, expressed as one declaration. `fleet_result_len` is
/// therefore *not* a name a script may write: the second pass is the
/// compiler's business, and a script that writes it gets the capability
/// diagnostic any undeclared name gets.
///
/// # Names
///
/// The script-visible name is the field name, unchanged. `HostFn` allows a
/// rename, and taking it would mean a script author reading `plan/`'s door
/// table could not tell what to write. Renaming is for later, when object
/// support lets a `.qjs`-authored `fleet.*` wrapper sit on top of these; the
/// raw names stay the raw names underneath it.
///
/// # Order
///
/// Declaration order is import order upstream, and this order matches
/// [`SIGNATURES`] -- `print`, `fleet_call`, then the two `fleet_result`
/// passes -- so the two tables read the same way down the page. Only the
/// declarations a script actually mentions become imports.
pub(crate) fn declarations() -> Vec<HostFn> {
    vec![
        HostFn {
            name: "print".to_string(),
            module: DOOR.to_string(),
            field: "print".to_string(),
            params: vec![HostParam::StrPtrLen],
            result: HostResult::Void,
        },
        HostFn {
            name: "fleet_call".to_string(),
            module: DOOR.to_string(),
            field: "fleet_call".to_string(),
            params: vec![HostParam::StrPtrLen, HostParam::StrPtrLen],
            result: HostResult::I32,
        },
        HostFn {
            name: "fleet_result".to_string(),
            module: DOOR.to_string(),
            field: "fleet_result".to_string(),
            params: Vec::new(),
            result: HostResult::Bytes {
                length: "fleet_result_len".to_string(),
            },
        },
    ]
}

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
    /// Why the door itself failed, when the reason is longer than the
    /// `&'static str` a `tinyvm::WasmError` can carry -- today, an embedder's
    /// bridge that panicked. Written on the way out of the callback and read
    /// by `slot.rs` on the invocation's error path, which is the only way an
    /// owned diagnostic can cross a boundary whose error type is fmt-free.
    fault: Option<String>,
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
    /// The call's host-side bill; both doors charge it.
    meter: Rc<RefCell<Meter>>,
    /// The `tool.*` door's state, present only in a slot that opened it.
    tool: Option<Rc<RefCell<tool::ToolState>>>,
}

impl HostState {
    /// Every `tool.*` operation the guest reached since the previous take,
    /// fully qualified and in call order -- the receipt's line. Draining,
    /// like [`take_stdout`](Self::take_stdout), and empty in a sandbox slot.
    pub(crate) fn take_tool_calls(&self) -> Vec<String> {
        self.tool
            .as_ref()
            .map(|t| t.borrow_mut().take_calls())
            .unwrap_or_default()
    }

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
    /// Why the door failed during the call that just ended, if it recorded a
    /// reason. Draining, like [`take_stdout`](Self::take_stdout) and for the
    /// same reason: one call's failure must not be attributed to the next.
    /// `(host_ops, host_bytes, waited_ms)` since the previous take.
    pub(crate) fn take_meter(&self) -> (u64, u64, u64) {
        self.meter.borrow_mut().take()
    }

    /// The budget a door refused to exceed, if this call ended on one --
    /// read before `take_fault`, since a refusal is not a door defect.
    pub(crate) fn take_budget_refusal(&self) -> Option<&'static str> {
        self.meter.borrow_mut().take_refusal()
    }

    /// Whether this call ended because [`Budget::cancel`] was set -- read
    /// before the door fault, since a cancel is neither a defect nor a budget.
    pub(crate) fn take_cancelled(&self) -> bool {
        self.meter.borrow_mut().take_cancelled()
    }

    pub(crate) fn take_fault(&self) -> Option<String> {
        self.pending
            .borrow_mut()
            .fault
            .take()
            .or_else(|| self.tool.as_ref().and_then(|t| t.borrow_mut().take_fault()))
    }

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
///
/// `tool` opens the `tool.*` door beside this one (see `src/tool.rs`). It is
/// a parameter here and a constructor choice on [`crate::Engine`], never a
/// default: a sandbox slot must not be able to acquire it by importing it.
pub(crate) fn install(
    module: &mut tinyvm::WasmModule,
    budget: &Budget,
    bridge: Option<FleetBridgeFn>,
    tool: Option<Vec<String>>,
) -> Result<HostState, QjswasmError> {
    check_declarations(module, tool.is_some())?;
    let meter = Rc::new(RefCell::new(Meter::new(
        budget.max_host_ops,
        budget.cancel.clone(),
    )));
    let tool = match tool {
        Some(args) => Some(tool::install(module, budget, args, Rc::clone(&meter))?),
        None => None,
    };

    let pending = Rc::new(RefCell::new(Pending {
        stdout: Vec::new(),
        stdout_truncated: false,
        max_stdout: budget.max_stdout_bytes,
        result: Vec::new(),
        fault: None,
    }));

    let state = Rc::clone(&pending);
    bind(module, DOOR, "print", move |args, memory| {
        let bytes = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        let mut state = state.borrow_mut();
        state.write_stdout(bytes);
        // A line per call, matching the engine this one replaces: the previous
        // `agenterm-qjs` host appends '\n' after every `print`, and so does
        // lua's. Without it a script that prints twice gets its two lines run
        // together, which is a silent formatting change for anyone porting a
        // script across engines -- exactly the kind of difference the
        // line-for-line binding discipline exists to prevent. The newline goes
        // through `write_stdout` so it is subject to `max_stdout_bytes` like
        // any other byte, rather than sneaking past the budget.
        state.write_stdout(b"\n");
        Ok(Vec::new())
    })?;

    let state = Rc::clone(&pending);
    let max_result = budget.max_bridge_result_bytes;
    let meter_for_fleet = Rc::clone(&meter);
    bind(module, DOOR, "fleet_call", move |args, memory| {
        // Bounds-check both regions before anything else runs: an out-of-range
        // pointer must not reach the bridge, let alone read host memory.
        let op = guest_slice(memory, arg(args, 0)?, arg(args, 1)?)?;
        let params = guest_slice(memory, arg(args, 2)?, arg(args, 3)?)?;
        // On the bill before the bridge is asked, and refused past the cap.
        meter_for_fleet
            .borrow_mut()
            .charge(op.len() + params.len())
            .map_err(WasmError::Trap)?;

        let (status, payload) = match (str::from_utf8(op), str::from_utf8(params)) {
            (Err(_), _) | (_, Err(_)) => (STATUS_ERR, NOT_UTF8.as_bytes().to_vec()),
            (Ok(op), Ok(params)) => match &bridge {
                None => (STATUS_NO_BRIDGE, NO_BRIDGE.as_bytes().to_vec()),
                Some(bridge) => match call_bridge(&meter_for_fleet, bridge, op, params) {
                    Ok(Ok(answer)) => (STATUS_OK, answer.into_bytes()),
                    Ok(Err(message)) => (STATUS_ERR, message.into_bytes()),
                    // A panic is neither of the two answers the bridge is
                    // allowed to give, so it is not turned into one. Record it
                    // for `slot.rs` and fail the call.
                    Err(panic) => {
                        state.borrow_mut().fault = Some(panic);
                        return Err(WasmError::Trap(BRIDGE_PANICKED));
                    }
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
    bind(module, DOOR, "fleet_result_len", move |_args, _memory| {
        Ok(vec![Val::I32(pending_len(&state)?)])
    })?;

    let state = Rc::clone(&pending);
    bind(module, DOOR, "fleet_result", move |args, memory| {
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

    Ok(HostState {
        pending,
        meter,
        tool,
    })
}

/// Reject a guest whose door declarations do not match the ABI, at load time
/// rather than on first use.
///
/// Both cases here would otherwise surface much later and much less clearly: a
/// mistyped import as a `host argument type` trap mid-run, and an unknown
/// `agenterm.*` name as an unbound-import trap. Catching them here keeps the
/// crate's promise that load-time refusal and run-time trap are distinguishable
/// failure classes.
pub(crate) fn check_declarations(
    module: &tinyvm::WasmModule,
    tool: bool,
) -> Result<(), QjswasmError> {
    for desc in module.imports() {
        let door: &str = &desc.module;
        // An import from any other module namespace can never be bound: these
        // doors are the whole world a guest gets, and PRD 36 forbids growing
        // an OS-shaped surface (`wasi_snapshot_preview1`) beside them. It is
        // refused here, at load time, naming the import -- because the
        // alternative is what happened before: such a module validated clean
        // and then died on the core's `Trap("call to unbound imported
        // function")` at the first call, which names nothing and reads as the
        // guest's fault. "Rejected before it could run" and "trapped while
        // running" must be tellable apart.
        //
        // `tool.*` in a slot that did not open it is refused the same way but
        // says something different: the capability exists, this slot was not
        // given it. That is the sandbox rule enforced at the only place it
        // can be -- a guest's bytes cannot open a door by naming it.
        let table: &[(&str, usize, usize)] = if door == DOOR {
            &SIGNATURES
        } else if door == tool::DOOR && tool {
            &tool::SIGNATURES
        } else if door == tool::DOOR {
            return Err(QjswasmError::Door(format!(
                "guest imports `{}.{}`, but the tool door is not open in this slot: \
                 only an engine built with `Engine::with_tool_door` offers `{}.*`",
                desc.module,
                desc.field,
                tool::DOOR
            )));
        } else {
            return Err(QjswasmError::Door(format!(
                "guest imports `{}.{}`; `{DOOR}.*` is the only host module this engine \
                 offers, so nothing can bind it",
                desc.module, desc.field
            )));
        };
        let Some(&(_, params, results)) = table.iter().find(|(field, _, _)| *field == desc.field)
        else {
            return Err(QjswasmError::Door(format!(
                "guest imports unknown door function `{door}.{}`",
                desc.field
            )));
        };
        if desc.n_params != params || desc.n_results != results || !desc.i32_only {
            return Err(QjswasmError::Door(format!(
                "guest declares `{door}.{}` with the wrong signature: the door takes \
                 {params} i32 parameter(s) and returns {results}",
                desc.field
            )));
        }
    }
    Ok(())
}

/// [`bind`], with the operation on the call's bill first: one host
/// operation and its argument bytes charged to `meter` before `f` runs, and
/// the call refused with [`HOST_OPS_EXHAUSTED`] once [`Budget::max_host_ops`]
/// is spent. Every `tool.*` import goes through this; `agenterm.*` charges
/// by hand, since `print` is output and not a host operation.
///
/// The argument bytes are read off the raw `i32`s by position: the door's
/// declaration says which parameters are `(ptr, len)` pairs, so the bill
/// sees every string a script hands the host without each operation
/// having to report what it read. A negative length is charged as nothing
/// and left for the operation's own `guest_slice` to refuse.
pub(crate) fn bind_metered<F>(
    module: &mut tinyvm::WasmModule,
    meter: &Rc<RefCell<Meter>>,
    door: &str,
    field: &str,
    f: F,
) -> Result<(), QjswasmError>
where
    F: Fn(&[Val], &mut [u8]) -> Result<Vec<Val>, WasmError> + 'static,
{
    let meter = Rc::clone(meter);
    let length_slots = if door == tool::DOOR {
        tool::argument_length_slots(field)
    } else {
        Vec::new()
    };
    bind(module, door, field, move |args, memory| {
        let bytes: usize = length_slots
            .iter()
            .filter_map(|&slot| match args.get(slot) {
                Some(Val::I32(len)) => usize::try_from(*len).ok(),
                _ => None,
            })
            .sum();
        meter.borrow_mut().charge(bytes).map_err(WasmError::Trap)?;
        f(args, memory)
    })
}

pub(crate) fn bind<F>(
    module: &mut tinyvm::WasmModule,
    door: &str,
    field: &str,
    f: F,
) -> Result<(), QjswasmError>
where
    F: Fn(&[Val], &mut [u8]) -> Result<Vec<Val>, WasmError> + 'static,
{
    // `bind_import_typed` matches against the same `import_descs` that
    // `imports()` exposes and reports an absent name as an error, so asking
    // first is exact -- and avoids deciding absence by matching on a message.
    if !module
        .imports()
        .iter()
        .any(|desc| desc.module == door && desc.field == field)
    {
        return Ok(());
    }
    module.bind_import_typed(door, field, f).map_err(|error| {
        QjswasmError::Door(format!("binding `{door}.{field}`: {}", error.message()))
    })
}

/// Call an embedder's bridge without letting a panic escape into the
/// interpreter.
///
/// `Ok` is whatever the bridge answered, `Err` is the panic's message. The
/// payload of a Rust panic is `Box<dyn Any>`; the two shapes `panic!` itself
/// produces are `&'static str` and `String`, and anything else -- a
/// `panic_any` with a custom type -- is still caught, it just cannot be
/// quoted.
///
/// [`std::panic::AssertUnwindSafe`] is required and is honest here: the only
/// state reachable across the boundary is the bridge's own captures, which are
/// the embedder's to reason about, and this slot's [`Pending`], which is not
/// borrowed at the call.
fn call_bridge(
    meter: &Rc<RefCell<Meter>>,
    bridge: &FleetBridgeFn,
    op: &str,
    params: &str,
) -> Result<Result<String, String>, String> {
    let started = Instant::now();
    let answer = contain(
        &format!("the fleet bridge panicked while serving `{op}`"),
        || bridge(op, params),
    );
    // A bridge answer is a wait from the guest's side: the broker round trip
    // is where a journey's wall clock goes, and it is not a step.
    let mut meter = meter.borrow_mut();
    meter.waited(started.elapsed());
    if let Ok(Ok(text)) = &answer {
        meter.answered(text.len());
    }
    answer
}

/// Run host code on a guest's behalf without letting a panic escape into the
/// interpreter. `Ok` is what it answered; `Err` is `what`, plus the panic's
/// own message when it has one. Shared by both doors.
pub(crate) fn contain<T>(what: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|payload| {
        let said = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned());
        match said {
            Some(said) => format!("{what}: {said}"),
            None => format!("{what} (its payload is not a string, so there is nothing to quote)"),
        }
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
pub(crate) fn arg(args: &[Val], index: usize) -> Result<i32, WasmError> {
    match args.get(index) {
        Some(Val::I32(value)) => Ok(*value),
        _ => Err(WasmError::Trap("agenterm door: argument")),
    }
}

/// `[ptr, ptr+len)` inside the guest's linear memory, or a trap.
///
/// Negative values fail the conversion rather than wrapping, so a guest cannot
/// reach behind the memory slice with a sign trick.
pub(crate) fn guest_slice(memory: &[u8], ptr: i32, len: i32) -> Result<&[u8], WasmError> {
    let range = guest_range(memory.len(), ptr, len)?;
    memory
        .get(range)
        .ok_or(WasmError::Trap("agenterm door: pointer out of bounds"))
}

pub(crate) fn guest_slice_mut(
    memory: &mut [u8],
    ptr: i32,
    len: i32,
) -> Result<&mut [u8], WasmError> {
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
        let state = match install(&mut module, budget, bridge, None) {
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
        match install(&mut module, &budget, None, None) {
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
        assert_eq!(state.take_stdout(), ("pong\n".to_owned(), false));
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
        assert_eq!(state.take_stdout(), ("pong\n".to_owned(), false));
    }

    #[test]
    fn bridge_error_is_status_1_with_a_readable_message() {
        let wasm = guest(RETRIEVE_INTO_STDOUT);
        let recorder = recorder(Err("no such op: fleet.ping".to_owned()));
        let (outcome, state) = run(&wasm, &Budget::default(), Some(recorder.bridge));
        assert_eq!(returned(&outcome), STATUS_ERR);
        assert_eq!(
            state.take_stdout(),
            ("no such op: fleet.ping\n".to_owned(), false)
        );
    }

    #[test]
    fn absent_bridge_is_status_2_with_a_fixed_diagnostic() {
        let wasm = guest(RETRIEVE_INTO_STDOUT);
        let (outcome, state) = run(&wasm, &Budget::default(), None);
        assert_eq!(returned(&outcome), STATUS_NO_BRIDGE);
        assert_eq!(state.take_stdout(), (format!("{NO_BRIDGE}\n"), false));
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
            ("................\n".to_owned(), false),
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
        assert_eq!(
            state.take_stdout(),
            ("................\n".to_owned(), false)
        );
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
        assert_eq!(
            state.take_stdout(),
            ("0123456789".to_owned(), true),
            "print's newline is budgeted like any other byte, so a full buffer cuts it"
        );
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
        assert_eq!(stdout, format!("{RESULT_TOO_LARGE}\n"));
        assert!(
            !"0123456789abcdef".starts_with(stdout.trim_end()),
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
        assert_eq!(state.take_stdout(), (format!("{NOT_UTF8}\n"), false));
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
        assert_eq!(state.take_stdout(), ("a\u{fffd}b\n".to_owned(), false));
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
        assert_eq!(state.take_stdout(), ("hi\n".to_owned(), false));
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

    /// The two tables that describe one door -- the raw [`SIGNATURES`] a guest
    /// is checked against, and the [`declarations`] a `.qjs` script is compiled
    /// against -- describe the *same* door.
    ///
    /// They are separate because they answer different questions, and separate
    /// tables drift. This derives the raw signatures the declarations imply,
    /// by the rules upstream documents on `HostParam` and `HostResult`, and
    /// requires them to be exactly `SIGNATURES` -- so adding a door function to
    /// one table and not the other fails here rather than at some script's
    /// first call.
    ///
    /// `tests/qjs_door.rs::the_emitted_imports_are_exactly_the_existing_door`
    /// is the other half: this checks the declarations against the door, that
    /// one checks the *emitted wasm* against it, which is the only evidence
    /// that upstream's unwrapping does what its documentation says.
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
                // A byte result is two imports: the length pass takes the
                // declared parameters, the copy pass takes them plus `(dst, cap)`.
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

    /// Every declared name is one a guest may actually import, and the
    /// declarations carry no name the door does not answer to. Checked through
    /// `check_declarations` itself rather than against the table again, so the
    /// load gate is what agrees, not a second copy of its rules.
    #[test]
    fn every_declared_name_passes_the_load_gate() {
        for decl in declarations() {
            let arity = |n: usize| "i32 ".repeat(n);
            let wasm = wat::parse_str(format!(
                "(module (import \"{}\" \"{}\" (func (param {}) {})))",
                decl.module,
                decl.field,
                arity(
                    decl.params
                        .iter()
                        .map(|p| match p {
                            HostParam::StrPtrLen => 2,
                            HostParam::I32 | HostParam::F64 => 1,
                        })
                        .sum::<usize>()
                        + if matches!(decl.result, HostResult::Bytes { .. }) {
                            2
                        } else {
                            0
                        }
                ),
                if matches!(decl.result, HostResult::Void) {
                    ""
                } else {
                    "(result i32)"
                },
            ))
            .expect("valid wat");
            let module = tinyvm::WasmModule::from_bytes_with(&wasm, tinyvm::Limits::default())
                .unwrap_or_else(|e| panic!("load gate: {}", e.message()));
            check_declarations(&module, false).unwrap_or_else(|e| {
                panic!("the door refuses its own declaration `{}`: {e}", decl.field)
            });
        }
    }

    /// A `tool.*` import in a slot that did not open the tool door is refused
    /// at load, with a diagnostic that says the door exists and was not
    /// given -- a different sentence from the one an unknown module gets,
    /// because it is a different mistake.
    #[test]
    fn a_tool_import_is_refused_in_a_sandbox_slot_and_says_why() {
        let wasm = wat::parse_str(
            r#"(module
                (import "tool" "fs.exists" (func $exists (param i32 i32) (result i32)))
                (memory 1)
                (func (export "main") (result i32) (i32.const 3)))"#,
        )
        .expect("valid wat");
        let error = install_error(&wasm);
        assert!(
            matches!(&error, QjswasmError::Door(message)
                if message.contains("tool.fs.exists") && message.contains("with_tool_door")),
            "expected a Door diagnostic naming the import and the constructor, got {error:?}"
        );
        // And the same bytes load once the door is open.
        let budget = Budget::default();
        let mut module = load(&wasm, &budget);
        install(&mut module, &budget, None, Some(Vec::new())).expect("the tool door binds it");
    }

    /// Every tool declaration passes the load gate of a slot that opened the
    /// door, through `check_declarations` itself.
    #[test]
    fn every_tool_declaration_passes_the_load_gate_when_the_door_is_open() {
        for decl in tool::declarations() {
            let raw: usize = decl
                .params
                .iter()
                .map(|p| match p {
                    HostParam::StrPtrLen => 2,
                    HostParam::I32 | HostParam::F64 => 1,
                })
                .sum::<usize>()
                + if matches!(decl.result, HostResult::Bytes { .. }) {
                    2
                } else {
                    0
                };
            let wasm = wat::parse_str(format!(
                "(module (import \"{}\" \"{}\" (func (param {}) (result i32))))",
                decl.module,
                decl.field,
                "i32 ".repeat(raw),
            ))
            .expect("valid wat");
            let module = tinyvm::WasmModule::from_bytes_with(&wasm, tinyvm::Limits::default())
                .unwrap_or_else(|e| panic!("load gate: {}", e.message()));
            check_declarations(&module, true).unwrap_or_else(|e| {
                panic!(
                    "the open door refuses its own declaration `{}`: {e}",
                    decl.field
                )
            });
            check_declarations(&module, false)
                .expect_err("the same declaration is refused with the door shut");
        }
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
        assert_eq!(state.take_stdout(), ("abc\n".to_owned(), false));
        assert_eq!(state.take_stdout(), (String::new(), false));
    }
}
