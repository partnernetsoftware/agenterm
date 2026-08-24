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
        _budget: &Budget,
    ) -> Result<Outcome, QjswasmError> {
        // The budget is not re-applied per call: `from_bytes_with` baked the
        // `Limits` into the module, the instance carries them, and the core
        // hands every top-level call a fresh `max_steps` on its own. The
        // parameter stays on the signature because it is the frozen face and
        // because the host-side caps are the door's business, not the core's.
        let vals = match self.convention {
            Convention::Wasm => args.iter().map(into_val).collect::<Result<Vec<_>, _>>()?,
            Convention::JsV1 => into_v1_args(args)?,
        };
        let result = self.instance.invoke_by_name(entry, &vals);

        // Read the cost counters before doing anything else with the result:
        // they are recorded for a call that trapped too, and a later invocation
        // would overwrite them.
        let steps = self.instance.last_steps();
        let peak_call_depth = self.instance.last_peak_call_depth();
        let peak_activation_slots = self.instance.last_peak_activation_slots();

        // Drain stdout unconditionally, including on the error paths below.
        // Output a trapping call already emitted has nowhere to go on the
        // `Result` face, but leaving it buffered would leak it into the *next*
        // call's `Outcome`, which is worse than losing it.
        let (stdout, truncated_stdout) = self.door.take_stdout();

        let returned = result.map_err(classify)?;
        let values = match self.convention {
            Convention::Wasm => returned
                .into_iter()
                .map(from_val)
                .collect::<Result<Vec<Value>, QjswasmError>>()?,
            // A JavaScript function returns one value however many wasm words
            // carry it, and the String case has to be read out of linear
            // memory *now*: `Engine::run_once` drops this instance before its
            // caller sees the result, so a pointer would already be dangling.
            Convention::JsV1 => vec![Value::Js(self.read_js_value(&returned)?)],
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

    /// Decode one V1 pair and resolve a String out of the guest's memory.
    fn read_js_value(&self, vals: &[tinyvm::Val]) -> Result<JsValue, QjswasmError> {
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
            tinyvm_qjs::Value::String(pointer) => JsValue::Str(self.read_guest_string(pointer)?),
        })
    }

    /// Read a `[len: i32][utf8 bytes]` record out of the guest's memory.
    ///
    /// Every failure here is the guest handing over a pointer that does not
    /// describe a string, so all of them are `Door`: the host reads only what
    /// the guest pointed at, in bounds, and refuses rather than inventing
    /// text.
    fn read_guest_string(&self, pointer: i32) -> Result<String, QjswasmError> {
        let view = self.instance.memory().map_err(|e| {
            QjswasmError::Door(format!(
                "a `.qjs` guest returned a string but has no linear memory: {}",
                e.message()
            ))
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
