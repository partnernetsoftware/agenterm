//! Unified per-engine `ScriptEngineBackend` trait + static-dispatch enum.
//!
//! Trait-M1-M4 of `plan/design-script-engine-trait.md`. This module defines
//! the shared invocation types (§2.2), the `ScriptEngineBackend` trait
//! (§2.3), the per-engine impls, and the `ScriptEngine` static-dispatch enum
//! (§2.4). `script_worker.rs`'s `execute_inner` dispatches only through this
//! trait (Trait-M3, commit 50ab1f7e).
//!
//! Trait-M4 folded the lua and qjs engine-specific invocation logic
//! (host-function wiring, `agenterm_lua`/`agenterm_qjs` calls) directly into
//! `LuaEngineBackend`/`QjsEngineBackend`'s `check`/`execute` — the old
//! `try_execute_lua_invocation`/`try_execute_qjs_invocation` and their
//! `LuaInvocationOptions`/`QjsInvocationOptions`/`LuaInvocationResult`/
//! `QjsInvocationResult` types are deleted from `script_backend.rs`; nothing
//! outside this module's own tests referenced them (verified by grep across
//! `src/`, `tests/`, `crates/`).
//!
//! `RhEngineBackend` was the one exception -- a thin delegation to
//! `try_execute_rh_invocation` in `script_backend.rs` -- until the rh engine
//! left this repository on 2026-08-29 (`partnernetsoftware/rh`). Every
//! variant of `ScriptEngine` is now behind a feature; a build with none of
//! them has an empty enum and answers every request with a named refusal
//! (`script_backend::BackendRefusal`).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::script_backend::ScriptBackend;
use crate::script_protocol::ScriptBudgets;
use crate::script_protocol::ScriptFailureCategory;

// ---------------------------------------------------------------------
// §2.2 — shared types
// ---------------------------------------------------------------------

/// Shared invocation options across every engine. Replaced the
/// previously-duplicated per-engine option structs.
#[derive(Clone, Debug, Default)]
pub struct ScriptInvocationOptions {
    pub project_root: Option<PathBuf>,
    /// The entry file's own directory, when the entry is a file. A `lib/x`
    /// import resolves here first and under `project_root` second: with
    /// `--project-root DIR` the root used to *replace* the entry's directory,
    /// and every entry beside a `lib/` lost its imports (wave 2, group 7).
    pub entry_dir: Option<PathBuf>,
    pub arguments: Option<Value>,
    pub budgets: Option<ScriptBudgets>,
    /// Whether this invocation may open the `tool.*` door. Set from
    /// `ScriptProfile::Tool` and nothing else; every other engine ignores it,
    /// because only qjswasm has a second door to open.
    pub tool_door: bool,
    /// True only when execution is inside the Script worker process supervised
    /// by `WorkerSupervisor`. It is not a permission bit: it records that a
    /// crash, hang, or UB caused by a falsely declared native signature is
    /// contained by a hard-timeout process-tree teardown.
    pub(crate) native_door_contained: bool,
    /// Set by the worker when a cancel frame names this invocation: the
    /// engine ends the call at its next host wait or operation.
    pub cancellation: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// `ScriptInvocation::fixed_clock_ms`: the replay clock, when one was
    /// asked for. Only the qjswasm engine has a clock to fix.
    pub fixed_clock_ms: Option<u64>,
    /// `ScriptInvocation::env_allow`: the secret-looking environment names
    /// this invocation may read (PRD_02_36 A1.16). Only the qjswasm tool
    /// door reads the environment.
    pub env_allow: Vec<String>,
}

/// Unified invocation result. `value` is `Option<serde_json::Value>` for
/// all three engines — lua's native `i64` is widened via
/// `serde_json::Value::from` in `LuaEngineBackend::execute`.
#[derive(Debug)]
pub struct ScriptInvocationResult {
    pub stdout: String,
    pub value: Option<Value>,
    /// What the run cost, for engines that count. `None` is not "it was
    /// free" -- it is "this engine does not measure", which is the honest
    /// answer for five of the six and the reason this is an `Option` rather
    /// than zeroes.
    ///
    /// The distinction is what makes a qualification receipt a *superset*
    /// rather than a differently-shaped equal: `agenterm-qjs` cannot produce
    /// these numbers because nothing in rquickjs counts them, while an
    /// engine whose ceilings live in its core has them already.
    pub cost: Option<ScriptCost>,
}

pub use crate::script_protocol::ScriptCost;

/// Unified error type. Every engine's typed error used to collapse to a bare
/// `String` here -- design §2.2 "哪里不吸收" called that lossy but not a new
/// loss, because callers flattened everything into
/// `ScriptFailureCategory::Configuration` anyway. That premise ended the day
/// a budget was actually enforced: a script that ran out of steps is a
/// `Limit`, one that threw is a `Script`, and reporting either as
/// `configuration` sends the operator to the wrong fix. The text still
/// travels as text; only the category is kept alongside it. `From<String>`
/// keeps every `.map_err(|e| e.to_string())?` site meaning what it did
/// (configuration), so an engine opts into a finer class one seam at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptEngineError {
    pub message: String,
    pub category: ScriptFailureCategory,
    /// What the script printed before it failed, when the engine can say.
    /// Reaches the caller as the result's `stdout` next to the failure, so a
    /// gate script's STEP lines are not lost exactly on the runs that matter.
    pub stdout: String,
    /// What the failed run cost, when the engine counts: a failed wait is
    /// the run whose bill matters most (A1.12).
    pub cost: Option<ScriptCost>,
}

impl From<String> for ScriptEngineError {
    fn from(message: String) -> Self {
        Self {
            message,
            category: ScriptFailureCategory::Configuration,
            stdout: String::new(),
            cost: None,
        }
    }
}

impl From<ScriptEngineError> for String {
    fn from(error: ScriptEngineError) -> Self {
        error.message
    }
}

