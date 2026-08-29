//! Script execution backend selection.
//!
//! **There is no default engine.** Until 2026-08-29 an unset
//! `AGENTERM_SCRIPT_BACKEND`, an unrecognised entry extension and a bare
//! `stdin` / `eval` / `api` label all fell to rh, which was compiled in
//! unconditionally. rh left this repository that day -- the crate and its
//! scripts live in `partnernetsoftware/rh` now -- and `.qjs` is the script
//! language here. Nothing inherited the fallback: a request that names no
//! engine, or names one this build cannot serve, is refused **by name**
//! ([`BackendRefusal`]) rather than answered by whichever engine happens to be
//! linked. A silent default was how a request for one language came to be
//! served by another's parser, and that is the failure this module exists to
//! stop.
//!
//! Every variant is behind a feature, so a build with none of them has an
//! empty enum. That is the honest shape: such a build has no script engine,
//! and every request is a refusal that says so.

/// Active script backend for pack execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptBackend {
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

/// Where the rh engine went, in the words every refusal that names it uses.
pub const RH_WHERE_NOW: &str = "the rh engine left this repository for partnernetsoftware/rh";

/// The sentence every "nothing selected" refusal ends with.
pub const SCRIPT_LANGUAGE_HINT: &str = ".qjs is the script language now";

/// Why a backend request could not be honoured, by name.
///
/// Four causes, each with its own sentence, because they send the reader to
/// four different places: nothing was asked for; the thing asked for has left
/// this repository; the thing asked for exists but this build did not compile
/// it in; the name is not a backend at all.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BackendRefusal {
    /// Nothing named an engine: the environment is unset and the entry label
    /// (a path, or `stdin` / `eval` / `api`) carries no routed extension.
    Unselected { label: Option<String> },
    /// A backend that left this repository. `where_now` says where.
    Retired {
        name: String,
        where_now: &'static str,
    },
    /// A name the product knows, but this build did not compile in.
    CompiledOut { name: String },
    /// Not a backend name at all.
    Unknown { name: String },
}

impl BackendRefusal {
    pub fn message(&self) -> String {
        match self {
            Self::Unselected { label } => {
                let servable = ScriptBackend::servable_names();
                let choose = if servable.is_empty() {
                    "this build compiles no script engine in; rebuild with \
                     --features script-qjswasm"
                        .to_owned()
                } else {
                    format!(
                        "name a .qjs file, or set AGENTERM_SCRIPT_BACKEND to one of {}",
                        servable.join(", ")
                    )
                };
                match label {
                    Some(label) => format!(
                        "no engine for this entry `{label}`; {SCRIPT_LANGUAGE_HINT} -- {choose}"
                    ),
                    None => {
                        format!("no script engine selected; {SCRIPT_LANGUAGE_HINT} -- {choose}")
                    }
                }
            }
            Self::Retired { name, where_now } => format!(
                "script backend {name} is gone: {where_now}, and {SCRIPT_LANGUAGE_HINT}. \
                 There is no default to fall back to"
            ),
            Self::CompiledOut { name } => format!(
                "script backend {name} is not compiled into this build; rebuild with its feature enabled"
            ),
            Self::Unknown { name } => format!(
                "unknown script backend {name}; expected one of {}",
                ScriptBackend::ALL_BACKEND_NAMES.join(", ")
            ),
        }
    }
}

