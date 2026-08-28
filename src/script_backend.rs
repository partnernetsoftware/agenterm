//! Script execution backend selection.
//!
//! Pack execution defaults to the rh AOT backend. Legacy `AGENTERM_SCRIPT_BACKEND=rhai`
//! and `.rhai` entry paths are retired and normalize to `rh`.
//!
//! Trait-M4 (`plan/design-script-engine-trait.md` §4) folded the lua and
//! qjs engine-specific invocation logic into `script_engine.rs`'s
//! `LuaEngineBackend`/`QjsEngineBackend`; this module kept only
//! `try_execute_rh_invocation` (and its `RhInvocationOptions`/
//! `RhInvocationResult` types) because `crates/agenterm-rh/src/main.rs`
//! (the `agenterm-rh` bin target of this root package, per `Cargo.toml`)
//! calls it directly and depends on its typed `agenterm_rh::RhError`
//! return — see `script_engine.rs`'s module doc for the full rationale.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use serde_json::Value;

use crate::script_protocol::{ScriptBudgets, ScriptOperation};
use crate::script_rh_run::RhRunContext;

/// Active script backend for pack execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptBackend {
    Rh,
    #[cfg(feature = "script-lua")]
    Lua,
    #[cfg(feature = "script-sql")]
    Sql,
    /// AgenTerm's own engine: `.qjs` compiled to `.wasm` in pure Rust, both run
    /// on tinyvm with no JIT. It is the only engine left that runs wasm: the
    /// wasmtime + WASI p1 one, `Wasmcore`, was archived on 2026-08-28 together
    /// with its crate, and `.wasm` routes here now. That trade is deliberate
    /// and priced -- see `prd/PRD_02_36_agenterm_qjswasm.md`.
    ///
    /// It is also what `qjs` now names. The rquickjs engine that used to
    /// answer to that name was archived once it had been replaced -- the
    /// three gates and their evidence are in
    /// `prd/PRD_02_36_agenterm_qjswasm.md`.
    #[cfg(feature = "script-qjswasm")]
    Qjswasm,
}

