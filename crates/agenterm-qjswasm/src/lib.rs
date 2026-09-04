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
//! Not `rquickjs`, not a QuickJS C binding, and not a JavaScript engine yet.
//! The subset the compiler lowers is real but small -- binary64 numbers,
//! strings, `let`/`const`/`var` with scoping and a TDZ, blocks, `if`/`else`,
//! `while`, three-part `for`, `return`, functions with parameters and
//! recursion, the operator ladder -- and it grows by real script demand
//! (PRD 36). Its first concrete milestone is compiling the equivalent of
//! `scripts/qjs/lib/fleet.js`, which is what gates retiring `agenterm-qjs`.
//!
//! # Two calling conventions, one face
//!
//! A hand-written `.wasm` guest speaks plain wasm numerics. A `.qjs` guest
//! speaks the compiler's V1 representation, where one JavaScript value is a
//! `(tag: i32, payload: i64)` pair. [`Value`] carries both, and the slot knows
//! which convention it was loaded under, so neither caller has to learn the
//! other's ABI. See [`JsValue`] for why a `.qjs` result is projected into
//! owned host data rather than handed over as the raw pair.
//!
//! # Slots
//!
//! One guest = one slot = one budget. Slots cannot see each other, reach the
//! world only through the `agenterm.*` host door, and a bad slot can only fail
//! itself.

/// The upstream `tinyvm` revision this crate is pinned to, as a string an
/// operator can read.
///
/// It is a `const` and not a build-script probe because there is nothing to
/// probe: the pin is two literals in this crate's own `Cargo.toml`, and the
/// only failure mode worth guarding is those three drifting apart. A test in
/// this crate reads that file and asserts all three agree, so the constant
/// cannot go stale silently -- which is the whole reason it is safe to print
/// it as fact.
///
/// Why print it at all: which `tinyvm` a build carries decides what the
/// language can do. Over one week this pin moved five times and each move
/// changed the answer to "does `[1,2,3]` compile" -- an operator holding a
/// binary has no other way to tell which one they have.
pub const UPSTREAM_TINYVM_REV: &str = "af47e4d";

/// This crate's own version, and the engine's name, as one line.
///
/// The `version` verb's answer. `agenterm-qjs` prints `agenterm-qjs 0.1.16`;
/// this prints the same shape plus the pin, because for a compiler-backed
/// engine the upstream revision is half of what "which build is this" means.
pub fn identity() -> String {
    format!(
        "agenterm-qjswasm {} (tinyvm {UPSTREAM_TINYVM_REV})",
        env!("CARGO_PKG_VERSION")
    )
}

pub mod check_many;
pub mod corpus_scan;

use std::sync::Arc;

mod host;
mod slot;
mod tool;

pub use tool::DEFAULT_COMMAND_TIMEOUT_MS;

/// The `.qjs` compiler's diagnostic types, re-exported so this crate's callers
/// see one door.
pub use tinyvm_qjs::{Boundary, CompileError};

/// The compiler's declaration vocabulary, re-exported so a caller can read
/// [`door_declarations`] without depending on `tinyvm-qjs` directly. The
/// declarations are agenterm's; the mechanism is upstream's and names nothing
/// of agenterm's.
pub use tinyvm_qjs::{HostFn, HostParam, HostResult};

/// Compile `.qjs` source to standard wasm bytes. Compile-only: it never
/// executes what it produces, which is what makes a `check` free of side
/// effects.
///
/// # Why this is a function and no longer a re-export
///
/// Upstream carries two entry points for one milestone: `compile_qjs` is the
/// original expression compiler (`i32` in, `i32` out) and `compile_qjs_m1` is
/// the language -- statements, declarations, control flow, functions, strings
/// -- lowered over the V1 value representation. Upstream's own note says the
/// M0 name belongs to M1 "when its callers move". This crate is that caller,
/// and this is the move: the name agenterm publishes is stable, the compiler
/// behind it is the current one.
///
/// It matters that this and [`Engine::spawn`] compile through the *same*
/// entry point. They did not have to be the same function before, because
/// there was only one; now a `check` that used M0 while `execute` used M1
/// would accept a script at run time that it had just refused at check time,
/// which is the worst shape a gate can have.
///
/// # The door is part of this entry point, not an option on it
///
/// A script compiled here may call the `agenterm.*` door by name --
/// `print`, `fleet_call`, `fleet_result`; see [`door_declarations`]. That is
/// deliberately not a flag: `check` (which calls this) and `execute` (which
/// reaches it through [`Engine::spawn`]) have to agree about what a script may
/// say, and a door that appeared only on the execute path would make `check`
/// refuse working scripts. A script that mentions no door name compiles
/// exactly as it did before and emits **no** imports, so the declaration costs
/// nothing to a guest that does not reach for it.
///
/// Callers who want a guest with no host surface at all -- one whose bytes
/// provably cannot name the door -- use [`compile_qjs_without_door`]. Callers
/// compiling a *tool* script -- one that may also name the `tool.*` door --
/// use [`compile_qjs_tool`]; this entry point does not know that door exists.
pub fn compile_qjs(source: &str) -> Result<Vec<u8>, CompileError> {
    tinyvm_qjs::compile_qjs_m1_with(
        source,
        tinyvm_qjs::Options {
            names: tinyvm_qjs::Names::Declared(host::declarations()),
        },
    )
}

/// [`compile_qjs`] for a **tool** script: the `agenterm.*` door plus the
/// `tool.*` door (`fs_*`, `process_*`, `env_*`, `tool_result`; see
/// [`tool_door_declarations`]).
///
/// # Opt-in, and why it is a separate function rather than a flag on the first
///
/// PRD 36 ("A1.1 的答案") decided there are two kinds of `.qjs`: a sandbox
/// script, which sees `agenterm.*` and nothing else, and a tool script -- a
/// CI gate, a build step, a qualification check -- which also gets the
/// filesystem, the environment and child processes. The difference is who
/// can open the second door, and the answer has to be "the product surface
/// that runs tool scripts, explicitly, in code". A flag defaulting to closed
/// would say the same thing; a separate name says it at every call site, and
/// makes `grep compile_qjs_tool` the complete list of places the tool door is
/// open.
///
/// A script that names no tool function compiles to **exactly** the bytes
/// [`compile_qjs`] emits -- the declarations cost nothing until mentioned --
/// so `check` and `execute` of a sandbox script are unchanged by this entry
/// point existing. `tests/tool_door.rs` holds both halves of that claim.
///
/// The bytes this produces load only into an engine built with
/// [`Engine::with_tool_door`]; a sandbox engine refuses them at load time,
/// naming the `tool.*` import, so a tool artifact cannot be run somewhere it
/// would be handed a door it should not have.
pub fn compile_qjs_tool(source: &str) -> Result<Vec<u8>, CompileError> {
    tinyvm_qjs::compile_qjs_m1_with(
        source,
        tinyvm_qjs::Options {
            names: tinyvm_qjs::Names::Declared(both_doors()),
        },
    )
}

