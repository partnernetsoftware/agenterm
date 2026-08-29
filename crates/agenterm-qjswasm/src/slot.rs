//! One slot: a persistent tinyvm instance with its own budget and host door.
//!
//! The shape here is dictated by tinyvm's execution model, not by taste:
//!
//! - `WasmModule::from_bytes_with` decodes *and* validates against [`Budget`]'s
//!   `Limits`, so a module that declares more memory or table space than the
//!   host allows is rejected before a single instruction runs.
//! - The limits travel with the decoded module into its `WasmInstance`, and the
//!   core resets the step counter at every *top-level* call. That is the whole
//!   reason `spawn` and `call` are separate on the public face: one load, many
//!   calls, a fresh `max_steps` each time. `eval_wasm` cannot express it.
//! - `Module::instantiate` already runs the start function (`Instance::new`
//!   invokes `module.start` after applying data segments and initial globals),
//!   so calling `Module::run_start` here as well would run it twice -- once
//!   against the module's throwaway instantiation state and once for real.
//!
//! A slot owns its instance and its host-door state together, so `Engine::kill`
//! dropping the slot drops the guest's linear memory, its pending bridge
//! buffer, and its captured bridge closure in one move.

use crate::host::{self, HostState};
use crate::{Budget, FleetBridgeFn, JsValue, Outcome, QjswasmError, Value};

/// Which calling convention this slot's entry points speak.
///
/// Not a property of the wasm bytes -- both conventions are ordinary wasm --
/// but of where the bytes came from, so it is recorded at load time and can
/// never be re-derived by guessing at a signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Convention {
    /// A hand-written guest: parameters and results are plain wasm numerics.
    Wasm,
    /// A compiled `.qjs` guest: every JavaScript value is a `(tag: i32,
    /// payload: i64)` pair, so the entry takes two wasm parameters per
    /// argument and returns two results.
    JsV1,
}

pub(crate) struct Slot {
    instance: tinyvm::WasmInstance,
    /// Kept alive for the lifetime of the slot: the door's closures share this
    /// state, so dropping it early would strand the guest's imports.
    door: HostState,
    convention: Convention,
}

impl Slot {
    pub(crate) fn load(
        bytes: &[u8],
        budget: &Budget,
        bridge: Option<FleetBridgeFn>,
        convention: Convention,
    ) -> Result<Self, QjswasmError> {
        let mut module = tinyvm::WasmModule::from_bytes_with(bytes, budget.limits)
            .map_err(QjswasmError::Load)?;
        let door = host::install(&mut module, budget, bridge)?;
        // Instantiation applies data segments and initial globals and runs the
        // start function, so a guest whose start traps or overruns its budget
        // fails here -- classified like any other execution fault rather than
        // reported as a malformed module.
        let instance = module.instantiate().map_err(classify)?;
        Ok(Self {
            instance,
            door,
            convention,
        })
    }

