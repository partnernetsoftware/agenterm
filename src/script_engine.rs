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
//! `RhEngineBackend` is the one exception: it still delegates to
//! `try_execute_rh_invocation` in `script_backend.rs`, which is NOT deleted
//! and NOT folded here. `crates/agenterm-rh/src/main.rs` — compiled as the
//! `agenterm-rh` *binary target of the root `agenterm` package* (see root
//! `Cargo.toml`'s `[[bin]] name = "agenterm-rh" path =
//! "crates/agenterm-rh/src/main.rs"`, distinct from the `agenterm-rh`
//! library crate under `crates/agenterm-rh/`) — calls
//! `agenterm::script_backend::try_execute_rh_invocation` directly and needs
//! its typed `agenterm_rh::RhError` return (it propagates the error with
//! `?` into its own `Result<_, RhError>`), which this trait's `String`-typed
//! `ScriptEngineError` cannot losslessly round-trip. Folding rh's logic into
//! `RhEngineBackend` while also keeping `try_execute_rh_invocation` for that
//! caller would mean maintaining the same logic twice; keeping the existing
//! thin-delegation shape for rh only avoids that duplication. See the M4
//! task report for the full grep trail.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::script_backend::{RhInvocationOptions, ScriptBackend, try_execute_rh_invocation};
use crate::script_protocol::{ScriptBudgets, ScriptOperation};

// ---------------------------------------------------------------------
// §2.2 — shared types
// ---------------------------------------------------------------------

/// Shared invocation options across all three engines. Replaced the
/// previously-duplicated `RhInvocationOptions`/`LuaInvocationOptions`/
/// `QjsInvocationOptions`. Trait-M4 deleted the lua/qjs versions entirely
/// (folded their logic in directly); `RhInvocationOptions` still exists in
/// `script_backend.rs` — `RhEngineBackend` below converts into it — because
/// `try_execute_rh_invocation` has a real external caller (see module doc).
#[derive(Clone, Debug, Default)]
pub struct ScriptInvocationOptions {
    pub project_root: Option<PathBuf>,
    pub arguments: Option<Value>,
    pub budgets: Option<ScriptBudgets>,
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

/// What one run cost, in the units the engine's own budget is denominated in.
///
/// Deliberately the counters a *budget* is made of and not a wall clock:
/// these are deterministic for a given artifact and input, so two runs of the
/// same module report the same numbers and a receipt is comparable across
/// machines. A duration is not, which is why there is not one here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ScriptCost {
    pub steps: u64,
    pub peak_call_depth: usize,
    pub peak_activation_slots: usize,
}

/// Unified error type. Trait boundary collapses `agenterm_rh::RhError`
/// (typed enum) and lua/qjs's `String` down to `String` — see design §2.2
/// "哪里不吸收" for the rationale (lossy but not a new loss: callers
/// already flatten all three into `ScriptFailureCategory::Configuration`).
pub type ScriptEngineError = String;

/// Fleet bridge callback shared by all three engines: (operation_id,
/// params_json) -> result_json. Unified to `Arc` (absorbs rh's `Box` vs
/// lua/qjs's `Arc` asymmetry noted in design §1.3 finding 1); rh's adapter
/// wraps this `Arc` in a closure to hand to `script_rh_host::FleetBridgeFn`
/// (`Box`), since `script_rh_host.rs` itself is out of scope this phase.
pub type ScriptFleetBridgeFn = Arc<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>;

// ---------------------------------------------------------------------
// §2.3 — trait body
// ---------------------------------------------------------------------

/// Unified per-engine "single invocation" interface (check one source,
/// execute one source). Does not cover check-many/corpus-scan (already
/// unified at the `agenterm-script-common` crate level) or pack/qualify CLI
/// verbs (engine-specific pack shapes, see design §3 non-goals).
pub trait ScriptEngineBackend {
    /// The corresponding `ScriptBackend` variant.
    fn backend_id(&self) -> ScriptBackend;

    /// Entry-file extensions this engine claims, mirroring
    /// `ScriptBackend::from_entry_path`'s routing table.
    fn entry_extensions(&self) -> &'static [&'static str];

    /// Whether this engine is selected via `AGENTERM_SCRIPT_BACKEND`.
    /// Default implementation reads the global `ScriptBackend::from_env()`.
    fn enabled(&self) -> bool {
        ScriptBackend::from_env() == self.backend_id()
    }

    /// Check operation. `source` may be empty only for rh's cached-pack
    /// deployment shape (see design §1.2 item 2) — non-rh engines return
    /// `Err` on empty source without any special-casing at the trait level.
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