/// [`compile_qjs_with_modules`] for a tool script. See [`compile_qjs_tool`].
pub fn compile_qjs_tool_with_modules(
    source: &str,
    resolve: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<u8>, CompileError> {
    tinyvm_qjs::compile_qjs_m1_with_modules(
        source,
        tinyvm_qjs::Options {
            names: tinyvm_qjs::Names::Declared(both_doors()),
        },
        resolve,
    )
}

/// The fleet door followed by the tool door: declaration order is import
/// order, so a tool script's `agenterm.*` imports come first, exactly where a
/// sandbox script's would be.
fn both_doors() -> Vec<HostFn> {
    let mut decls = host::declarations();
    decls.extend(tool::declarations());
    decls
}

/// [`compile_qjs`], plus a way for the source to `import` other sources.
///
/// `resolve` turns a module specifier into source and is the **only** thing
/// that knows about files: the compiler upstream never touches a filesystem,
/// so where a specifier points -- relative to the entry file, under a project
/// root, with what extension -- is this product's policy to state and this
/// callback's job to apply. Returning `None` refuses the specifier and the
/// caller gets a diagnostic naming it.
///
/// The result is still one `.wasm`. An `import` is compile-time inclusion, not
/// a link, so the load gate, the budgets and the receipt are unchanged.
///
/// # Why this exists
///
/// `scripts/qjs/lib/fleet.qjs` binds all 29 fleet operations and had no
/// consumers, because the only way to use it was
/// `format!("{lib}\n{driver}")` from Rust. A `.qjs` file on disk could not
/// reach it. This is the function that makes the library usable as a library.
pub fn compile_qjs_with_modules(
    source: &str,
    resolve: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<u8>, CompileError> {
    tinyvm_qjs::compile_qjs_m1_with_modules(
        source,
        tinyvm_qjs::Options {
            names: tinyvm_qjs::Names::Declared(host::declarations()),
        },
        resolve,
    )
}

/// [`compile_qjs`] with no host surface: every free name is a capability
/// diagnostic, and the emitted module imports nothing at all.
///
/// This is the shape the crate compiled everything as until the door landed.
/// It is kept because "this guest cannot reach the host, and that is checkable
/// from its bytes" is a real thing to want -- a corpus scan, a pure-computation
/// benchmark, a guest whose import table must be empty by construction rather
/// than by inspection. It is **not** what `check` or `execute` use: a script
/// checked with this and run with the door would be checked against a smaller
/// language than it runs in.
pub fn compile_qjs_without_door(source: &str) -> Result<Vec<u8>, CompileError> {
    tinyvm_qjs::compile_qjs_m1(source)
}

/// The whole of a `.qjs` check: compile the source, then put the bytes it
/// produced through the load gate they will have to pass at run time.
///
/// # Why compiling is not a check on its own
///
/// [`compile_qjs`] answers "is this the language?" and nothing else. It says
/// nothing about whether the *module* it just emitted fits the budget it will
/// be run under, and the compiler can quite legitimately emit a module that
/// does not: a script whose string literals need more pages than
/// [`Budget::limits`]`.max_memory_pages` allows compiles clean and then fails
/// [`Engine::spawn`] with [`QjswasmError::Load`]. The `.wasm` half of a check
/// has always applied that gate -- [`validate_wasm`] is decode plus the gate --
/// so the compiler's own output was the one artifact in the pipeline that
/// never met it.
///
/// A check that passes what execute cannot run is the worst shape a gate can
/// have: it is the same argument that put the door on [`compile_qjs`] itself
/// rather than behind a flag, and the same one `src/host.rs` makes about import
/// names. This closes it for the memory declaration.
///
/// Nothing is executed: the gate stops before instantiation, which is what
/// would run a start function, so a check still cannot have side effects.
pub fn check_qjs(source: &str) -> Result<(), QjswasmError> {
    check_qjs_with(source, &Budget::default())
}

/// [`check_qjs`] against a caller-supplied budget, so a check refuses exactly
/// what an [`Engine`] built on the same [`Budget`] would refuse to load.
///
/// Pass the budget the script will actually run under. The default one is not
/// a safe over-approximation in either direction -- a bigger budget accepts
/// modules a smaller engine will reject, and a smaller one rejects modules a
/// bigger engine would have run.
pub fn check_qjs_with(source: &str, budget: &Budget) -> Result<(), QjswasmError> {
    let bytes = compile_qjs(source)?;
    validate_wasm_with(&bytes, budget)
}

/// [`check_qjs_with`] for a tool script: compiled with [`compile_qjs_tool`],
/// gated as [`validate_wasm_tool_with`] gates. A check must refuse exactly
/// what the engine that will run the script refuses, and a tool script runs
/// in an [`Engine::with_tool_door`], so it is checked as one.
pub fn check_qjs_tool_with(source: &str, budget: &Budget) -> Result<(), QjswasmError> {
    let bytes = compile_qjs_tool(source)?;
    validate_wasm_tool_with(&bytes, budget)
}

/// [`check_qjs_tool_with`] for a script that may `import`: the resolver is
/// the embedder's, as for [`compile_qjs_tool_with_modules`]. The corpus scan
/// needs this shape -- without a resolver every entry that imports a library
/// was "not ok" for a reason that had nothing to do with the entry.
pub fn check_qjs_tool_with_modules(
    source: &str,
    resolve: &dyn Fn(&str) -> Option<String>,
    budget: &Budget,
) -> Result<(), QjswasmError> {
    let bytes = compile_qjs_tool_with_modules(source, resolve)?;
    validate_wasm_tool_with(&bytes, budget)
}

/// The `tool.*` door as declarations: what a tool script may call beyond the
/// fleet door, and which raw import each call becomes. See `src/tool.rs`.
///
/// Thirteen declarations, fourteen imports: `tool_result` is the shared
/// two-pass fetch and brings `result_len` with it.
pub fn tool_door_declarations() -> Vec<HostFn> {
    tool::declarations()
}

/// The `agenterm.*` door as declarations the `.qjs` compiler can unwrap onto:
/// what a script may call, which raw import each call becomes.
///
/// Exposed so an embedder can print the surface a script is compiled against
/// without reading this crate's source, and so the door's two tables -- the
/// raw signatures in `src/host.rs` and these declarations -- can be checked
/// against each other from outside.
///
/// Three declarations, four imports: `fleet_result` is a two-pass byte
/// result, so it brings `fleet_result_len` with it. See `src/host.rs`.
pub fn door_declarations() -> Vec<HostFn> {
    host::declarations()
}

/// Bounds on one guest. Execution limits live in the tinyvm core; the three
/// host-side caps bound every buffer the host itself allocates on a guest's
/// behalf -- the print buffer, the pending bridge answer, and the string the
/// seam copies out when it projects a returned value.
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
    /// Largest string the seam will copy out of a guest when projecting a
    /// returned [`JsValue::Str`].
    ///
    /// This is the third host-side buffer and it needs its own number, because
    /// the other two do not bound it: the string is allocated by the *host*,
    /// sized by the *guest*, and neither `max_stdout_bytes` nor
    /// `max_bridge_result_bytes` is consulted on that path. Before this field
    /// existed the only ceiling was incidental -- `max_steps` runs out first
    /// under the default budget, because concatenation is O(n) in steps -- and
    /// an incidental ceiling moves the moment a guest can produce a large
    /// string cheaply or an embedder raises `max_steps`. The real bound was
    /// then `max_memory_pages * 64 KiB`, per call, on a persistent slot.
    ///
    /// Exceeding it is [`QjswasmError::Budget`] rather than a truncation, for
    /// the same reason as `max_bridge_result_bytes`: half a string is worse
    /// than a refusal, because the caller cannot tell it was cut.
    pub max_result_string_bytes: usize,
    /// Host operations one call may make: every `tool.*` operation and every
    /// `fleet_call`, counted at the door before the operation runs. The
    /// core's step budget prices what the guest *computes*; a script that
    /// spends its life in the host -- polling a window every 25 ms, running
    /// a command per file -- is invisible to it, which is why a weekend of
    /// raising `max_operations` never made such scripts cheaper (PRD_02_36
    /// A1.12). Exceeding this ends the call as [`QjswasmError::Budget`]
    /// (`"max_host_ops"`), the same class as running out of steps.
    pub max_host_ops: usize,
    /// Set by the embedder to end the call at the next host operation or
    /// wait: `time.sleep_ms` sleeps in slices and looks; `process.wait` /
    /// `process.command` look between polls of the child; every `tool.*`
    /// and `fleet_call` entry looks before running, and `fleet_call` looks
    /// again when the bridge answers, so a bridge that returns early on the
    /// same flag ends the call rather than parking a refusal. The call then
    /// ends as [`QjswasmError::Cancelled`]. Pure compute is not interrupted
    /// -- the step budget bounds that -- which is why this is a flag the
    /// doors read and not a signal. `None` means no one can cancel.
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// A clock the script can replay against. `Some(origin)` makes
    /// `time.now_ms` answer `origin` plus the milliseconds the script has
    /// *asked* to sleep so far, instead of the wall clock: two runs of the
    /// same script read the same times, and a script that waits by polling
    /// `now_ms` around `sleep_ms` still terminates, because its own sleeps
    /// move the clock. Nothing else moves it -- a child's run, a bridge
    /// round trip -- since those are the non-determinism a replay is
    /// meant to hold still. The sleeps themselves still happen. `None` is
    /// the wall clock.
    pub fixed_clock_ms: Option<u64>,
    /// Environment names a script may read even though they look like
    /// secrets (`*_TOKEN`, `*_SECRET`, `*_KEY`, `*_PASSWORD`, `AWS_*`, ...;
    /// the list is `tool::secret_looking`). Compared case-insensitively.
    /// In the tool profile `env.get` / `env.has` on such a name answers a
    /// refusal unless it is listed here, and a listed read goes on the
    /// receipt by name (never by value). The task contract's `env_allow`
    /// is where a script's author declares them.
    pub env_allow: Vec<String>,
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
            .field("max_result_string_bytes", &self.max_result_string_bytes)
            .field("max_host_ops", &self.max_host_ops)
            .field("cancellable", &self.cancel.is_some())
            .field("fixed_clock_ms", &self.fixed_clock_ms)
            .field("env_allow", &self.env_allow)
            .finish()
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            limits: tinyvm::Limits::default(),
            max_stdout_bytes: 1 << 20,
            max_bridge_result_bytes: 1 << 20,
            max_result_string_bytes: 1 << 20,
            max_host_ops: 4096,
            cancel: None,
            fixed_clock_ms: None,
            env_allow: Vec::new(),
        }
    }
}