impl std::fmt::Display for ScriptEngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Fleet bridge callback shared by every engine: (operation_id,
/// params_json) -> result_json.
pub type ScriptFleetBridgeFn = Arc<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>;

// ---------------------------------------------------------------------
// §2.3 — trait body
// ---------------------------------------------------------------------

/// Unified per-engine "single invocation" interface (check one source,
/// execute one source). Does not cover check-many/corpus-scan (already
/// unified at the `agenterm-script-common` crate level) or pack/qualify CLI
/// verbs (engine-specific pack shapes, see design §3 non-goals).
// The error intentionally retains bounded partial stdout and the complete
// deterministic cost receipt. Boxing it would add an allocation to every
// engine failure and would only move, not reduce, that owned diagnostic data.
#[allow(clippy::result_large_err)]
pub trait ScriptEngineBackend {
    /// The corresponding `ScriptBackend` variant.
    fn backend_id(&self) -> ScriptBackend;

    /// Entry-file extensions this engine claims, mirroring
    /// `ScriptBackend::from_entry_path`'s routing table.
    fn entry_extensions(&self) -> &'static [&'static str];

    /// Whether this engine is selected via `AGENTERM_SCRIPT_BACKEND`.
    /// Default implementation reads the global `ScriptBackend::from_env()`;
    /// an unset variable selects nothing, so this is `false` for every engine.
    fn enabled(&self) -> bool {
        ScriptBackend::from_env().ok() == Some(self.backend_id())
    }

    /// Check operation. Engines return `Err` on empty source without any
    /// special-casing at the trait level.
    fn check(
        &self,
        source: &str,
        options: &ScriptInvocationOptions,
    ) -> Result<(), ScriptEngineError>;

    /// Build the self-contained artifact this engine would deploy, and name
    /// its file extension. `None` when this engine has no artifact to build
    /// through this face.
    ///
    /// The first half of the byte-carrying face. `agenterm cli script` reads
    /// files with `read_to_string` and every method above takes `&str`, so a
    /// module has no route through any of them -- which is why `pack`,
    /// `qualify` and `run-smoke` had nowhere to live and why wasmcore needs
    /// [`Self::source_is_a_path`] to work at all. These two methods are that
    /// missing face, and the four verbs are one face rather than four
    /// adapters.
    fn pack_artifact(&self, source: &str) -> Option<Result<(Vec<u8>, &'static str), String>>;

    /// Run an already-built artifact. `None` when this engine cannot load
    /// bytes.
    ///
    /// Separate from [`Self::execute`] and not a widening of it: that one
    /// takes `&str` and six engines are green on those terms. A module is not
    /// a program's text and pretending it is, is precisely the confusion
    /// `source_is_a_path` documents.
    fn execute_artifact(
        &self,
        artifact: &[u8],
        options: &ScriptInvocationOptions,
        fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Option<Result<ScriptInvocationResult, ScriptEngineError>>;

    /// Whether this engine reads `source` as a **filesystem path** instead of
    /// as the program's text.
    ///
    /// Exactly one does, and it is a wart rather than a design: `check`/
    /// `execute` take `&str` and every other engine puts a program in it.
    /// The CLI has to know, because it decides what to put there -- and the
    /// decision must follow *which engine will run*, not the file's
    /// extension. Keyed on the extension it silently handed a path to
    /// whichever engine was selected, and rh, lua and qjswasm each parsed the
    /// path as a program (measured; see the CLI's own comment at the branch).
    ///
    /// No default implementation, so an engine cannot acquire this behaviour
    /// by omission -- which is how it would come back.
    fn source_is_a_path(&self) -> bool;

    /// What `hash FILE` should print for this engine: the digest, and **the
    /// name of the thing that was digested**.
    ///
    /// The second half is the point. `agenterm-qjs hash` prints a sha256 of
    /// the *source*; a compiler-backed engine can hash the artifact it would
    /// actually produce, which is a different and stronger claim. Printing
    /// both under one verb without saying which is which is the "one verb,
    /// two questions" shape [`Self::corpus_scan`] refuses for `.wasm`, so this
    /// returns the label with the digest and the caller prints both.
    ///
    /// There is a live reason not to trust an unlabelled hash here: PRD 02.36
    /// records that `agenterm-qjs`'s `pack` writes a `bytecode_hash`
    /// documented as "a genuine reproducibility fingerprint" which **is not
    /// one** -- the same source built to two different `--dir`s produces two
    /// different values, because the compile label is the absolute output path
    /// and `Module::write` embeds it. A reader who cannot see what was hashed
    /// cannot see that either.
    ///
    /// `None` when this engine has nothing to hash beyond bytes the caller
    /// already has.
    fn artifact_hash(&self, source: &str) -> Option<Result<(String, &'static str), String>>;

    /// Scan a directory recursively for this engine's source files and check
    /// each, or `None` when this engine has no corpus scanner.
    ///
    /// The `Option` is the same distinction [`Self::eval_entry_source`] makes
    /// and for the same reason: "this engine cannot do that" and "the scan
    /// failed" are two different answers, and a single `Result` would make a
    /// caller read the first as the second. rh's corpus-scan lives in its own
    /// dev CLI rather than here, and wasmcore has no source to scan.
    fn corpus_scan(
        &self,
        dir: &std::path::Path,
    ) -> Option<Result<agenterm_script_common::corpus_scan::CorpusScanReport, String>>;

    /// One line naming this engine and the build it is: the `version` verb's
    /// answer for whichever engine is selected.
    ///
    /// Every engine has an identity worth printing and they are not the same
    /// shape -- a compiler-backed engine's upstream pin is half of "which
    /// build is this", and an engine that is a thin binding has no such thing.
    /// So each answers for itself, for the reason
    /// [`Self::eval_entry_source`] gives at length about a shared verb that
    /// bakes in one engine's assumptions.
    fn identity(&self) -> String;

    /// Wrap one expression as a program this engine will run for its value:
    /// the source `script eval EXPRESSION` should hand [`Self::execute`].
    ///
    /// `None` means this engine has no expression form, and `script eval`
    /// refuses by name rather than running something.
    ///
    /// # Why this is on the trait and not one function in the CLI
    ///
    /// It *was* one function in the CLI, `script_eval_entry_source`, and it
    /// emitted **rh source** -- `fn __agenterm_eval_expression() { … }` plus a
    /// `rh::json::stringify` and a marker line -- for whichever engine was
    /// selected. Measured 2026-08-26, `script eval '1 + 2'` on each engine:
    ///
    /// | engine | what it said |
    /// |--------|--------------|
    /// | lua | `lua_runtime: syntax error` |
    /// | qjs | `qjs parse error: Error: expecting` … |
    /// | qjswasm | ``compiling .qjs: this engine needs a `;` `` … |
    /// | wasmcore | `wasm execution: loading wasm module fn` … |
    ///
    /// Four engines, four parsers complaining about a fifth engine's dialect.
    /// A shared verb that bakes in one engine's syntax is not shared, and the
    /// only structural fix is to let each engine answer for its own -- which
    /// is what having no default implementation here enforces: a new engine
    /// cannot compile without deciding.
    ///
    /// Every wrapper below is measured, not assumed. See each implementation.
    fn eval_entry_source(&self, expression: &str) -> Option<String>;

    /// Run/Eval operation. `ScriptOperation::Api` is short-circuited by
    /// `execute_inner`'s caller before reaching any backend (design §1.3
    /// finding 2) so it is not represented in this trait's surface.
    fn execute(
        &self,
        source: &str,
        options: &ScriptInvocationOptions,
        fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Result<ScriptInvocationResult, ScriptEngineError>;
}

/// Compile-time object-safety assertion (design §2.4 concludes the trait is
/// object-safe; this function's mere existence is the proof — it never runs).
#[allow(dead_code)]
fn _assert_object_safe(_backend: &dyn ScriptEngineBackend) {}

// ---------------------------------------------------------------------
// §4 Trait-M2 — per-engine thin adapters
// ---------------------------------------------------------------------

#[cfg(feature = "script-qjswasm")]
/// The engine budget for one invocation. `--max-operations` is the only CLI
/// budget the core enforces itself (as the step ceiling per top-level call);
/// `--timeout-ms` is the worker's deadline and never reaches the guest.
/// Without this the CLI accepted the flag and the guest ran under the
/// 16M default anyway.
/// The one seam where a qjswasm failure keeps its class. `Budget` is the
/// engine refusing to spend more (steps, pages, depth): a `Limit`, and the
/// fix is a `--max-*` flag. An uncaught `throw` or a guest trap is the
/// script's own doing: a `Script`. Source compilation and unsupported source
/// methods are also `Script`: the author must change the program, and
/// `check-many` already reports that same class. Load, door, and signature
/// failures remain invocation `Configuration` errors.
fn qjs_engine_error(error: agenterm_qjswasm::QjswasmError) -> ScriptEngineError {
    use agenterm_qjswasm::QjswasmError as E;
    let category = match &error {
        E::Budget(_) => ScriptFailureCategory::Limit,
        E::Cancelled => ScriptFailureCategory::Cancelled,
        E::UncaughtThrow(_)
        | E::Trap(_)
        | E::Compile(_)
        | E::UnsupportedMethod(_)
        | E::HostArgument(_)
        | E::PropertyOfNonObject(_)
        | E::NotAFunction(_)
        | E::NoPrimitiveForm(_)
        | E::InvalidWrite(_)
        // A boundary of the engine's representation (`split("")`, a mid-surrogate
        // `slice`, a String property it lacks) is still the script's to rewrite
        // around -- the sentence says how -- not an invocation set up wrong.
        | E::CapabilityBoundary(_) => ScriptFailureCategory::Script,
        _ => ScriptFailureCategory::Configuration,
    };
    ScriptEngineError {
        message: error.to_string(),
        category,
        stdout: String::new(),
        cost: None,
    }
}

#[cfg(feature = "script-qjswasm")]
fn qjs_compile_error(error: agenterm_qjswasm::CompileError) -> ScriptEngineError {
    ScriptEngineError {
        message: error.to_string(),
        category: ScriptFailureCategory::Script,
        stdout: String::new(),
        cost: None,
    }
}

#[cfg(feature = "script-qjswasm")]
fn script_cost(cost: agenterm_qjswasm::Cost) -> ScriptCost {
    ScriptCost {
        steps: cost.steps,
        peak_call_depth: cost.peak_call_depth,
        peak_activation_slots: cost.peak_activation_slots,
        host_ops: cost.host_ops,
        host_bytes: cost.host_bytes,
        waited_ms: cost.waited_ms,
        heap_pages: cost.heap_pages,
        heap_bytes: cost.heap_bytes,
        heap_start_bytes: cost.heap_start_bytes,
        json_parse_bytes: cost.json_parse_bytes,
        json_stringify_bytes: cost.json_stringify_bytes,
        immediate_stringify_host_argument_bytes: cost.immediate_stringify_host_argument_bytes,
    }
}

#[cfg(feature = "script-qjswasm")]
fn qjs_budget(options: &ScriptInvocationOptions) -> agenterm_qjswasm::Budget {
    let mut budget = agenterm_qjswasm::Budget::default();
    if let Some(budgets) = options.budgets.as_ref() {
        budget.limits.max_steps = budgets.operations;
        budget.max_host_ops = budgets.host_operations;
        // A tool result becomes a guest string, so the public invocation's
        // string ceiling is also the qjswasm door's all-or-nothing result
        // ceiling. Keeping the engine default here used to reject a valid
        // 3.5 MiB `fs_read_to_string` in the supply-chain task even though
        // that task explicitly requested the reviewed 8 MiB string budget.
        budget.max_bridge_result_bytes = budgets.string_bytes;
    }
    budget.cancel = options.cancellation.clone();
    budget.fixed_clock_ms = options.fixed_clock_ms;
    budget.env_allow = options.env_allow.clone();
    // The guest heap is a bump allocator with no collector: everything a
    // script parses or concatenates stays until the call ends. tinyvm's
    // default of 256 pages (16 MiB) stopped unix-frontend-smoke at its fifth
    // step on 2026-08-30, after two clipboard journeys' worth of answers.
    // 1024 pages is 64 MiB, still a hard cap on a runaway, and the wall-clock
    // and step budgets stand beside it.
    budget.limits.max_memory_pages = std::env::var("AGENTERM_QJS_MAX_MEMORY_PAGES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|pages| (1..=65_536).contains(pages))
        .unwrap_or(QJS_MAX_MEMORY_PAGES);
    budget
}

#[cfg(feature = "script-qjswasm")]
/// The guest heap ceiling for a `.qjs` invocation, in 64 KiB pages.
/// `AGENTERM_QJS_MAX_MEMORY_PAGES` overrides it for one process -- the knob
/// a journey's author turns to price the heap it needs, the way
/// `--max-operations` prices its steps.
pub(crate) const QJS_MAX_MEMORY_PAGES: usize = 1024;

#[cfg(feature = "script-qjswasm")]
/// Compile through the doors selected for this phase. Tool-vs-sandbox remains
/// symmetric between check and execute. The native door is the deliberate
/// exception: only production execution inside the supervised worker enables
/// it, because compiling/checking does not provide crash or hang containment.
fn compile_qjs_for(
    options: &ScriptInvocationOptions,
    source: &str,
    resolve: &dyn Fn(&str) -> Option<String>,
    execution: bool,
) -> Result<Vec<u8>, agenterm_qjswasm::CompileError> {
    let native_door = execution && options.native_door_contained;
    let allocation_probe = std::env::var_os("AGENTERM_QJS_ALLOCATION_PROBE").is_some();
    if options.tool_door && native_door && allocation_probe {
        agenterm_qjswasm::compile_qjs_tool_native_with_modules_and_allocation_probe(source, resolve)
    } else if options.tool_door && native_door {
        agenterm_qjswasm::compile_qjs_tool_native_with_modules(source, resolve)
    } else if native_door && allocation_probe {
        agenterm_qjswasm::compile_qjs_native_with_modules_and_allocation_probe(source, resolve)
    } else if native_door {
        agenterm_qjswasm::compile_qjs_native_with_modules(source, resolve)
    } else if options.tool_door && allocation_probe {
        agenterm_qjswasm::compile_qjs_tool_with_modules_and_allocation_probe(source, resolve)
    } else if options.tool_door {
        agenterm_qjswasm::compile_qjs_tool_with_modules(source, resolve)
    } else if allocation_probe {
        agenterm_qjswasm::compile_qjs_with_modules_and_allocation_probe(source, resolve)
    } else {
        agenterm_qjswasm::compile_qjs_with_modules(source, resolve)
    }
}

#[cfg(feature = "script-qjswasm")]
fn qjs_execution_engine(options: &ScriptInvocationOptions) -> agenterm_qjswasm::Engine {
    let engine = if options.tool_door {
        agenterm_qjswasm::Engine::with_tool_door(qjs_budget(options))
    } else {
        agenterm_qjswasm::Engine::with_budget(qjs_budget(options))
    };
    if options.native_door_contained {
        // This is crash/hang/UB containment, not an Agent permission decision.
        // Dynamic symbol lookup cannot validate the signature declared by the
        // guest. The Script worker sets this fact only inside its supervised
        // child process; direct client pack/load/qualify calls leave it false.
        engine.enable_native_door()
    } else {
        engine
    }
}

/// What a `.qjs` module specifier points at, for this product.
///
/// The compiler never touches a filesystem, so somebody has to say what a
/// specifier means, and this is where the product says it: **a path under the
/// invocation's project root, with the extension left off**.
///
/// That root defaults to the *entry file's own directory*
/// (`client::direct_script_context`), so a script beside `lib/fleet.qjs`
/// writes `import * as lib from "lib/fleet"` and needs nothing configured --
/// which is ECMA-262's relative-to-the-importing-file shape in practice.
/// `--project-root` widens it when a script genuinely needs to reach further,
/// and then the specifier is relative to that instead. One rule, two
/// behaviours that follow from what the root is, rather than two rules.
///
/// The extension is left off because `scripts/rh/**` already writes its 42
/// imports that way. There is no reason for the two languages to disagree
/// about a detail neither of them cares about.
///
/// Three refusals, each returning `None` so the caller gets a diagnostic
/// naming the specifier:
///
/// * no project root -- nothing to be relative *to*, which is the honest
///   answer for a script run from stdin;
/// * a specifier that leaves the root once resolved, `../` or a symlink or an
///   absolute path alike. The check is on the **canonical** path, because a
///   textual one is defeated by any of the three;
/// * a file that is not there or is not UTF-8.
pub(crate) const AGENTERM_ACU_MODULE_SOURCE: &str = r#"
function request(payload) {
  const status = acu_call(JSON.stringify(payload));
  const result = acu_result();
  if (status !== 0) { throw "agenterm:acu bridge status " + status + ": " + result; }
  return JSON.parse(result);
}

export function call(command) {
  return request({acu_request: 1, kind: "command", command: command});
}

export function argv(args) {
  return request({acu_request: 1, kind: "argv", argv: args});
}
"#;

pub(crate) const AGENTERM_ACU_ENTRY_LABEL: &str = "agenterm-acu.qjs";
pub(crate) const AGENTERM_ACU_ENTRY_SOURCE: &str = include_str!("../skills/acu/acu.qjs");
pub(crate) const AGENTERM_ACU_ARGV_SOURCE: &str = include_str!("../skills/acu/lib/argv.qjs");
pub(crate) const AGENTERM_ACU_LEGACY_ARGS_SOURCE: &str =
    include_str!("../skills/acu/lib/legacy_args.qjs");
pub(crate) const AGENTERM_ACU_REWRITE_SOURCE: &str = include_str!("../skills/acu/lib/rewrite.qjs");
pub(crate) const AGENTERM_ACU_COMPAT_SOURCE: &str = include_str!("../skills/acu/lib/compat.qjs");
pub(crate) const AGENTERM_ACU_COMPOUND_SOURCE: &str =
    include_str!("../skills/acu/lib/compound.qjs");

/// Product-owned qjswasm modules. This is the one registry used by runtime,
/// single-file checking and bounded check-many, so a built-in cannot resolve
/// to different bytes depending on which public door reached the compiler.
pub(crate) fn qjs_builtin_module_source(specifier: &str) -> Option<&'static str> {
    match specifier {
        "agenterm:acu" => Some(AGENTERM_ACU_MODULE_SOURCE),
        "agenterm:acu/argv" => Some(AGENTERM_ACU_ARGV_SOURCE),
        "agenterm:acu/legacy-args" => Some(AGENTERM_ACU_LEGACY_ARGS_SOURCE),
        "agenterm:acu/rewrite" => Some(AGENTERM_ACU_REWRITE_SOURCE),
        "agenterm:acu/compat" => Some(AGENTERM_ACU_COMPAT_SOURCE),
        "agenterm:acu/compound" => Some(AGENTERM_ACU_COMPOUND_SOURCE),
        _ => None,
    }
}

fn qjs_module_resolver(roots: &[PathBuf]) -> impl Fn(&str) -> Option<String> + use<> {
    // Roots in order of preference, each confined to itself; the first that
    // has the file answers. Duplicates (the usual case: the entry's directory
    // *is* the project root) collapse.
    let mut canonical: Vec<PathBuf> = Vec::new();
    for root in roots {
        if let Ok(c) = root.canonicalize()
            && !canonical.contains(&c)
        {
            canonical.push(c);
        }
    }
    move |specifier: &str| {
        // Product-owned built-in: resolve before the filesystem so a project
        // cannot shadow the typed ACU adapter with a same-named file.
        if let Some(source) = qjs_builtin_module_source(specifier) {
            return Some(source.to_owned());
        }
        canonical.iter().find_map(|root| {
            let mut candidate = root.join(specifier);
            if candidate.extension().is_none() {
                candidate.set_extension("qjs");
            }
            let resolved = candidate.canonicalize().ok()?;
            if !resolved.starts_with(root) {
                return None;
            }
            std::fs::read_to_string(resolved).ok()
        })
    }
}

/// The resolver roots an invocation gets: the entry's directory, then the
/// project root.
#[cfg(feature = "script-qjswasm")]
fn qjs_roots(options: &ScriptInvocationOptions) -> Vec<PathBuf> {
    [options.entry_dir.clone(), options.project_root.clone()]
        .into_iter()
        .flatten()
        .collect()
}

#[cfg(feature = "script-qjswasm")]
/// The invocation's `-- ARGS...` as text, in order. Every CLI argument is
/// text; a script that wants a number writes `Number(tool_result())` after
/// `arg(n)`, which is the conversion ECMA-262 would apply and the one the
/// `Number` fold exists for.
fn qjs_arguments(arguments: Option<&Value>) -> Vec<String> {
    arguments
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| item.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Build the `args_len`/`arg` host-function closures shared by the lua and
/// qjs backends from a script invocation's `arguments` value.
///
/// `LuaHostFunctions` and `QjsHostFunctions` declare `args_len`/`arg` with
/// structurally identical trait-object types: `Arc<dyn Fn() -> i64 + Send +
/// Sync>` for `args_len`, and `Arc<dyn Fn(i64) -> Result<String, String> +
/// Send + Sync>` for `arg` (aliased per-crate as `ArgFn`). Type aliases
/// don't create distinct types, so the same pair of closures can be
/// assigned directly to either engine's host-function struct.
///
/// Moved here (Trait-M4) from `script_backend.rs` — only `LuaEngineBackend`/
/// `QjsEngineBackend::execute` call this now.
#[cfg(feature = "script-lua")]
type ScriptArgsAccessors = (
    Arc<dyn Fn() -> i64 + Send + Sync>,
    Arc<dyn Fn(i64) -> Result<String, String> + Send + Sync>,
);

#[cfg(feature = "script-lua")]
fn script_args_accessors(arguments: Value) -> ScriptArgsAccessors {
    let args_for_len = arguments.clone();
    let args_for_arg = arguments;
    let args_len: Arc<dyn Fn() -> i64 + Send + Sync> =
        Arc::new(move || args_for_len.as_array().map(|a| a.len() as i64).unwrap_or(0));
    let arg: Arc<dyn Fn(i64) -> Result<String, String> + Send + Sync> =
        Arc::new(move |index: i64| {
            args_for_arg
                .as_array()
                .and_then(|a| a.get(index as usize))
                .and_then(|v| v.as_str())
                .map(|s| s.to_owned())
                .ok_or_else(|| format!("argument {index} is unavailable"))
        });
    (args_len, arg)
}

#[cfg(feature = "script-lua")]
pub struct LuaEngineBackend;

#[cfg(feature = "script-lua")]
impl ScriptEngineBackend for LuaEngineBackend {
    fn backend_id(&self) -> ScriptBackend {
        ScriptBackend::Lua
    }

    fn entry_extensions(&self) -> &'static [&'static str] {
        &["lua"]
    }

    fn identity(&self) -> String {
        format!("agenterm-lua {}", env!("CARGO_PKG_VERSION"))
    }

    /// `None`: this engine's deployable artifact is its own CLI's shape (rh's
    /// native pack, lua's and qjs's bytecode directories), which is a
    /// directory plus a manifest rather than one file of bytes. Offering half
    /// of it here would be a second, thinner answer to a question that
    /// already has one.
    fn pack_artifact(&self, _source: &str) -> Option<Result<(Vec<u8>, &'static str), String>> {
        None
    }

    fn execute_artifact(
        &self,
        _artifact: &[u8],
        _options: &ScriptInvocationOptions,
        _fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Option<Result<ScriptInvocationResult, ScriptEngineError>> {
        None
    }

    fn source_is_a_path(&self) -> bool {
        false
    }

    fn artifact_hash(&self, source: &str) -> Option<Result<(String, &'static str), String>> {
        Some(Ok((
            agenterm_script_common::hex::sha256_hex(source.as_bytes()),
            "source",
        )))
    }

    fn corpus_scan(
        &self,
        dir: &std::path::Path,
    ) -> Option<Result<agenterm_script_common::corpus_scan::CorpusScanReport, String>> {
        Some(agenterm_lua::corpus_scan::scan_directory(dir))
    }

    /// A top-level `return`, which is what a lua chunk answers with.
    ///
    /// Measured: `print("lua ok") return 1 + 2` through `script run` prints
    /// and answers `3`, while a file defining only `entry()` answers `0` --
    /// lua's entry point is the chunk itself and nothing calls `entry`.
    fn eval_entry_source(&self, expression: &str) -> Option<String> {
        Some(format!("return ({expression})"))
    }

    fn check(
        &self,
        source: &str,
        _options: &ScriptInvocationOptions,
    ) -> Result<(), ScriptEngineError> {
        let engine = agenterm_lua::LuaEngine::new().map_err(|e| e.to_string())?;

        engine.check(source).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn execute(
        &self,
        source: &str,
        options: &ScriptInvocationOptions,
        fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Result<ScriptInvocationResult, ScriptEngineError> {
        let engine = agenterm_lua::LuaEngine::new().map_err(|e| e.to_string())?;

        let mut host = agenterm_lua::LuaHostFunctions::default();

        // Wire fleet bridge
        if let Some(bridge) = fleet_bridge {
            host.fleet_call = Some(Arc::new(
                move |op_id: String, params: String| -> Result<String, String> {
                    bridge(op_id.as_str(), params.as_str())
                },
            ));
        }

        // Wire args_len / arg from options.arguments
        if let Some(arguments) = options.arguments.clone() {
            let (args_len, arg) = script_args_accessors(arguments);
            host.args_len = Some(args_len);
            host.arg = Some(arg);
        }

        let result = engine
            .eval(source, &host)
            .map_err(|e| format!("lua_eval: {e}"))?;

        Ok(ScriptInvocationResult {
            stdout: result.stdout,
            value: Some(Value::from(result.value)),
            cost: None,
        })
    }
}

/// sql engine adapter — see `plan/design-sql-execution-target.md` (the M1
/// design doc this impl now implements) and
/// `crates/agenterm-sql/src/lib.rs`'s module doc (the honest "what's real
/// vs. placeholder" writeup this impl matches).
///
/// `check` is real: delegates to `agenterm_sql::check`, which really parses
/// via `sqlparser`. `execute` is ALSO real as of M1: delegates to
/// `agenterm_sql::execute_entry`, which runs `source` against a private,
/// in-process, ephemeral SQLite database (`rusqlite`, `bundled`) — see that
/// function's module doc for the full value-mapping/budget/dialect-skew
/// writeup. `fleet_bridge` remains unused here — M2 (host-state-as-virtual-
/// tables) is the design doc's own plan for wiring it in, not M1; §2.6 of
/// the earlier trait-design doc predicted "sql 不需要 trait 新增方法，只需要
/// execute 内部把 fleet_bridge 参数忽略掉", and that still holds for M1's
/// scope even though execute() itself is no longer a placeholder.
#[cfg(feature = "script-sql")]
pub struct SqlEngineBackend;

#[cfg(feature = "script-sql")]
impl ScriptEngineBackend for SqlEngineBackend {
    fn backend_id(&self) -> ScriptBackend {
        ScriptBackend::Sql
    }

    fn entry_extensions(&self) -> &'static [&'static str] {
        &["sql"]
    }

    fn identity(&self) -> String {
        format!("agenterm-sql {}", env!("CARGO_PKG_VERSION"))
    }

    /// `None`: this engine's deployable artifact is its own CLI's shape (rh's
    /// native pack, lua's and qjs's bytecode directories), which is a
    /// directory plus a manifest rather than one file of bytes. Offering half
    /// of it here would be a second, thinner answer to a question that
    /// already has one.
    fn pack_artifact(&self, _source: &str) -> Option<Result<(Vec<u8>, &'static str), String>> {
        None
    }

    fn execute_artifact(
        &self,
        _artifact: &[u8],
        _options: &ScriptInvocationOptions,
        _fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Option<Result<ScriptInvocationResult, ScriptEngineError>> {
        None
    }

    fn source_is_a_path(&self) -> bool {
        false
    }

    fn artifact_hash(&self, source: &str) -> Option<Result<(String, &'static str), String>> {
        Some(Ok((
            agenterm_script_common::hex::sha256_hex(source.as_bytes()),
            "source",
        )))
    }

    fn corpus_scan(
        &self,
        dir: &std::path::Path,
    ) -> Option<Result<agenterm_script_common::corpus_scan::CorpusScanReport, String>> {
        Some(agenterm_sql::corpus_scan::scan_directory(dir))
    }

    /// `SELECT`, because SQL has no expression that is also a statement.
    /// Measured: `SELECT 1 + 2;` runs and answers rows; a bare `1 + 2` is
    /// `sql parser error: Expected: an SQL statement, found: 1`.
    ///
    /// The answer is therefore a one-row result set rather than a scalar,
    /// which is this engine's shape for every answer and not something this
    /// verb should flatten.
    fn eval_entry_source(&self, expression: &str) -> Option<String> {
        Some(format!("SELECT ({expression});"))
    }

    fn check(
        &self,
        source: &str,
        _options: &ScriptInvocationOptions,
    ) -> Result<(), ScriptEngineError> {
        agenterm_sql::check(source, "invocation.sql").map_err(|e| e.to_string())?;
        Ok(())
    }

    fn execute(
        &self,
        source: &str,
        options: &ScriptInvocationOptions,
        _fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Result<ScriptInvocationResult, ScriptEngineError> {
        // agenterm_sql::ExecuteBudgets is a small crate-local mirror of
        // ScriptBudgets (agenterm-sql can't depend on this crate's types —
        // see that struct's doc) covering only the M1-enforced subset:
        // wall_time_ms, collection_items, and output_bytes (standing in for
        // both output_bytes and string_bytes — see its doc for why).
        let sql_budgets = options
            .budgets
            .as_ref()
            .map(|budgets| agenterm_sql::ExecuteBudgets {
                wall_time_ms: budgets.wall_time_ms,
                collection_items: budgets.collection_items,
                output_bytes: budgets.output_bytes,
            });

        let outcome = agenterm_sql::execute_entry(source, "invocation.sql", sql_budgets.as_ref())
            .map_err(|e| e.to_string())?;

        Ok(ScriptInvocationResult {
            stdout: outcome.stdout,
            value: outcome.value,
            cost: None,
        })
    }
}

// `WasmcoreEngineBackend` lived here: a `.wasm` adapter over wasmtime that
// JIT-compiled the guest and ran it with the shared fleet bridge. It went with
// `agenterm-wasmcore` on 2026-08-28. `.wasm` is `qjswasm`'s to run now -- on
// tinyvm, interpreted, with the per-call budgets and slot isolation the
// wasmtime path never had. What that trade costs on compute-heavy work was
// measured before the removal and is recorded in
// `prd/PRD_02_36_agenterm_qjswasm.md`, as an input to tinyvm's native-lowering
// track rather than as a regret.

// ---------------------------------------------------------------------
// §2.5 — qjswasm: agenterm's own engine (tinyvm core, no JIT)
// ---------------------------------------------------------------------

/// `.qjs` compiled to `.wasm` in pure Rust, and `.wasm` run directly, both on
/// tinyvm. Product truth: `prd/PRD_02_36_agenterm_qjswasm.md`.
///
/// Deliberately a *separate* backend from `Qjs` rather than a second execution
/// mode of it. `Qjs` links native QuickJS through `rquickjs` and runs trusted
/// local scripts with a full modern-JS surface; this one compiles a growing JS
/// subset and runs the result as a budgeted, validated wasm guest that reaches
/// the world only through the `agenterm.*` door. Same language family, very
/// different capability set — collapsing them into one backend would make
/// "which of these can my script use?" unanswerable.
///
/// `check` compiles (`.qjs`) or load-validates (`.wasm`) without executing, so
/// a start function's side effects never fire during a check. `execute` goes
/// through `Engine::run_once`, which spawns a slot, calls the entry, and
/// reclaims it.
#[cfg(feature = "script-qjswasm")]
pub struct QjswasmEngineBackend;

#[cfg(feature = "script-qjswasm")]
impl ScriptEngineBackend for QjswasmEngineBackend {
    fn backend_id(&self) -> ScriptBackend {
        ScriptBackend::Qjswasm
    }

    fn entry_extensions(&self) -> &'static [&'static str] {
        // `.wasm` is this engine's compiled artifact; since 2026-08-30
        // `ScriptBackend::from_entry_path` routes it here by extension.
        &["qjs", "wasm"]
    }

    /// Carries the upstream pin, which the other engines have no equivalent
    /// of: this one is a compiler, and which `tinyvm` revision it was built
    /// against decides what the language can do. `agenterm_qjswasm::identity`
    /// owns the string and a test in that crate holds it to `Cargo.toml`.
    fn identity(&self) -> String {
        agenterm_qjswasm::identity()
    }

    /// One self-contained `.wasm`, which is the whole of this engine's pack
    /// shape: no bytecode file beside a source directory, no manifest, nothing
    /// that has to be kept in step with anything. That is the difference PRD
    /// 02.36 records as "形状必然不同", and it is the direction that makes it
    /// simpler rather than the direction that makes it worse.
    fn pack_artifact(&self, source: &str) -> Option<Result<(Vec<u8>, &'static str), String>> {
        Some(
            agenterm_qjswasm::compile_qjs(source)
                .map(|wasm| (wasm, "wasm"))
                .map_err(|e| e.to_string()),
        )
    }

    /// `Guest::CompiledQjs`, which exists for exactly this and not for
    /// `Guest::Wasm`: a module compiled from `.qjs` speaks the V1 calling
    /// convention, and loading it as an anonymous wasm guest would lose that
    /// and hand the caller a raw `(i32, i64)` pair where a JavaScript value
    /// was returned. `tests/qjs_guest.rs::a_compiled_artifact_reloaded_gives_the_same_value_as_its_source`
    /// is the assertion that keeps the two routes agreeing.
    fn execute_artifact(
        &self,
        artifact: &[u8],
        options: &ScriptInvocationOptions,
        fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Option<Result<ScriptInvocationResult, ScriptEngineError>> {
        let bridges = qjs_host_bridges(fleet_bridge);
        // A packed `.qjs` artifact is still the same invocation: its tool
        // door, arguments, ceilings, replay clock, environment projection and
        // cancellation identity must not disappear merely because compilation
        // happened earlier. Source execution builds the engine from this exact
        // budget above; keep the artifact route symmetric.
        let mut engine = qjs_execution_engine(options);
        engine.set_tool_args(qjs_arguments(options.arguments.as_ref()));
        Some(
            engine
                .run_once_with_bridges(
                    agenterm_qjswasm::Guest::CompiledQjs(artifact),
                    bridges,
                    "main",
                    &[],
                )
                .map(|outcome| ScriptInvocationResult {
                    stdout: outcome.stdout,
                    value: outcome.values.first().and_then(qjswasm_value_as_json),
                    cost: Some(ScriptCost {
                        steps: outcome.steps,
                        peak_call_depth: outcome.peak_call_depth,
                        peak_activation_slots: outcome.peak_activation_slots,
                        host_ops: outcome.host_ops,
                        host_bytes: outcome.host_bytes,
                        waited_ms: outcome.waited_ms,
                        heap_pages: outcome.heap_pages,
                        heap_bytes: outcome.heap_bytes,
                        heap_start_bytes: outcome.heap_start_bytes,
                        json_parse_bytes: outcome.json_parse_bytes,
                        json_stringify_bytes: outcome.json_stringify_bytes,
                        immediate_stringify_host_argument_bytes: outcome
                            .immediate_stringify_host_argument_bytes,
                    }),
                })
                .map_err(|e| {
                    let mut error = qjs_engine_error(e);
                    error.stdout = engine.take_failed_stdout();
                    error.cost = engine.take_failed_cost().map(script_cost);
                    error
                }),
        )
    }

    /// False, and it was not always: this backend's `execute` documents having
    /// read `source` as a path once, which meant it could never run anything
    /// (`File name too long (os error 63)`, with the whole program in the
    /// message).
    fn source_is_a_path(&self) -> bool {
        false
    }

    /// **The compiled `.wasm`**, not the source -- the one engine here that
    /// can answer the question the verb is actually for.
    ///
    /// Two sources that differ only in whitespace or comments compile to the
    /// same module and hash the same, which a source hash cannot say; and the
    /// digest is of the bytes that will be loaded, so it survives being
    /// written to a file and read back. It has no path in it and no toolchain
    /// in it, which is what makes it reproducible where PRD 02.36's recorded
    /// `bytecode_hash` defect is not.
    ///
    /// A source the compiler refuses has no artifact and no hash, and says so
    /// with the compiler's own diagnostic rather than falling back to hashing
    /// the text -- a digest of a program that cannot be built is a fingerprint
    /// of nothing.
    fn artifact_hash(&self, source: &str) -> Option<Result<(String, &'static str), String>> {
        Some(
            agenterm_qjswasm::compile_qjs(source)
                .map(|wasm| (agenterm_script_common::hex::sha256_hex(&wasm), "wasm"))
                .map_err(|e| e.to_string()),
        )
    }

    fn corpus_scan(
        &self,
        dir: &std::path::Path,
    ) -> Option<Result<agenterm_script_common::corpus_scan::CorpusScanReport, String>> {
        // Rooted at the corpus directory, so `import "lib/x"` resolves the
        // way `run` resolves it for an entry in that directory.
        let resolve = qjs_module_resolver(&[dir.to_path_buf()]);
        Some(agenterm_qjswasm::corpus_scan::scan_directory_with(
            dir, &resolve,
        ))
    }

    /// A top-level `return`, whose value is the script's ECMA-262 completion
    /// value and reaches the caller as `ScriptInvocationResult::value` with no
    /// marker in between. Measured: `return 1 + 2;` through `script run`
    /// answers `3`.
    ///
    /// Parenthesised so an object literal works: `return ({a: 1});` is an
    /// expression where `return {a: 1};` would be read as a block.
    fn eval_entry_source(&self, expression: &str) -> Option<String> {
        Some(format!("return ({expression});"))
    }

    fn check(
        &self,
        source: &str,
        _options: &ScriptInvocationOptions,
    ) -> Result<(), ScriptEngineError> {
        // Same source and tool-door contract as `execute`: `source` is text.
        // Native is intentionally absent here. A native declaration is an
        // unsafe assertion `dlsym` cannot verify, and check has no supervised
        // worker process to contain a crash or hang.
        // The same resolver `execute` uses. A `check` that could not follow an
        // `import` would refuse working scripts, which is the failure the door
        // declaration comment above this impl already records for host names.
        let resolve = qjs_module_resolver(&qjs_roots(_options));
        let wasm = compile_qjs_for(_options, source, &resolve, false).map_err(qjs_compile_error)?;
        // The validator has to know the door too, or `check` refuses bytes
        // `execute` would run: a tool script's `tool.*` imports are exactly
        // what a sandbox validator exists to reject.
        let budget = qjs_budget(_options);
        if _options.tool_door {
            agenterm_qjswasm::validate_wasm_tool_with(&wasm, &budget)
        } else {
            agenterm_qjswasm::validate_wasm_with(&wasm, &budget)
        }
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn execute(
        &self,
        source: &str,
        options: &ScriptInvocationOptions,
        fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Result<ScriptInvocationResult, ScriptEngineError> {
        // `ScriptFleetBridgeFn` and `agenterm_qjswasm::FleetBridgeFn` are the
        // same `Arc<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>`
        // shape, so this is a rebind, not a wrapper.
        let bridges = qjs_host_bridges(fleet_bridge);

        // `"main"` with no arguments is still the entry convention after the
        // `049ebba` bump, re-checked rather than assumed: the compiler exports
        // exactly one function, still named `main`, and its parameters are the
        // script's `$N`. A script that names none — which is every script this
        // path can run, since there is no route for passing arguments in from
        // here — compiles to a zero-parameter `main`. What did change is the
        // *result*: a `.qjs` entry returns one JavaScript value rather than an
        // `i32`, which is why the value below is no longer discarded.
        // `source` is the script's TEXT, not a path -- `ScriptInvocation`
        // carries the filename separately as `source_label`, purely for
        // diagnostics. An earlier version of this backend read it as a path and
        // so could never run anything: the failure was
        // `File name too long (os error 63)` with the whole program in the
        // message. Nothing caught it, because the crate's own tests drive
        // `Engine` directly and never come through this trait.
        //
        // A `.wasm` guest therefore cannot arrive here at all -- bytes do not
        // survive a `&str` -- and it is refused rather than guessed at. That
        // delivery path is the library API (`Guest::Wasm`), not this one.
        // Compiled here rather than inside `run_once`, because a script may
        // `import` and the resolver is this product's policy rather than the
        // engine's. A script with no `import` never calls it and compiles to
        // the same bytes either way.
        let resolve = qjs_module_resolver(&qjs_roots(options));
        let wasm = compile_qjs_for(options, source, &resolve, true).map_err(qjs_compile_error)?;
        // Built after both ordinary door selection and the supervised native
        // containment boundary are known.
        let mut engine = qjs_execution_engine(options);
        // The CLI's `-- ARGS...` arrive as `options.arguments` (a JSON array
        // of strings) and become the script's `$0`, `$1`, ... -- the only way
        // a `.qjs` task entry can be told what to act on. `main`'s arity is
        // fixed at compile time from the highest `$N` the script mentions, so
        // a count mismatch is refused by the engine as a `Signature` error
        // rather than padded with `undefined`: a script that reads `$2` and
        // was given two arguments has a bug, and hiding it was the shape
        // `rh::fail("expected: REPO PROFILE OUTPUT_PATH")` existed to prevent.
        // Through the tool door, not the engine face: the face cannot carry a
        // string into a guest (no door onto its allocator), so a task entry
        // reads `arg_count()` / `arg(n)` instead of `$N`. A sandbox script
        // has no arguments to read and no door to read them through, which
        // is the right answer for a script that is not a task entry.
        engine.set_tool_args(qjs_arguments(options.arguments.as_ref()));
        let outcome = engine
            .run_once_with_bridges(
                agenterm_qjswasm::Guest::CompiledQjs(&wasm),
                bridges,
                "main",
                &[],
            )
            .map_err(|e| {
                let mut error = qjs_engine_error(e);
                error.stdout = engine.take_failed_stdout();
                error.cost = engine.take_failed_cost().map(script_cost);
                error
            })?;

        Ok(ScriptInvocationResult {
            stdout: outcome.stdout,
            value: outcome.values.first().and_then(qjswasm_value_as_json),
            cost: Some(ScriptCost {
                steps: outcome.steps,
                peak_call_depth: outcome.peak_call_depth,
                peak_activation_slots: outcome.peak_activation_slots,
                host_ops: outcome.host_ops,
                host_bytes: outcome.host_bytes,
                waited_ms: outcome.waited_ms,
                heap_pages: outcome.heap_pages,
                heap_bytes: outcome.heap_bytes,
                heap_start_bytes: outcome.heap_start_bytes,
                json_parse_bytes: outcome.json_parse_bytes,
                json_stringify_bytes: outcome.json_stringify_bytes,
                immediate_stringify_host_argument_bytes: outcome
                    .immediate_stringify_host_argument_bytes,
            }),
        })
    }
}

#[cfg(feature = "script-qjswasm")]
fn qjs_host_bridges(fleet: Option<ScriptFleetBridgeFn>) -> agenterm_qjswasm::HostBridges {
    #[cfg(feature = "script-acu-embedder")]
    let acu: agenterm_qjswasm::AcuBridgeFn = Arc::new(|request_json, cancel, acknowledged| {
        let probe = || cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire));
        let reply = agenterm_cu::embedder::execute_request_from_environment_controlled(
            request_json,
            agenterm_cu::execution_control::ExecutionControl::with_cancel_probe(&probe),
        );
        if reply.error.as_ref().is_some_and(|error| {
            error.code == "cancelled"
                && error.detail.as_ref().is_some_and(|detail| {
                    detail.get("effect").and_then(serde_json::Value::as_str)
                        == Some("not_performed")
                })
        }) {
            acknowledged.store(true, std::sync::atomic::Ordering::Release);
        }
        serde_json::to_string(&reply).map_err(|error| format!("serializing ACU reply: {error}"))
    });
    #[cfg(not(feature = "script-acu-embedder"))]
    let acu = crate::acu_provider::bridge();
    agenterm_qjswasm::HostBridges {
        fleet,
        acu: Some(acu),
    }
}

/// Project one engine value into the JSON shape every backend reports through.
///
/// `None` means "this value has no JSON counterpart", which is a real answer
/// and not a failure: `undefined` is absent by definition, and JSON numbers
/// exclude `NaN` and the infinities. Inventing `null` for those would make a
/// script that returned nothing indistinguishable from one that returned
/// `null`, and it is the `.qjs` subset that just gained the ability to tell
/// those apart.
#[cfg(feature = "script-qjswasm")]
fn qjswasm_value_as_json(value: &agenterm_qjswasm::Value) -> Option<Value> {
    use agenterm_qjswasm::{JsValue, Value as EngineValue};
    match value {
        EngineValue::I32(v) => Some(Value::from(*v)),
        EngineValue::I64(v) => Some(Value::from(*v)),
        EngineValue::F32(v) => number_as_json(f64::from(*v)),
        EngineValue::F64(v) => number_as_json(*v),
        EngineValue::Js(JsValue::Null) => Some(Value::Null),
        EngineValue::Js(JsValue::Bool(b)) => Some(Value::Bool(*b)),
        EngineValue::Js(JsValue::Number(x)) => number_as_json(*x),
        EngineValue::Js(JsValue::Str(text)) => Some(Value::String(text.clone())),
        EngineValue::Js(JsValue::Undefined) => None,
        // `JsValue` is `#[non_exhaustive]` because the language is still
        // growing. A kind that did not exist when this was written is reported
        // as "no JSON counterpart" rather than guessed at; the commit that adds
        // it upstream is the one that decides what it looks like here.
        _ => None,
    }
}

/// One binary64 as JSON, integral where it can be.
///
/// A JavaScript Number is always a double, so `42` arrives here as `42.0`.
/// Emitting it as a JSON float would make every whole number read back as one,
/// and a consumer asking for an integer would get nothing. ECMA-262's own
/// `JSON.stringify` writes `42` for that value, so matching it is the faithful
/// answer rather than a convenience. `NaN` and the infinities have no JSON
/// spelling at all and are reported absent.
#[cfg(feature = "script-qjswasm")]
fn number_as_json(x: f64) -> Option<Value> {
    if x.is_finite() && x.fract() == 0.0 && x >= i64::MIN as f64 && x <= i64::MAX as f64 {
        return Some(Value::from(x as i64));
    }
    serde_json::Number::from_f64(x).map(Value::Number)
}

// ---------------------------------------------------------------------
// §2.4 — enum static dispatch
// ---------------------------------------------------------------------

/// Static-dispatch registry over the compiled-in engines. Not a `dyn` trait
/// object list — see design §2.4 for why enum+match is preferred as the
/// default over `Box<dyn ScriptEngineBackend>` (the trait remains
/// object-safe as a documented, unused escape hatch).
///
/// Every variant is feature-gated. With no engine feature on this enum is
/// empty, `all()` is empty, and `for_backend` can never be called because
/// `ScriptBackend` is empty too -- which is the truthful shape of a build
/// with no script engine, not a defect to paper over with a placeholder.
pub enum ScriptEngine {
    #[cfg(feature = "script-lua")]
    Lua(LuaEngineBackend),
    #[cfg(feature = "script-sql")]
    Sql(SqlEngineBackend),
    #[cfg(feature = "script-qjswasm")]
    Qjswasm(QjswasmEngineBackend),
}

impl ScriptEngine {
    pub fn all() -> Vec<ScriptEngine> {
        vec![
            #[cfg(feature = "script-lua")]
            Self::Lua(LuaEngineBackend),
            #[cfg(feature = "script-sql")]
            Self::Sql(SqlEngineBackend),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(QjswasmEngineBackend),
        ]
    }

    /// Construct the engine variant corresponding to `id`.
    pub fn for_backend(id: ScriptBackend) -> Self {
        match id {
            #[cfg(feature = "script-lua")]
            ScriptBackend::Lua => Self::Lua(LuaEngineBackend),
            #[cfg(feature = "script-sql")]
            ScriptBackend::Sql => Self::Sql(SqlEngineBackend),
            #[cfg(feature = "script-qjswasm")]
            ScriptBackend::Qjswasm => Self::Qjswasm(QjswasmEngineBackend),
        }
    }
}

/// Free-function alias for `ScriptEngine::for_backend`, matching the task
/// brief's literal `engine_for(backend)` naming.
pub fn engine_for(backend: ScriptBackend) -> ScriptEngine {
    ScriptEngine::for_backend(backend)
}

#[cfg_attr(
    not(any(
        feature = "script-lua",
        feature = "script-sql",
        feature = "script-qjswasm"
    )),
    allow(unused_variables)
)]
impl ScriptEngineBackend for ScriptEngine {
    fn backend_id(&self) -> ScriptBackend {
        match self {
            // With no engine compiled in the enum is empty, `self` is
            // uninhabited, and this arm is the proof: it can only be reached
            // by a value that cannot exist.
            #[cfg(not(any(
                feature = "script-lua",
                feature = "script-sql",
                feature = "script-qjswasm"
            )))]
            _ => match *self {},
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.backend_id(),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.backend_id(),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.backend_id(),
        }
    }

    fn entry_extensions(&self) -> &'static [&'static str] {
        match self {
            // With no engine compiled in the enum is empty, `self` is
            // uninhabited, and this arm is the proof: it can only be reached
            // by a value that cannot exist.
            #[cfg(not(any(
                feature = "script-lua",
                feature = "script-sql",
                feature = "script-qjswasm"
            )))]
            _ => match *self {},
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.entry_extensions(),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.entry_extensions(),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.entry_extensions(),
        }
    }

    fn eval_entry_source(&self, expression: &str) -> Option<String> {
        match self {
            // With no engine compiled in the enum is empty, `self` is
            // uninhabited, and this arm is the proof: it can only be reached
            // by a value that cannot exist.
            #[cfg(not(any(
                feature = "script-lua",
                feature = "script-sql",
                feature = "script-qjswasm"
            )))]
            _ => match *self {},
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.eval_entry_source(expression),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.eval_entry_source(expression),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.eval_entry_source(expression),
        }
    }

    fn identity(&self) -> String {
        match self {
            // With no engine compiled in the enum is empty, `self` is
            // uninhabited, and this arm is the proof: it can only be reached
            // by a value that cannot exist.
            #[cfg(not(any(
                feature = "script-lua",
                feature = "script-sql",
                feature = "script-qjswasm"
            )))]
            _ => match *self {},
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.identity(),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.identity(),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.identity(),
        }
    }

    fn corpus_scan(
        &self,
        dir: &std::path::Path,
    ) -> Option<Result<agenterm_script_common::corpus_scan::CorpusScanReport, String>> {
        match self {
            // With no engine compiled in the enum is empty, `self` is
            // uninhabited, and this arm is the proof: it can only be reached
            // by a value that cannot exist.
            #[cfg(not(any(
                feature = "script-lua",
                feature = "script-sql",
                feature = "script-qjswasm"
            )))]
            _ => match *self {},
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.corpus_scan(dir),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.corpus_scan(dir),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.corpus_scan(dir),
        }
    }

    fn artifact_hash(&self, source: &str) -> Option<Result<(String, &'static str), String>> {
        match self {
            // With no engine compiled in the enum is empty, `self` is
            // uninhabited, and this arm is the proof: it can only be reached
            // by a value that cannot exist.
            #[cfg(not(any(
                feature = "script-lua",
                feature = "script-sql",
                feature = "script-qjswasm"
            )))]
            _ => match *self {},
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.artifact_hash(source),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.artifact_hash(source),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.artifact_hash(source),
        }
    }

    fn source_is_a_path(&self) -> bool {
        match self {
            // With no engine compiled in the enum is empty, `self` is
            // uninhabited, and this arm is the proof: it can only be reached
            // by a value that cannot exist.
            #[cfg(not(any(
                feature = "script-lua",
                feature = "script-sql",
                feature = "script-qjswasm"
            )))]
            _ => match *self {},
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.source_is_a_path(),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.source_is_a_path(),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.source_is_a_path(),
        }
    }

    fn execute_artifact(
        &self,
        artifact: &[u8],
        options: &ScriptInvocationOptions,
        fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Option<Result<ScriptInvocationResult, ScriptEngineError>> {
        match self {
            // With no engine compiled in the enum is empty, `self` is
            // uninhabited, and this arm is the proof: it can only be reached
            // by a value that cannot exist.
            #[cfg(not(any(
                feature = "script-lua",
                feature = "script-sql",
                feature = "script-qjswasm"
            )))]
            _ => match *self {},
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.execute_artifact(artifact, options, fleet_bridge),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.execute_artifact(artifact, options, fleet_bridge),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.execute_artifact(artifact, options, fleet_bridge),
        }
    }

    fn pack_artifact(&self, source: &str) -> Option<Result<(Vec<u8>, &'static str), String>> {
        match self {
            // With no engine compiled in the enum is empty, `self` is
            // uninhabited, and this arm is the proof: it can only be reached
            // by a value that cannot exist.
            #[cfg(not(any(
                feature = "script-lua",
                feature = "script-sql",
                feature = "script-qjswasm"
            )))]
            _ => match *self {},
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.pack_artifact(source),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.pack_artifact(source),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.pack_artifact(source),
        }
    }

    fn check(
        &self,
        source: &str,
        options: &ScriptInvocationOptions,
    ) -> Result<(), ScriptEngineError> {
        match self {
            // With no engine compiled in the enum is empty, `self` is
            // uninhabited, and this arm is the proof: it can only be reached
            // by a value that cannot exist.
            #[cfg(not(any(
                feature = "script-lua",
                feature = "script-sql",
                feature = "script-qjswasm"
            )))]
            _ => match *self {},
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.check(source, options),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.check(source, options),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.check(source, options),
        }
    }

    fn execute(
        &self,
        source: &str,
        options: &ScriptInvocationOptions,
        fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Result<ScriptInvocationResult, ScriptEngineError> {
        match self {
            // With no engine compiled in the enum is empty, `self` is
            // uninhabited, and this arm is the proof: it can only be reached
            // by a value that cannot exist.
            #[cfg(not(any(
                feature = "script-lua",
                feature = "script-sql",
                feature = "script-qjswasm"
            )))]
            _ => match *self {},
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.execute(source, options, fleet_bridge),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.execute(source, options, fleet_bridge),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.execute(source, options, fleet_bridge),
        }
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // One process, one variable, one lock: the writers here and in
    // script_backend.rs, and the readers in script_worker.rs, all hold
    // `script_backend::ENV_LOCK`. Two module-local mutexes used to leave a
    // window in which a label resolved by extension saw another test's env.
    use crate::script_backend::ENV_LOCK;

    #[test]
    #[cfg(feature = "script-qjswasm")]
    fn qjs_budget_maps_the_public_string_ceiling_to_bridge_results() {
        let defaults = agenterm_qjswasm::Budget::default();
        let without_override = qjs_budget(&ScriptInvocationOptions::default());
        assert_eq!(
            without_override.max_bridge_result_bytes,
            defaults.max_bridge_result_bytes
        );

        let invocation_budget = ScriptBudgets {
            string_bytes: 3 * 1024 * 1024,
            ..ScriptBudgets::default()
        };
        let options = ScriptInvocationOptions {
            budgets: Some(invocation_budget),
            ..ScriptInvocationOptions::default()
        };
        assert_eq!(
            qjs_budget(&options).max_bridge_result_bytes,
            3 * 1024 * 1024
        );
    }

    #[test]
    #[cfg(feature = "script-qjswasm")]
    fn qjs_compiled_artifact_keeps_the_invocation_cancellation_identity() {
        use std::sync::atomic::AtomicBool;

        let source = "let i = 0; while (true) { i = i + 1; } return i;";
        let (artifact, extension) = QjswasmEngineBackend
            .pack_artifact(source)
            .expect("qjswasm owns a compiled artifact")
            .expect("compile infinite-loop fixture");
        assert_eq!(extension, "wasm");

        let options = ScriptInvocationOptions {
            cancellation: Some(Arc::new(AtomicBool::new(true))),
            budgets: Some(ScriptBudgets {
                operations: u64::MAX,
                ..ScriptBudgets::default()
            }),
            ..ScriptInvocationOptions::default()
        };
        let error = QjswasmEngineBackend
            .execute_artifact(&artifact, &options, None)
            .expect("qjswasm executes its compiled artifact")
            .expect_err("the compiled route must observe the same cancel token as source");
        assert_eq!(error.category, ScriptFailureCategory::Cancelled);
    }

    // `gate_two_trait_equivalence` lived here: four assertions that the
    // rquickjs engine and this one agreed on stdout and on values, refused the
    // same sources, and differed only where their subsets did. It was archive
    // gate 1's evidence, it came back **six agreements and zero divergences**,
    // and it went when the engine it compared against did. The finding is
    // recorded in `prd/PRD_02_36_agenterm_qjswasm.md`; there is nothing left
    // to compare it with.

    struct EnvGuard {
        prior: Option<String>,
    }

    impl EnvGuard {
        #[cfg_attr(
            not(any(
                feature = "script-lua",
                feature = "script-sql",
                feature = "script-qjswasm"
            )),
            allow(dead_code)
        )]
        fn set(value: &str) -> Self {
            let prior = std::env::var("AGENTERM_SCRIPT_BACKEND").ok();
            unsafe {
                std::env::set_var("AGENTERM_SCRIPT_BACKEND", value);
            }
            Self { prior }
        }

        fn clear() -> Self {
            let prior = std::env::var("AGENTERM_SCRIPT_BACKEND").ok();
            unsafe {
                std::env::remove_var("AGENTERM_SCRIPT_BACKEND");
            }
            Self { prior }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prior {
                Some(value) => unsafe {
                    std::env::set_var("AGENTERM_SCRIPT_BACKEND", value);
                },
                None => unsafe {
                    std::env::remove_var("AGENTERM_SCRIPT_BACKEND");
                },
            }
        }
    }

    /// With nothing in the environment nothing is enabled: there is no default
    /// engine any more, and `enabled()` must not invent one.
    #[test]
    fn no_engine_is_enabled_with_no_env_set() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::clear();
        for engine in ScriptEngine::all() {
            assert!(
                !engine.enabled(),
                "{:?} enabled by default",
                engine.backend_id()
            );
        }
    }

    #[test]
    fn script_invocation_options_default_field_shape() {
        let options = ScriptInvocationOptions::default();
        assert!(options.project_root.is_none());
        assert!(options.arguments.is_none());
        assert!(options.budgets.is_none());
    }

    // ---- rh ----

    // ---- lua ----

    #[cfg(feature = "script-lua")]
    const LUA_VALID_SOURCE: &str = "return 42";
    #[cfg(feature = "script-lua")]
    const LUA_BROKEN_SOURCE: &str = "return !!";

    #[test]
    #[cfg(feature = "script-lua")]
    fn lua_engine_enabled_matches_env() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::set("lua");
        let engine = LuaEngineBackend;
        assert_eq!(engine.backend_id(), ScriptBackend::Lua);
        assert!(engine.enabled());

        let _env = EnvGuard::set("qjs");
        assert!(!LuaEngineBackend.enabled());
    }

    #[test]
    #[cfg(feature = "script-lua")]
    fn lua_engine_check_valid_and_broken_source() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::set("lua");
        let engine = LuaEngineBackend;
        let options = ScriptInvocationOptions::default();

        engine
            .check(LUA_VALID_SOURCE, &options)
            .expect("valid lua source should check clean");
        assert!(
            engine.check(LUA_BROKEN_SOURCE, &options).is_err(),
            "broken lua source should fail check"
        );
    }

    #[test]
    #[cfg(feature = "script-lua")]
    fn lua_engine_execute_returns_evaluated_value() {
        // Trait-M4: was an equivalence test against try_execute_lua_invocation
        // (now folded/deleted); asserts the same expected shape (stdout
        // empty, value == 42 widened to serde_json::Value) directly.
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::set("lua");
        let engine = LuaEngineBackend;
        let options = ScriptInvocationOptions::default();

        let result = engine
            .execute(LUA_VALID_SOURCE, &options, None)
            .expect("trait execute should succeed");

        assert_eq!(result.stdout, "");
        assert_eq!(result.value, Some(Value::from(42i64)));
    }

    /// An engine reached directly runs what it is handed; selection is not
    /// its job.
    ///
    /// This test was `lua_engine_execute_errors_when_not_enabled` and asserted
    /// the opposite: that `execute` with no backend in the environment returns
    /// `Err`. That gate -- `if !self.enabled()` at the top of every engine's
    /// `check`/`execute` -- was removed on 2026-08-28 when `ScriptBackend::
    /// resolve` became the one place an engine is chosen, because a second
    /// copy of that decision is a second place for it to be wrong (it was:
    /// routed to qjswasm by the dispatcher, refused by qjswasm's own gate).
    /// The test kept asserting the deleted behaviour, and stayed green-looking
    /// for a day because it is behind `script-lua` and the workspace run does
    /// not enable it.
    ///
    /// It also poisoned `ENV_LOCK` when it failed, taking eleven unrelated
    /// tests down as `PoisonError` -- the exact cascade PRD 36's verification
    /// section warns about. A test that panics inside a shared lock is two
    /// failures wearing one name.
    #[test]
    #[cfg(feature = "script-lua")]
    fn lua_engine_runs_when_called_directly_regardless_of_env() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::clear();
        let engine = LuaEngineBackend;
        let options = ScriptInvocationOptions::default();

        assert!(
            engine.execute(LUA_VALID_SOURCE, &options, None).is_ok(),
            "selection happens in `resolve`, not in the engine; a direct call runs"
        );
    }

    #[test]
    #[cfg(feature = "script-lua")]
    fn lua_engine_entry_extensions_match_from_entry_path() {
        for ext in LuaEngineBackend.entry_extensions() {
            let path = format!("script.{ext}");
            assert_eq!(
                ScriptBackend::from_entry_path(&path),
                Some(ScriptBackend::Lua),
                "extension {ext} should route to lua"
            );
        }
    }

    // ---- qjs ----
    //
    // `QJS_VALID_SOURCE` and `QJS_BROKEN_SOURCE` lived here and went with the
    // five adapter tests below. The two sources they held now appear inline in
    // `gate_two_trait_equivalence::both_backends_agree_on_check`, where they
    // are compared against qjswasm's answers rather than asserted alone.

    #[cfg(feature = "script-qjswasm")]
    #[test]
    fn qjswasm_native_door_tracks_the_supervised_worker_fact_and_keeps_tool_selection() {
        let sandbox = qjs_execution_engine(&ScriptInvocationOptions::default());
        assert!(!sandbox.has_native_door());
        assert!(!sandbox.has_tool_door());

        let tool = qjs_execution_engine(&ScriptInvocationOptions {
            tool_door: true,
            native_door_contained: true,
            ..ScriptInvocationOptions::default()
        });
        assert!(tool.has_native_door());
        assert!(tool.has_tool_door());
    }

    #[cfg(feature = "script-qjswasm")]
    #[test]
    fn direct_backend_artifact_execution_refuses_the_native_import() {
        let wasm = wat::parse_str(include_str!(
            "../crates/agenterm-qjswasm/tests/fixtures/native/getpid.wat"
        ))
        .expect("native fixture is valid WAT");
        let error = QjswasmEngineBackend
            .execute_artifact(&wasm, &ScriptInvocationOptions::default(), None)
            .expect("qjswasm owns wasm artifacts")
            .expect_err("an in-process backend call has no native containment");
        assert!(error.message.contains("agenterm.native_call"), "{error:?}");
        assert!(error.message.contains("with_native_door"), "{error:?}");
    }

    /// A `.qjs` script's completion value reaches the caller.
    ///
    /// Before the `049ebba` bump this backend could not have reported one: the
    /// compiled entry returned an `i32` and `.qjs` had no way to reach
    /// `agenterm.print`, so a script's entire observable output was nothing at
    /// all. The value is the whole result of running a `.qjs` script through
    /// this path, which is why it is worth a test rather than a `None`.
    #[cfg(feature = "script-qjswasm")]
    #[test]
    fn qjswasm_reports_a_qjs_scripts_completion_value() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::set("qjswasm");
        let engine = QjswasmEngineBackend;
        let options = ScriptInvocationOptions::default();

        for (source, want) in [
            ("return \"ok\";", Some(Value::String("ok".into()))),
            ("let x = 20; return x * 2 + 2;", Some(Value::from(42))),
            ("return true;", Some(Value::Bool(true))),
            ("return null;", Some(Value::Null)),
            // `undefined` has no JSON counterpart, so absent is the answer --
            // not `null`, which the subset can now return in its own right.
            ("return undefined;", None),
        ] {
            // `source` is the script's TEXT. This test used to write a file
            // and pass its path, which stopped working when the backend was
            // corrected to match `ScriptInvocation`'s contract -- the path
            // string was compiled as a program and failed on its leading `/`.
            engine.check(source, &options).expect("checks clean");
            let result = engine.execute(source, &options, None).expect("runs");
            assert_eq!(result.value, want, "{source:?}");
            assert!(result.stdout.is_empty(), "{source:?}");
        }
    }

    #[cfg(feature = "script-acu-embedder")]
    #[test]
    fn agenterm_acu_module_is_built_in_and_preserves_error_replies_as_data() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _backend = EnvGuard::set("qjswasm");
        let prior_grant = std::env::var("AGENTERM_CU_GRANT").ok();
        unsafe { std::env::remove_var("AGENTERM_CU_GRANT") };
        let source = r#"
