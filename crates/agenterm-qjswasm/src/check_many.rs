//! Bounded multi-file qjswasm validation for repository gates.
//!
//! The shared driver owns manifest limits, path confinement, deadlines and
//! report shape. This wrapper supplies qjswasm's manifest identity and checks
//! every entry through the tool door, with imports rooted first beside the
//! entry and then at the declared project root.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use agenterm_script_common::check_many::{self, CheckFailure};

pub use agenterm_script_common::check_many::{
    CheckManyManifest, CheckManyOptions, CheckManyReport, ParsedCheckManyCli,
};

pub const QJS_CHECK_MANIFEST_KIND: &str = "agenterm-qjs-check-manifest";
pub const IMPORT_MODULES_MAX: usize = 1_024;
pub type BuiltinModuleResolver = fn(&str) -> Option<&'static str>;

pub fn read_manifest(path: &Path) -> Result<CheckManyManifest, String> {
    check_many::read_manifest(path, &[QJS_CHECK_MANIFEST_KIND])
}

pub fn run_check_many(manifest: CheckManyManifest, options: CheckManyOptions) -> CheckManyReport {
    run_check_many_with_builtins(manifest, options, |_| None)
}

/// Check many entries with the product host's non-shadowable built-in module
/// set. The same source must be supplied by the runtime resolver; this hook
/// keeps compile-only validation honest without making qjswasm own product
/// module names.
pub fn run_check_many_with_builtins(
    manifest: CheckManyManifest,
    options: CheckManyOptions,
    builtin: BuiltinModuleResolver,
) -> CheckManyReport {
    let deadline = Instant::now() + Duration::from_millis(options.wall_time_ms);
    let per_source_max = options.source_bytes;
    let aggregate_source_max = check_many::TOTAL_SOURCE_MAX_BYTES;
    let mut compile_source_bytes = 0_usize;
    let mut imported_modules = 0_usize;
    let resolved_modules = Rc::new(RefCell::new(HashMap::<PathBuf, String>::new()));
    let resolved_builtins = Rc::new(RefCell::new(HashSet::<String>::new()));
    check_many::run_check_many(
        manifest,
        options,
        "agenterm-qjswasm-check-many",
        |source, path, root| {
            compile_source_bytes = compile_source_bytes.saturating_add(source.len());
            if compile_source_bytes > aggregate_source_max {
                return Err(CheckFailure::new(
                    "limit_import_source_bytes",
                    format!(
                        "entry and imported source exceeds aggregate limit of {aggregate_source_max} bytes"
                    ),
                    "limit",
                ));
            }
            let script_root = root.join("scripts/qjs");
            let module_root = if script_root.is_dir() {
                script_root
            } else {
                root.to_path_buf()
            };
            let roots = [
                path.parent().map(Path::to_path_buf),
                Some(module_root.clone()),
                Some(root.to_path_buf()),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            let resolver_state = Rc::new(RefCell::new(ResolverState {
                deadline,
                per_source_max,
                aggregate_source_max,
                compile_source_bytes,
                imported_modules,
                failure: None,
            }));
            let resolve = resolver(
                &roots,
                Rc::clone(&resolver_state),
                Rc::clone(&resolved_modules),
                Rc::clone(&resolved_builtins),
                builtin,
            );
            let is_library = source
                .lines()
                .any(|line| line.trim_start().starts_with("export "));
            let importer = path
                .strip_prefix(&module_root)
                .ok()
                .map(|relative| relative.with_extension(""))
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                .map(|specifier| {
                    format!("import * as lib from \"{specifier}\"; return typeof lib;")
                });
            let checked_source = if is_library {
                importer.as_deref().unwrap_or(source)
            } else {
                source
            };
            let checked = crate::check_qjs_tool_with_modules(
                checked_source,
                &resolve,
                &crate::Budget::default(),
            );
            let state = resolver_state.borrow();
            compile_source_bytes = state.compile_source_bytes;
            imported_modules = state.imported_modules;
            if let Some(failure) = &state.failure {
                return Err(failure.clone());
            }
            if Instant::now() >= deadline {
                return Err(CheckFailure::new(
                    "limit_wall_time",
                    "check-many reached its aggregate wall-time budget while compiling imports",
                    "limit",
                ));
            }
            checked.map_err(|error| CheckFailure::new("qjs_check", error.to_string(), "script"))
        },
    )
}

pub fn parse_check_many_cli<I>(args: I) -> Result<ParsedCheckManyCli, String>
where
    I: Iterator<Item = String>,
{
    agenterm_script_common::cli::parse_check_many_cli(args)
}

#[derive(Debug)]
struct ResolverState {
    deadline: Instant,
    per_source_max: usize,
    aggregate_source_max: usize,
    compile_source_bytes: usize,
    imported_modules: usize,
    failure: Option<CheckFailure>,
}

fn resolver(
    roots: &[PathBuf],
    state: Rc<RefCell<ResolverState>>,
    resolved_modules: Rc<RefCell<HashMap<PathBuf, String>>>,
    resolved_builtins: Rc<RefCell<HashSet<String>>>,
    builtin: BuiltinModuleResolver,
) -> impl Fn(&str) -> Option<String> + use<> {
    let mut canonical = Vec::new();
    for root in roots {
        if let Ok(root) = root.canonicalize()
            && !canonical.contains(&root)
        {
            canonical.push(root);
        }
    }
    move |specifier: &str| {
        if state.borrow().failure.is_some() {
            return None;
        }
        if Instant::now() >= state.borrow().deadline {
            state.borrow_mut().failure = Some(CheckFailure::new(
                "limit_wall_time",
                "check-many reached its aggregate wall-time budget while resolving imports",
                "limit",
            ));
            return None;
        }
        if let Some(source) = builtin(specifier) {
            if !resolved_builtins.borrow().contains(specifier) {
                let mut budget = state.borrow_mut();
                if source.len() > budget.per_source_max
                    || budget.compile_source_bytes.saturating_add(source.len())
                        > budget.aggregate_source_max
                {
                    budget.failure = Some(CheckFailure::new(
                        "limit_import_source_bytes",
                        format!("built-in module {specifier:?} exceeds the source budget"),
                        "limit",
                    ));
                    return None;
                }
                if budget.imported_modules >= IMPORT_MODULES_MAX {
                    budget.failure = Some(CheckFailure::new(
                        "limit_import_modules",
                        format!("recursive imports exceed {IMPORT_MODULES_MAX} resolved modules"),
                        "limit",
                    ));
                    return None;
                }
                budget.compile_source_bytes =
                    budget.compile_source_bytes.saturating_add(source.len());
                budget.imported_modules += 1;
                drop(budget);
                resolved_builtins.borrow_mut().insert(specifier.to_owned());
            }
            return Some(source.to_owned());
        }
        for root in &canonical {
            let mut candidate = root.join(specifier);
            if candidate.extension().is_none() {
                candidate.set_extension("qjs");
            }
            let Ok(resolved) = candidate.canonicalize() else {
                continue;
            };
            if !resolved.starts_with(root) {
                continue;
            }
            if let Some(source) = resolved_modules.borrow().get(&resolved).cloned() {
                return Some(source);
            }
            let metadata = match std::fs::metadata(&resolved) {
                Ok(metadata) if metadata.is_file() => metadata,
                Ok(_) => continue,
                Err(error) => {
                    state.borrow_mut().failure = Some(CheckFailure::new(
                        "host_import_read",
                        format!("cannot inspect imported module {specifier:?}: {error}"),
                        "host",
                    ));
                    return None;
                }
            };
            let source_len = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
            let mut budget = state.borrow_mut();
            if source_len > budget.per_source_max {
                budget.failure = Some(CheckFailure::new(
                    "limit_import_source_bytes",
                    format!(
                        "imported module {specifier:?} exceeds per-source limit of {} bytes",
                        budget.per_source_max
                    ),
                    "limit",
                ));
                return None;
            }
            if budget.imported_modules >= IMPORT_MODULES_MAX {
                budget.failure = Some(CheckFailure::new(
                    "limit_import_modules",
                    format!("recursive imports exceed {IMPORT_MODULES_MAX} resolved modules"),
                    "limit",
                ));
                return None;
            }
            let next_total = budget.compile_source_bytes.saturating_add(source_len);
            if next_total > budget.aggregate_source_max {
                budget.failure = Some(CheckFailure::new(
                    "limit_import_source_bytes",
                    format!(
                        "entry and imported source exceeds aggregate limit of {} bytes",
                        budget.aggregate_source_max
                    ),
                    "limit",
                ));
                return None;
            }
            drop(budget);
            let source = match std::fs::read_to_string(&resolved) {
                Ok(source) => source,
                Err(error) => {
                    state.borrow_mut().failure = Some(CheckFailure::new(
                        "host_import_read",
                        format!("cannot read imported module {specifier:?}: {error}"),
                        "host",
                    ));
                    return None;
                }
            };
            let mut budget = state.borrow_mut();
            let actual_len = source.len();
            if actual_len > budget.per_source_max
                || budget.compile_source_bytes.saturating_add(actual_len)
                    > budget.aggregate_source_max
            {
                budget.failure = Some(CheckFailure::new(
                    "limit_import_source_bytes",
                    format!(
                        "imported module {specifier:?} changed while reading and exceeds the source budget"
                    ),
                    "limit",
                ));
                return None;
            }
            budget.compile_source_bytes = budget.compile_source_bytes.saturating_add(actual_len);
            budget.imported_modules += 1;
            drop(budget);
            resolved_modules
                .borrow_mut()
                .insert(resolved, source.clone());
            return Some(source);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_multiple_entries_and_imports() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("lib")).unwrap();
        std::fs::write(dir.path().join("lib/value.qjs"), "export const value = 42;").unwrap();
        std::fs::write(
            dir.path().join("ok.qjs"),
            "import * as lib from \"lib/value\"; return lib.value;",
        )
        .unwrap();
        std::fs::write(dir.path().join("bad.qjs"), "let broken = ;").unwrap();
        let report = run_check_many(
            CheckManyManifest {
                schema_version: 1,
                kind: QJS_CHECK_MANIFEST_KIND.to_owned(),
                files: vec!["ok.qjs".to_owned(), "bad.qjs".to_owned()],
            },
            CheckManyOptions {
                project_root: dir.path().to_path_buf(),
                ..Default::default()
            },
        );
        assert!(!report.ok);
        assert_eq!(report.checked_files, 2);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].path, "bad.qjs");
        assert_eq!(report.failures[0].code, "qjs_check");
    }

    #[test]
    fn rejects_another_engines_manifest_kind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("manifest.json");
        std::fs::write(
            &path,
            r#"{"schema_version":1,"kind":"agenterm-lua-check-manifest","files":[]}"#,
        )
        .unwrap();
        let error = read_manifest(&path).expect_err("wrong kind");
        assert!(error.contains("schema"), "{error}");
    }

    #[test]
    fn imported_modules_share_the_per_source_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("large.qjs"),
            format!("export const value = \"{}\";", "x".repeat(256)),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("entry.qjs"),
            "import * as large from \"large\"; return large.value;",
        )
        .unwrap();
        let report = run_check_many(
            CheckManyManifest {
                schema_version: 1,
                kind: QJS_CHECK_MANIFEST_KIND.to_owned(),
                files: vec!["entry.qjs".to_owned()],
            },
            CheckManyOptions {
                project_root: dir.path().to_path_buf(),
                source_bytes: 128,
                ..Default::default()
            },
        );
        assert!(!report.ok);
        assert_eq!(report.failures[0].code, "limit_import_source_bytes");
        assert_eq!(report.failures[0].exit_class, "limit");
    }

    #[test]
    fn resolver_enforces_one_recursive_module_and_byte_ledger() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("one.qjs"), "export const one = 1;").unwrap();
        let root = dir.path().canonicalize().unwrap();
        let state = Rc::new(RefCell::new(ResolverState {
            deadline: Instant::now() + Duration::from_secs(1),
            per_source_max: 128,
            aggregate_source_max: 128,
            compile_source_bytes: 100,
            imported_modules: IMPORT_MODULES_MAX - 1,
            failure: None,
        }));
        let modules = Rc::new(RefCell::new(HashMap::new()));
        let resolve = resolver(
            &[root],
            Rc::clone(&state),
            Rc::clone(&modules),
            Rc::new(RefCell::new(HashSet::new())),
            |_| None,
        );
        assert_eq!(resolve("one"), Some("export const one = 1;".to_owned()));
        assert_eq!(state.borrow().imported_modules, IMPORT_MODULES_MAX);
        assert_eq!(resolve("one"), Some("export const one = 1;".to_owned()));
        assert_eq!(state.borrow().imported_modules, IMPORT_MODULES_MAX);
        std::fs::write(dir.path().join("two.qjs"), "export const two = 2;").unwrap();
        assert_eq!(resolve("two"), None);
        let failure = state.borrow().failure.clone().expect("typed limit");
        assert_eq!(failure.code, "limit_import_modules");
        assert_eq!(failure.exit_class, "limit");
    }

    #[test]
    fn resolver_refuses_import_work_after_the_shared_deadline() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("late.qjs"), "export const late = 1;").unwrap();
        let root = dir.path().canonicalize().unwrap();
        let state = Rc::new(RefCell::new(ResolverState {
            deadline: Instant::now(),
            per_source_max: 128,
            aggregate_source_max: 128,
            compile_source_bytes: 0,
            imported_modules: 0,
            failure: None,
        }));
        let resolve = resolver(
            &[root],
            Rc::clone(&state),
            Rc::new(RefCell::new(HashMap::new())),
            Rc::new(RefCell::new(HashSet::new())),
            |_| None,
        );
        assert_eq!(resolve("late"), None);
        let failure = state.borrow().failure.clone().expect("typed deadline");
        assert_eq!(failure.code, "limit_wall_time");
        assert_eq!(failure.exit_class, "limit");
    }

    #[test]
    fn indented_export_is_checked_as_a_library_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("library.qjs"),
            "  export const value = 42;\n",
        )
        .unwrap();
        let report = run_check_many(
            CheckManyManifest {
                schema_version: 1,
                kind: QJS_CHECK_MANIFEST_KIND.to_owned(),
                files: vec!["library.qjs".to_owned()],
            },
            CheckManyOptions {
                project_root: dir.path().to_path_buf(),
                ..Default::default()
            },
        );
        assert!(report.ok, "{report:?}");
    }

    #[test]
    fn product_builtin_is_checked_without_a_filesystem_shadow() {
        fn builtin(specifier: &str) -> Option<&'static str> {
            (specifier == "product-typed").then_some("export const value = 42;")
        }

        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("entry.qjs"),
            "import * as typed from \"product-typed\"; return typed.value;",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("product-typed.qjs"),
            "this filesystem shadow must not compile",
        )
        .unwrap();
        let report = run_check_many_with_builtins(
            CheckManyManifest {
                schema_version: 1,
                kind: QJS_CHECK_MANIFEST_KIND.to_owned(),
                files: vec!["entry.qjs".to_owned()],
            },
            CheckManyOptions {
                project_root: dir.path().to_path_buf(),
                ..Default::default()
            },
            builtin,
        );
        assert!(report.ok, "{report:?}");
    }

    #[test]
    fn builtin_is_counted_once_and_obeys_source_budget() {
        fn builtin(specifier: &str) -> Option<&'static str> {
            (specifier == "product-typed").then_some("export const value = 42;")
        }

        let state = Rc::new(RefCell::new(ResolverState {
            deadline: Instant::now() + Duration::from_secs(1),
            per_source_max: 1_024,
            aggregate_source_max: 1_024,
            compile_source_bytes: 10,
            imported_modules: 7,
            failure: None,
        }));
        let resolve = resolver(
            &[],
            Rc::clone(&state),
            Rc::new(RefCell::new(HashMap::new())),
            Rc::new(RefCell::new(HashSet::new())),
            builtin,
        );
        assert_eq!(
            resolve("product-typed"),
            Some("export const value = 42;".to_owned())
        );
        let first_bytes = state.borrow().compile_source_bytes;
        assert_eq!(state.borrow().imported_modules, 8);
        assert!(resolve("product-typed").is_some());
        assert_eq!(state.borrow().compile_source_bytes, first_bytes);
        assert_eq!(state.borrow().imported_modules, 8);

        let limited = Rc::new(RefCell::new(ResolverState {
            deadline: Instant::now() + Duration::from_secs(1),
            per_source_max: 4,
            aggregate_source_max: 1_024,
            compile_source_bytes: 0,
            imported_modules: 0,
            failure: None,
        }));
        let resolve = resolver(
            &[],
            Rc::clone(&limited),
            Rc::new(RefCell::new(HashMap::new())),
            Rc::new(RefCell::new(HashSet::new())),
            builtin,
        );
        assert_eq!(resolve("product-typed"), None);
        assert_eq!(
            limited
                .borrow()
                .failure
                .as_ref()
                .map(|failure| failure.code.as_str()),
            Some("limit_import_source_bytes")
        );
    }
}