impl ScriptBackend {
    /// Every backend name the product has ever accepted, independent of which
    /// ones this build compiled in. `from_env`'s arms are `#[cfg]`-gated, so
    /// without this list a name belonging to an absent backend is
    /// indistinguishable from a typo -- and both were silently answered with
    /// rh, meaning a request for one language was served by another's
    /// transpiler. Add a name here in the same change that adds its arm.
    pub const ALL_BACKEND_NAMES: &'static [&'static str] = &[
        "rh", "rhai", "lua", "qjs", "qjswasm", "sql", "wasmcore", "wasm",
    ];

    /// What the environment asked for that this build cannot serve.
    ///
    /// `None` means the request is honourable: either the variable is unset
    /// (rh is the documented default) or it names a backend compiled in.
    /// `Some` carries the requested name and whether the product knows it at
    /// all, so the caller can tell "you need a build with this feature" from
    /// "no such backend" instead of running something else and reporting that
    /// thing's error.
    /// Resolve a backend name without touching the environment.
    ///
    /// `None` covers both "no such backend" and "this build did not compile it
    /// in" -- the arms are `#[cfg]`-gated, so the two are the same thing here.
    /// [`unavailable_for`](Self::unavailable_for) tells them apart.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "rh" | "rhai" => Some(Self::Rh),
            #[cfg(feature = "script-lua")]
            "lua" => Some(Self::Lua),
            #[cfg(feature = "script-sql")]
            "sql" => Some(Self::Sql),
            // `qjs` is a **deprecated spelling of `qjswasm`**, an alias pair
            // in this match rather than a new mechanism -- `rh` takes `rhai`
            // for the same reason a name outlives the thing it first named.
            //
            // `wasm` and `wasmcore` get **no arm**, and that asymmetry is
            // deliberate. They named `agenterm-wasmcore` (wasmtime + WASI p1,
            // Cranelift JIT) until it was archived on 2026-08-28. qjswasm is
            // not a stand-in for it: it takes script *text* and compiles it,
            // where those names took a *path* to an already-built module, so
            // aliasing them here would hand a `.wasm` file to a `.qjs`
            // compiler. They stay in `ALL_BACKEND_NAMES` and are answered
            // with an honest "compiled out", never silently served.
            //
            // It used to select `agenterm-qjs`, the rquickjs engine. That
            // engine is being retired (PRD 02.36 archive gate), and the two
            // are equivalent on the Fleet surface -- six agreements, zero
            // divergences, asserted through this very trait in
            // `script_engine::tests::gate_two_trait_equivalence`. They are
            // **not** equivalent on the language: the new one is a growing
            // subset, so a script using arrow functions, template literals,
            // `Math` or a closure over an outer local now fails. It fails
            // *loudly*, at compile time, with a named capability diagnostic --
            // that same test pins all four -- and `scripts/` ships no `.js`
            // task script for it to break, only the `fleet.js` binding library.
            //
            // Serving the old name is the kinder half of the trade: every
            // existing invocation keeps running, and only genuinely
            // out-of-subset source stops.
            #[cfg(feature = "script-qjswasm")]
            "qjs" | "qjswasm" => Some(Self::Qjswasm),
            _ => None,
        }
    }

    /// What a request cannot be served, given the raw requested value.
    ///
    /// Pure so it can be tested without the process-global environment, which
    /// parallel tests race on. `None` means the request is honourable: absent,
    /// blank, or a backend this build serves. `Some((name, known))` reports the
    /// normalized name and whether the product knows it at all, so the caller
    /// can say "rebuild with that feature" rather than "no such backend" --
    /// and above all so it does not answer by running a different language.
    pub fn unavailable_for(requested: Option<&str>) -> Option<(String, bool)> {
        let normalized = requested?.trim().to_ascii_lowercase();
        if normalized.is_empty() || Self::from_name(&normalized).is_some() {
            return None;
        }
        Some((
            normalized.clone(),
            Self::ALL_BACKEND_NAMES.contains(&normalized.as_str()),
        ))
    }

    /// [`unavailable_for`](Self::unavailable_for) against the live environment.
    pub fn unavailable_request() -> Option<(String, bool)> {
        Self::unavailable_for(std::env::var("AGENTERM_SCRIPT_BACKEND").ok().as_deref())
    }

    pub fn from_env() -> Self {
        // Unchanged behaviour: an unset, blank, unknown or not-compiled value
        // still resolves to rh, because rh is the documented default and many
        // callers depend on an infallible answer. What changed is that the
        // dispatch path now asks `unavailable_request` FIRST, so a request this
        // build cannot serve is refused by name instead of quietly arriving
        // here and being answered by rh.
        std::env::var("AGENTERM_SCRIPT_BACKEND")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .and_then(|value| Self::from_name(&value))
            .unwrap_or(Self::Rh)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rh => "rh",
            #[cfg(feature = "script-lua")]
            Self::Lua => "lua",
            #[cfg(feature = "script-sql")]
            Self::Sql => "sql",
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm => "qjswasm",
        }
    }

    /// **The one place a backend gets chosen for an invocation.**
    ///
    /// Precedence: an explicit `AGENTERM_SCRIPT_BACKEND` wins; failing that,
    /// the entry file's extension; failing that, rh. Explicit has to win --
    /// a filename quietly overriding someone's stated choice would be this
    /// function's own bug committed in the other direction.
    ///
    /// # Why this exists as a named function
    ///
    /// [`Self::from_entry_path`] had **zero callers in production code** until
    /// 2026-08-28: it was exercised only by tests, one of which is literally
    /// named `lua_task_entry_backend_selection` and commented "Verify
    /// path-based backend selection" while verifying nothing but the pure
    /// function. Routing was `AGENTERM_SCRIPT_BACKEND` and nothing else, so
    /// `agenterm cli script run t.qjs` landed on **rh** and reported rh's
    /// parse error for a JavaScript file. `.lua` did the same.
    ///
    /// Measured that day, before and after:
    ///
    /// ```text
    /// $ agenterm cli script run t.qjs      # before: rh parse error at `map`
    /// $ agenterm cli script run t.qjs      # after:  sum=3
    /// ```
    ///
    /// The repair is one function rather than a call added at each of the nine
    /// `from_env()` sites, because the defect was never a missing call -- it
    /// was that "which engine runs this" had no single answer to be wrong in
    /// one place. `script_worker::dispatch` asks this and nothing else.
    ///
    /// `label` is `ScriptInvocation::source_label`, which is the entry path for
    /// a file and `"stdin"` / `"eval"` / `"api"` otherwise -- those have no
    /// extension, fall to rh, and are exactly the cases where the caller must
    /// say what they want.
    pub fn resolve(label: &str) -> Self {
        match std::env::var("AGENTERM_SCRIPT_BACKEND") {
            Ok(name) if !name.trim().is_empty() => Self::from_env(),
            _ => Self::from_entry_path(label),
        }
    }

    /// Select backend from task entry file extension.
    ///
    /// `.qjs` is the QuickJS-family extension for agenterm's own engine, named
    /// so that it is not confused with Node/Bun `.js`.
    ///
    /// Note what this deliberately does NOT do: **`.wasm` routes nowhere.** It
    /// reached `Wasmcore` (wasmtime + WASI p1) until that crate was archived
    /// on 2026-08-28, and the tempting move was to point it at qjswasm, the
    /// one engine left that runs wasm. That would be wrong: this verb reads
    /// the entry file as UTF-8 script *text*, and qjswasm's input shape is
    /// `.qjs` source it compiles itself -- so a `.wasm` file would arrive at a
    /// compiler as if it were a program's text. Instead it falls through, the
    /// read fails as "not UTF-8", and `non_text_script_hint` says why in a
    /// sentence naming `.wasm` specifically. Loud beats convenient here.
    /// `.js`/`.mjs` route nowhere for the neighbouring reason: qjswasm's
    /// language is a growing subset, so a Node-shaped script must ask for it
    /// by name and fail loudly if it does not fit.
    pub fn from_entry_path(path: &str) -> Self {
        #[cfg(feature = "script-lua")]
        if path.ends_with(".lua") {
            return Self::Lua;
        }
        #[cfg(feature = "script-sql")]
        if path.ends_with(".sql") {
            return Self::Sql;
        }
        #[cfg(feature = "script-qjswasm")]
        if path.ends_with(".qjs") {
            return Self::Qjswasm;
        }
        if path.ends_with(".rh") {
            return Self::Rh;
        }
        if path.ends_with(".rhai") {
            return Self::Rh;
        }
        Self::Rh
    }
}

