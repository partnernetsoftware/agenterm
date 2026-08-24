//! AgenTerm's own script engine: `.qjs` compiled to `.wasm` in pure Rust, both
//! executed on [`tinyvm`] with no JIT.
//!
//! Product truth: `prd/PRD_02_36_agenterm_qjswasm.md`. Implementation design:
//! `plan/design-agenterm-qjswasm.md`. Execution goal: `plan/goal-agenterm-qjswasm.md`.
//!
//! ```text
//! .qjs source
//!    │  lex / parse / lower        (tinyvm-qjs, pure Rust, ECMA-262 as authority)
//!    ▼
//! standard .wasm bytes
//!    │  decode / validate / Limits (tinyvm)
//!    ▼
//! interpreted execution, no machine code
//! ```
//!
//! A `.wasm` input skips the first stage and enters at the second. Both inputs
//! get identical treatment at the core -- that is the exact meaning of "one
//! engine runs both", as opposed to two pipelines sharing a name.
//!
//! # Where the compiler lives, and why it is not here
//!
//! It was here, in `src/lower/**`, until 2026-08-24. It moved up into
//! [`tinyvm_qjs`] because it contained no agenterm vocabulary at all: lex,
//! parse, IR and wasm encoding are generic dynamic-engine capability, and the
//! layering rule puts that in tinyvm. What stayed is what is actually agenterm:
//! the `agenterm.*` door, slots, budget policy, the typed failure classes.
//!
//! [`compile_qjs`] and [`CompileError`] are re-exported unchanged, so this
//! crate's face did not move with the implementation. The retraction of the
//! earlier "do not depend on `tinyvm-qjs`" decision is recorded in PRD 36 and
//! `plan/design-agenterm-qjswasm.md` 2.
//!
//! # What this crate is not
//!
//! Not `rquickjs`, not a QuickJS C binding, and not a JavaScript engine yet:
//! the compiler lowers integer expressions only. It grows by real script demand
//! (PRD 36); its first concrete milestone is compiling the equivalent of
//! `scripts/qjs/lib/fleet.js`, which is what gates retiring `agenterm-qjs`.
//!
//! # Slots
//!
//! One guest = one slot = one budget. Slots cannot see each other, reach the
//! world only through the `agenterm.*` host door, and a bad slot can only fail
//! itself.

use std::sync::Arc;

mod host;
mod slot;

/// The `.qjs` compiler's face, re-exported so this crate's callers see one
/// door. `compile_qjs` is compile-only: it never executes what it produces.
pub use tinyvm_qjs::{Boundary, CompileError, compile_qjs};

/// Bounds on one guest. Execution limits live in the tinyvm core; the two
/// host-side caps bound what the door itself will buffer.
///
/// `Debug` is hand-written: `tinyvm::Limits` derives none, because the core is
/// `no_std` and fmt-free by design.
#[derive(Clone)]
pub struct Budget {
    /// Core-enforced limits: instruction steps per top-level call, linear
    /// memory pages, table elements, guest call depth, activation slots.
    pub limits: tinyvm::Limits,
    /// Cumulative `agenterm.print` bytes retained for one call. Exceeding this
    /// truncates and sets [`Outcome::truncated_stdout`] -- never a silent drop.
    pub max_stdout_bytes: usize,
    /// Largest single `fleet_call` result the door will hold. Exceeding this is
    /// an error, deliberately *not* a truncation: half a JSON document is worse
    /// than a refusal, because the guest cannot tell it was cut.
    pub max_bridge_result_bytes: usize,
}

impl std::fmt::Debug for Budget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Budget")
            .field("max_steps", &self.limits.max_steps)
            .field("max_memory_pages", &self.limits.max_memory_pages)
            .field("max_table_elems", &self.limits.max_table_elems)
            .field("max_call_depth", &self.limits.max_call_depth)
            .field("max_activation_slots", &self.limits.max_activation_slots)
            .field("max_stdout_bytes", &self.max_stdout_bytes)
            .field("max_bridge_result_bytes", &self.max_bridge_result_bytes)
            .finish()
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            limits: tinyvm::Limits::default(),
            max_stdout_bytes: 1 << 20,
            max_bridge_result_bytes: 1 << 20,
        }
    }
}

/// A neutral projection of the wasm numeric value types the engine face
/// exchanges with callers. Reference types stay inside the core.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
}