    pub(crate) fn call(
        &mut self,
        entry: &str,
        args: &[Value],
        budget: &Budget,
    ) -> Result<Outcome, QjswasmError> {
        // The *core* budget is not re-applied per call: `from_bytes_with` baked
        // the `Limits` into the module, the instance carries them, and the core
        // hands every top-level call a fresh `max_steps` on its own. The
        // host-side caps in `budget` are this layer's business, and one of them
        // -- `max_result_string_bytes` -- is spent below, on the way out.
        let vals = match self.convention {
            Convention::Wasm => args.iter().map(into_val).collect::<Result<Vec<_>, _>>()?,
            Convention::JsV1 => into_v1_args(args)?,
        };
        self.check_entry(entry, &vals, args.len())?;
        let result = self.instance.invoke_by_name(entry, &vals);

        // Read the cost counters before doing anything else with the result:
        // they are recorded for a call that trapped too, and a later invocation
        // would overwrite them.
        let steps = self.instance.last_steps();
        let peak_call_depth = self.instance.last_peak_call_depth();
        let peak_activation_slots = self.instance.last_peak_activation_slots();

        // Drain stdout unconditionally, including on the error paths below.
        //
        // This is a stated cost, not an emergent one: a call that fails *after*
        // the guest has run -- a trap, a budget, a malformed V1 pair -- loses
        // what it printed, because there is nowhere to put it on the `Result`
        // face and leaving it buffered would attribute it to the *next* call's
        // `Outcome`, which is worse than losing it.
        //
        // The avoidable half of that cost is gone. Refusals the face can decide
        // in advance -- no such export, a signature mismatch, a result type
        // this face cannot carry -- are raised by `check_entry` above, before
        // the guest is entered, so there is no output to lose. What remains is
        // only failures that could not be known until the guest had already
        // run.
        let (stdout, truncated_stdout) = self.door.take_stdout();

        let returned = match result {
            Ok(values) => values,
            Err(fault) => return Err(self.explain(fault)),
        };
        let values = match self.convention {
            Convention::Wasm => returned
                .into_iter()
                .map(from_val)
                .collect::<Result<Vec<Value>, QjswasmError>>()?,
            // A JavaScript function returns one value however many wasm words
            // carry it, and the String case has to be read out of linear
            // memory *now*: `Engine::run_once` drops this instance before its
            // caller sees the result, so a pointer would already be dangling.
            Convention::JsV1 => vec![Value::Js(self.read_js_value(&returned, budget)?)],
        };

        Ok(Outcome {
            values,
            stdout,
            truncated_stdout,
            steps,
            peak_call_depth,
            peak_activation_slots,
        })
    }