pub fn rh_backend_enabled() -> bool {
    matches!(ScriptBackend::from_env(), ScriptBackend::Rh)
}

#[derive(Clone, Debug, Default)]
pub struct RhInvocationOptions {
    pub project_root: Option<PathBuf>,
    pub arguments: Option<Value>,
    pub budgets: Option<ScriptBudgets>,
}

pub struct RhInvocationResult {
    pub stdout: String,
    pub value: Option<serde_json::Value>,
}

pub fn try_execute_rh_invocation(
    operation: ScriptOperation,
    source: &str,
    options: RhInvocationOptions,
    fleet_bridge: Option<crate::script_rh_host::FleetBridgeFn>,
) -> Result<Option<RhInvocationResult>, agenterm_rh::RhError> {
    if !rh_backend_enabled() {
        return Ok(None);
    }

    let output_limit = options.budgets.as_ref().map_or_else(
        || ScriptBudgets::default().output_bytes,
        |budgets| budgets.output_bytes,
    );
    let output_capture = Arc::new(crate::script_rh_run::RhOutputCapture::new(output_limit));
    let run_context = RhRunContext {
        project_root: options.project_root.clone(),
        arguments: options.arguments.clone(),
        budgets: options.budgets.clone(),
        output_capture: Some(Arc::clone(&output_capture)),
    };

    match operation {
        // Unreachable in practice: `script_worker.rs::execute_inner` short-circuits
        // `ScriptOperation::Api` before ever calling into this backend dispatch.
        // Kept only so this match stays exhaustive over `ScriptOperation`.
        ScriptOperation::Api => Ok(None),
        ScriptOperation::Check => {
            if !source.is_empty() {
                rh_check_with_project_validation(source, &options)?;
            } else if crate::script_rh_pack::cached_rh_pack().is_none() {
                return Err(agenterm_rh::RhError::Compile(
                    "AGENTERM_SCRIPT_BACKEND=rh requires AGENTERM_RH_PACK or non-empty source"
                        .into(),
                ));
            }
            Ok(Some(RhInvocationResult {
                stdout: String::new(),
                value: None,
            }))
        }
        ScriptOperation::Run | ScriptOperation::Eval => {
            // Deliberately NOT wrapping a bare expression in `fn entry()` here.
            // The client command boundary adapts `script eval '1 + 1'` before
            // dispatch. This function is also reached through
            // `ScriptEngineBackend::execute`, whose fail-closed
            // behaviour for a source with no `entry` is a documented, tested
            // contract (`script_engine_exec_parity_execute_missing_entry_fails_closed`
            // asserts `execute("40 + 2")` errors). Wrapping here would make
            // that invalid program succeed.
            let (pack, native_path) = resolve_rh_pack(source, options.project_root.as_deref())?;
            let entry_result = crate::script_rh_host::call_pack_entry_with_host_result(
                &native_path,
                fleet_bridge,
                run_context,
            )?;
            let entry_value = entry_result.entry_value;
            let mut stdout = output_capture.finish()?;
            let eval_value = if operation == ScriptOperation::Eval {
                take_rh_eval_value(&mut stdout)?
            } else {
                None
            };
            for line in &pack.cc_lines {
                if stdout.len().saturating_add(line.len()).saturating_add(1) > output_limit {
                    return Err(agenterm_rh::RhError::Compile(
                        "rh invocation output exceeds its byte budget".into(),
                    ));
                }
                stdout.push_str(line);
                stdout.push('\n');
            }
            Ok(Some(RhInvocationResult {
                stdout,
                value: eval_value.or_else(|| match entry_result.host_value {
                    Some(crate::script_rh_host::RhHostEntryValue::Unit) => None,
                    Some(crate::script_rh_host::RhHostEntryValue::Value(value)) => Some(value),
                    None => json_value_from_entry(entry_value),
                }),
            }))
        }
    }
}

