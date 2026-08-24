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
/// tinyvm is `no_std` and fmt-free, so `WasmError` carries a `&'static str` and
/// nothing else -- there is no structured "this was a limit" flag to read.
/// Matching the message is therefore the only mechanism available, and it is
/// exact rather than a heuristic over free text: each literal below is emitted
/// by the limit checks in `wasm.rs` and by nothing else.
///
/// - `"step budget"` -- the per-top-level-call step counter passed `max_steps`.
/// - `"call depth"` -- a guest call passed `max_call_depth`.
/// - `"call stack"` -- activation storage passed `max_activation_slots`. This
///   one is not a clean 1:1: tinyvm reuses it for its fixed `WASM_STACK_LIMIT`
///   operand-stack ceiling and for internal `checked_add` overflows on slot
///   accounting. All three are resource exhaustion, so `Budget` is the honest
///   class, but a caller cannot tell *which* ceiling it hit.
///
/// Deliberately **not** mapped to `Budget`: `Trap("memory size")`, which
/// instantiation raises both for "declared minimum exceeds `max_memory_pages`"
/// and for a plain allocation failure or size overflow. Calling that a budget
/// exhaustion would be a guess, so it stays an ordinary trap. Everything else
/// is an ordinary guest fault.
fn classify(error: tinyvm::WasmError) -> QjswasmError {
    match error {
        tinyvm::WasmError::Decode(_) => QjswasmError::Load(error),
        tinyvm::WasmError::Trap(message) => match message {
            "step budget" | "call depth" | "call stack" => QjswasmError::Budget(message),
            _ => QjswasmError::Trap(error),
        },
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
