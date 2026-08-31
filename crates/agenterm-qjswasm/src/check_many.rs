//! Bounded multi-file qjswasm validation for repository gates.
//!
//! The shared driver owns manifest limits, path confinement, deadlines and
//! report shape. This wrapper supplies qjswasm's manifest identity and checks
//! every entry through the tool door, with imports rooted first beside the
//! entry and then at the declared project root.

use std::path::{Path, PathBuf};

use agenterm_script_common::check_many::{self, CheckFailure};

pub use agenterm_script_common::check_many::{
    CheckManyManifest, CheckManyOptions, CheckManyReport, ParsedCheckManyCli,
};

pub const QJS_CHECK_MANIFEST_KIND: &str = "agenterm-qjs-check-manifest";

pub fn read_manifest(path: &Path) -> Result<CheckManyManifest, String> {
    check_many::read_manifest(path, &[QJS_CHECK_MANIFEST_KIND])
}

pub fn run_check_many(manifest: CheckManyManifest, options: CheckManyOptions) -> CheckManyReport {
    check_many::run_check_many(
        manifest,
        options,
        "agenterm-qjswasm-check-many",
        |source, path, root| {
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
            let resolve = resolver(&roots);
            let is_library = source.lines().any(|line| line.starts_with("export "));
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
            crate::check_qjs_tool_with_modules(checked_source, &resolve, &crate::Budget::default())
                .map_err(|error| CheckFailure::new("qjs_check", error.to_string(), "script"))
        },
    )
}

pub fn parse_check_many_cli<I>(args: I) -> Result<ParsedCheckManyCli, String>
where
    I: Iterator<Item = String>,
{
    agenterm_script_common::cli::parse_check_many_cli(args)
}

fn resolver(roots: &[PathBuf]) -> impl Fn(&str) -> Option<String> + use<> {
    let mut canonical = Vec::new();
    for root in roots {
        if let Ok(root) = root.canonicalize()
            && !canonical.contains(&root)
        {
            canonical.push(root);
        }
    }
    move |specifier: &str| {
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
}