/// One JavaScript value, projected into owned host data.
///
/// # Why owned, and resolved here
///
/// The compiler's V1 representation makes a JS value a `(tag: i32, payload:
/// i64)` pair, and a String's payload is a pointer into *that slot's* linear
/// memory. Handing the pointer to a caller would hand out a reference whose
/// referent dies with the slot -- and [`Engine::run_once`] kills the slot
/// before it returns, so the common path would hand back a dangling one every
/// time. The seam therefore resolves a value while the instance is still
/// alive, and what crosses the face is host data with no guest lifetime in it.
///
/// # Why this shape survives objects
///
/// The variant list is short today because the language is. What the shape
/// fixes is not the list but the *resolution point*: there is one place, on
/// the way out of a call, where guest representation becomes host data. When
/// arrays and objects land (M4) they arrive as further variants resolved at
/// that same point, not as a second mechanism callers have to learn. A value
/// the projection genuinely cannot carry -- a function, a cyclic object --
/// is [`QjswasmError::UnsupportedValue`], which already means exactly that:
/// the guest was fine, this face cannot express what it produced.
///
/// `#[non_exhaustive]` says the same thing to the compiler: code that matches
/// on this has to decide what to do about a kind of value that did not exist
/// when it was written.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    /// ECMA-262 binary64, not an `i32`: `1/0` is `Infinity`, `2147483647 + 1`
    /// does not wrap, and `-0` is distinguishable from `0`.
    Number(f64),
    /// The text, already read out of the guest's linear memory.
    Str(String),
}