impl std::fmt::Display for BackendRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl ScriptBackend {
    /// Every backend name the product still accepts, independent of which
    /// ones this build compiled in. `from_name`'s arms are `#[cfg]`-gated, so
    /// without this list a name belonging to an absent backend is
    /// indistinguishable from a typo. Add a name here in the same change that
    /// adds its arm. The retired names live in [`Self::RETIRED_BACKEND_NAMES`]
    /// instead, because "rebuild with the feature" is the wrong advice for
    /// them.
    pub const ALL_BACKEND_NAMES: &'static [&'static str] =
        &["lua", "qjs", "qjswasm", "sql", "wasmcore", "wasm"];

    /// Names of engines that left this repository, and where each went.
    ///
    /// `rh` and its former spelling `rhai` were the unconditional default
    /// until 2026-08-29. They are kept here so a caller with the old
    /// invocation is told where the engine went, not told the name is a typo.
    pub const RETIRED_BACKEND_NAMES: &'static [(&'static str, &'static str)] =
        &[("rh", RH_WHERE_NOW), ("rhai", RH_WHERE_NOW)];

    /// The backends this build can actually run, by canonical name.
    pub fn servable_names() -> Vec<&'static str> {
        Self::all().into_iter().map(Self::as_str).collect()
    }

    /// Every backend compiled into this build, in routing order.
    pub fn all() -> Vec<Self> {
        vec![
            #[cfg(feature = "script-lua")]
            Self::Lua,
            #[cfg(feature = "script-sql")]
            Self::Sql,
            #[cfg(feature = "script-qjswasm")]
            Self::Qjswasm,
        ]
    }

    /// Resolve a backend name without touching the environment.
    ///
    /// `None` covers "no such backend", "this build did not compile it in"
    /// and "it left" alike -- the arms are `#[cfg]`-gated, so the three are
    /// the same thing here. [`refusal_for`](Self::refusal_for) tells them
    /// apart.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            #[cfg(feature = "script-lua")]
            "lua" => Some(Self::Lua),
            #[cfg(feature = "script-sql")]
            "sql" => Some(Self::Sql),
            // `qjs` is a **deprecated spelling of `qjswasm`**, an alias pair
            // in this match rather than a new mechanism.
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
            // `qjs` used to select `agenterm-qjs`, the rquickjs engine. That
            // engine was retired (PRD 02.36 archive gate), and the two are
            // equivalent on the Fleet surface -- six agreements, zero
            // divergences. They are **not** equivalent on the language: the
            // new one is a growing subset, and out-of-subset source fails
            // *loudly*, at compile time, with a named capability diagnostic.
            #[cfg(feature = "script-qjswasm")]
            "qjs" | "qjswasm" => Some(Self::Qjswasm),
            _ => None,
        }
    }

    /// Why `requested` cannot be served, given the raw requested value.
    ///
    /// Pure so it can be tested without the process-global environment, which
    /// parallel tests race on. `None` means the request is honourable: a
    /// backend this build serves. An absent or blank value is
    /// [`BackendRefusal::Unselected`] -- it is *not* a request for a default,
    /// because there is none.
    pub fn refusal_for(requested: Option<&str>) -> Result<Self, BackendRefusal> {
        let normalized = requested
            .map(|value| value.trim().to_ascii_lowercase())
            .unwrap_or_default();
        if normalized.is_empty() {
            return Err(BackendRefusal::Unselected { label: None });
        }
        if let Some(backend) = Self::from_name(&normalized) {
            return Ok(backend);
        }
        if let Some((_, where_now)) = Self::RETIRED_BACKEND_NAMES
            .iter()
            .find(|(name, _)| *name == normalized)
        {
            return Err(BackendRefusal::Retired {
                name: normalized,
                where_now,
            });
        }
        if Self::ALL_BACKEND_NAMES.contains(&normalized.as_str()) {
            return Err(BackendRefusal::CompiledOut { name: normalized });
        }
        Err(BackendRefusal::Unknown { name: normalized })
    }

    /// [`refusal_for`](Self::refusal_for) against the live environment.
    ///
    /// This returned an infallible `Self` -- rh -- until 2026-08-29. Every
    /// caller that needs an engine without a file to route by (`eval`,
    /// `version`, `corpus-scan`) now gets the refusal and prints it.
    pub fn from_env() -> Result<Self, BackendRefusal> {
        Self::refusal_for(std::env::var("AGENTERM_SCRIPT_BACKEND").ok().as_deref())
    }

    pub fn as_str(self) -> &'static str {
        match self {
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
    /// the entry file's extension; failing that, a refusal that names the
    /// entry. Explicit has to win -- a filename quietly overriding someone's
    /// stated choice would be this function's own bug committed in the other
    /// direction. And explicit-but-unservable has to refuse rather than fall
    /// through to the extension, or `AGENTERM_SCRIPT_BACKEND=rh t.qjs` would
    /// run on qjswasm while the caller believes rh ran it.
    ///
    /// # Why this exists as a named function
    ///
    /// [`Self::from_entry_path`] had **zero callers in production code** until
    /// 2026-08-28: routing was `AGENTERM_SCRIPT_BACKEND` and nothing else, so
    /// `agenterm cli script run t.qjs` landed on rh and reported rh's parse
    /// error for a JavaScript file. The repair is one function rather than a
    /// call added at each `from_env()` site, because the defect was never a
    /// missing call -- it was that "which engine runs this" had no single
    /// answer to be wrong in one place. `script_worker::dispatch` asks this
    /// and nothing else.
    ///
    /// `label` is `ScriptInvocation::source_label`, which is the entry path for
    /// a file and `"stdin"` / `"eval"` / `"api"` otherwise -- those have no
    /// extension and are exactly the cases where the caller must say what
    /// they want.
    pub fn resolve(label: &str) -> Result<Self, BackendRefusal> {
        match std::env::var("AGENTERM_SCRIPT_BACKEND") {
            Ok(name) if !name.trim().is_empty() => Self::refusal_for(Some(&name)),
            _ => Self::from_entry_path(label).ok_or_else(|| BackendRefusal::Unselected {
                label: Some(label.to_owned()),
            }),
        }
    }

    /// Select backend from task entry file extension.
    ///
    /// `.qjs` is the QuickJS-family extension for agenterm's own engine, named
    /// so that it is not confused with Node/Bun `.js`.
    ///
    /// `None` is the answer for everything else, and it is a real answer:
    /// `resolve` turns it into a refusal that names the entry. Note what this
    /// deliberately does NOT do: **`.wasm` routes nowhere.** It reached
    /// `Wasmcore` (wasmtime + WASI p1) until that crate was archived on
    /// 2026-08-28, and the tempting move was to point it at qjswasm, the one
    /// engine left that runs wasm. That would be wrong: this verb reads the
    /// entry file as UTF-8 script *text*, and qjswasm's input shape is `.qjs`
    /// source it compiles itself -- so a `.wasm` file would arrive at a
    /// compiler as if it were a program's text. `.js`/`.mjs` route nowhere for
    /// the neighbouring reason: qjswasm's language is a growing subset, so a
    /// Node-shaped script must ask for it by name and fail loudly if it does
    /// not fit. `.rh`/`.rhai` route nowhere because their engine left: see
    /// [`RH_WHERE_NOW`].
    pub fn from_entry_path(path: &str) -> Option<Self> {
        #[cfg(feature = "script-lua")]
        if path.ends_with(".lua") {
            return Some(Self::Lua);
        }
        #[cfg(feature = "script-sql")]
        if path.ends_with(".sql") {
            return Some(Self::Sql);
        }
        #[cfg(feature = "script-qjswasm")]
        if path.ends_with(".qjs") {
            return Some(Self::Qjswasm);
        }
        let _ = path;
        None
    }
}