import * as acu from "agenterm:acu";
const reply = acu.call({verb:"capabilities", target:"current"});
return "ok=" + reply.ok + ";code=" + reply.error.code;
"#;
        let result = QjswasmEngineBackend
            .execute(source, &ScriptInvocationOptions::default(), None)
            .expect("ok:false is returned to qjs as ordinary CuReply data");
        match prior_grant {
            Some(value) => unsafe { std::env::set_var("AGENTERM_CU_GRANT", value) },
            None => unsafe { std::env::remove_var("AGENTERM_CU_GRANT") },
        }
        let text = result
            .value
            .and_then(|value| value.as_str().map(str::to_owned));
        assert!(text.is_some_and(|text| text.starts_with("ok=false;code=")));
    }

    #[cfg(feature = "script-acu-embedder")]
    #[test]
    fn agenterm_acu_argv_uses_the_same_library_parser_without_a_child_process() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _backend = EnvGuard::set("qjswasm");
        let source = r#"
import * as acu from "agenterm:acu";
const reply = acu.argv(["--target", "current", "--grant", "observe", "capabilities"]);
return reply.ok + ":" + reply.command;
"#;
        let result = QjswasmEngineBackend
            .execute(source, &ScriptInvocationOptions::default(), None)
            .expect("argv request runs through the in-process adapter");
        assert_eq!(result.value, Some(serde_json::json!("true:capabilities")));
    }

    #[cfg(feature = "script-qjswasm")]
    #[test]
    fn filesystem_cannot_shadow_agenterm_acu_module() {
        let root = tempfile::tempdir().expect("tempdir");
        #[cfg(not(windows))]
        std::fs::write(
            root.path().join("agenterm:acu.qjs"),
            "this is not valid qjs",
        )
        .expect("write shadow candidate");
        let resolve = qjs_module_resolver(&[root.path().to_path_buf()]);
        let built_in = resolve("agenterm:acu").expect("built-in");
        assert!(built_in.contains("acu_call"));
        assert!(built_in.contains("export function argv"));
        assert!(built_in.contains("acu_request: 1"));
        assert!(!built_in.contains("not valid"));
    }

    #[cfg(feature = "script-qjswasm")]
    #[test]
    fn embedded_acu_entry_and_all_reserved_modules_compile_from_one_registry() {
        for specifier in [
            "agenterm:acu",
            "agenterm:acu/argv",
            "agenterm:acu/legacy-args",
            "agenterm:acu/rewrite",
            "agenterm:acu/compat",
            "agenterm:acu/compound",
        ] {
            assert!(
                qjs_builtin_module_source(specifier).is_some(),
                "missing reserved module {specifier}"
            );
        }
        assert!(qjs_builtin_module_source("agenterm:acu/unknown").is_none());
        let options = ScriptInvocationOptions {
            tool_door: true,
            ..ScriptInvocationOptions::default()
        };
        QjswasmEngineBackend
            .check(AGENTERM_ACU_ENTRY_SOURCE, &options)
            .expect("embedded ACU entry and its reserved imports compile together");
    }

    /// The retirement contract is about the compiled production closure, not
    /// spelling in source files.  A source grep can be bypassed by aliases or
    /// formatting and used to miss the inline `agenterm:acu` module entirely.
    /// Decode the artifact emitted by the real product resolver and keep its
    /// host door exact: argv/result/print plus the in-process ACU bridge, and
    /// no `tool.process_*` import that could shell out to a second runtime.
    #[cfg(feature = "script-acu-embedder")]
    #[test]
    fn embedded_acu_compiled_closure_has_only_the_exact_host_imports() {
        let options = ScriptInvocationOptions {
            tool_door: true,
            ..ScriptInvocationOptions::default()
        };
        let resolve = qjs_module_resolver(&[]);
        let wasm = compile_qjs_for(&options, AGENTERM_ACU_ENTRY_SOURCE, &resolve, false)
            .expect("production ACU closure compiles");
        assert_eq!(
            wasm_function_import_names(&wasm),
            [
                "agenterm.print",
                "agenterm.acu_call",
                "agenterm.acu_result_len",
                "agenterm.acu_result",
                "tool.arg_count",
                "tool.arg",
                "tool.result_len",
                "tool.result",
            ]
        );
    }

    /// Causal composite evidence for CU-JW1: the production controlled
    /// executor completes one resident-owner IPC round before its second
    /// cancellation probe sets the same flag consumed by qjswasm.  This is a
    /// production controlled executor + production ack predicate integration,
    /// not a byte-for-byte wrapper around `qjs_host_bridges(None).acu`.
    #[cfg(feature = "script-acu-embedder")]
    mod jw1_recording_bridge {
        use super::*;
        use std::process::{Command, Stdio};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        const CHILD_ENV: &str = "AGENTERM_JW1_RECORDING_CHILD";
        const ROOT_ENV: &str = "AGENTERM_JW1_RECORDING_ROOT";
        const CU_ENV: &str = "AGENTERM_JW1_AGENTERM_CU_EXE";
        const TEST_NAME: &str =
            "script_engine::tests::jw1_recording_bridge::jw1_recording_bridge_causal_composite";
        const PAYLOAD_NAME: &str =
            "script_engine::tests::jw1_recording_bridge::jw1_managed_job_payload";

        fn invoke_cli(executable: &std::path::Path, argv: &[String]) -> agenterm_cu::CuReply {
            let output = Command::new(executable)
                .args(argv)
                .stdin(Stdio::null())
                .output()
                .expect("run public agenterm-cu argv");
            assert!(
                output.status.success(),
                "agenterm-cu failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            serde_json::from_slice(&output.stdout).expect("public argv emits one CuReply")
        }

        fn data(reply: agenterm_cu::CuReply) -> serde_json::Value {
            assert!(reply.ok, "unexpected CU reply: {reply:?}");
            reply.data.expect("successful CU reply has data")
        }

        fn run_engine(
            argv: &[String],
            cancel: Arc<AtomicBool>,
            bridge: agenterm_qjswasm::AcuBridgeFn,
        ) -> agenterm_qjswasm::QjswasmError {
            let source = format!(
                "import * as acu from \"agenterm:acu\"; return acu.argv({});",
                serde_json::to_string(argv).expect("encode argv")
            );
            let options = ScriptInvocationOptions {
                tool_door: true,
                cancellation: Some(Arc::clone(&cancel)),
                ..ScriptInvocationOptions::default()
            };
            let wasm = compile_qjs_for(&options, &source, &qjs_module_resolver(&[]), false)
                .expect("compile recording fixture");
            let mut budget = qjs_budget(&options);
            budget.cancel = Some(cancel);
            let mut engine = agenterm_qjswasm::Engine::with_tool_door(budget);
            engine
                .run_once_with_bridges(
                    agenterm_qjswasm::Guest::CompiledQjs(&wasm),
                    agenterm_qjswasm::HostBridges {
                        fleet: None,
                        acu: Some(bridge),
                    },
                    "main",
                    &[],
                )
                .expect_err("the shared cancellation flag ends the script")
        }

        fn run_child(root: &std::path::Path, executable: &std::path::Path) -> serde_json::Value {
            std::fs::create_dir_all(root).expect("create isolated CU root");
            unsafe {
                std::env::set_var("AGENTERM_CU_AUDIT_PATH", root.join("audit.jsonl"));
                std::env::set_var("AGENTERM_CU_RUNTIME_PATH", root.join("runtime.json"));
                std::env::set_var(
                    "AGENTERM_CU_IDEMPOTENCY_PATH",
                    root.join("idempotency.json"),
                );
                std::env::set_var(
                    "AGENTERM_CU_MANAGED_JOB_PATH",
                    root.join("managed-jobs.json"),
                );
            }

            let session = data(invoke_cli(
                executable,
                &[
                    "--target",
                    "current",
                    "--grant",
                    "actuate",
                    "session-start",
                    "--label",
                    "jw1-recording-bridge",
                    "--ttl-seconds",
                    "120",
                ]
                .map(str::to_owned),
            ));
            let session_id = session["session_id"]
                .as_str()
                .expect("session id")
                .to_owned();
            let lease = session["lease"].as_str().expect("session lease").to_owned();
            let request_id = format!("jw1{:028x}", std::process::id());
            let current = std::env::current_exe().expect("test executable");
            let spawn = data(invoke_cli(
                executable,
                &vec![
                    "--target".into(),
                    "current".into(),
                    "--grant".into(),
                    "actuate".into(),
                    "--request-id".into(),
                    request_id,
                    "--session".into(),
                    session_id.clone(),
                    "--session-lease".into(),
                    lease.clone(),
                    "job-spawn".into(),
                    "--cwd".into(),
                    root.to_string_lossy().into_owned(),
                    "--ttl-seconds".into(),
                    "60".into(),
                    "--".into(),
                    current.to_string_lossy().into_owned(),
                    "--exact".into(),
                    PAYLOAD_NAME.into(),
                    "--ignored".into(),
                    "--nocapture".into(),
                ],
            ));
            let job_id = spawn["job_id"].as_str().expect("job id").to_owned();
            let generation = spawn["generation"].as_u64().expect("generation");
            let wait_argv = vec![
                "--target".into(),
                "current".into(),
                "--grant".into(),
                "observe".into(),
                "job-wait".into(),
                job_id.clone(),
                generation.to_string(),
                "--timeout-ms".into(),
                "10000".into(),
            ];

            let cancel = Arc::new(AtomicBool::new(false));
            let calls = Arc::new(AtomicUsize::new(0));
            let entered = Arc::new(AtomicBool::new(false));
            let ack_seen = Arc::new(AtomicBool::new(false));
            let returned = Arc::new(AtomicBool::new(false));
            let cancel_at = Arc::new(Mutex::new(None::<Instant>));
            let returned_at = Arc::new(Mutex::new(None::<Instant>));
            let bridge: agenterm_qjswasm::AcuBridgeFn = {
                let calls = Arc::clone(&calls);
                let entered = Arc::clone(&entered);
                let ack_seen = Arc::clone(&ack_seen);
                let returned = Arc::clone(&returned);
                let cancel_at = Arc::clone(&cancel_at);
                let returned_at = Arc::clone(&returned_at);
                Arc::new(move |request, signal, acknowledged| {
                    entered.store(true, Ordering::SeqCst);
                    let probe = || {
                        let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                        if call == 1 {
                            return false;
                        }
                        if call == 2 {
                            *cancel_at.lock().expect("cancel timestamp") = Some(Instant::now());
                            signal
                                .expect("Budget.cancel reaches CU")
                                .store(true, Ordering::Release);
                        }
                        true
                    };
                    let reply = agenterm_cu::embedder::execute_request_from_environment_controlled(
                        request,
                        agenterm_cu::execution_control::ExecutionControl::with_cancel_probe(&probe),
                    );
                    if reply.error.as_ref().is_some_and(|error| {
                        error.code == "cancelled"
                            && error
                                .detail
                                .as_ref()
                                .and_then(|detail| detail.get("effect"))
                                == Some(&serde_json::Value::String("not_performed".into()))
                    }) {
                        acknowledged.store(true, Ordering::Release);
                        ack_seen.store(true, Ordering::SeqCst);
                    }
                    *returned_at.lock().expect("return timestamp") = Some(Instant::now());
                    returned.store(true, Ordering::SeqCst);
                    serde_json::to_string(&reply).map_err(|error| error.to_string())
                })
            };
            let error = run_engine(&wait_argv, Arc::clone(&cancel), bridge);
            assert!(matches!(error, agenterm_qjswasm::QjswasmError::Cancelled));
            let tail = returned_at
                .lock()
                .expect("return timestamp")
                .expect("returned")
                .duration_since(
                    cancel_at
                        .lock()
                        .expect("cancel timestamp")
                        .expect("cancelled"),
                );

            let status = data(invoke_cli(
                executable,
                &[
                    "--target",
                    "current",
                    "--grant",
                    "observe",
                    "job-status",
                    &job_id,
                ]
                .map(str::to_owned),
            ));
            assert_eq!(status["state"], "running");
            assert!(status["owner_pid"].as_u64().is_some());

            let negative_cancel = Arc::new(AtomicBool::new(false));
            let negative_ack = Arc::new(AtomicBool::new(false));
            let negative_bridge: agenterm_qjswasm::AcuBridgeFn = {
                let negative_ack = Arc::clone(&negative_ack);
                Arc::new(move |_request, signal, acknowledged| {
                    signal
                        .expect("negative Budget.cancel reaches bridge")
                        .store(true, Ordering::Release);
                    negative_ack.store(acknowledged.load(Ordering::Acquire), Ordering::Release);
                    Ok(r#"{"ok":false,"target":"current","command":"job-wait","error":{"code":"fixture_noncooperative","message":"negative control"}}"#.into())
                })
            };
            let negative_error = run_engine(&wait_argv, negative_cancel, negative_bridge);
            assert!(matches!(
                negative_error,
                agenterm_qjswasm::QjswasmError::Cancelled
            ));

            let stop_id = format!("jw1stop{:024x}", std::process::id());
            let _ = invoke_cli(
                executable,
                &vec![
                    "--target".into(),
                    "current".into(),
                    "--grant".into(),
                    "actuate".into(),
                    "--request-id".into(),
                    stop_id,
                    "--session".into(),
                    session_id.clone(),
                    "--session-lease".into(),
                    lease.clone(),
                    "job-stop".into(),
                    job_id,
                    generation.to_string(),
                    "--grace-ms".into(),
                    "5000".into(),
                ],
            );
            let _ = invoke_cli(
                executable,
                &[
                    "--target",
                    "current",
                    "--grant",
                    "actuate",
                    "session-end",
                    &session_id,
                    "--lease",
                    &lease,
                    "--confirm",
                ]
                .map(str::to_owned),
            );

            serde_json::json!({
                "entered": entered.load(Ordering::SeqCst),
                "probe_calls": calls.load(Ordering::SeqCst),
                "rounds_completed_before_cancel": 1,
                "cancel_set": cancel.load(Ordering::Acquire),
                "ack_seen": ack_seen.load(Ordering::SeqCst),
                "returned": returned.load(Ordering::SeqCst),
                "tail_micros": tail.as_micros(),
                "job_status_available": true,
                "owner_and_job_not_cancelled": status["state"] == "running",
                "negative_ack_seen": negative_ack.load(Ordering::Acquire),
                "negative_engine_cancelled": true,
            })
        }

        #[test]
        fn jw1_recording_bridge_causal_composite() {
            if std::env::var_os(CHILD_ENV).is_some() {
                let root =
                    std::path::PathBuf::from(std::env::var_os(ROOT_ENV).expect("child root"));
                let executable =
                    std::path::PathBuf::from(std::env::var_os(CU_ENV).expect("CU exe"));
                let report = run_child(&root, &executable);
                std::fs::write(
                    root.join("report.json"),
                    serde_json::to_vec(&report).unwrap(),
                )
                .expect("write child report");
                return;
            }
            // CU durable stores reject link-like ancestors.  macOS' system
            // temporary directory is reached through `/var -> /private/var`,
            // so keep this invocation-owned root in Cargo's repo-local lane.
            let root = std::env::current_dir()
                .expect("repository root")
                .join("target/cu-jw1-item1")
                .join(format!("jw1-state-{}", std::process::id()));
            assert!(!root.exists(), "owned JW1 root must start absent");
            let current = std::env::current_exe().expect("test executable");
            let name = if cfg!(windows) {
                "agenterm-cu.exe"
            } else {
                "agenterm-cu"
            };
            let inferred = current
                .parent()
                .and_then(std::path::Path::parent)
                .expect("target profile directory")
                .join(name);
            let executable = std::env::var_os(CU_ENV)
                .map(std::path::PathBuf::from)
                .unwrap_or(inferred);
            assert!(
                executable.is_file(),
                "build agenterm-cu in this target lane first: {}",
                executable.display()
            );
            let mut child = Command::new(current)
                .args(["--exact", TEST_NAME, "--nocapture"])
                .env(CHILD_ENV, "1")
                .env(ROOT_ENV, &root)
                .env(CU_ENV, &executable)
                .spawn()
                .expect("re-exec isolated test child");
            let deadline = Instant::now() + Duration::from_secs(30);
            let status = loop {
                if let Some(status) = child.try_wait().expect("poll child") {
                    break status;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("JW1 recording child exceeded 30 seconds");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert!(status.success(), "isolated JW1 child failed");
            let report: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join("report.json")).expect("child report"),
            )
            .expect("parse child report");
            for key in [
                "entered",
                "cancel_set",
                "ack_seen",
                "returned",
                "job_status_available",
                "owner_and_job_not_cancelled",
                "negative_engine_cancelled",
            ] {
                assert_eq!(report[key], true, "{key}: {report}");
            }
            assert_eq!(report["rounds_completed_before_cancel"], 1);
            assert!(
                report["probe_calls"]
                    .as_u64()
                    .is_some_and(|calls| calls >= 2)
            );
            assert!(
                report["tail_micros"]
                    .as_u64()
                    .is_some_and(|micros| micros < 150_000)
            );
            assert_eq!(report["negative_ack_seen"], false);
            println!("JW1 causal composite evidence: {report}");
            std::fs::remove_dir_all(&root).expect("clean exact owned JW1 root");
            assert!(!root.exists(), "owned JW1 root is gone");
        }

        #[test]
        #[ignore]
        fn jw1_managed_job_payload() {
            std::thread::sleep(Duration::from_secs(30));
        }
    }

    #[cfg(feature = "script-acu-embedder")]
    fn wasm_function_import_names(wasm: &[u8]) -> Vec<String> {
        fn uleb(bytes: &[u8], at: &mut usize) -> usize {
            let mut value = 0usize;
            let mut shift = 0;
            loop {
                let byte = bytes[*at];
                *at += 1;
                value |= usize::from(byte & 0x7f) << shift;
                if byte & 0x80 == 0 {
                    return value;
                }
                shift += 7;
            }
        }

        fn name(bytes: &[u8], at: &mut usize) -> String {
            let len = uleb(bytes, at);
            let value = std::str::from_utf8(&bytes[*at..*at + len])
                .expect("Wasm import names are UTF-8")
                .to_owned();
            *at += len;
            value
        }

        assert_eq!(&wasm[..8], b"\0asm\x01\0\0\0", "valid Wasm header");
        let mut at = 8;
        let mut imports = Vec::new();
        while at < wasm.len() {
            let section_id = wasm[at];
            at += 1;
            let section_len = uleb(wasm, &mut at);
            let section_end = at + section_len;
            if section_id == 2 {
                let count = uleb(wasm, &mut at);
                for _ in 0..count {
                    let module = name(wasm, &mut at);
                    let field = name(wasm, &mut at);
                    assert_eq!(wasm[at], 0, "ACU closure imports functions only");
                    at += 1;
                    let _type_index = uleb(wasm, &mut at);
                    imports.push(format!("{module}.{field}"));
                }
            }
            at = section_end;
        }
        imports
    }

    /// `check` and `execute` must agree about what the subset is.
    ///
    /// They compile through the same entry point, and this is the test that
    /// says so: a construct outside the subset has to be refused by `check`,
    /// not accepted there and then refused at run time.
    #[cfg(feature = "script-qjswasm")]
    #[test]
    fn qjswasm_check_refuses_what_execute_would_refuse() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::set("qjswasm");
        let engine = QjswasmEngineBackend;
        let options = ScriptInvocationOptions::default();
        // This has now been overtaken twice, which is the assertion working:
        // `%` stood here until the bump to `1271a00` implemented it (4b02663),
        // and `1 ? 2 : 3` replaced it until the bump to `14a641a` implemented
        // the conditional (32fc548).
        //
        // **The second one went unnoticed for a day**, and the reason is worth
        // more than the fix: this test lives in the root crate's lib behind
        // `script-qjswasm`, which is not a default feature, so neither
        // `cargo test -p agenterm-qjswasm` nor a plain `cargo test --workspace`
        // reaches it. Both were run and both were green. The command that sees
        // it is
        // `cargo test --features script-qjswasm --lib script_engine`.
        //
        // `switch` is the replacement, and it is deliberately **not** one of
        // the six in `crates/agenterm-qjswasm/tests/qjs_guest.rs`: two lists
        // that share a source die on the same upstream commit and tell you
        // once, where two that do not tell you twice.
        let source = "switch (1) { case 1: return 2; }";

        let checked = engine
            .check(source, &options)
            .expect_err("`switch` is not lowered");
        assert!(
            checked.message.contains("this engine does not support"),
            "{checked}"
        );
        assert!(
            engine.execute(source, &options, None).is_err(),
            "execute must refuse what check refused"
        );
    }

    #[cfg(feature = "script-qjswasm")]
    #[test]
    fn qjswasm_source_failures_have_the_script_category_at_both_public_engine_paths() {
        let engine = QjswasmEngineBackend;
        let options = ScriptInvocationOptions::default();

        let syntax = engine
            .check("return @;", &options)
            .expect_err("invalid source must fail check");
        assert_eq!(syntax.category, ScriptFailureCategory::Script);

        let unsupported = engine
            .execute("return \"x\".trimStart();", &options, None)
            .expect_err("an unsupported source method must fail execution");
        assert_eq!(unsupported.category, ScriptFailureCategory::Script);
    }

    // ── the retired adapter's tests ──────────────────────────────────
    //
    // Five tests lived here and drove `QjsEngineBackend` through the trait:
    // `enabled` against the env, `check` on valid and broken source,
    // `execute`'s value projection, its not-enabled refusal, and its
    // fleet-bridge plus args wiring.
    //
    // **They cannot run any more, and that is the migration working.**
    // `AGENTERM_SCRIPT_BACKEND=qjs` resolves to `Qjswasm` since 2026-08-26
    // (PRD 02.36 archive gate 2), so no environment value selects this
    // adapter and every one of those tests began by selecting it. Keeping
    // them alive would have meant reopening a route to the retired engine
    // for the tests' own benefit, which is the tail wagging the dog.
    //
    // Where the coverage went, so nothing is lost silently:
    //
    // * `tests/script_engine_equivalence.rs` drives **the crate** --
    //   `eval_entry_with_host` -- through the shipped `fleet.js`, and asserts
    //   the same operation on the wire and the same value back as qjswasm.
    //   Six cases, zero divergences. The bridge and value projection this
    //   group covered are exercised there, on the layer that still exists.
    // * `gate_two_trait_equivalence` above holds the two engines to the same
    //   stdout, value and check verdict, and pins the four language
    //   constructs where they *disagree*.
    // * `the_old_adapter_is_unreachable_from_the_environment` is the one
    //   assertion this group leaves behind: no env value selects it.
    //
    // They are deleted rather than `#[ignore]`d because an ignored test that
    // can never pass is a claim of coverage that is not there.

    // ---- sql ----

    #[cfg(feature = "script-sql")]
    const SQL_VALID_SOURCE: &str = "SELECT 1;";
    #[cfg(feature = "script-sql")]
    const SQL_BROKEN_SOURCE: &str = "SELEC 1 FORM;";

    #[test]
    #[cfg(feature = "script-sql")]
    fn sql_engine_enabled_matches_env() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::set("sql");
        let engine = SqlEngineBackend;
        assert_eq!(engine.backend_id(), ScriptBackend::Sql);
        assert!(engine.enabled());

        let _env = EnvGuard::set("rh");
        assert!(!SqlEngineBackend.enabled());
    }

    #[test]
    #[cfg(feature = "script-sql")]
    fn sql_engine_check_valid_and_broken_source() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::set("sql");
        let engine = SqlEngineBackend;
        let options = ScriptInvocationOptions::default();

        engine
            .check(SQL_VALID_SOURCE, &options)
            .expect("valid sql source should check clean");
        assert!(
            engine.check(SQL_BROKEN_SOURCE, &options).is_err(),
            "broken sql source should fail check"
        );
    }

    #[test]
    #[cfg(feature = "script-sql")]
    /// Same correction as the lua twin above: the engine no longer gates on
    /// the environment, so a direct `check` checks.
    fn sql_engine_check_runs_when_called_directly_regardless_of_env() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::clear();
        let engine = SqlEngineBackend;
        let options = ScriptInvocationOptions::default();

        assert!(engine.check(SQL_VALID_SOURCE, &options).is_ok());
    }

    #[test]
    #[cfg(feature = "script-sql")]
    fn sql_engine_execute_returns_rows_for_a_select() {
        // M1: execute() is real (plan/design-sql-execution-target.md).
        // Replaces the old placeholder-pinning test
        // (sql_engine_execute_returns_the_not_implemented_error), which
        // asserted the opposite — that execute() always failed closed.
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::set("sql");
        let engine = SqlEngineBackend;
        let options = ScriptInvocationOptions::default();

        let result = engine
            .execute(SQL_VALID_SOURCE, &options, None)
            .expect("sql execute should succeed for a valid SELECT");
        assert_eq!(result.stdout, "");
        assert_eq!(result.value, Some(serde_json::json!([{"1": 1}])));
    }

    #[test]
    #[cfg(feature = "script-sql")]
    fn sql_engine_execute_runs_multi_statement_scripts_in_order() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::set("sql");
        let engine = SqlEngineBackend;
        let options = ScriptInvocationOptions::default();

        let result = engine
            .execute(
                "CREATE TABLE widgets (id INTEGER, name TEXT); \
                 INSERT INTO widgets (id, name) VALUES (1, 'gizmo'); \
                 SELECT id, name FROM widgets;",
                &options,
                None,
            )
            .expect("multi-statement sql execute should succeed");
        assert_eq!(
            result.value,
            Some(serde_json::json!([{"id": 1, "name": "gizmo"}]))
        );
    }

    #[test]
    #[cfg(feature = "script-sql")]
    fn sql_engine_execute_errors_not_panics_on_bad_sql_at_execute_time() {
        // "bad sql at execute time" here means execution-time (SQLite)
        // rejection, not a check()-catchable parse error — querying a
        // table that doesn't exist parses fine, fails when run.
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::set("sql");
        let engine = SqlEngineBackend;
        let options = ScriptInvocationOptions::default();

        let error = engine
            .execute("SELECT * FROM does_not_exist;", &options, None)
            .expect_err("querying a nonexistent table should error, not panic");
        assert!(!error.message.is_empty());
    }

    #[test]
    #[cfg(feature = "script-sql")]
    /// Same correction as the lua twin above, for `execute`.
    fn sql_engine_execute_runs_when_called_directly_regardless_of_env() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::clear();
        let engine = SqlEngineBackend;
        let options = ScriptInvocationOptions::default();

        assert!(
            engine.execute(SQL_VALID_SOURCE, &options, None).is_ok(),
            "selection happens in `resolve`, not in the engine; a direct call runs"
        );
    }

    #[test]
    #[cfg(feature = "script-sql")]
    fn sql_engine_entry_extensions_match_from_entry_path() {
        for ext in SqlEngineBackend.entry_extensions() {
            let path = format!("script.{ext}");
            assert_eq!(
                ScriptBackend::from_entry_path(&path),
                Some(ScriptBackend::Sql),
                "extension {ext} should route to sql"
            );
        }
    }

    // ---- ScriptEngine enum (static dispatch) ----

    #[test]
    fn script_engine_for_backend_and_engine_for_agree() {
        // Single-element with every engine feature off — the list grows
        // with features, so the loop shape is the point.
        for id in ScriptBackend::all() {
            assert_eq!(ScriptEngine::for_backend(id).backend_id(), id);
            assert_eq!(engine_for(id).backend_id(), id);
        }
    }

    #[test]
    fn script_engine_all_covers_every_backend_id() {
        let ids: Vec<ScriptBackend> = ScriptEngine::all().iter().map(|e| e.backend_id()).collect();
        // Must match `ScriptBackend::all`'s own order. An enumeration test
        // that does not enumerate the newest member is worse than none,
        // because it reads as coverage.
        assert_eq!(ids, ScriptBackend::all());
    }

    /// Every extension a backend claims must route to that backend -- with one
    /// documented exception, checked here rather than silently skipped.
    ///
    /// `QjswasmEngineBackend` lists `wasm` because it genuinely runs it, while
    /// `ScriptBackend::from_entry_path` deliberately keeps `.wasm` on
    /// wasmcore (its doc comment says why: rerouting would take `fd_write`
    /// away from an existing guest, and PRD 36 marks taking that route as
    /// excluded). Reaching qjswasm for `.wasm` is an explicit
    /// `AGENTERM_SCRIPT_BACKEND=qjswasm` decision. The test used to assert the
    /// unqualified invariant and therefore failed whenever
    /// `--features script-qjswasm` was on; asserting the exception *by name*
    /// keeps the invariant sharp for every other pair and makes a second
    /// exception impossible to add quietly.
    #[test]
    fn script_engine_entry_extensions_match_from_entry_path_for_all() {
        for engine in ScriptEngine::all() {
            for ext in engine.entry_extensions() {
                let path = format!("script.{ext}");
                let routed = ScriptBackend::from_entry_path(&path);
                assert_eq!(
                    routed,
                    Some(engine.backend_id()),
                    "extension {ext} should route to {:?}",
                    engine.backend_id()
                );
            }
        }
    }
}