/// What the engine face exchanges with callers, over both calling conventions
/// it serves.
///
/// The two are genuinely different worlds and the enum says so rather than
/// blurring them:
///
/// - `I32` / `I64` / `F32` / `F64` are a neutral projection of the wasm
///   numeric value types. That is what a hand-written `.wasm` guest speaks,
///   and it is unchanged. Reference and vector types stay inside the core.
/// - [`Js`](Value::Js) is one JavaScript value. That is what a `.qjs` guest
///   speaks, because the compiled entry point takes two wasm parameters per
///   argument and returns two results -- the V1 pair. Collapsing that back to
///   `I32` would throw away every type the language just gained.
///
/// A slot is loaded under one convention and accepts only that one: handing a
/// raw wasm numeric to a `.qjs` guest, or a [`JsValue`] to a hand-written
/// module, is [`QjswasmError::UnsupportedValue`] rather than a silent
/// reinterpretation of the bits.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Js(JsValue),
}

impl From<JsValue> for Value {
    fn from(value: JsValue) -> Self {
        Value::Js(value)
    }
}

/// What to load into a slot. The result of extension routing, not a file.
///
/// The variant is also how the slot learns its calling convention, which is
/// why [`Wasm`](Guest::Wasm) and [`CompiledQjs`](Guest::CompiledQjs) are two
/// variants over the same `&[u8]` rather than one: nothing in wasm bytes says
/// which convention their exports speak, and guessing from a signature would
/// be a guess -- `(i32, i64) -> (i32, i64)` is a perfectly ordinary
/// hand-written type. The caller states it, once, at load time.
#[derive(Clone, Copy, Debug)]
pub enum Guest<'a> {
    /// Standard WebAssembly bytes (`\0asm`), speaking plain wasm numerics.
    Wasm(&'a [u8]),
    /// `.qjs` source, compiled to wasm before it reaches the core.
    Qjs(&'a str),
    /// Wasm bytes that were compiled from `.qjs` earlier and speak the V1
    /// calling convention.
    ///
    /// # Why an artifact needs its own variant
    ///
    /// A `.wasm` file does not remember that it came from `.qjs`. Compile to
    /// disk and load the artifact back through [`Wasm`](Guest::Wasm) and the
    /// convention is lost: the V1 pair crosses the face unresolved, so a
    /// `String` arrives as its tag and a raw pointer into a linear memory the
    /// caller is about to drop. Every compile-once-run-later shape -- a `pack`
    /// artifact, a cached build, a guest fetched over the wire -- needs this
    /// variant to exist, and it is what makes the seam's hostile-pointer
    /// refusals reachable from outside the crate at all.
    ///
    /// It grants nothing extra. The bytes take the same load-time validation,
    /// the same `Limits`, and the same door as any other guest; the only
    /// difference is which convention the slot records.
    CompiledQjs(&'a [u8]),
}

/// Handle to one live slot. Invalid after [`Engine::kill`], and meaningless in
/// any [`Engine`] other than the one that minted it.
///
/// # Why it carries an engine tag
///
/// The id used to be the slot's index alone. Two engines therefore both minted
/// `SlotId(0)`, and handing one engine the other's id was not an error: the
/// call ran the *local* slot at that index and returned its value, and
/// [`Engine::kill`] destroyed the local slot. Both failures were silent in both
/// directions -- the wrong guest ran, or the wrong guest was destroyed, and
/// nothing was reported. A type that is `Copy`, `Eq` and `Hash` is built to be
/// stored and passed around, so "do not mix them up" is not a workable
/// contract.
///
/// The engine tag comes from a process-wide counter, so every `Engine` in the
/// process has a distinct one and a foreign id is [`QjswasmError::NoSuchSlot`]
/// rather than a misdirected call. Within one engine, indices are still never
/// recycled: `next_index` only increments and the slot table is never
/// compacted, so a stale id can never come to mean a different slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotId {
    engine: u64,
    index: u64,
}

/// What one call cost, on either path. An [`Outcome`] carries these lines;
/// a call that failed keeps them on the slot, because a failed wait is
/// exactly the run whose bill matters (PRD_02_36 A1.12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cost {
    pub steps: u64,
    pub peak_call_depth: usize,
    pub peak_activation_slots: usize,
    pub host_ops: u64,
    pub host_bytes: u64,
    pub waited_ms: u64,
    pub heap_pages: usize,
}

/// One call's result plus its deterministic cost, so "is this script
/// expensive?" is measurable rather than a guess.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// What the entry point returned. A hand-written `.wasm` guest returns as
    /// many wasm values as its signature declares; a `.qjs` guest returns
    /// exactly one [`Value::Js`], because a JavaScript function returns one
    /// value however many wasm words carry it.
    pub values: Vec<Value>,
    pub stdout: String,
    /// `true` when `stdout` hit [`Budget::max_stdout_bytes`] and was cut.
    pub truncated_stdout: bool,
    /// Every `tool.*` operation this call reached, fully qualified
    /// (`tool.fs.read_to_string`) and in call order, one entry per call --
    /// the line a receipt names host capabilities by. Always empty for a
    /// sandbox slot, which has no such door to reach.
    pub tool_calls: Vec<String>,
    pub steps: u64,
    pub peak_call_depth: usize,
    pub peak_activation_slots: usize,
    /// Host operations this call made (`tool.*` and `fleet_call`), the count
    /// [`Budget::max_host_ops`] bounds.
    pub host_ops: u64,
    /// Bytes that crossed the door in those operations: arguments in, parked
    /// answers out.
    pub host_bytes: u64,
    /// Wall-clock milliseconds the call spent *waiting* in the host --
    /// `time.sleep_ms`, `process.wait`, a command's run, a bridge answer.
    /// The one non-deterministic line on the receipt, and deliberately so:
    /// it is what separates "computed a lot" from "waited a lot", which
    /// `steps` alone cannot tell apart.
    pub waited_ms: u64,
    /// Guest linear memory at the end of the call, in 64 KiB pages. The
    /// heap is a bump allocator that only grows, so on a slot that is
    /// called again this is the high-water mark so far, and against
    /// `Budget::limits.max_memory_pages` it is the fourth number a budget is
    /// made of.
    pub heap_pages: usize,
}