fn not_enabled_error(backend: ScriptBackend) -> ScriptEngineError {
    format!("{} backend not enabled", backend.as_str())
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
#[cfg(any(feature = "script-lua", feature = "script-qjs"))]
type ScriptArgsAccessors = (
    Arc<dyn Fn() -> i64 + Send + Sync>,
    Arc<dyn Fn(i64) -> Result<String, String> + Send + Sync>,
);

#[cfg(any(feature = "script-lua", feature = "script-qjs"))]
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

/// rh engine adapter. Delegates to `try_execute_rh_invocation` — does not
/// re-derive native-pack resolution, host binding, or output-budget logic.
///
/// Unlike `LuaEngineBackend`/`QjsEngineBackend` (Trait-M4 folded their logic
/// in directly), this adapter is intentionally left delegating: see the
/// module doc comment for why (`crates/agenterm-rh/src/main.rs`'s bin
/// target is a real external caller of `try_execute_rh_invocation` that
/// needs its typed `agenterm_rh::RhError`).
pub struct RhEngineBackend;

impl ScriptEngineBackend for RhEngineBackend {
    fn backend_id(&self) -> ScriptBackend {
        ScriptBackend::Rh
    }

    fn entry_extensions(&self) -> &'static [&'static str] {
        &["rh", "rhai"]
    }

    fn identity(&self) -> String {
        format!("agenterm-rh {}", env!("CARGO_PKG_VERSION"))
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

    /// The source, because rh's artifact is a native pack whose bytes depend
    /// on the host toolchain -- hashing it would answer "which machine built
    /// this" rather than "which program is this".
    fn artifact_hash(&self, source: &str) -> Option<Result<(String, &'static str), String>> {
        Some(Ok((
            agenterm_script_common::hex::sha256_hex(source.as_bytes()),
            "source",
        )))
    }

    /// `None`: rh's corpus-scan is a verb of its own dev CLI
    /// (`agenterm rh corpus-scan`), reached through `script_rh_cli`'s
    /// `RH_DEV_COMMANDS`, and there is no scanner in the `agenterm-rh` crate
    /// for this face to call. Pointing at the verb that exists beats
    /// re-implementing one that would then be a second answer.
    fn corpus_scan(
        &self,
        _dir: &std::path::Path,
    ) -> Option<Result<agenterm_script_common::corpus_scan::CorpusScanReport, String>> {
        None
    }

    /// The wrapper this verb has always used, now stated where it belongs.
    ///
    /// rh has no completion value, so the expression's value has to come back
    /// out of `stdout` through a marker line that
    /// `script_backend::take_rh_eval_value` strips. That is why this one is
    /// shaped so differently from the others -- and why it read as a
    /// reasonable shared default for as long as nobody ran the other engines.
    fn eval_entry_source(&self, expression: &str) -> Option<String> {
        Some(format!(
            "fn __agenterm_eval_expression() {{ {expression} }}\nfn entry() {{ let __agenterm_eval_value = __agenterm_eval_expression(); print(\"{}\" + rh::json::stringify(__agenterm_eval_value)); 0 }}",
            crate::script_protocol::RH_EVAL_VALUE_MARKER
        ))
    }

    fn check(
        &self,
        source: &str,
        options: &ScriptInvocationOptions,
    ) -> Result<(), ScriptEngineError> {
        let rh_options = RhInvocationOptions {
            project_root: options.project_root.clone(),
            arguments: options.arguments.clone(),
            budgets: options.budgets.clone(),
        };
        match try_execute_rh_invocation(ScriptOperation::Check, source, rh_options, None) {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(not_enabled_error(self.backend_id())),
            Err(error) => Err(error.to_string()),
        }
    }

    fn execute(
        &self,
        source: &str,
        options: &ScriptInvocationOptions,
        fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Result<ScriptInvocationResult, ScriptEngineError> {
        let rh_options = RhInvocationOptions {
            project_root: options.project_root.clone(),
            arguments: options.arguments.clone(),
            budgets: options.budgets.clone(),
        };
        // rh's try_execute_rh_invocation wants Option<script_rh_host::FleetBridgeFn>
        // (Box<dyn Fn...>); wrap the shared Arc in a closure to bridge the two
        // smart-pointer types (design §2.2 — rh internal adapter does this, not
        // script_rh_host.rs itself).
        let rh_bridge: Option<crate::script_rh_host::FleetBridgeFn> = fleet_bridge.map(|bridge| {
            let boxed: crate::script_rh_host::FleetBridgeFn =
                Box::new(move |op_id: &str, params: &str| bridge(op_id, params));
            boxed
        });
        match try_execute_rh_invocation(ScriptOperation::Eval, source, rh_options, rh_bridge) {
            Ok(Some(result)) => Ok(ScriptInvocationResult {
                stdout: result.stdout,
                value: result.value,
                cost: None,
            }),
            Ok(None) => Err(not_enabled_error(self.backend_id())),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// Lua engine adapter. Invocation logic folded in directly (Trait-M4) from
/// the former `try_execute_lua_invocation` in `script_backend.rs` — no
/// other caller referenced it (grep-verified across `src/`, `tests/`,
/// `crates/`; `tests/lua_task_entry_regression.rs` referenced it too, but
/// that file was already failing to compile beforehand — pre-existing,
/// unrelated `ScriptBackend::Rhai` reference — so it did not count as a
/// live caller).
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
        if !self.enabled() {
            return Err(not_enabled_error(self.backend_id()));
        }

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
        if !self.enabled() {
            return Err(not_enabled_error(self.backend_id()));
        }

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

/// qjs engine adapter. Invocation logic folded in directly (Trait-M4) from
/// the former `try_execute_qjs_invocation` in `script_backend.rs` — no
/// other caller referenced it (grep-verified across `src/`, `tests/`,
/// `crates/`).
///
/// Structurally mirrors `LuaEngineBackend` — same "not enabled -> error",
/// same fleet-bridge/args wiring shape — because qjs, like lua, is an
/// interpreted engine with no AOT/native-codegen step (unlike rh's
/// `RhEngineBackend`, which resolves/loads a compiled native pack). `value`
/// is `Option<serde_json::Value>` rather than lua's widened-from-`i64`
/// because `agenterm_qjs::eval_entry_with_host` already produces a typed
/// JSON value (via `JSON.stringify`) — a strict superset, not a divergence:
/// any lua-shaped i64 result is also representable here.
#[cfg(feature = "script-qjs")]
pub struct QjsEngineBackend;

#[cfg(feature = "script-qjs")]
impl ScriptEngineBackend for QjsEngineBackend {
    fn backend_id(&self) -> ScriptBackend {
        ScriptBackend::Qjs
    }

    fn entry_extensions(&self) -> &'static [&'static str] {
        &["js", "mjs"]
    }

    fn identity(&self) -> String {
        format!("agenterm-qjs {}", env!("CARGO_PKG_VERSION"))
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

    /// The source, which is what `agenterm-qjs hash` has always printed. Its
    /// *bytecode* hash is the one PRD 02.36 records as not reproducible; this
    /// verb does not offer it, and labelling this one `source` is what stops a
    /// reader assuming it is the other.
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
        Some(agenterm_qjs::corpus_scan::scan_directory(dir))
    }

    /// `entry()`, which is this engine's convention and not a habit: `eval.rs`
    /// evaluates the source and then calls a top-level `entry`. Measured:
    /// `function entry() { return 1 + 2; }` through `script run` answers `3`.
    fn eval_entry_source(&self, expression: &str) -> Option<String> {
        Some(format!("function entry() {{ return ({expression}); }}"))
    }

    fn check(
        &self,
        source: &str,
        _options: &ScriptInvocationOptions,
    ) -> Result<(), ScriptEngineError> {
        if !self.enabled() {
            return Err(not_enabled_error(self.backend_id()));
        }

        agenterm_qjs::check(source, "invocation.js").map_err(|e| e.to_string())?;
        Ok(())
    }

    fn execute(
        &self,
        source: &str,
        options: &ScriptInvocationOptions,
        fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Result<ScriptInvocationResult, ScriptEngineError> {
        if !self.enabled() {
            return Err(not_enabled_error(self.backend_id()));
        }

        let mut host = agenterm_qjs::QjsHostFunctions::default();

        // Wire fleet bridge
        if let Some(bridge) = fleet_bridge {
            host.fleet_call = Some(Arc::new(
                move |op_id: &str, params: &str| -> Result<String, String> {
                    bridge(op_id, params)
                },
            ));
        }

        // Wire args_len / arg from options.arguments
        if let Some(arguments) = options.arguments.clone() {
            let (args_len, arg) = script_args_accessors(arguments);
            host.args_len = Some(args_len);
            host.arg = Some(arg);
        }

        let result = agenterm_qjs::eval_entry_with_host(source, "invocation.js", &host)
            .map_err(|e| format!("qjs_eval: {e}"))?;

        Ok(ScriptInvocationResult {
            stdout: result.stdout,
            value: result.value,
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
        if !self.enabled() {
            return Err(not_enabled_error(self.backend_id()));
        }

        agenterm_sql::check(source, "invocation.sql").map_err(|e| e.to_string())?;
        Ok(())
    }

    fn execute(
        &self,
        source: &str,
        options: &ScriptInvocationOptions,
        _fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Result<ScriptInvocationResult, ScriptEngineError> {
        if !self.enabled() {
            return Err(not_enabled_error(self.backend_id()));
        }

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

/// wasmcore engine adapter. Loads a `.wasm` file (source is the file path),
/// JIT-compiles it via Cranelift, and runs it with the shared fleet bridge.
///
/// `check` validates the WASM binary via `wasmtime::Module::validate_binary`;
/// `execute` calls `WasmCoreHost::run_module` with a `WasmFleetBridgeFn`
/// passthrough (identical shape to `ScriptFleetBridgeFn` — zero-cost re-wrap).
#[cfg(feature = "script-wasmcore")]
pub struct WasmcoreEngineBackend {
    host: agenterm_wasmcore::WasmCoreHost,
}

#[cfg(feature = "script-wasmcore")]
impl Default for WasmcoreEngineBackend {
    fn default() -> Self {
        Self {
            host: agenterm_wasmcore::WasmCoreHost::new(),
        }
    }
}

#[cfg(feature = "script-wasmcore")]
impl ScriptEngineBackend for WasmcoreEngineBackend {
    fn backend_id(&self) -> ScriptBackend {
        ScriptBackend::Wasmcore
    }

    fn entry_extensions(&self) -> &'static [&'static str] {
        &["wasm"]
    }

    fn identity(&self) -> String {
        format!("agenterm-wasmcore {}", env!("CARGO_PKG_VERSION"))
    }

    /// `None` to build: this engine's input already *is* the artifact. There
    /// is nothing to compile, and a `pack build` that copied the file would be
    /// `cp` wearing a verb.
    fn pack_artifact(&self, _source: &str) -> Option<Result<(Vec<u8>, &'static str), String>> {
        None
    }

    /// **Yes to load**, and this is the method wasmcore should have had all
    /// along: `run_module_from_bytes` exists and takes exactly this. Its
    /// `execute` reaches the same engine through a *path* instead, which is
    /// the wart [`Self::source_is_a_path`] names -- a caller who comes through
    /// here never needs it.
    fn execute_artifact(
        &self,
        artifact: &[u8],
        _options: &ScriptInvocationOptions,
        fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Option<Result<ScriptInvocationResult, ScriptEngineError>> {
        let bridge: Option<agenterm_wasmcore::WasmFleetBridgeFn> = fleet_bridge;
        Some(
            agenterm_wasmcore::WasmCoreHost::run_module_from_bytes(&self.host, artifact, bridge)
                .map(|result| ScriptInvocationResult {
                    stdout: result.stdout,
                    value: None,
                    cost: None,
                })
                .map_err(|e| format!("wasm execution: {e}")),
        )
    }

    /// **True, and this is the wart.** `WasmcoreEngineBackend::check` does
    /// `std::fs::read(source)`: it takes a path where the trait says program
    /// text. It is answered honestly rather than hidden because the CLI has
    /// to know, and a hidden one is what let the extension-keyed branch hand
    /// paths to four other engines.
    ///
    /// Fixing it means a byte-carrying face -- the `&str` cannot hold a
    /// module -- which is the same missing face `pack`/`qualify` need. Until
    /// that exists, `true` is the truth.
    fn source_is_a_path(&self) -> bool {
        true
    }

    /// `None`. This engine's input *is* the artifact -- hashing it would hand
    /// the caller a digest of the file they just named, which `sha256sum` does
    /// without an engine.
    fn artifact_hash(&self, _source: &str) -> Option<Result<(String, &'static str), String>> {
        None
    }

    /// `None`, for the reason this engine has no `eval` either: its input is
    /// a `.wasm` module. "Check this file as text" has no meaning here -- the
    /// question a corpus of modules answers is the load gate, which takes
    /// bytes, and calling that a corpus scan would put two different
    /// questions under one verb.
    fn corpus_scan(
        &self,
        _dir: &std::path::Path,
    ) -> Option<Result<agenterm_script_common::corpus_scan::CorpusScanReport, String>> {
        None
    }

    /// **None.** This engine's source is a `.wasm` module, and there is no
    /// text an expression could be wrapped in that would become one -- it has
    /// no compiler. Refusing by name is the only honest answer; the
    /// alternative is what this verb used to do, which was hand it rh source
    /// and let the module loader complain about the bytes.
    fn eval_entry_source(&self, _expression: &str) -> Option<String> {
        None
    }

    fn check(
        &self,
        source: &str,
        _options: &ScriptInvocationOptions,
    ) -> Result<(), ScriptEngineError> {
        if !self.enabled() {
            return Err(not_enabled_error(self.backend_id()));
        }

        let wasm_bytes =
            std::fs::read(source).map_err(|e| format!("reading wasm file {source}: {e}"))?;
        agenterm_wasmcore::WasmCoreHost::validate_binary(&self.host, &wasm_bytes)
            .map_err(|e| format!("wasm validation: {e}"))
    }

    fn execute(
        &self,
        source: &str,
        _options: &ScriptInvocationOptions,
        fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Result<ScriptInvocationResult, ScriptEngineError> {
        if !self.enabled() {
            return Err(not_enabled_error(self.backend_id()));
        }

        let wasm_bridge: Option<agenterm_wasmcore::WasmFleetBridgeFn> =
            fleet_bridge.map(|bridge| {
                let arc: agenterm_wasmcore::WasmFleetBridgeFn = bridge;
                arc
            });

        let result = self
            .host
            .run_module(source, wasm_bridge)
            .map_err(|e| format!("wasm execution: {e}"))?;

        Ok(ScriptInvocationResult {
            stdout: result.stdout,
            value: None,
            cost: None,
        })
    }
}

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
        // `.wasm` is listed because this engine can run it, but
        // `ScriptBackend::from_entry_path` still routes `.wasm` to wasmcore by
        // default; reaching this backend for wasm is an explicit env choice.
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
        _options: &ScriptInvocationOptions,
        fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Option<Result<ScriptInvocationResult, ScriptEngineError>> {
        let bridge: Option<agenterm_qjswasm::FleetBridgeFn> = fleet_bridge;
        let mut engine = agenterm_qjswasm::Engine::new();
        Some(
            engine
                .run_once(
                    agenterm_qjswasm::Guest::CompiledQjs(artifact),
                    bridge,
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
                    }),
                })
                .map_err(|e| e.to_string()),
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
        Some(agenterm_qjswasm::corpus_scan::scan_directory(dir))
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
        if !self.enabled() {
            return Err(not_enabled_error(self.backend_id()));
        }
        // Same contract as `execute`: `source` is text. `check_qjs` compiles
        // and load-validates under the budget the run will spend, and executes
        // nothing -- the two must share an entry point, or a check would accept
        // what the run then refuses.
        agenterm_qjswasm::check_qjs(source).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn execute(
        &self,
        source: &str,
        _options: &ScriptInvocationOptions,
        fleet_bridge: Option<ScriptFleetBridgeFn>,
    ) -> Result<ScriptInvocationResult, ScriptEngineError> {
        if !self.enabled() {
            return Err(not_enabled_error(self.backend_id()));
        }

        // `ScriptFleetBridgeFn` and `agenterm_qjswasm::FleetBridgeFn` are the
        // same `Arc<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>`
        // shape, so this is a rebind, not a wrapper.
        let bridge: Option<agenterm_qjswasm::FleetBridgeFn> = fleet_bridge;

        let mut engine = agenterm_qjswasm::Engine::new();
        // `"main"` with no arguments is still the entry convention after the
        // `df8decd` bump, re-checked rather than assumed: the compiler exports
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
        let outcome = engine
            .run_once(agenterm_qjswasm::Guest::Qjs(source), bridge, "main", &[])
            .map_err(|e| e.to_string())?;

        Ok(ScriptInvocationResult {
            stdout: outcome.stdout,
            value: outcome.values.first().and_then(qjswasm_value_as_json),
            cost: Some(ScriptCost {
                steps: outcome.steps,
                peak_call_depth: outcome.peak_call_depth,
                peak_activation_slots: outcome.peak_activation_slots,
            }),
        })
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

/// Static-dispatch registry over the three engines. Not a `dyn` trait
/// object list — see design §2.4 for why enum+match is preferred as the
/// default over `Box<dyn ScriptEngineBackend>` (the trait remains
/// object-safe as a documented, unused escape hatch).
pub enum ScriptEngine {
    Rh(RhEngineBackend),
    #[cfg(feature = "script-lua")]
    Lua(LuaEngineBackend),
    #[cfg(feature = "script-qjs")]
    Qjs(QjsEngineBackend),
    #[cfg(feature = "script-sql")]
    Sql(SqlEngineBackend),
    #[cfg(feature = "script-wasmcore")]
    Wasmcore(WasmcoreEngineBackend),
    #[cfg(feature = "script-qjswasm")]
    Qjswasm(QjswasmEngineBackend),
}

impl ScriptEngine {
    pub fn all() -> Vec<ScriptEngine> {
        #[allow(unused_mut)]
        let mut engines = vec![Self::Rh(RhEngineBackend)];
        #[cfg(feature = "script-qjs")]
        engines.push(Self::Qjs(QjsEngineBackend));
        #[cfg(feature = "script-lua")]
        engines.push(Self::Lua(LuaEngineBackend));
        #[cfg(feature = "script-sql")]
        engines.push(Self::Sql(SqlEngineBackend));
        #[cfg(feature = "script-wasmcore")]
        engines.push(Self::Wasmcore(WasmcoreEngineBackend::default()));
        #[cfg(feature = "script-qjswasm")]
        engines.push(Self::Qjswasm(QjswasmEngineBackend));
        engines
    }

    /// Construct the engine variant corresponding to `id`.
    pub fn for_backend(id: ScriptBackend) -> Self {
        match id {
            ScriptBackend::Rh => Self::Rh(RhEngineBackend),
            #[cfg(feature = "script-lua")]
            ScriptBackend::Lua => Self::Lua(LuaEngineBackend),
            #[cfg(feature = "script-qjs")]
            ScriptBackend::Qjs => Self::Qjs(QjsEngineBackend),
            #[cfg(feature = "script-sql")]
            ScriptBackend::Sql => Self::Sql(SqlEngineBackend),
            #[cfg(feature = "script-wasmcore")]
            ScriptBackend::Wasmcore => Self::Wasmcore(WasmcoreEngineBackend::default()),
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

impl ScriptEngineBackend for ScriptEngine {
    fn backend_id(&self) -> ScriptBackend {
        match self {
            Self::Rh(backend) => backend.backend_id(),
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.backend_id(),
            #[cfg(feature = "script-qjs")]
            Self::Qjs(backend) => backend.backend_id(),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.backend_id(),
            #[cfg(feature = "script-wasmcore")]
            Self::Wasmcore(backend) => backend.backend_id(),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.backend_id(),
        }
    }

    fn entry_extensions(&self) -> &'static [&'static str] {
        match self {
            Self::Rh(backend) => backend.entry_extensions(),
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.entry_extensions(),
            #[cfg(feature = "script-qjs")]
            Self::Qjs(backend) => backend.entry_extensions(),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.entry_extensions(),
            #[cfg(feature = "script-wasmcore")]
            Self::Wasmcore(backend) => backend.entry_extensions(),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.entry_extensions(),
        }
    }

    fn eval_entry_source(&self, expression: &str) -> Option<String> {
        match self {
            Self::Rh(backend) => backend.eval_entry_source(expression),
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.eval_entry_source(expression),
            #[cfg(feature = "script-qjs")]
            Self::Qjs(backend) => backend.eval_entry_source(expression),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.eval_entry_source(expression),
            #[cfg(feature = "script-wasmcore")]
            Self::Wasmcore(backend) => backend.eval_entry_source(expression),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.eval_entry_source(expression),
        }
    }

    fn identity(&self) -> String {
        match self {
            Self::Rh(backend) => backend.identity(),
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.identity(),
            #[cfg(feature = "script-qjs")]
            Self::Qjs(backend) => backend.identity(),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.identity(),
            #[cfg(feature = "script-wasmcore")]
            Self::Wasmcore(backend) => backend.identity(),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.identity(),
        }
    }

    fn corpus_scan(
        &self,
        dir: &std::path::Path,
    ) -> Option<Result<agenterm_script_common::corpus_scan::CorpusScanReport, String>> {
        match self {
            Self::Rh(backend) => backend.corpus_scan(dir),
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.corpus_scan(dir),
            #[cfg(feature = "script-qjs")]
            Self::Qjs(backend) => backend.corpus_scan(dir),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.corpus_scan(dir),
            #[cfg(feature = "script-wasmcore")]
            Self::Wasmcore(backend) => backend.corpus_scan(dir),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.corpus_scan(dir),
        }
    }

    fn artifact_hash(&self, source: &str) -> Option<Result<(String, &'static str), String>> {
        match self {
            Self::Rh(backend) => backend.artifact_hash(source),
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.artifact_hash(source),
            #[cfg(feature = "script-qjs")]
            Self::Qjs(backend) => backend.artifact_hash(source),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.artifact_hash(source),
            #[cfg(feature = "script-wasmcore")]
            Self::Wasmcore(backend) => backend.artifact_hash(source),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.artifact_hash(source),
        }
    }

    fn source_is_a_path(&self) -> bool {
        match self {
            Self::Rh(backend) => backend.source_is_a_path(),
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.source_is_a_path(),
            #[cfg(feature = "script-qjs")]
            Self::Qjs(backend) => backend.source_is_a_path(),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.source_is_a_path(),
            #[cfg(feature = "script-wasmcore")]
            Self::Wasmcore(backend) => backend.source_is_a_path(),
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
            Self::Rh(backend) => backend.execute_artifact(artifact, options, fleet_bridge),
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.execute_artifact(artifact, options, fleet_bridge),
            #[cfg(feature = "script-qjs")]
            Self::Qjs(backend) => backend.execute_artifact(artifact, options, fleet_bridge),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.execute_artifact(artifact, options, fleet_bridge),
            #[cfg(feature = "script-wasmcore")]
            Self::Wasmcore(backend) => backend.execute_artifact(artifact, options, fleet_bridge),
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm(backend) => backend.execute_artifact(artifact, options, fleet_bridge),
        }
    }

    fn pack_artifact(&self, source: &str) -> Option<Result<(Vec<u8>, &'static str), String>> {
        match self {
            Self::Rh(backend) => backend.pack_artifact(source),
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.pack_artifact(source),
            #[cfg(feature = "script-qjs")]
            Self::Qjs(backend) => backend.pack_artifact(source),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.pack_artifact(source),
            #[cfg(feature = "script-wasmcore")]
            Self::Wasmcore(backend) => backend.pack_artifact(source),
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
            Self::Rh(backend) => backend.check(source, options),
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.check(source, options),
            #[cfg(feature = "script-qjs")]
            Self::Qjs(backend) => backend.check(source, options),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.check(source, options),
            #[cfg(feature = "script-wasmcore")]
            Self::Wasmcore(backend) => backend.check(source, options),
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
            Self::Rh(backend) => backend.execute(source, options, fleet_bridge),
            #[cfg(feature = "script-lua")]
            Self::Lua(backend) => backend.execute(source, options, fleet_bridge),
            #[cfg(feature = "script-qjs")]
            Self::Qjs(backend) => backend.execute(source, options, fleet_bridge),
            #[cfg(feature = "script-sql")]
            Self::Sql(backend) => backend.execute(source, options, fleet_bridge),
            #[cfg(feature = "script-wasmcore")]
            Self::Wasmcore(backend) => backend.execute(source, options, fleet_bridge),
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
    use crate::script_backend::{RhInvocationOptions as RhOpts, try_execute_rh_invocation};

    // Mirrors script_backend.rs's ENV_LOCK pattern (serialize env-var
    // manipulation across tests in this module — this is a *different*
    // mutex instance than script_backend.rs's, but since `cargo test`
    // runs all `#[test]` functions in one process across all `mod`s
    // sharing the same env var, tests in this module also risk racing
    // against script_backend.rs's own env-mutating tests. Each guard here
    // still gives serialization *within* this file, and both files restore
    // the prior value before releasing the lock, minimizing cross-file
    // interference to a narrow window.)
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Gate 2's equivalence, **at the layer the migration actually happens**.
    ///
    /// `tests/script_engine_equivalence.rs` compares the two *crates*, driving
    /// `agenterm_qjs::eval_entry_with_host` and `agenterm_qjswasm::Engine`
    /// directly through the shipped fleet bindings. That is the right test for
    /// "do the two engines produce the same Fleet operation", and it is not
    /// the layer the call sites use: `script_worker.rs` and the CLI reach
    /// `ScriptEngineBackend::check` and `::execute`, which add the enablement
    /// gate, the host wiring and the result projection on top. Migrating means
    /// repointing *those*, so the equivalence that licenses it has to be
    /// asserted through *those*.
    ///
    /// Both engines are driven with the env var set to their own name, because
    /// both methods refuse when `enabled()` is false -- which is itself a piece
    /// of behaviour the migration must not change.
    #[cfg(all(feature = "script-qjs", feature = "script-qjswasm"))]
    mod gate_two_trait_equivalence {
        use super::*;

        /// The old engine, at the layer that still exists: the crate. Its
        /// adapter is unreachable now -- see the module doc and
        /// [`the_old_adapter_is_unreachable_from_the_environment`].
        fn on_qjs(source: &str) -> Result<(String, Option<serde_json::Value>), String> {
            agenterm_qjs::eval_entry_with_host(
                source,
                "invocation.js",
                &agenterm_qjs::QjsHostFunctions::default(),
            )
            .map(|outcome| (outcome.stdout, outcome.value))
            .map_err(|e| e.to_string())
        }

        /// The new engine, at the layer production uses. The caller holds
        /// `ENV_LOCK`.
        fn on_qjswasm(source: &str) -> Result<ScriptInvocationResult, ScriptEngineError> {
            let _env = EnvGuard::set("qjswasm");
            QjswasmEngineBackend.execute(source, &ScriptInvocationOptions::default(), None)
        }

        fn checked_qjswasm(source: &str) -> Result<(), ScriptEngineError> {
            let _env = EnvGuard::set("qjswasm");
            QjswasmEngineBackend.check(source, &ScriptInvocationOptions::default())
        }

        /// The same program, written in each engine's entry convention,
        /// produces the same stdout and the same value through the trait.
        ///
        /// Not the same *source*: `agenterm-qjs` calls a top-level `entry()`
        /// and `agenterm-qjswasm` takes the script's completion value. That
        /// difference is the engines', not this test's, and
        /// `ScriptEngineBackend::eval_entry_source` is where the product
        /// already encodes it.
        #[test]
        fn both_backends_agree_on_stdout_and_value() {
            let _guard = ENV_LOCK.lock().expect("lock");
            for (js, qjs, want_out, want_value) in [
                (
                    "function entry() { print(\"hello\"); return 42; }",
                    "print(\"hello\"); return 42;",
                    "hello\n",
                    serde_json::json!(42),
                ),
                (
                    "function entry() { return \"tab\" + \"s.list\"; }",
                    "return \"tab\" + \"s.list\";",
                    "",
                    serde_json::json!("tabs.list"),
                ),
                (
                    "function entry() { var o = {a: 1}; return o.a + 1; }",
                    "let o = {a: 1}; return o.a + 1;",
                    "",
                    serde_json::json!(2),
                ),
            ] {
                let (a_out, a_val) = on_qjs(js).expect("qjs runs it");
                let b = on_qjswasm(qjs).expect("qjswasm runs it");
                assert_eq!(a_out, want_out, "qjs stdout for {js:?}");
                assert_eq!(b.stdout, want_out, "qjswasm stdout for {qjs:?}");
                assert_eq!(a_val, Some(want_value.clone()), "qjs value for {js:?}");
                assert_eq!(b.value, Some(want_value), "qjswasm value for {qjs:?}");
            }
        }

        /// `check` accepts and refuses on the same terms for a program inside
        /// both subsets, and a broken one.
        #[test]
        fn both_backends_agree_on_check() {
            let _guard = ENV_LOCK.lock().expect("lock");
            agenterm_qjs::check("function entry() { return 1; }", "invocation.js")
                .expect("qjs accepts");
            checked_qjswasm("return 1;").expect("qjswasm accepts");

            // Neither engine parses this, and both must say so rather than
            // accept and fail later.
            let broken = "function entry( { return";
            assert!(agenterm_qjs::check(broken, "invocation.js").is_err());
            assert!(checked_qjswasm(broken).is_err());
        }

        /// The new engine refuses to run when it is not the selected one, and
        /// that is load-bearing: the worker asks `enabled()` and then calls,
        /// so a backend that ran anyway would execute under another engine's
        /// name.
        #[test]
        fn the_new_backend_refuses_when_it_is_not_selected() {
            let _guard = ENV_LOCK.lock().expect("lock");
            let _env = EnvGuard::set("rh");
            assert!(
                QjswasmEngineBackend
                    .check("return 1;", &ScriptInvocationOptions::default())
                    .is_err()
            );
        }

        /// **The migration, as an assertion.** `qjs` names the new engine, and
        /// the old adapter can no longer be reached from the environment.
        ///
        /// Both halves matter. The first keeps every existing invocation
        /// working across the rename. The second is what makes the old engine
        /// safe to archive: nothing in production can route to it, so removing
        /// it cannot change what any caller gets.
        #[test]
        fn the_old_adapter_is_unreachable_from_the_environment() {
            let _guard = ENV_LOCK.lock().expect("lock");
            let _env = EnvGuard::set("qjs");
            assert_eq!(
                crate::script_backend::ScriptBackend::from_env(),
                crate::script_backend::ScriptBackend::Qjswasm,
                "`qjs` must name the engine that replaced it"
            );
            assert!(
                !QjsEngineBackend.enabled(),
                "no environment value may still select the retired engine"
            );
            assert!(QjswasmEngineBackend.enabled());
        }

        /// **The migration's actual risk, pinned rather than hidden.**
        ///
        /// The two engines are equivalent on the Fleet surface and *not* on
        /// the language. `agenterm-qjs` is rquickjs -- full modern JS;
        /// `agenterm-qjswasm` is a growing subset. Repointing a call site
        /// moves every script through the narrower one, and these are the
        /// constructs that stop working the day it happens.
        ///
        /// Two facts make that acceptable, and both are checked elsewhere
        /// rather than assumed here: `scripts/` ships no `.js` task script,
        /// only the `fleet.js` binding library, and every refusal below is a
        /// **named capability diagnostic** rather than a wrong answer -- the
        /// failure is loud at compile time, not silent at run time.
        ///
        /// When one of these starts compiling, upstream grew it, and the row
        /// moves to `both_backends_agree_on_stdout_and_value`.
        #[test]
        fn the_subset_is_narrower_and_this_is_what_breaks() {
            let _guard = ENV_LOCK.lock().expect("lock");
            for (js, qjs) in [
                // An arrow with a **default parameter**, not a plain arrow:
                // upstream `9e02e37` landed arrows, so the plain one moved out
                // of this table the way the closure and template rows did.
                // What is left is parameter syntax, which is a queue position
                // -- unlike `Math` below. When it lands, this row moves too.
                (
                    "function entry() { const f = (a = 1) => a; return f(); }",
                    "let f = (a = 1) => a; return f();",
                ),
                // A *tagged* template, not a plain one: upstream `653cebe`
                // landed templates, so `` `x` `` moved out of this table the
                // way the closure row did. A tag stays because it is not a
                // queue position either -- it needs a frozen cooked array
                // carrying `raw`, which is array methods and property
                // definition this engine does not have.
                (
                    "function entry() { const t = (s) => s; return t`x`; }",
                    "function t(s) { return s; } return t`x`;",
                ),
                (
                    "function entry() { return Math.max(1, 2); }",
                    "return Math.max(1, 2);",
                ),
                // A **closure over an outer local** was the fourth row here
                // and it is gone: upstream `68afb35` landed captures, so both
                // engines run it and it is no longer a divergence. This test
                // said "when one of these starts compiling, upstream grew it,
                // and the row moves"; it moved.
                //
                // `Math` stays, and is worth keeping over the other candidates
                // for a reason: it is not a syntax the compiler could grow but
                // a *binding* -- there is no global scope here, `JSON` is the
                // one name this engine binds, and that is a design position
                // rather than a queue position.
                (
                    "function entry() { return Object.keys({}); }",
                    "return Object.keys({});",
                ),
            ] {
                on_qjs(js).unwrap_or_else(|e| panic!("rquickjs should run {js:?}: {e}"));
                let refused = on_qjswasm(qjs).expect_err("this is outside the subset");
                assert!(
                    refused.contains("this engine ") || refused.contains("no host function"),
                    "a narrower subset must refuse by naming the capability, got {refused:?}"
                );
            }
        }
    }

    struct EnvGuard {
        prior: Option<String>,
    }

    impl EnvGuard {
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

    #[test]
    fn rh_engine_enabled_by_default_with_no_env_set() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::clear();
        assert!(RhEngineBackend.enabled());
        #[cfg(feature = "script-lua")]
        assert!(!LuaEngineBackend.enabled());
        #[cfg(feature = "script-qjs")]
        assert!(!QjsEngineBackend.enabled());
        #[cfg(feature = "script-sql")]
        assert!(!SqlEngineBackend.enabled());
    }

    #[test]
    fn script_invocation_options_default_field_shape() {
        let options = ScriptInvocationOptions::default();
        assert!(options.project_root.is_none());
        assert!(options.arguments.is_none());
        assert!(options.budgets.is_none());
    }

    // ---- rh ----

    const RH_VALID_SOURCE: &str = "fn entry() { 42 }";
    const RH_BROKEN_SOURCE: &str = "fn entry() { 1 ";

    #[test]
    fn rh_engine_enabled_matches_env() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::set("rh");
        let engine = RhEngineBackend;
        assert_eq!(engine.backend_id(), ScriptBackend::Rh);
        assert!(engine.enabled());

        #[cfg(feature = "script-lua")]
        {
            let _env = EnvGuard::set("lua");
            assert!(!RhEngineBackend.enabled());
        }
    }

    #[test]
    fn rh_engine_check_valid_and_broken_source() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::set("rh");
        let engine = RhEngineBackend;
        let options = ScriptInvocationOptions::default();

        engine
            .check(RH_VALID_SOURCE, &options)
            .expect("valid rh source should check clean");
        assert!(
            engine.check(RH_BROKEN_SOURCE, &options).is_err(),
            "broken rh source should fail check"
        );
    }

    #[test]
    fn rh_engine_execute_matches_direct_call() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::set("rh");
        let engine = RhEngineBackend;
        let options = ScriptInvocationOptions::default();

        let via_trait = engine
            .execute(RH_VALID_SOURCE, &options, None)
            .expect("trait execute should succeed");

        let direct = try_execute_rh_invocation(
            ScriptOperation::Eval,
            RH_VALID_SOURCE,
            RhOpts::default(),
            None,
        )
        .expect("direct call should not error")
        .expect("rh backend should be enabled");

        assert_eq!(via_trait.stdout, direct.stdout);
        assert_eq!(via_trait.value, direct.value);
    }

    #[test]
    fn rh_engine_entry_extensions_match_from_entry_path() {
        for ext in RhEngineBackend.entry_extensions() {
            let path = format!("script.{ext}");
            assert_eq!(
                ScriptBackend::from_entry_path(&path),
                ScriptBackend::Rh,
                "extension {ext} should route to rh"
            );
        }
    }

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

    #[test]
    #[cfg(feature = "script-lua")]
    fn lua_engine_execute_errors_when_not_enabled() {
        // Migrated from script_backend.rs's lua_backend_not_enabled_without_env
        // (was: try_execute_lua_invocation returns Ok(None) when the lua
        // backend isn't selected). The trait surface has no Option-wrapping
        // "not enabled" case, so the equivalent is an Err from execute().
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::clear();
        let engine = LuaEngineBackend;
        let options = ScriptInvocationOptions::default();

        assert!(engine.execute(LUA_VALID_SOURCE, &options, None).is_err());
    }

    #[test]
    #[cfg(feature = "script-lua")]
    fn lua_engine_entry_extensions_match_from_entry_path() {
        for ext in LuaEngineBackend.entry_extensions() {
            let path = format!("script.{ext}");
            assert_eq!(
                ScriptBackend::from_entry_path(&path),
                ScriptBackend::Lua,
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

    /// A `.qjs` script's completion value reaches the caller.
    ///
    /// Before the `df8decd` bump this backend could not have reported one: the
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
        // `%` stood here until the bump to `6920c60` implemented it (dd35c44),
        // and `1 ? 2 : 3` replaced it until the bump to `f21f0f2` implemented
        // the conditional (5bdb557).
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
            checked.contains("this engine does not support"),
            "{checked}"
        );
        assert!(
            engine.execute(source, &options, None).is_err(),
            "execute must refuse what check refused"
        );
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

    #[cfg(feature = "script-qjs")]
    #[test]
    fn qjs_engine_entry_extensions_match_from_entry_path() {
        for ext in QjsEngineBackend.entry_extensions() {
            let path = format!("script.{ext}");
            assert_eq!(
                ScriptBackend::from_entry_path(&path),
                ScriptBackend::Qjs,
                "extension {ext} should route to qjs"
            );
        }
    }

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
    fn sql_engine_check_errors_when_not_enabled() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::clear();
        let engine = SqlEngineBackend;
        let options = ScriptInvocationOptions::default();

        assert!(engine.check(SQL_VALID_SOURCE, &options).is_err());
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
        assert!(!error.is_empty());
    }

    #[test]
    #[cfg(feature = "script-sql")]
    fn sql_engine_execute_errors_when_not_enabled() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let _env = EnvGuard::clear();
        let engine = SqlEngineBackend;
        let options = ScriptInvocationOptions::default();

        let error = engine
            .execute(SQL_VALID_SOURCE, &options, None)
            .expect_err("execute must fail when the backend isn't enabled");
        // "not enabled" must win over "not implemented" when both are
        // true, matching lua/qjs's same enabled()-gate-first ordering.
        assert!(error.contains("not enabled"), "{error}");
    }

    #[test]
    #[cfg(feature = "script-sql")]
    fn sql_engine_entry_extensions_match_from_entry_path() {
        for ext in SqlEngineBackend.entry_extensions() {
            let path = format!("script.{ext}");
            assert_eq!(
                ScriptBackend::from_entry_path(&path),
                ScriptBackend::Sql,
                "extension {ext} should route to sql"
            );
        }
    }

    // ---- ScriptEngine enum (static dispatch) ----

    #[test]
    fn script_engine_for_backend_and_engine_for_agree() {
        // Single-element with every engine feature off — the list grows
        // with features, so the loop shape is the point.
        #[allow(clippy::single_element_loop)]
        for id in [
            ScriptBackend::Rh,
            #[cfg(feature = "script-lua")]
            ScriptBackend::Lua,
            #[cfg(feature = "script-qjs")]
            ScriptBackend::Qjs,
            #[cfg(feature = "script-sql")]
            ScriptBackend::Sql,
        ] {
            assert_eq!(ScriptEngine::for_backend(id).backend_id(), id);
            assert_eq!(engine_for(id).backend_id(), id);
        }
    }

    #[test]
    fn script_engine_all_covers_every_backend_id() {
        let ids: Vec<ScriptBackend> = ScriptEngine::all().iter().map(|e| e.backend_id()).collect();
        #[allow(unused_mut)]
        let mut expected = vec![ScriptBackend::Rh];
        #[cfg(feature = "script-qjs")]
        expected.push(ScriptBackend::Qjs);
        #[cfg(feature = "script-lua")]
        expected.push(ScriptBackend::Lua);
        #[cfg(feature = "script-sql")]
        expected.push(ScriptBackend::Sql);
        #[cfg(feature = "script-wasmcore")]
        expected.push(ScriptBackend::Wasmcore);
        // Must match `ScriptEngine::all`'s own push order. This arm was missing
        // until 2026-08-25, so the test failed under `--features
        // script-qjswasm` -- an enumeration test that does not enumerate the
        // newest member is worse than none, because it reads as coverage.
        #[cfg(feature = "script-qjswasm")]
        expected.push(ScriptBackend::Qjswasm);
        assert_eq!(ids, expected);
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
                #[cfg(feature = "script-qjswasm")]
                if engine.backend_id() == ScriptBackend::Qjswasm && *ext == "wasm" {
                    assert_ne!(
                        routed,
                        ScriptBackend::Qjswasm,
                        "`.wasm` is documented as NOT routing to qjswasm by default"
                    );
                    continue;
                }
                assert_eq!(
                    routed,
                    engine.backend_id(),
                    "extension {ext} should route to {:?}",
                    engine.backend_id()
                );
            }
        }
    }
}
