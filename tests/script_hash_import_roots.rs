//! `script hash` must resolve a recursive import for every spelling of the
//! same entry, exactly as `script check` and `script run` already do.
//!
//! # The defect this file pins
//!
//! `hash` built its module roots as `Path::new(path).parent()` on the **raw**
//! command-line literal, while `check`/`run` canonicalize their entry before
//! asking for a context. `Path::parent` of a bare relative filename answers an
//! **empty path**, not `None`; an empty root cannot be canonicalized, so the
//! root set came out empty and every import was refused. The program that
//! `check main.qjs` and `run main.qjs` both accepted was therefore reported by
//! `hash main.qjs` as unbuildable, and the provenance path answered "no
//! runnable artifact" for a program the runtime loads.
//!
//! # Why the spellings are the assertion, not an extra
//!
//! The three spellings name the same file, so they must produce the same
//! digest and the same acceptance. A test that used one canonical absolute
//! path would have passed before the repair, which is how the defect survived
//! an existing import court in `script_entry_extension_routing.rs`: every case
//! there is written with an absolute entry. This file is the black-box
//! counterpart of the PRD line that says an import-bearing source uses the
//! same entry/project roots as single-file `check`.
//!
//! The import graph is deliberately two levels deep, because a shallow graph
//! would not distinguish "the entry directory was reached" from "the whole
//! closure was reached".

use std::path::Path;
use std::process::Command;

const AGENTERM_BIN: &str = env!("CARGO_BIN_EXE_agenterm");

/// The supervisor caps live workers at `GLOBAL_CONCURRENCY_LIMIT` (8) for the
/// whole machine, so one CLI at a time keeps a refusal that has nothing to do
/// with this contract from arriving as `host_concurrency_limit`.
static CLI_SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn cli_slot() -> std::sync::MutexGuard<'static, ()> {
    CLI_SLOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Run `agenterm cli script <verb>` from `dir`, passing `spelling` verbatim as
/// the operand. Returns `(stdout, stderr, exit code)`.
fn script_verb(dir: &Path, verb: &str, spelling: &str) -> (String, String, i32) {
    let mut command = Command::new(AGENTERM_BIN);
    command
        .args(["cli", "script", verb])
        .arg(spelling)
        .current_dir(dir)
        .env_remove("AGENTERM_SCRIPT_BACKEND");
    let output = command.output().expect("the CLI binary runs");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

/// The digest column of a successful `script hash` line.
fn digest_of(stdout: &str) -> String {
    stdout
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// A two-level import closure beside the entry:
/// `main.qjs` -> `lib/a.qjs` -> `lib/b.qjs`, answering `42`.
fn import_graph(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("agenterm-hash-roots-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("lib")).expect("temp dir");
    std::fs::write(
        dir.join("lib/b.qjs"),
        "export function base() { return 41; }\n",
    )
    .expect("fixture is writable");
    std::fs::write(
        dir.join("lib/a.qjs"),
        "import * as b from \"lib/b\";\nexport function answer() { return b.base() + 1; }\n",
    )
    .expect("fixture is writable");
    std::fs::write(
        dir.join("main.qjs"),
        "import * as a from \"lib/a\";\nreturn a.answer();\n",
    )
    .expect("fixture is writable");
    dir
}

/// All three spellings of one entry build, and build to the **same** module.
#[cfg(feature = "script-qjswasm")]
#[test]
fn hash_resolves_recursive_imports_for_every_spelling_of_one_entry() {
    let _slot = cli_slot();
    let dir = import_graph("spellings");
    let absolute = dir.join("main.qjs").display().to_string();

    let mut digests = Vec::new();
    for spelling in ["main.qjs", "./main.qjs", absolute.as_str()] {
        let (stdout, stderr, code) = script_verb(&dir, "hash", spelling);
        assert_eq!(
            code, 0,
            "hash {spelling} must resolve the import closure; stdout={stdout} stderr={stderr}"
        );
        let digest = digest_of(&stdout);
        assert!(
            digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit()),
            "hash {spelling} must print a sha256 digest; got {stdout}"
        );
        digests.push((spelling, digest));
    }

    let (first_spelling, first) = &digests[0];
    for (spelling, digest) in &digests[1..] {
        assert_eq!(
            digest, first,
            "{spelling} and {first_spelling} name the same file and must share one digest"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// The bare entry is one program, so `check` and `run` must accept it too --
/// otherwise "hash agrees with check" would be satisfiable by refusing
/// everywhere.
#[cfg(feature = "script-qjswasm")]
#[test]
fn check_and_run_accept_the_same_bare_entry_that_hash_now_hashes() {
    let _slot = cli_slot();
    let dir = import_graph("acceptance");

    let (stdout, stderr, code) = script_verb(&dir, "check", "main.qjs");
    assert_eq!(
        code, 0,
        "check main.qjs must accept the import closure; stdout={stdout} stderr={stderr}"
    );

    let (stdout, stderr, code) = script_verb(&dir, "run", "main.qjs");
    assert_eq!(
        code, 0,
        "run main.qjs must run the import closure; stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("42"),
        "the two-level import must evaluate; got stdout={stdout} stderr={stderr}"
    );

    let (stdout, stderr, code) = script_verb(&dir, "hash", "main.qjs");
    assert_eq!(
        code, 0,
        "hash main.qjs must agree with check and run; stdout={stdout} stderr={stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