/// The repository-wide fleet bridge shape, reused verbatim. This crate exposes
/// that existing capability to wasm guests; it does not invent a second one.
pub type FleetBridgeFn = Arc<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>;

/// Ten distinguishable failure classes. A caller must be able to tell "this
/// syntax is not supported yet" from "the guest ran out of budget" from "the
/// script threw" without matching on strings.
///
/// The count has been wrong here before -- it said five while nine were
/// listed -- so it is worth saying why it is worth keeping right: this is the
/// enum a caller matches on, and a doc that undercounts it reads as a promise
/// that the remaining arms are variations on the listed ones rather than
/// separate answers.
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
    /// Rejected while validating one function's body, and the module said
    /// which: the function index (imports first), its `name`-section name
    /// and, when the module has a `qjs.lines` section, the source line --
    /// for a `.qjs` guest, the script's own function name and the line the
    /// author wrote it on, which the compiler writes for every function.
    /// "validation: type mismatch" with no location once sent an author to
    /// bisect a 900-line script.
    LoadInFunction {
        error: tinyvm::WasmError,
        index: u32,
        name: Option<String>,
        line: Option<u32>,
        /// The 1-based column on that line, in UTF-16 code units the way an
        /// editor counts; `None` with `line`.
        column: Option<u32>,
    },
    /// The guest trapped during execution.
    Trap(tinyvm::WasmError),
    /// The script threw a value and nothing caught it.
    ///
    /// Its own class rather than a [`Trap`](Self::Trap), even though the core
    /// sees the same `unreachable` either way: ECMA-262 says a program whose
    /// exception reaches the top terminates with it, so a script that throws
    /// ran exactly as written. Nothing is broken and no budget was hit, and
    /// telling the author their script faulted is a third wrong answer next to
    /// those two. The compiler writes the distinction into the guest's own
    /// memory precisely so a host can make it (see
    /// [`tinyvm_qjs::guest_fault`]); a host holding that evidence and
    /// reporting a bare trap anyway is guessing when it did not have to --
    /// which is the same mistake the heap-exhaustion arm above was added to
    /// stop making.
    ///
    /// The thrown value does not come with it. A compiled module exports no
    /// global holding it, so it is not readable from outside -- upstream says
    /// so at `GuestFault::UncaughtThrow` and calls handing it out a decision
    /// about the host boundary rather than about throwing. A script that wants
    /// the host to see *what* went wrong must catch it and return or print it.
    /// The message, when the thrown value was a String -- which is what
    /// every gate script throws (`"gate_id:reason"`). `None` for a thrown
    /// Number or object: upstream states that narrowing rather than hiding
    /// it, and this carries it through unchanged.
    UncaughtThrow(Option<String>),
    /// The script asked for something the engine does not have, and it was
    /// only knowable at run time.
    ///
    /// Distinct from [`QjswasmError::Trap`] on purpose: a trap is "the guest
    /// stopped and the core does not know why", which for an engine boundary
    /// sends the reader to look for a bug that is not there. This one names a
    /// capability, so the answer to it is "write it differently, or ask for
    /// the capability" rather than "file a defect".
    ///
    /// One producer upstream today: reading a property other than `length`
    /// from a String. `"ab".toUpperCase` is a real function in ECMA-262, so
    /// answering `undefined` would be a wrong answer wearing a right answer's
    /// clothes -- the engine refuses instead, and this is that refusal
    /// arriving with a name.
    ///
    /// Since tinyvm's A11 the guest can say *which* boundary when it is one
    /// of the two its representation cannot cross -- `split("")`, a `slice`
    /// boundary inside a surrogate pair -- and `None` is the older, unnamed
    /// String-property arm.
    CapabilityBoundary(Option<String>),
    /// The script read a String property this engine does not have --
    /// `"ab".substring`, say -- and the guest named it before stopping. An engine
    /// boundary like [`CapabilityBoundary`](Self::CapabilityBoundary), reached
    /// at run time because a receiver's type is a run-time fact; unlike it,
    /// this one says which property. `None` only if the name could not be
    /// read back, which no build of tinyvm-qjs since 9847658 produces.
    UnsupportedMethod(Option<String>),
    /// A host function was handed a value of the wrong type at run time --
    /// `print(s.length)`, a Number where the door says String. Not knowable
    /// at compile time (a literal is refused there), so the guest names the
    /// call and the 1-based argument position before stopping. The script's
    /// own doing: `"" + x` is the spelling it wants.
    HostArgument(Option<(String, u32)>),
    /// The script read a property of `undefined`, `null`, a Number or a
    /// Boolean -- ECMA-262's TypeError, which this engine cannot throw yet
    /// but can name: the key. The script's own doing; `=== undefined` first
    /// is the guard.
    PropertyOfNonObject(Option<String>),
    /// The script called a value that is not a function -- `undefined()`, a
    /// method this engine does not have (`[].concat()`), a number. ECMA-262's
    /// other TypeError, which a script that can catch does catch; uncaught,
    /// the engine names the callee. The script's own doing.
    NotAFunction(Option<String>),
    /// The script asked for the string or number form of an Object, an Array
    /// or a function -- `"" + o`, `o * 2`, `o[k]` with an Object key. ECMA-262
    /// would answer `[object Object]`; this engine never converts one quietly,
    /// and names the kind. The script's own doing; `JSON.stringify` is the
    /// spelling that says what was meant.
    NoPrimitiveForm(Option<String>),
    /// The script wrote where this engine refuses to: a key that is not an
    /// index on an Array (`a["x"] = 1` -- ECMA-262 would hang a named
    /// property on it), or a property on a value that has none (`s[0] = "x"`
    /// on a String, which the specification ignores). Never silently; the
    /// script's own doing, and the sentence says what was written.
    InvalidWrite(Option<String>),
    /// The embedder set [`Budget::cancel`] while the guest was in a host
    /// wait or between host operations; the call ended there. Not the
    /// script's doing and not a budget: the class the worker already has
    /// for a cancel frame, produced for the first time here.
    Cancelled,
    /// A core budget was exhausted.
    Budget(&'static str),
    /// A contract at the host boundary was violated: one of the `agenterm.*`
    /// door's rules, or -- for a `.qjs` guest -- the V1 calling convention its
    /// entry point is compiled to speak.
    ///
    /// Usually the guest broke it. Not always: an embedder's fleet bridge that
    /// panics is the *host* side failing, and it is reported here rather than
    /// as a trap, because the guest did nothing wrong and rather than as an
    /// `Err` status, because "the capability is broken" is not the same answer
    /// as "the capability said no" (see `src/host.rs`).
    Door(String),
    /// The slot does not exist, or was killed.
    NoSuchSlot(SlotId),
    /// The slot exists but exports no function under the name the caller
    /// asked for. Carries the name.
    ///
    /// Its own class for the same reason [`NoSuchSlot`](Self::NoSuchSlot) is:
    /// "this slot has no such export" is the same shape of mistake as "this
    /// engine has no such slot", and both are the caller's. Folding it into
    /// [`Trap`](Self::Trap) -- which is what the core's own
    /// `"no exported function named"` fault would do -- tells a caller who
    /// mistyped a name that their script faulted, when the guest never ran.
    NoSuchExport(String),
    /// The export exists, but the call does not fit its declared signature:
    /// the wrong number of arguments, or an argument of a type the signature
    /// does not take.
    ///
    /// Checked before the guest is entered, and its own class for the reason
    /// [`UnsupportedValue`](Self::UnsupportedValue) gives: nothing went wrong
    /// inside the guest. Without this check a wasm arity or type mismatch
    /// surfaces as the core's `Trap("function")` -- blaming a guest that did
    /// nothing wrong -- and a *type* mismatch may not surface at all, because
    /// the core only notices when the guest touches the value.
    ///
    /// A `String` rather than a `&'static str` because the useful part is the
    /// numbers: which parameter, what it declares, what was handed in.
    Signature(String),
    /// A value the engine's [`Value`] face cannot carry, in either direction:
    /// a wasm reference or vector type coming out, a [`JsValue`] the seam
    /// cannot hand in, or a value offered under the wrong calling convention
    /// for the slot.
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

            Self::LoadInFunction {
                error,
                index,
                name,
                line,
                column,
            } => f
                .debug_struct("LoadInFunction")
                .field("error", error)
                .field("index", index)
                .field("name", name)
                .field("line", line)
                .field("column", column)
                .finish(),
            Self::Trap(e) => f.debug_tuple("Trap").field(&e.message()).finish(),
            Self::UncaughtThrow(m) => f.debug_tuple("UncaughtThrow").field(m).finish(),
            Self::CapabilityBoundary(which) => {
                f.debug_tuple("CapabilityBoundary").field(which).finish()
            }
            Self::UnsupportedMethod(name) => {
                f.debug_tuple("UnsupportedMethod").field(name).finish()
            }
            Self::HostArgument(at) => f.debug_tuple("HostArgument").field(at).finish(),
            Self::PropertyOfNonObject(key) => {
                f.debug_tuple("PropertyOfNonObject").field(key).finish()
            }
            Self::NotAFunction(callee) => f.debug_tuple("NotAFunction").field(callee).finish(),
            Self::NoPrimitiveForm(kind) => f.debug_tuple("NoPrimitiveForm").field(kind).finish(),
            Self::InvalidWrite(what) => f.debug_tuple("InvalidWrite").field(what).finish(),
            Self::Cancelled => f.write_str("Cancelled"),
            Self::Budget(what) => f.debug_tuple("Budget").field(what).finish(),
            Self::Door(what) => f.debug_tuple("Door").field(what).finish(),
            Self::NoSuchSlot(id) => f.debug_tuple("NoSuchSlot").field(id).finish(),
            Self::NoSuchExport(what) => f.debug_tuple("NoSuchExport").field(what).finish(),
            Self::Signature(what) => f.debug_tuple("Signature").field(what).finish(),
            Self::UnsupportedValue(what) => f.debug_tuple("UnsupportedValue").field(what).finish(),
        }
    }
}