fn take_rh_eval_value(stdout: &mut String) -> Result<Option<Value>, agenterm_rh::RhError> {
    let marker = crate::script_protocol::RH_EVAL_VALUE_MARKER;
    let mut value = None;
    let mut retained = String::with_capacity(stdout.len());
    for segment in stdout.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(payload) = line.strip_prefix(marker) {
            if value.is_some() {
                return Err(agenterm_rh::RhError::Compile(
                    "rh eval emitted more than one typed value".into(),
                ));
            }
            value = Some(serde_json::from_str(payload).map_err(|error| {
                agenterm_rh::RhError::Compile(format!("rh eval value is invalid JSON: {error}"))
            })?);
        } else {
            retained.push_str(segment);
        }
    }
    *stdout = retained;
    Ok(value)
}

fn json_value_from_entry(entry_value: i64) -> Option<serde_json::Value> {
    Some(serde_json::Value::from(entry_value))
}

fn resolve_rh_pack(
    source: &str,
    project_root: Option<&std::path::Path>,
) -> Result<(crate::script_rh_pack::LoadedRhPack, std::path::PathBuf), agenterm_rh::RhError> {
    if let Some(pack) = crate::script_rh_pack::cached_rh_pack() {
        let native = crate::script_rh_pack::cached_native_path()
            .ok_or_else(|| {
                agenterm_rh::RhError::Compile("AGENTERM_RH_PACK native path is unavailable".into())
            })?
            .to_path_buf();
        return Ok((pack.clone(), native));
    }
    if !source.is_empty() {
        let pack =
            crate::script_rh_cache::loaded_pack_for_source_with_project(source, project_root)?;
        let native =
            crate::script_rh_cache::native_path_for_source_with_project(source, project_root)?;
        return Ok((pack, native));
    }
    Err(agenterm_rh::RhError::Compile(
        "AGENTERM_SCRIPT_BACKEND=rh requires AGENTERM_RH_PACK or non-empty rh source".into(),
    ))
}