/// What to load into a slot. The result of extension routing, not a file.
#[derive(Clone, Copy, Debug)]
pub enum Guest<'a> {
    /// Standard WebAssembly bytes (`\0asm`).
    Wasm(&'a [u8]),
    /// `.qjs` source, compiled to wasm before it reaches the core.
    Qjs(&'a str),
}

/// Handle to one live slot. Invalid after [`Engine::kill`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotId(pub(crate) u64);

/// One call's result plus its deterministic cost, so "is this script
/// expensive?" is measurable rather than a guess.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub values: Vec<Value>,
    pub stdout: String,
    /// `true` when `stdout` hit [`Budget::max_stdout_bytes`] and was cut.
    pub truncated_stdout: bool,
    pub steps: u64,
    pub peak_call_depth: usize,
    pub peak_activation_slots: usize,
}

/// The repository-wide fleet bridge shape, reused verbatim. This crate exposes
/// that existing capability to wasm guests; it does not invent a second one.
pub type FleetBridgeFn = Arc<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>;

/// Five distinguishable failure classes. A caller must be able to tell "this
/// syntax is not supported yet" from "the guest ran out of budget" without
/// matching on strings.
///
/// `Debug` is hand-written rather than derived: tinyvm is deliberately
/// fmt-free (it is a `no_std`, sub-100 KiB core), so `WasmError` implements no
/// `Debug` and only exposes `message()`.
pub enum QjswasmError {
    /// `.qjs` did not compile. Carries an engine-capability diagnostic, never a
    /// bare "syntax error".
    Compile(CompileError),
    /// Rejected before execution: not wasm, malformed, or over a declared limit.
    Load(tinyvm::WasmError),
    /// The guest trapped during execution.
    Trap(tinyvm::WasmError),
    /// A core budget was exhausted.
    Budget(&'static str),
    /// A host-door contract was violated by the guest.
    Door(String),
    /// The slot does not exist, or was killed.
    NoSuchSlot(SlotId),
    /// The guest behaved correctly but returned a wasm value this engine's
    /// neutral [`Value`] projection cannot carry (a reference or vector type).
    ///
    /// Deliberately its own class rather than a `Trap`: nothing went wrong
    /// inside the guest, so reporting a trap would blame it for a limitation of
    /// *this* face. Callers who see this are hitting the edge of the engine's
    /// public surface, not a misbehaving script.
    UnsupportedValue(&'static str),
}

impl std::fmt::Debug for QjswasmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Compile(e) => f.debug_tuple("Compile").field(e).finish(),
            Self::Load(e) => f.debug_tuple("Load").field(&e.message()).finish(),
            Self::Trap(e) => f.debug_tuple("Trap").field(&e.message()).finish(),
            Self::Budget(what) => f.debug_tuple("Budget").field(what).finish(),
            Self::Door(what) => f.debug_tuple("Door").field(what).finish(),
            Self::NoSuchSlot(id) => f.debug_tuple("NoSuchSlot").field(id).finish(),
            Self::UnsupportedValue(what) => f.debug_tuple("UnsupportedValue").field(what).finish(),
        }
    }
}

impl std::fmt::Display for QjswasmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Compile(e) => write!(f, "compiling .qjs: {e}"),
            Self::Load(e) => write!(f, "loading wasm: {}", e.message()),
            Self::Trap(e) => write!(f, "guest trapped: {}", e.message()),
            Self::Budget(what) => write!(f, "budget exhausted: {what}"),
            Self::Door(what) => write!(f, "host door: {what}"),
            Self::NoSuchSlot(SlotId(id)) => write!(f, "no such slot: {id}"),
            Self::UnsupportedValue(what) => {
                write!(f, "value not representable at the engine face: {what}")
            }
        }
    }
}

impl std::error::Error for QjswasmError {}

impl From<CompileError> for QjswasmError {
    fn from(e: CompileError) -> Self {
        Self::Compile(e)
    }
}

