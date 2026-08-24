//! Load through tinyvm's gate, run for real, read the numbers back.
//!
//! Criterion 1 is a boolean gate with two halves, and this file is where both
//! are enforced: `Module::from_bytes_with` is the strict load gate, and
//! `invoke_by_name` is the actual execution. "My encoder thinks it is correct"
//! is not evidence, so no product reaches the results table without both.

use tinyvm::{Limits, Val, WasmInstance, WasmModule};

use crate::repr::{Expect, HostVal, Repr};

/// One executed call: what it returned and what it cost.
#[derive(Clone, Debug)]
pub struct Run {
    pub value: HostVal,
    /// Resolved string content, when the value was a string.
    pub text: Option<String>,
    pub steps: u64,
    pub peak_activation_slots: usize,
    pub peak_call_depth: usize,
}

/// Host budget for every run in this experiment. Stated once so the numbers in
/// `RESULTS.md` all share one measurement condition.
pub fn limits() -> Limits {
    Limits::default()
}

/// Load and instantiate. Fails exactly where tinyvm's load gate fails.
pub fn instantiate(wasm: &[u8]) -> Result<WasmInstance, String> {
    let module = WasmModule::from_bytes_with(wasm, limits())
        .map_err(|e| format!("load gate rejected the product: {}", e.message()))?;
    module
        .instantiate()
        .map_err(|e| format!("instantiate failed: {}", e.message()))
}

pub fn run(wasm: &[u8], repr: &dyn Repr, args: &[Val]) -> Result<Run, String> {
    let mut instance = instantiate(wasm)?;
    let out = instance
        .invoke_by_name("main", args)
        .map_err(|e| format!("trap during main: {}", e.message()))?;
    let value = repr.host_decode(&out)?;
    let text = match value {
        HostVal::StrPtr(ptr) => Some(read_string(&instance, ptr)?),
        _ => None,
    };
    Ok(Run {
        value,
        text,
        steps: instance.last_steps(),
        peak_activation_slots: instance.last_peak_activation_slots(),
        peak_call_depth: instance.last_peak_call_depth(),
    })
}

/// `[len: i32][bytes]` in the guest heap. The layout is representation
/// independent -- both variants hand back the same pointer to the same bytes.
fn read_string(instance: &WasmInstance, ptr: i32) -> Result<String, String> {
    let view = instance
        .memory()
        .map_err(|e| format!("no guest memory: {}", e.message()))?;
    let at = ptr as usize;
    let bytes: &[u8] = &view;
    let len_field = bytes
        .get(at..at + 4)
        .ok_or_else(|| format!("string header at {ptr} is out of bounds"))?;
    let len = u32::from_le_bytes([len_field[0], len_field[1], len_field[2], len_field[3]]) as usize;
    let body = bytes
        .get(at + 4..at + 4 + len)
        .ok_or_else(|| format!("string body at {ptr} (len {len}) is out of bounds"))?;
    String::from_utf8(body.to_vec()).map_err(|_| "string is not valid UTF-8".to_string())
}

/// Compare one run against the shared expected-value table.
///
/// Numbers are compared bit-exactly. `-0` must not satisfy `+0`: the sign of a
/// zero is observable in JavaScript, and a representation that loses it has
/// lost information, not rounded it.
pub fn matches(run: &Run, want: &Expect) -> bool {
    match (&run.value, want) {
        (HostVal::Undefined, Expect::Undefined) => true,
        (HostVal::Number(got), Expect::Number(w)) => got.to_bits() == w.to_bits(),
        (HostVal::Number(got), Expect::NumberBits(w)) => got.to_bits() == *w,
        (HostVal::Bool(got), Expect::Bool(w)) => got == w,
        (HostVal::StrPtr(_), Expect::Str(w)) => run.text.as_deref() == Some(*w),
        _ => false,
    }
}

pub fn describe(run: &Run) -> String {
    match &run.value {
        HostVal::Undefined => "undefined".to_string(),
        HostVal::Number(x) => format!("{x} (bits {:#018x})", x.to_bits()),
        HostVal::Bool(b) => format!("{b}"),
        HostVal::StrPtr(p) => format!("{:?} @ {p}", run.text.as_deref().unwrap_or("<unread>")),
    }
}