impl QjswasmError {
    /// A refused module, as [`Load`](Self::Load) when the refusal is
    /// section-level and as [`LoadInFunction`](Self::LoadInFunction) when
    /// tinyvm could say which function body failed.
    pub(crate) fn from_load(refused: tinyvm::LoadError) -> Self {
        match refused.function {
            Some(site) => Self::LoadInFunction {
                error: refused.error,
                index: site.index,
                name: site.name,
                line: site.line,
                column: site.column,
            },
            None => Self::Load(refused.error),
        }
    }
}

impl std::fmt::Display for QjswasmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Compile(e) => write!(f, "compiling .qjs: {e}"),
            Self::Load(e) => write!(f, "loading wasm: {}", e.message()),
            Self::LoadInFunction {
                error,
                index,
                name,
                line,
                column,
            } => {
                match name {
                    Some(name) => write!(
                        f,
                        "loading wasm: {} in function `{name}` (#{index})",
                        error.message()
                    )?,
                    None => write!(f, "loading wasm: {} in function #{index}", error.message())?,
                }
                match (line, column) {
                    (Some(line), Some(column)) => {
                        write!(f, " (line {line}, column {column})")
                    }
                    (Some(line), None) => write!(f, " (line {line})"),
                    _ => Ok(()),
                }
            }
            Self::Trap(e) => write!(f, "guest trapped: {}", e.message()),
            Self::UncaughtThrow(Some(message)) => {
                write!(f, "the script threw and nothing caught it: {message}")
            }
            Self::UncaughtThrow(None) => {
                write!(f, "the script threw a value and nothing caught it")
            }
            Self::PropertyOfNonObject(Some(key)) => write!(
                f,
                "the script read `{key}` off a value that has no properties (undefined, null, a Number or a Boolean); check it with `=== undefined` first"
            ),
            Self::PropertyOfNonObject(None) => f.write_str(
                "the script read a property off a value that has no properties, and the guest could not say which"
            ),
            Self::NotAFunction(Some(callee)) => write!(
                f,
                "the script called `{callee}`, which is not a function (TypeError); a value the engine \
                 does not provide reads as undefined -- check the name and this engine's method list"
            ),
            Self::NotAFunction(None) => f.write_str(
                "the script called a value that is not a function (TypeError)",
            ),
            Self::NoPrimitiveForm(Some(kind)) => write!(
                f,
                "the script used {kind} where a String or a Number was needed; this engine never converts one quietly -- write `JSON.stringify(x)` for its text, or pick the property you meant"
            ),
            Self::NoPrimitiveForm(None) => f.write_str(
                "the script used an Object, an Array or a function where a String or a Number was needed; this engine never converts one quietly -- write `JSON.stringify(x)` for its text",
            ),
            Self::HostArgument(Some((host, position))) => write!(
                f,
                "host function `{host}` needs a String for argument {position}; the script passed something else (write `\"\" + x` to make it one)"
            ),
            Self::HostArgument(None) => f.write_str(
                "a host function was given an argument of the wrong type, and the guest could not say which"
            ),
            Self::UnsupportedMethod(Some(name)) => write!(
                f,
                "this engine does not support `{name}` on a String yet; the script reached it at run time"
            ),
            Self::UnsupportedMethod(None) => f.write_str(
                "this engine does not support a String property the script asked for, and could not read back which"
            ),
            Self::CapabilityBoundary(Some(which)) => write!(
                f,
                "the script asked for {which}, which this engine cannot represent (UTF-8 has no lone \
                 surrogate); split by a real separator, or slice at a character boundary"
            ),
            Self::CapabilityBoundary(None) => write!(
                f,
                "the script reached something this engine does not have yet; \
                 the only String property it answers is `length`"
            ),
            Self::InvalidWrite(Some(what)) => write!(
                f,
                "the script wrote {what}; this engine never accepts such a write quietly -- an Array \
                 takes integer indices, and only an Object or an Array takes properties"
            ),
            Self::InvalidWrite(None) => f.write_str(
                "the script wrote where this engine refuses to (a non-index Array key, or a property on \
                 a value that has none)",
            ),
            Self::Budget(what) => write!(f, "budget exhausted: {what}"),
            Self::Cancelled => f.write_str("cancelled by the host while waiting"),
            Self::Door(what) => write!(f, "host door: {what}"),
            Self::NoSuchSlot(SlotId { engine, index }) => {
                write!(f, "no such slot: {index} in engine {engine}")
            }
            Self::NoSuchExport(what) => write!(f, "no such entry point: {what}"),
            Self::Signature(what) => write!(f, "entry point signature: {what}"),
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
    /// See [`Engine::take_failed_stdout`].
    failed_stdout: String,
    /// The bill of the call that last failed, kept for the same reason.
    failed_cost: Option<Cost>,
    budget: Budget,
    /// Whether slots here get the `tool.*` door. Set only by
    /// [`with_tool_door`](Self::with_tool_door), never by a guest.
    tool_door: bool,
    /// What `arg_count()` / `arg(n)` answer inside a tool script. Set by the
    /// embedder per invocation with [`set_tool_args`](Self::set_tool_args);
    /// meaningless -- and unreachable -- without the tool door.
    tool_args: Vec<String>,
    slots: Vec<Option<slot::Slot>>,
    /// This engine's tag, stamped into every [`SlotId`] it mints so another
    /// engine's id cannot address a slot here. Process-wide and monotonic --
    /// see [`SlotId`] for the silent misrouting this closes.
    id: u64,
    next_index: u64,
}