pub fn rh_check(source: &str) -> Result<(), agenterm_rh::RhError> {
    agenterm_rh::check(source)
}

fn rh_check_with_project_validation(
    source: &str,
    options: &RhInvocationOptions,
) -> Result<(), agenterm_rh::RhError> {
    agenterm_rh::check_with_project_validation(source, options.project_root.as_deref())
}

pub fn rh_transpile(source: &str) -> Result<String, agenterm_rh::RhError> {
    agenterm_rh::transpile(source)
}

pub fn rh_compile(
    source: &str,
    output: &Path,
) -> Result<agenterm_rh::CompileOutput, agenterm_rh::RhError> {
    agenterm_rh::compile_native(source, output)
}

pub fn rh_run_smoke(native: &Path) -> Result<i64, agenterm_rh::RhError> {
    // Host callbacks registered: see the note on the `eval` arm in
    // `script_rh_cli_main.rs`. A hostless run now aborts by design.
    crate::script_rh_host::call_pack_entry_with_host_registration(native)
}

pub fn rh_load_pack(
    path: &Path,
) -> Result<crate::script_rh_pack::LoadedRhPack, agenterm_rh::RhError> {
    crate::script_rh_pack::load_rh_pack(path)
}

#[cfg(test)]
mod tests {

    /// Every name this build can actually serve must also be in
    /// `ALL_BACKEND_NAMES`, or a request for an absent backend becomes
    /// indistinguishable from a typo -- and both were silently answered by rh,
    /// which is how a request for one language came to be served by another's
    /// transpiler. The lists are separate because `from_name`'s arms are
    /// `#[cfg]`-gated and this one must not be; that is exactly why they drift.
    #[test]
    fn every_servable_name_is_listed() {
        for name in ScriptBackend::ALL_BACKEND_NAMES {
            if ScriptBackend::from_name(name).is_some() {
                assert!(
                    ScriptBackend::unavailable_for(Some(name)).is_none(),
                    "{name} is servable but reported unavailable"
                );
            } else {
                let (reported, known) = ScriptBackend::unavailable_for(Some(name))
                    .unwrap_or_else(|| panic!("{name} is not servable and must be reported"));
                assert_eq!(&reported, name);
                assert!(known, "{name} is in the product's own list, so it is known");
            }
        }
    }

    #[test]
    fn asking_for_nothing_is_not_a_failed_request() {
        // rh is the documented default; absent and blank are both "no request".
        assert!(ScriptBackend::unavailable_for(None).is_none());
        assert!(ScriptBackend::unavailable_for(Some("")).is_none());
        assert!(ScriptBackend::unavailable_for(Some("   ")).is_none());
    }

    use super::{
        RhInvocationOptions, ScriptBackend, rh_backend_enabled, take_rh_eval_value,
        try_execute_rh_invocation,
    };
    use crate::script_protocol::ScriptOperation;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn eval_value_marker_is_typed_and_removed_from_stdout() {
        let marker = crate::script_protocol::RH_EVAL_VALUE_MARKER;
        let mut stdout = format!("visible\n{marker}{{\"answer\":42}}\n");
        let value = take_rh_eval_value(&mut stdout)
            .expect("marker parses")
            .expect("typed value");
        assert_eq!(value["answer"], 42);
        assert_eq!(stdout, "visible\n");
    }