/// A set of isolated, budgeted slots over one tinyvm core.
///
/// [`spawn`](Self::spawn) and [`call`](Self::call) are separate because a
/// tinyvm `Instance` is *persistent*: load once, call many times, and every
/// top-level call receives a fresh `max_steps` budget. The one-shot
/// `eval_wasm` sugar cannot express that, so this crate goes through
/// `Module::from_bytes_with` + import binding + `instantiate` instead.
pub struct Engine {
    budget: Budget,
    slots: Vec<Option<slot::Slot>>,
    next_id: u64,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self::with_budget(Budget::default())
    }

    pub fn with_budget(budget: Budget) -> Self {
        Self {
            budget,
            slots: Vec::new(),
            next_id: 0,
        }
    }

    pub fn budget(&self) -> &Budget {
        &self.budget
    }

    /// Compile (for `.qjs`), validate, bind the host door, instantiate, and run
    /// the start function. Does **not** call an entry point.
    pub fn spawn(
        &mut self,
        guest: Guest<'_>,
        bridge: Option<FleetBridgeFn>,
    ) -> Result<SlotId, QjswasmError> {
        let owned;
        let bytes = match guest {
            Guest::Wasm(bytes) => bytes,
            Guest::Qjs(source) => {
                owned = compile_qjs(source)?;
                &owned
            }
        };
        let slot = slot::Slot::load(bytes, &self.budget, bridge)?;
        let id = SlotId(self.next_id);
        self.next_id += 1;
        self.slots.push(Some(slot));
        Ok(id)
    }

    /// Invoke one export on a live slot. Each call receives a fresh
    /// `max_steps` budget.
    ///
    /// # A trap does not retire the slot
    ///
    /// If the guest traps or exhausts a budget, the slot stays live and
    /// callable: a later `call` reports the failure again rather than
    /// [`QjswasmError::NoSuchSlot`]. This is a deliberate commitment, not an
    /// accident of the implementation. A trap in WebAssembly unwinds to the
    /// call boundary and leaves the instance's memory and globals intact, so
    /// discarding the slot would throw away recoverable state the embedder may
    /// still want to inspect. Reclaiming is the caller's decision, via
    /// [`kill`](Self::kill).
    ///
    /// # Budget exhaustion is its own class
    ///
    /// Hitting `max_steps` or `max_call_depth` reports
    /// [`QjswasmError::Budget`], not [`QjswasmError::Trap`], even though the
    /// core surfaces both as wasm traps. They are `Limits` fields the embedder
    /// chose, so "your guest was too expensive for the budget you set" must be
    /// answerable without matching on a message string.
    pub fn call(
        &mut self,
        slot: SlotId,
        entry: &str,
        args: &[Value],
    ) -> Result<Outcome, QjswasmError> {
        let s = self
            .slots
            .get_mut(slot.0 as usize)
            .and_then(Option::as_mut)
            .ok_or(QjswasmError::NoSuchSlot(slot))?;
        s.call(entry, args, &self.budget)
    }

    /// Spawn, call, and reclaim. The common path for a one-shot guest.
    pub fn run_once(
        &mut self,
        guest: Guest<'_>,
        bridge: Option<FleetBridgeFn>,
        entry: &str,
        args: &[Value],
    ) -> Result<Outcome, QjswasmError> {
        let id = self.spawn(guest, bridge)?;
        let out = self.call(id, entry, args);
        self.kill(id);
        out
    }

    /// Reclaim a slot. A later [`call`](Self::call) on it reports
    /// [`QjswasmError::NoSuchSlot`] rather than panicking.
    pub fn kill(&mut self, slot: SlotId) {
        if let Some(entry) = self.slots.get_mut(slot.0 as usize) {
            *entry = None;
        }
    }

    pub fn live_slots(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }
}

/// Decode and validate wasm bytes against the default budget without running
/// anything.
///
/// This is the `.wasm` half of a `check`: the start function must not fire, so
/// a check can never have side effects. Instantiation is deliberately not
/// performed, because that is what would run `start`.
pub fn validate_wasm(bytes: &[u8]) -> Result<(), QjswasmError> {
    validate_wasm_with(bytes, &Budget::default())
}

/// Like [`validate_wasm`], but against a caller-supplied budget, so a check
/// rejects a module that the same budget would refuse to load at run time.
pub fn validate_wasm_with(bytes: &[u8], budget: &Budget) -> Result<(), QjswasmError> {
    tinyvm::WasmModule::from_bytes_with(bytes, budget.limits)
        .map(|_| ())
        .map_err(QjswasmError::Load)
}

/// Route a path to a guest kind by extension. `.wasm` and `.qjs` only.
pub fn guest_kind_for_path(path: &str) -> Option<GuestKind> {
    if path.ends_with(".wasm") {
        Some(GuestKind::Wasm)
    } else if path.ends_with(".qjs") {
        Some(GuestKind::Qjs)
    } else {
        None
    }
}

/// The extension-routing result, without borrowing any content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestKind {
    Wasm,
    Qjs,
}