/// Source of [`Engine::id`]. Monotonic for the life of the process, so no two
/// live engines ever share a tag. Wrapping would need 2^64 engines in one
/// process; `Relaxed` is enough because the only requirement is uniqueness,
/// not ordering against anything else.
static NEXT_ENGINE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
            tool_door: false,
            tool_args: Vec::new(),
            slots: Vec::new(),
            failed_stdout: String::new(),
            failed_cost: None,
            id: NEXT_ENGINE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            next_index: 0,
        }
    }

    /// An engine whose slots also get the `tool.*` door: filesystem, child
    /// processes, environment (see [`tool_door_declarations`]).
    ///
    /// This is the only way to open it. A `Guest::Qjs` spawned here is
    /// compiled with [`compile_qjs_tool`]; a `Guest::Wasm` or
    /// `Guest::CompiledQjs` may import `tool.*` and is bound to the real
    /// door. The same bytes in an engine built any other way are refused at
    /// load time, naming the import. Every tool operation a call reaches is
    /// listed in [`Outcome::tool_calls`], so the caller that chose to open the
    /// door can also say what came through it.
    /// What a tool script's `arg_count()` / `arg(n)` answer. Applies to slots
    /// spawned after the call; a sandbox engine keeps the values and never
    /// exposes them, because it has no door they could leave through.
    pub fn set_tool_args(&mut self, args: Vec<String>) {
        self.tool_args = args;
    }

    pub fn with_tool_door(budget: Budget) -> Self {
        Self {
            tool_door: true,
            tool_args: Vec::new(),
            ..Self::with_budget(budget)
        }
    }

    pub fn budget(&self) -> &Budget {
        &self.budget
    }

    /// Whether this engine's slots have the `tool.*` door.
    pub fn has_tool_door(&self) -> bool {
        self.tool_door
    }

    /// Compile (for `.qjs`), validate, bind the host door, instantiate, and run
    /// the start function. Does **not** call an entry point.
    pub fn spawn(
        &mut self,
        guest: Guest<'_>,
        bridge: Option<FleetBridgeFn>,
    ) -> Result<SlotId, QjswasmError> {
        let owned;
        // The convention travels with the bytes: a hand-written module speaks
        // wasm numerics, and what the compiler emits speaks the V1 pair. The
        // slot has to remember which, because by the time `call` runs there is
        // nothing in the bytes that says where they came from.
        let (bytes, convention) = match guest {
            Guest::Wasm(bytes) => (bytes, slot::Convention::Wasm),
            Guest::CompiledQjs(bytes) => (bytes, slot::Convention::JsV1),
            Guest::Qjs(source) => {
                // Compiled against the doors this engine will actually bind,
                // so a tool engine's `check` and `execute` agree just as a
                // sandbox engine's do.
                owned = if self.tool_door {
                    compile_qjs_tool(source)?
                } else {
                    compile_qjs(source)?
                };
                (&owned[..], slot::Convention::JsV1)
            }
        };
        let tool = self.tool_door.then(|| self.tool_args.clone());
        let slot = slot::Slot::load(bytes, &self.budget, bridge, convention, tool)?;
        let id = SlotId {
            engine: self.id,
            index: self.next_index,
        };
        self.next_index += 1;
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
    /// # A `.qjs` slot whose heap ran out is spent, and says so every time
    ///
    /// The one exception to the paragraph above, stated rather than left to be
    /// discovered. The compiled guest's allocator is a bump pointer that is
    /// advanced *before* it tries to grow linear memory, so a refused growth
    /// leaves the pointer past the end of memory. Every later allocation in
    /// that slot fails too, however small -- and there is no host-side way to
    /// wind the pointer back, because it is the guest's own global.
    ///
    /// What the engine guarantees is that it keeps saying so honestly:
    /// [`QjswasmError::Budget`]`("max_memory_pages")` on that call and on every
    /// call after it, never an opaque trap. A caller who sees it should
    /// [`kill`](Self::kill) the slot and spawn another, or raise
    /// [`Budget::limits`]`.max_memory_pages` for the new one. Non-allocating
    /// work in the same slot still runs, which is why the slot is not retired
    /// automatically: that remains the caller's decision, as it is for a trap.
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
        if slot.engine != self.id {
            // An id minted by another engine. It used to address the slot at
            // the same index here; see `SlotId`.
            return Err(QjswasmError::NoSuchSlot(slot));
        }
        let s = self
            .slots
            .get_mut(slot.index as usize)
            .and_then(Option::as_mut)
            .ok_or(QjswasmError::NoSuchSlot(slot))?;
        let outcome = s.call(entry, args, &self.budget);
        if outcome.is_err() {
            self.failed_stdout = s.take_failed_stdout();
            self.failed_cost = s.take_failed_cost();
        }
        outcome
    }

    /// What the most recent failed call printed before it failed, once. A
    /// script that prints STEP lines and then throws used to lose them: the
    /// error face has no room, and every wave-2 migration group asked. Empty
    /// after a successful call or a second take.
    pub fn take_failed_stdout(&mut self) -> String {
        std::mem::take(&mut self.failed_stdout)
    }

    /// What the call that last failed cost, if it ran at all -- read once,
    /// like [`take_failed_stdout`](Self::take_failed_stdout).
    pub fn take_failed_cost(&mut self) -> Option<Cost> {
        self.failed_cost.take()
    }

    /// Spawn, call, and reclaim. The common path for a one-shot guest.
    ///
    /// # The slot is reclaimed even if the call unwinds
    ///
    /// "Reclaim" has to hold unconditionally, because the caller never receives
    /// the [`SlotId`]: a slot leaked here is unreachable for the life of the
    /// engine while still holding its linear memory, up to
    /// `max_memory_pages`. A plain `spawn` / `call` / `kill` sequence does not
    /// hold it -- a panic anywhere inside `call` jumps straight past the
    /// `kill`.
    ///
    /// The panic is re-raised afterwards, unchanged: this is a `finally`, not a
    /// `catch`. A panic is a bug and swallowing it here would hide it from the
    /// embedder whose code raised it.
    ///
    /// The one panic this crate knows how to provoke -- an embedder's fleet
    /// bridge -- no longer reaches here, because the door contains it and
    /// reports [`QjswasmError::Door`] (see `src/host.rs`). This guard is for
    /// the ones nobody has thought of.
    pub fn run_once(
        &mut self,
        guest: Guest<'_>,
        bridge: Option<FleetBridgeFn>,
        entry: &str,
        args: &[Value],
    ) -> Result<Outcome, QjswasmError> {
        let id = self.spawn(guest, bridge)?;
        let out =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.call(id, entry, args)));
        self.kill(id);
        match out {
            Ok(out) => out,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// Reclaim a slot. A later [`call`](Self::call) on it reports
    /// [`QjswasmError::NoSuchSlot`] rather than panicking.
    ///
    /// An id minted by a different [`Engine`] reclaims nothing here, silently,
    /// because it names nothing here -- see [`SlotId`]. Silence is right for
    /// `kill` specifically: it is idempotent by design (killing an already-dead
    /// slot is not an error either), so there is no failure to report.
    pub fn kill(&mut self, slot: SlotId) {
        if slot.engine != self.id {
            // A foreign id names nothing here. It used to destroy this
            // engine's slot at the same index, silently; see `SlotId`.
            return;
        }
        if let Some(entry) = self.slots.get_mut(slot.index as usize) {
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
///
/// # Imports are checked here, not left to the first call
///
/// Decoding alone used to be the whole check, and it let through a guest whose
/// imports nobody can bind -- anything outside `agenterm.*`, most obviously a
/// `wasi_snapshot_preview1` guest. Such a module validated clean and then died
/// at run time on `Trap("call to unbound imported function")`, naming no
/// import. That is exactly the confusion PRD 36 requires the engine to avoid:
/// a load-time rejection and an execution-time trap have to be tellable apart,
/// and a `check` that passes what `execute` cannot run is the worst shape a
/// gate can have. [`host::check_declarations`] answers it from the import
/// section, statically, without instantiating anything.
pub fn validate_wasm_with(bytes: &[u8], budget: &Budget) -> Result<(), QjswasmError> {
    let module = tinyvm::WasmModule::from_bytes_explained(bytes, budget.limits)
        .map_err(QjswasmError::from_load)?;
    host::check_declarations(&module, false)
}

/// [`validate_wasm_with`] as an [`Engine::with_tool_door`] would gate: a
/// `tool.*` import is accepted (and still signature-checked) instead of
/// refused. Use it to check a module that will run in a tool engine; the
/// sandbox function stays the right check for everything else.
pub fn validate_wasm_tool_with(bytes: &[u8], budget: &Budget) -> Result<(), QjswasmError> {
    let module = tinyvm::WasmModule::from_bytes_explained(bytes, budget.limits)
        .map_err(QjswasmError::from_load)?;
    host::check_declarations(&module, true)
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
