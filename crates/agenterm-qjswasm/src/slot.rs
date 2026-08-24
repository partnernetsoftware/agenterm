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
use crate::{Budget, FleetBridgeFn, Outcome, QjswasmError, Value};

pub(crate) struct Slot {
    instance: tinyvm::WasmInstance,
    /// Kept alive for the lifetime of the slot: the door's closures share this
    /// state, so dropping it early would strand the guest's imports.
    door: HostState,
}

impl Slot {
    pub(crate) fn load(
        bytes: &[u8],
        budget: &Budget,
        bridge: Option<FleetBridgeFn>,
    ) -> Result<Self, QjswasmError> {
        let mut module = tinyvm::WasmModule::from_bytes_with(bytes, budget.limits)
            .map_err(QjswasmError::Load)?;
        let door = host::install(&mut module, budget, bridge)?;
        // Instantiation applies data segments and initial globals and runs the
        // start function, so a guest whose start traps or overruns its budget
        // fails here -- classified like any other execution fault rather than
        // reported as a malformed module.
        let instance = module.instantiate().map_err(classify)?;
        Ok(Self { instance, door })
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
        let vals: Vec<tinyvm::Val> = args.iter().copied().map(into_val).collect();
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

        let values = result
            .map_err(classify)?
            .into_iter()
            .map(from_val)
            .collect::<Result<Vec<Value>, QjswasmError>>()?;

        Ok(Outcome {
            values,
            stdout,
            truncated_stdout,
            steps,
            peak_call_depth,
            peak_activation_slots,
        })
    }
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

fn into_val(value: Value) -> tinyvm::Val {
    match value {
        Value::I32(v) => tinyvm::Val::I32(v),
        Value::I64(v) => tinyvm::Val::I64(v),
        Value::F32(v) => tinyvm::Val::F32(v),
        Value::F64(v) => tinyvm::Val::F64(v),
    }
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
