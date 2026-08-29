//! Directory scanning: recursively find `.qjs` files, check each, produce a
//! report.
//!
//! A thin wrapper over the shared driver
//! (`agenterm_script_common::corpus_scan`), the same shape `agenterm-lua`'s
//! and `agenterm-qjs`'s wrappers are. This module's own job is two decisions:
//! which extensions, and what "check" means.
//!
//! # Only `.qjs`, not `.wasm`
//!
//! This engine runs both, and only one of them is *scannable*. A corpus scan
//! reads a file as text and asks the compiler whether it would accept it; a
//! `.wasm` has no compiler to ask -- the equivalent question for one is the
//! load gate, which takes bytes and is [`crate::validate_wasm`]. Folding both
//! into one verb would mean a report whose rows mean two different things,
//! and a green scan that never opened half the corpus.

use std::path::Path;

pub use agenterm_script_common::corpus_scan::{CorpusScanReport, FailedFile};

/// Scan a directory recursively for `.qjs` files and check each one.
///
/// "Check" is [`crate::check_qjs`], which is compile **plus this crate's load
/// gate** -- not compile alone. That matters for a corpus scan more than
/// anywhere else: a file that compiles to a module the engine would then
/// refuse to load is not a file that passes, and a scan that said otherwise
/// would be green about a corpus that cannot run.
pub fn scan_directory(dir: &Path) -> Result<CorpusScanReport, String> {
    agenterm_script_common::corpus_scan::scan_directory(dir, &["qjs"], |source, _label| {
        crate::check_qjs(source).map_err(|e| e.to_string())
    })
}

/// [`scan_directory`] for a corpus whose entries `import` libraries and
/// call the tool door: each file is checked through the tool door's
/// declarations (a scan asks "does this compile", not "may this run", and
/// the tool set is the superset) with the embedder's resolver. The product
/// passes a resolver rooted at `dir`, which is where the libraries sit.
pub fn scan_directory_with(
    dir: &Path,
    resolve: &dyn Fn(&str) -> Option<String>,
) -> Result<CorpusScanReport, String> {
    let budget = crate::Budget::default();
    agenterm_script_common::corpus_scan::scan_directory(dir, &["qjs"], |source, label| {
        // A library is a file with a top-level `export`. Compiled as an entry
        // it is refused -- correctly: nothing imports an entry -- so it is
        // checked the way it is used, through a one-line importer resolved
        // against the same root. `label` is the file's path as walked.
        let specifier = std::path::Path::new(label)
            .strip_prefix(dir)
            .ok()
            .map(|rel| rel.with_extension(""))
            .map(|rel| rel.to_string_lossy().replace('\\', "/"));
        let is_library = source.lines().any(|line| line.starts_with("export "));
        let checked = match (is_library, specifier) {
            (true, Some(specifier)) => {
                let importer = format!("import * as lib from \"{specifier}\"; return typeof lib;");
                crate::check_qjs_tool_with_modules(&importer, resolve, &budget)
            }
            _ => crate::check_qjs_tool_with_modules(source, resolve, &budget),
        };
        checked.map_err(|e| e.to_string())
    })
}

#[cfg(test)]
mod tests {
    use agenterm_script_common::test_support::CorpusScanContract;

    use super::*;

    /// The scenarios live in script-common's `test_support` -- they are the
    /// shared contract every engine's corpus-scan wrapper satisfies. This
    /// block supplies only what is `.qjs`-specific.
    ///
    /// `bad` is a template literal rather than gibberish: it is *syntactically
    /// fine JavaScript* that this engine does not lower yet, which is the
    /// failure a real corpus actually contains. Gibberish would only prove the
    /// lexer runs.
    const CONTRACT: CorpusScanContract<'_> = CorpusScanContract {
        good_a: ("a.qjs", "return 42;"),
        good_b: ("b.qjs", "return 1 + 1;"),
        // An elision, not a template: templates landed with `653cebe`, and
        // this row wants a refusal that a language bump will not overtake.
        // A hole is not an `undefined` and this engine cannot tell them
        // apart, so `[1, , 2]` is refused by name and will stay refused.
        bad: ("bad.qjs", "return [1, , 2];"),
    };

    #[test]
    fn scan_empty_dir() {
        CONTRACT.assert_empty_dir(&scan_directory);
    }

    #[test]
    fn scan_all_green() {
        CONTRACT.assert_all_green(&scan_directory);
    }

    #[test]
    fn scan_reports_the_unlowerable_file() {
        CONTRACT.assert_syntax_error_reported(&scan_directory);
    }

    #[test]
    fn scan_ignores_foreign_files() {
        CONTRACT.assert_ignores_foreign_files(&scan_directory);
    }

    #[test]
    fn scan_recurses_into_subdirectories() {
        CONTRACT.assert_recurses_into_subdirectories(&scan_directory);
    }
}
