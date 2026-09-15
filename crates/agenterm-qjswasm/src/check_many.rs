//! Bounded multi-file qjswasm validation for repository gates.
//!
//! The shared driver owns manifest limits, path confinement, deadlines and
//! report shape. This wrapper supplies qjswasm's manifest identity and checks
//! every entry through the tool door, with imports rooted first beside the
//! entry and then at the declared project root.

use std::path::Path;
use std::time::{Duration, Instant};

use agenterm_script_common::check_many::{self, CheckFailure};

use crate::module_resolver::{ResolverFailure, ResolverLedger, ResolverLedgerConfig};

pub use agenterm_script_common::check_many::{
    CheckManyManifest, CheckManyOptions, CheckManyReport, ParsedCheckManyCli,
};
// The resolver and its ledger moved to their own module so every door that
// resolves modules shares one implementation instead of a lookalike. Both
// names keep their `check_many` path for callers that already wrote it.
pub use crate::module_resolver::{BuiltinModuleResolver, IMPORT_MODULES_MAX};

pub const QJS_CHECK_MANIFEST_KIND: &str = "agenterm-qjs-check-manifest";

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
    // One ledger for the whole manifest, which is what makes the aggregate
    // charge and the canonical cache aggregate: a module imported by two
    // entries is read and charged once, and every entry's bytes land in the
    // same total. `check-many` is the label the wall-time sentence carries.
    let ledger = ResolverLedger::new(ResolverLedgerConfig::new(
        "check-many",
        deadline,
        options.source_bytes,
        check_many::TOTAL_SOURCE_MAX_BYTES,
    ));
    check_many::run_check_many(
        manifest,
        options,
        "agenterm-qjswasm-check-many",
        |source, path, root| {
            // An entry is not resolved, so its own bytes are charged here.
            if let Err(failure) = ledger.charge_entry_bytes(source.len()) {
                return Err(check_failure(failure));
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
            let resolve = ledger.resolver(&roots, builtin);
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
            // Taking rather than reading clears the record for the next entry:
            // the compiler's own sentence for a specifier it could not resolve
            // reads the same whether the cause was a budget or an absent file,
            // and only this record tells them apart.
            if let Some(failure) = ledger.take_failure() {
                return Err(check_failure(failure));
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

/// The shared ledger's refusal in the report's own failure shape.
fn check_failure(failure: ResolverFailure) -> CheckFailure {
    CheckFailure::new(failure.code, failure.message, failure.category)
}

pub fn parse_check_many_cli<I>(args: I) -> Result<ParsedCheckManyCli, String>
where
    I: Iterator<Item = String>,
{
    agenterm_script_common::cli::parse_check_many_cli(args)
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
}