/// The one lock for `AGENTERM_SCRIPT_BACKEND` in the whole lib test binary.
/// `resolve` lets that variable beat the file extension, and every `#[test]`
/// in this crate runs in one process, so a test that sets it races every
/// test that resolves a label -- not only its neighbours in the same module.
/// Two module-local mutexes used to guard the writers and nothing guarded
/// the readers; `unit.rh` then resolved to `lua` under a parallel run and a
/// refusal test saw a success.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::{BackendRefusal, ENV_LOCK, RH_WHERE_NOW, SCRIPT_LANGUAGE_HINT, ScriptBackend};

    fn with_backend_env<T>(value: Option<&str>, run: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().expect("lock");
        let prior = std::env::var("AGENTERM_SCRIPT_BACKEND").ok();
        unsafe {
            match value {
                Some(value) => std::env::set_var("AGENTERM_SCRIPT_BACKEND", value),
                None => std::env::remove_var("AGENTERM_SCRIPT_BACKEND"),
            }
        }
        let result = run();
        unsafe {
            match prior {
                Some(value) => std::env::set_var("AGENTERM_SCRIPT_BACKEND", value),
                None => std::env::remove_var("AGENTERM_SCRIPT_BACKEND"),
            }
        }
        result
    }

    /// Every name this build can actually serve must also be in
    /// `ALL_BACKEND_NAMES`, or a request for an absent backend becomes
    /// indistinguishable from a typo. The lists are separate because
    /// `from_name`'s arms are `#[cfg]`-gated and this one must not be; that
    /// is exactly why they drift.
    #[test]
    fn every_servable_name_is_listed() {
        for name in ScriptBackend::ALL_BACKEND_NAMES {
            match ScriptBackend::refusal_for(Some(name)) {
                Ok(backend) => assert!(
                    ScriptBackend::from_name(name) == Some(backend),
                    "{name} is servable but reported unavailable"
                ),
                Err(BackendRefusal::CompiledOut { name: reported }) => {
                    assert_eq!(&reported, name);
                    assert!(ScriptBackend::from_name(name).is_none());
                }
                Err(other) => panic!("{name} is in the product's own list, got {other:?}"),
            }
        }
        for backend in ScriptBackend::all() {
            assert!(
                ScriptBackend::ALL_BACKEND_NAMES.contains(&backend.as_str()),
                "{backend:?} is servable and must be listed"
            );
        }
    }

    /// **There is no default.** Absent and blank are refusals that say so,
    /// and the sentence names the language that replaced the default.
    #[test]
    fn asking_for_nothing_is_refused_by_name() {
        for requested in [None, Some(""), Some("   ")] {
            let refusal = ScriptBackend::refusal_for(requested).expect_err("no default");
            assert_eq!(refusal, BackendRefusal::Unselected { label: None });
            let message = refusal.message();
            assert!(message.contains(SCRIPT_LANGUAGE_HINT), "{message}");
            assert!(
                message.starts_with("no script engine selected"),
                "{message}"
            );
        }
    }

    /// `rh` and `rhai` are not typos and not "rebuild with the feature": they
    /// are a departure, and the refusal says where to.
    #[test]
    fn the_retired_engines_names_say_where_it_went() {
        for name in ["rh", "rhai", "RH", " rhai "] {
            let refusal = ScriptBackend::refusal_for(Some(name)).expect_err("rh left");
            assert!(
                matches!(&refusal, BackendRefusal::Retired { where_now, .. } if *where_now == RH_WHERE_NOW),
                "{name}: {refusal:?}"
            );
            let message = refusal.message();
            assert!(message.contains("partnernetsoftware/rh"), "{message}");
            assert!(message.contains(SCRIPT_LANGUAGE_HINT), "{message}");
            assert!(!message.contains("rebuild"), "{message}");
            assert!(!message.contains("unknown"), "{message}");
        }
    }

    #[test]
    fn an_unknown_name_is_still_a_typo() {
        let refusal = ScriptBackend::refusal_for(Some("python")).expect_err("no such engine");
        assert_eq!(
            refusal,
            BackendRefusal::Unknown {
                name: "python".to_owned()
            }
        );
        assert!(refusal.message().contains("expected one of"));
    }

    /// The archived engine's two names stay **known but unserved**, and that
    /// is the whole job of `ALL_BACKEND_NAMES`.
    ///
    /// `wasm` and `wasmcore` selected `agenterm-wasmcore` until it was
    /// archived on 2026-08-28. Aliasing them onto qjswasm would have been a
    /// silent substitution of a different thing, so they resolve to `None`
    /// and are refused as "compiled out".
    #[test]
    fn the_archived_engines_names_are_refused_rather_than_substituted() {
        for name in ["wasm", "wasmcore"] {
            assert!(ScriptBackend::ALL_BACKEND_NAMES.contains(&name));
            assert_eq!(ScriptBackend::from_name(name), None);
            assert_eq!(
                ScriptBackend::refusal_for(Some(name)),
                Err(BackendRefusal::CompiledOut {
                    name: name.to_owned()
                })
            );
        }
    }

    /// `.rh`, `.rhai`, `.js`, `.wasm` and a bare label all route nowhere, and
    /// `resolve` turns "nowhere" into a refusal that names the entry.
    #[test]
    fn unrouted_entries_are_refused_with_the_entry_named() {
        for path in [
            "test.rh",
            "test.rhai",
            "scripts/qjs/test.js",
            "t.mjs",
            "m.wasm",
            "eval",
            "stdin",
        ] {
            assert_eq!(ScriptBackend::from_entry_path(path), None, "{path}");
        }
        with_backend_env(None, || {
            let refusal = ScriptBackend::resolve("test.rh").expect_err("no engine for .rh");
            assert_eq!(
                refusal,
                BackendRefusal::Unselected {
                    label: Some("test.rh".to_owned())
                }
            );
            let message = refusal.message();
            assert!(message.contains("`test.rh`"), "{message}");
            assert!(message.contains(SCRIPT_LANGUAGE_HINT), "{message}");
        });
    }

    /// An explicit name that cannot be served refuses; it does not fall
    /// through to the extension and run something else.
    #[test]
    fn an_explicit_unservable_backend_beats_a_routable_extension() {
        with_backend_env(Some("rh"), || {
            let refusal = ScriptBackend::resolve("t.qjs").expect_err("rh was asked for");
            assert!(
                matches!(refusal, BackendRefusal::Retired { .. }),
                "{refusal:?}"
            );
            assert!(matches!(
                ScriptBackend::from_env(),
                Err(BackendRefusal::Retired { .. })
            ));
        });
    }

    #[test]
    #[cfg(feature = "script-lua")]
    fn lua_backend_from_env_and_entry_path() {
        with_backend_env(Some("lua"), || {
            assert_eq!(ScriptBackend::from_env(), Ok(ScriptBackend::Lua));
            assert_eq!(
                ScriptBackend::resolve("anything.qjs"),
                Ok(ScriptBackend::Lua)
            );
        });
        assert_eq!(
            ScriptBackend::from_entry_path("scripts/lua/test.lua"),
            Some(ScriptBackend::Lua)
        );
        assert_eq!(ScriptBackend::Lua.as_str(), "lua");
    }

    /// `qjs` is a **deprecated spelling of `qjswasm`**, not its own backend.
    /// The name keeps working so no existing invocation breaks.
    #[cfg(feature = "script-qjswasm")]
    #[test]
    fn qjs_backend_from_env_and_entry_path() {
        with_backend_env(Some("qjs"), || {
            assert_eq!(
                ScriptBackend::from_env(),
                Ok(ScriptBackend::Qjswasm),
                "`qjs` must resolve to the engine that replaced it"
            );
        });
        with_backend_env(None, || {
            assert_eq!(ScriptBackend::resolve("t.qjs"), Ok(ScriptBackend::Qjswasm));
        });
        assert_eq!(ScriptBackend::Qjswasm.as_str(), "qjswasm");
    }

    #[test]
    #[cfg(feature = "script-sql")]
    fn sql_backend_from_env_and_entry_path() {
        with_backend_env(Some("sql"), || {
            assert_eq!(ScriptBackend::from_env(), Ok(ScriptBackend::Sql));
        });
        assert_eq!(
            ScriptBackend::from_entry_path("scripts/sql/test.sql"),
            Some(ScriptBackend::Sql)
        );
        assert_eq!(ScriptBackend::Sql.as_str(), "sql");
    }
}