    /// Refuse a call that does not fit the export it names, *before* the guest
    /// is entered.
    ///
    /// Three separate misreports collapse into this one check, and all three
    /// used to be blamed on the guest:
    ///
    /// - **No such export.** The core answers `Trap("no exported function
    ///   named")`, without the name, because it is `no_std` and its messages
    ///   are static prefixes. The guest never ran; the caller mistyped.
    /// - **The wrong number of arguments.** `Trap("function")`, again for a
    ///   guest that did nothing wrong. This crate already guards the *other*
    ///   half of the same mistake -- a raw numeric handed to a `.qjs` entry is
    ///   `UnsupportedValue`, precisely so an arity mismatch is not reported as
    ///   a trap -- and the count half was simply missed.
    /// - **An argument of the wrong type.** This one did not fail at all. The
    ///   core notices only where the value is *used*, so `(param i32)` handed
    ///   an `I64` returned `Ok([I64(..)])` -- a result contradicting the
    ///   export's own `(result i32)` -- and became a trap only if the guest
    ///   happened to touch it. Whether a host type error is reported must not
    ///   depend on what the guest does with the value.
    ///
    /// The declared type is available and exact: `exported_function_handle`
    /// carries the function type, parameters and results both. Consulting it
    /// costs one lookup per call and is the only place that can tell these
    /// three apart from a real fault.
    ///
    /// Results are checked here too, so a declared `funcref`/`externref`/`v128`
    /// result is refused before the guest runs rather than after. That is not
    /// tidiness: the guest may have printed on its way to returning, and a
    /// refusal issued afterwards drains that output to nowhere. Refusing first
    /// means there is nothing to lose. [`from_val`] keeps its own arm as the
    /// backstop for a core whose declared and actual result types disagree.
    fn check_entry(
        &self,
        entry: &str,
        vals: &[tinyvm::Val],
        js_arg_count: usize,
    ) -> Result<(), QjswasmError> {
        let handle = self
            .instance
            .exported_function_handle(entry)
            .map_err(classify)?
            .ok_or_else(|| {
                QjswasmError::NoSuchExport(format!("this slot exports no function named `{entry}`"))
            })?;

        if handle.parameter_count() != vals.len() {
            return Err(QjswasmError::Signature(match self.convention {
                // Say it in the units the caller counted in. A `.qjs` caller
                // passed JavaScript values and never chose two wasm words per
                // value; reporting "expected 4 parameters, got 2" would be
                // arithmetic they have to undo.
                Convention::JsV1 => format!(
                    "`{entry}` takes {} JavaScript argument(s), {js_arg_count} given",
                    handle.parameter_count() / V1_WORDS_PER_VALUE
                ),
                Convention::Wasm => format!(
                    "`{entry}` takes {} wasm parameter(s), {} given",
                    handle.parameter_count(),
                    vals.len()
                ),
            }));
        }

        for (index, val) in vals.iter().enumerate() {
            let declared = handle.parameter_type(index);
            let given = value_type_of(val);
            if declared.is_none() || declared != given {
                return Err(QjswasmError::Signature(format!(
                    "`{entry}` parameter {index}: the signature declares {}, the caller gave {}",
                    declared.map_or("an unsupported type", type_name),
                    given.map_or("an unsupported value", type_name)
                )));
            }
        }

        for index in 0..handle.result_count() {
            match handle.result_type(index) {
                Some(
                    tinyvm::ValueType::I32
                    | tinyvm::ValueType::I64
                    | tinyvm::ValueType::F32
                    | tinyvm::ValueType::F64,
                ) => {}
                _ => {
                    return Err(QjswasmError::UnsupportedValue(
                        "the export declares a reference or vector result type",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Say what a failed invocation actually was, consulting the two parties
    /// that know more than the core does before falling back to it.
    ///
    /// The core sees one thing: the guest executed `unreachable`, or a ceiling
    /// was hit. Two failures are indistinguishable from an ordinary guest fault
    /// at that level and are not the guest's fault at all, so each is asked
    /// about first, in the order of who is nearest:
    ///
    /// 1. **The door.** A host callback that failed for a reason the door
    ///    recorded -- today, an embedder's bridge that panicked -- left the
    ///    explanation in the slot's own state on its way out, because a
    ///    `tinyvm::WasmError` carries only a `&'static str` and cannot hold
    ///    it. That is a [`QjswasmError::Door`]: the boundary failed, not the
    ///    script.
    /// 2. **The guest.** A compiled `.qjs` guest writes down *why* it is about
    ///    to fail, in the first word of its own linear memory, before it
    ///    executes the `unreachable` the core will report. Two reasons are
    ///    recorded there today and neither is "this script is broken":
    ///
    ///    - the bump heap could not grow -- a refused `memory.grow` returns
    ///      `-1` rather than trapping, so the allocator has nowhere else to
    ///      put the reason -- which is a budget the embedder can raise;
    ///    - the script threw a value and nothing caught it, which is the
    ///      script running exactly as ECMA-262 says it should and is neither
    ///      a budget nor a defect.
    ///
    ///    [`tinyvm_qjs::guest_fault`] reads the word back. Asking it is not
    ///    optional politeness: the word exists *only* so a host can tell these
    ///    apart, and a host that has the answer and reports a bare trap anyway
    ///    is guessing when the guest already told it.
    ///
    /// Only a `JsV1` slot is asked the second question. The fault word is a
    /// convention of the compiler's runtime; a hand-written guest's byte zero
    /// means whatever that guest decided it means, and reading either answer
    /// out of it would be exactly the host-side guess this crate refuses to
    /// make.
    ///
    /// Everything else goes to [`classify`], unchanged -- including a `JsV1`
    /// guest that recorded nothing, which upstream documents as the honest
    /// answer for three situations at once (an ordinary fault, a module with
    /// no memory, a call that never started) and none of them is a throw.
    fn explain(&self, fault: tinyvm::WasmError) -> QjswasmError {
        if let Some(door) = self.door.take_fault() {
            return QjswasmError::Door(door);
        }
        if self.convention == Convention::JsV1 {
            match self.guest_fault() {
                Some(tinyvm_qjs::GuestFault::HeapExhausted) => {
                    return QjswasmError::Budget("max_memory_pages");
                }
                Some(tinyvm_qjs::GuestFault::UncaughtThrow) => {
                    return QjswasmError::UncaughtThrow;
                }
                // The fourth reason the comment here anticipated, which
                // arrived at tinyvm `ec67034`. Reported as its own thing and
                // not as a trap: a trap says "the guest stopped and the core
                // does not know why", which for a capability boundary sends
                // the reader looking for a defect that is not there.
                Some(tinyvm_qjs::GuestFault::CapabilityBoundary) => {
                    return QjswasmError::CapabilityBoundary;
                }
                // `GuestFault` is `#[non_exhaustive]`: a later upstream may
                // record a *fifth* reason at the same word. Falling through to
                // `classify` stays the right default -- a reason this build
                // does not understand is reported as the trap the core saw,
                // which is true if unhelpful, rather than mapped onto
                // whichever known arm happens to be nearest.
                _ => {}
            }
        }
        classify(fault)
    }

    /// What the guest recorded about its own failure, if anything.
    ///
    /// `None` when the memory cannot be read at all: a guest with no linear
    /// memory never allocated and never threw, and inventing either answer for
    /// a module that has no heap would misreport a genuine fault.
    fn guest_fault(&self) -> Option<tinyvm_qjs::GuestFault> {
        let Ok(Some(view)) = self.instance.memory_at(0) else {
            return None;
        };
        tinyvm_qjs::guest_fault(&view)
    }

    /// Decode one V1 pair and resolve a String out of the guest's memory.
    fn read_js_value(
        &self,
        vals: &[tinyvm::Val],
        budget: &Budget,
    ) -> Result<JsValue, QjswasmError> {
        // A malformed pair is not the script's fault and not this face's
        // limitation: it means the module in the slot did not honour the
        // calling convention it was compiled to speak. That is a boundary
        // contract, so it is reported as one.
        let decoded = tinyvm_qjs::Value::returned(vals)
            .map_err(|e| QjswasmError::Door(format!("the `.qjs` entry point returned {e}")))?;
        Ok(match decoded {
            tinyvm_qjs::Value::Undefined => JsValue::Undefined,
            tinyvm_qjs::Value::Null => JsValue::Null,
            tinyvm_qjs::Value::Bool(b) => JsValue::Bool(b),
            tinyvm_qjs::Value::Number(x) => JsValue::Number(x),
            tinyvm_qjs::Value::String(pointer) => {
                JsValue::Str(self.read_guest_string(pointer, budget)?)
            }
        })
    }

    /// Read a `[len: i32][utf8 bytes]` record out of the guest's memory.
    ///
    /// Every failure here is the guest handing over a pointer that does not
    /// describe a string, so all of them are `Door`: the host reads only what
    /// the guest pointed at, in bounds, and refuses rather than inventing
    /// text.
    fn read_guest_string(&self, pointer: i32, budget: &Budget) -> Result<String, QjswasmError> {
        // `memory_at(0)` rather than `memory()`: the latter substitutes an
        // empty view for an absent memory, which would report "a guest with no
        // memory at all" as an ordinary out-of-bounds header. The two are
        // different mistakes and the caller should be able to tell them apart.
        let view = self
            .instance
            .memory_at(0)
            .map_err(|e| {
                QjswasmError::Door(format!(
                    "reading a `.qjs` guest's linear memory: {}",
                    e.message()
                ))
            })?
            .ok_or_else(|| {
                QjswasmError::Door(
                    "a `.qjs` guest returned a string but declares no linear memory".to_string(),
                )
            })?;
        let bytes: &[u8] = &view;
        let at = usize::try_from(pointer).map_err(|_| {
            QjswasmError::Door(format!("string pointer {pointer} is not a guest address"))
        })?;
        let header = bytes
            .get(at..at + 4)
            .ok_or_else(|| QjswasmError::Door(format!("string header at {at} is out of bounds")))?;
        let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let body = bytes.get(at + 4..at + 4 + len).ok_or_else(|| {
            QjswasmError::Door(format!("string body at {at} (len {len}) is out of bounds"))
        })?;
        // The host-side cap, after the bounds check and before the copy.
        //
        // After, because order is a classification: a length that does not fit
        // the guest's own memory is a broken guest (`Door`), and calling that a
        // budget would tell the embedder to raise a number that would not have
        // helped. Before the copy, because this allocation is the host's, sized
        // by the guest, and no core `Limits` field bounds it -- slicing costs
        // nothing, `to_vec` is the megabyte. Refused rather than truncated, for
        // the reason `max_bridge_result_bytes` is: half a string is worse than
        // a refusal, because the caller cannot tell it was cut.
        if body.len() > budget.max_result_string_bytes {
            return Err(QjswasmError::Budget("max_result_string_bytes"));
        }
        String::from_utf8(body.to_vec())
            .map_err(|_| QjswasmError::Door("a returned string is not valid UTF-8".to_string()))
    }
}

/// Flatten JavaScript arguments into the wasm values a compiled entry takes.
///
/// A String argument is refused rather than faked. Handing one in means
/// writing a string record into the guest's heap and moving its bump pointer,
/// which is the guest allocator's job and has no host-side door yet; a
/// fabricated pointer would be read as whatever bytes happen to be there.
fn into_v1_args(args: &[Value]) -> Result<Vec<tinyvm::Val>, QjswasmError> {
    let mut js = Vec::with_capacity(args.len());
    for arg in args {
        let Value::Js(value) = arg else {
            return Err(QjswasmError::UnsupportedValue(
                "a `.qjs` guest takes JavaScript values; this is a raw wasm numeric",
            ));
        };
        js.push(match value {
            JsValue::Undefined => tinyvm_qjs::Value::Undefined,
            JsValue::Null => tinyvm_qjs::Value::Null,
            JsValue::Bool(b) => tinyvm_qjs::Value::Bool(*b),
            JsValue::Number(x) => tinyvm_qjs::Value::Number(*x),
            JsValue::Str(_) => {
                return Err(QjswasmError::UnsupportedValue(
                    "a string argument would have to be allocated in the guest's heap, \
                     and this face has no door onto that allocator yet",
                ));
            }
        });
    }
    Ok(tinyvm_qjs::Value::args(&js))
}

/// Sort one tinyvm fault raised at instantiation or call time into the crate's
/// error classes.
///
/// This reads [`tinyvm::WasmError::class`] and [`tinyvm::WasmError::ceiling`], never the
/// message text. That is not a style preference -- it is the fix for a real
/// near-miss. This function used to carry its own table of message literals
/// (`"step budget" | "call depth" | "call stack"`). Upstream then split
/// `"call stack"` into four distinct conditions, and the moment the revision
/// pin moved, activation-slot exhaustion would have silently reclassified from
/// `Budget` to `Trap`: no compile error, and the existing tests still green,
/// because they only asserted the two literals that happened to survive.
/// Accessors cannot drift that way -- a renamed message changes nothing here,
/// and a new ceiling arrives as a fault class this match already handles.
///
/// The mapping:
///
/// - [`tinyvm::WasmFaultClass::ResourceCeiling`] -> [`QjswasmError::Budget`]. A limit the
///   embedder chose was reached, including the VM's own fixed operand-stack
///   bound, which [`tinyvm::WasmError::ceiling`] reports as `None` because no `Limits`
///   field controls it. Either way the guest was too expensive for the room it
///   was given, which is what the caller needs to know.
/// - [`tinyvm::WasmFaultClass::Load`] -> [`QjswasmError::Load`]. Rejected before it could
///   run.
/// - Everything else -> [`QjswasmError::Trap`]: an ordinary guest fault, an
///   allocation refusal, or a VM invariant. None of those is the embedder's
///   budget, and calling them one would be a guess.
fn classify(error: tinyvm::WasmError) -> QjswasmError {
    match error.class() {
        tinyvm::WasmFaultClass::ResourceCeiling => QjswasmError::Budget(ceiling_name(&error)),
        tinyvm::WasmFaultClass::Load => QjswasmError::Load(error),
        tinyvm::WasmFaultClass::Allocation
        | tinyvm::WasmFaultClass::Guest
        | tinyvm::WasmFaultClass::Internal => QjswasmError::Trap(error),
    }
}

/// Which budget ran out, named after the [`tinyvm::Limits`] field the embedder
/// would raise -- not after the core's trap wording, so the text this crate
/// reports stays stable across upstream rewordings.
fn ceiling_name(error: &tinyvm::WasmError) -> &'static str {
    match error.ceiling() {
        Some(tinyvm::WasmCeiling::Steps) => "max_steps",
        Some(tinyvm::WasmCeiling::CallDepth) => "max_call_depth",
        Some(tinyvm::WasmCeiling::ActivationSlots) => "max_activation_slots",
        Some(tinyvm::WasmCeiling::MemoryPages) => "max_memory_pages",
        Some(tinyvm::WasmCeiling::TableElems) => "max_table_elems",
        // A ceiling with no `Limits` field behind it: the core's own fixed
        // operand-stack bound. Still exhaustion, but raising a number will not
        // help, so say which one it is rather than name a field that is not
        // there.
        None => "the core's fixed operand-stack bound",
    }
}

/// Wasm words per JavaScript value in the V1 calling convention: the `(tag:
/// i32, payload: i64)` pair.
const V1_WORDS_PER_VALUE: usize = 2;

/// The declared type of one runtime value, or `None` for a value this crate's
/// [`Value`] face cannot build.
///
/// Hand-written rather than an upstream accessor because the core has none:
/// `Val` and `ValueType` are separate types there and neither implements
/// `Debug` outside tinyvm's own test build, which is also why [`type_name`]
/// exists. `None` is unreachable from the public face today -- every `Val` here
/// came from [`into_val`] or [`into_v1_args`], which emit only the four numeric
/// types -- and it is `None` rather than a stand-in reference type so that a
/// diagnostic never names a type nobody passed.
fn value_type_of(value: &tinyvm::Val) -> Option<tinyvm::ValueType> {
    Some(match value {
        tinyvm::Val::I32(_) => tinyvm::ValueType::I32,
        tinyvm::Val::I64(_) => tinyvm::ValueType::I64,
        tinyvm::Val::F32(_) => tinyvm::ValueType::F32,
        tinyvm::Val::F64(_) => tinyvm::ValueType::F64,
        _ => return None,
    })
}

/// The wasm spelling of a value type, for a diagnostic a caller can act on.
///
/// Exhaustive on purpose and with no wildcard: a value type this crate has
/// never seen -- `v128` under tinyvm's `simd` feature, or whatever the core
/// gains next -- should arrive as a compile error asking what to call it, not
/// as a diagnostic quietly naming the wrong thing.
fn type_name(value_type: tinyvm::ValueType) -> &'static str {
    match value_type {
        tinyvm::ValueType::I32 => "i32",
        tinyvm::ValueType::I64 => "i64",
        tinyvm::ValueType::F32 => "f32",
        tinyvm::ValueType::F64 => "f64",
        tinyvm::ValueType::FuncRef => "funcref",
        tinyvm::ValueType::ExternRef => "externref",
    }
}

/// The wasm direction. A [`JsValue`] has no meaning here: a hand-written
/// module's signature says `i32`, not "one JavaScript value", so there is
/// nothing to reinterpret it as.
fn into_val(value: &Value) -> Result<tinyvm::Val, QjswasmError> {
    Ok(match value {
        Value::I32(v) => tinyvm::Val::I32(*v),
        Value::I64(v) => tinyvm::Val::I64(*v),
        Value::F32(v) => tinyvm::Val::F32(*v),
        Value::F64(v) => tinyvm::Val::F64(*v),
        Value::Js(_) => {
            return Err(QjswasmError::UnsupportedValue(
                "a hand-written wasm guest takes wasm numerics; this is a JavaScript value",
            ));
        }
    })
}

/// The other direction is partial: tinyvm's `Val` also covers `funcref`,
/// `externref` and (under its `simd` feature) `v128`, none of which the engine
/// face exchanges with callers. A guest that returns one is refused rather than
/// silently coerced.
fn from_val(value: tinyvm::Val) -> Result<Value, QjswasmError> {
    match value {
        tinyvm::Val::I32(v) => Ok(Value::I32(v)),
        tinyvm::Val::I64(v) => Ok(Value::I64(v)),
        tinyvm::Val::F32(v) => Ok(Value::F32(v)),
        tinyvm::Val::F64(v) => Ok(Value::F64(v)),
        // Not a trap: the guest returned a valid wasm value, and it is this
        // engine's neutral `Value` projection that cannot carry it. Blaming the
        // guest for the face's limit would misreport who is at fault.
        _ => Err(QjswasmError::UnsupportedValue(
            "export returned a reference or vector value type",
        )),
    }
}