    #[test]
    fn default_backend_is_rh() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let prior = std::env::var("AGENTERM_SCRIPT_BACKEND").ok();
        unsafe {
            std::env::remove_var("AGENTERM_SCRIPT_BACKEND");
        }
        assert_eq!(ScriptBackend::from_env(), ScriptBackend::Rh);
        assert!(rh_backend_enabled());
        assert!(
            try_execute_rh_invocation(
                ScriptOperation::Check,
                "fn entry() { 1 }",
                RhInvocationOptions::default(),
                None,
            )
            .expect("probe")
            .is_some()
        );
        match prior {
            Some(value) => unsafe {
                std::env::set_var("AGENTERM_SCRIPT_BACKEND", value);
            },
            None => unsafe {
                std::env::remove_var("AGENTERM_SCRIPT_BACKEND");
            },
        }
    }

    #[test]
    #[cfg(feature = "script-lua")]
    fn lua_backend_from_env() {
        // Trait-M4: was mixed with a try_execute_lua_invocation check-path
        // probe and a lua_backend_enabled() assertion; both are now covered
        // in script_engine.rs (LuaEngineBackend::enabled /
        // lua_engine_check_valid_and_broken_source). This test stays
        // ScriptBackend-enum-routing-only.
        let _guard = ENV_LOCK.lock().expect("lock");
        let prior = std::env::var("AGENTERM_SCRIPT_BACKEND").ok();
        unsafe {
            std::env::set_var("AGENTERM_SCRIPT_BACKEND", "lua");
        }
        assert_eq!(ScriptBackend::from_env(), ScriptBackend::Lua);

        match prior {
            Some(value) => unsafe {
                std::env::set_var("AGENTERM_SCRIPT_BACKEND", value);
            },
            None => unsafe {
                std::env::remove_var("AGENTERM_SCRIPT_BACKEND");
            },
        }
    }

    #[test]
    #[cfg(feature = "script-lua")]
    fn lua_backend_from_entry_path() {
        assert_eq!(
            ScriptBackend::from_entry_path("scripts/lua/test.lua"),
            ScriptBackend::Lua
        );
        assert_eq!(ScriptBackend::from_entry_path("test.rh"), ScriptBackend::Rh);
        assert_eq!(
            ScriptBackend::from_entry_path("test.rhai"),
            ScriptBackend::Rh
        );
    }

    #[test]
    #[cfg(feature = "script-lua")]
    fn lua_backend_as_str() {
        assert_eq!(ScriptBackend::Lua.as_str(), "lua");
        assert_eq!(ScriptBackend::Rh.as_str(), "rh");
    }

    #[test]
    fn retired_rh_backend_env_defaults_to_rh() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let prior = std::env::var("AGENTERM_SCRIPT_BACKEND").ok();
        unsafe {
            std::env::set_var("AGENTERM_SCRIPT_BACKEND", "rhai");
        }
        assert_eq!(ScriptBackend::from_env(), ScriptBackend::Rh);
        assert!(rh_backend_enabled());
        match prior {
            Some(value) => unsafe {
                std::env::set_var("AGENTERM_SCRIPT_BACKEND", value);
            },
            None => unsafe {
                std::env::remove_var("AGENTERM_SCRIPT_BACKEND");
            },
        }
    }

    /// `qjs` is a **deprecated spelling of `qjswasm`**, not its own backend.
    ///
    /// It selected `agenterm-qjs` until 2026-08-26, when PRD 02.36's archive
    /// gate 2 moved the three production call sites off that engine; that
    /// crate was removed once all three gates were green. The name keeps
    /// working -- an alias pair in `from_name`, beside `rh`/`rhai` and
    /// `wasm`/`qjswasm` -- so no existing invocation breaks on the removal.
    ///
    /// This is the test that says an old invocation still runs, which is the
    /// whole reason the spelling was kept rather than retired with the crate.
    #[cfg(feature = "script-qjswasm")]
    #[test]
    fn qjs_backend_from_env() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let prior = std::env::var("AGENTERM_SCRIPT_BACKEND").ok();
        unsafe {
            std::env::set_var("AGENTERM_SCRIPT_BACKEND", "qjs");
        }
        assert_eq!(
            ScriptBackend::from_env(),
            ScriptBackend::Qjswasm,
            "`qjs` must resolve to the engine that replaced it"
        );

        match prior {
            Some(value) => unsafe {
                std::env::set_var("AGENTERM_SCRIPT_BACKEND", value);
            },
            None => unsafe {
                std::env::remove_var("AGENTERM_SCRIPT_BACKEND");
            },
        }
    }

    /// `.js` and `.mjs` no longer route anywhere by extension.
    ///
    /// They selected the rquickjs engine until it was archived. Nothing
    /// inherited the extensions: `qjswasm` compiles `.qjs`, and whether an
    /// extension should pick an engine at all is an open product question
    /// recorded in PRD 02.36 -- routing is environment-only today. So a `.js`
    /// path falls to the default, and this test says so rather than leaving
    /// the silence to be discovered.
    #[test]
    fn a_js_path_no_longer_selects_an_engine_by_extension() {
        assert_eq!(
            ScriptBackend::from_entry_path("scripts/qjs/test.js"),
            ScriptBackend::Rh
        );
        assert_eq!(
            ScriptBackend::from_entry_path("scripts/qjs/test.mjs"),
            ScriptBackend::Rh
        );
        #[cfg(feature = "script-lua")]
        assert_eq!(
            ScriptBackend::from_entry_path("test.lua"),
            ScriptBackend::Lua
        );
    }

    #[test]
    #[cfg(feature = "script-sql")]
    fn sql_backend_from_env() {
        // Mirrors qjs_backend_from_env: ScriptBackend-enum-routing-only, no
        // enabled()/check-path probe here (sql has no such probe yet — its
        // check/execute story lives in script_engine.rs's SqlEngineBackend
        // tests, once that exists).
        let _guard = ENV_LOCK.lock().expect("lock");
        let prior = std::env::var("AGENTERM_SCRIPT_BACKEND").ok();
        unsafe {
            std::env::set_var("AGENTERM_SCRIPT_BACKEND", "sql");
        }
        assert_eq!(ScriptBackend::from_env(), ScriptBackend::Sql);

        match prior {
            Some(value) => unsafe {
                std::env::set_var("AGENTERM_SCRIPT_BACKEND", value);
            },
            None => unsafe {
                std::env::remove_var("AGENTERM_SCRIPT_BACKEND");
            },
        }
    }

    #[test]
    #[cfg(feature = "script-sql")]
    fn sql_backend_from_entry_path() {
        assert_eq!(
            ScriptBackend::from_entry_path("scripts/sql/test.sql"),
            ScriptBackend::Sql
        );
        // `.js` stopped selecting an engine when the rquickjs one was
        // archived; nothing inherited the extension.
        assert_eq!(
            ScriptBackend::from_entry_path("test.js"),
            ScriptBackend::Rh
        );
    }

    #[test]
    #[cfg(feature = "script-sql")]
    fn sql_backend_as_str() {
        assert_eq!(ScriptBackend::Sql.as_str(), "sql");
    }

    /// The archived engine's two names stay **known but unserved**, and that
    /// is the whole job of `ALL_BACKEND_NAMES`.
    ///
    /// `wasm` and `wasmcore` selected `agenterm-wasmcore` (wasmtime + WASI p1,
    /// Cranelift JIT) until it was archived on 2026-08-28. The tempting move
    /// was to alias them onto qjswasm, the one engine left that runs wasm --
    /// and it would have been a silent substitution of a different thing: an
    /// interpreter with no POSIX surface, reached through a door that takes
    /// script text where these names took a path to a built module.
    ///
    /// So they resolve to `None`, which `unservable_request` turns into an
    /// honest "compiled out" rather than the rh fallback that a name absent
    /// from this list would get. A request for one language is never answered
    /// by another's transpiler -- the failure this list was added to stop.
    #[test]
    fn the_archived_engines_names_are_refused_rather_than_substituted() {
        for name in ["wasm", "wasmcore"] {
            assert!(
                ScriptBackend::ALL_BACKEND_NAMES.contains(&name),
                "{name} must stay known so it is not mistaken for a typo"
            );
            assert_eq!(
                ScriptBackend::from_name(name),
                None,
                "{name} must not be silently served by another engine"
            );
        }
    }

    /// `.wasm` falls through rather than landing on qjswasm.
    #[test]
    fn a_wasm_entry_path_routes_nowhere_now_that_its_engine_is_archived() {
        assert_eq!(
            ScriptBackend::from_entry_path("scripts/wasm/test.wasm"),
            ScriptBackend::Rh,
            "handing a built module to a source compiler would be worse than \
             the loud not-UTF-8 failure this produces"
        );
        assert_eq!(ScriptBackend::from_entry_path("test.rh"), ScriptBackend::Rh);
    }
}
