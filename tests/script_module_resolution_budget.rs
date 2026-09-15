//! Recursive imports obey the same source ceiling through single-file check,
//! run, and artifact hashing.
//!
//! The entry is deliberately small enough to pass the selected ceiling while
//! its imported module is larger. Before the shared resolver ledger, all three
//! doors bounded only the entry and silently read the larger module.

#![cfg(feature = "script-qjswasm")]

use std::path::Path;
use std::process::{Command, Output};

const AGENTERM_BIN: &str = env!("CARGO_BIN_EXE_agenterm");

static CLI_SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn invoke(dir: &Path, verb: &str, entry: &Path) -> Output {
    Command::new(AGENTERM_BIN)
        .args(["cli", "script", verb, "--max-source-bytes", "128"])
        .arg(entry)
        .current_dir(dir)
        .env_remove("AGENTERM_SCRIPT_BACKEND")
        .output()
        .expect("the CLI binary runs")
}

#[test]
fn imported_source_ceiling_is_shared_by_check_run_and_hash() {
    let _slot = CLI_SLOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("lib")).expect("module dir");
    let entry = dir.path().join("main.qjs");
    std::fs::write(
        &entry,
        "import * as big from \"lib/big\";\nreturn big.value();\n",
    )
    .expect("entry fixture");
    std::fs::write(
        dir.path().join("lib/big.qjs"),
        format!(
            "export function value() {{ return \"{}\"; }}\n",
            "x".repeat(256)
        ),
    )
    .expect("import fixture");

    for verb in ["check", "run"] {
        let output = invoke(dir.path(), verb, &entry);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(3),
            "{verb}: stdout={stdout} stderr={stderr}"
        );
        assert!(
            stderr.contains("\"code\":\"limit_import_source_bytes\""),
            "{verb}: stdout={stdout} stderr={stderr}"
        );
        assert!(
            stderr.contains("\"exit_class\":\"limit\""),
            "{verb}: stdout={stdout} stderr={stderr}"
        );
        assert!(
            stderr.contains("per-source limit of 128 bytes"),
            "{verb}: stdout={stdout} stderr={stderr}"
        );
    }

    let output = invoke(dir.path(), "hash", &entry);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(3),
        "hash: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains("limit_import_source_bytes:"),
        "hash: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains("per-source limit of 128 bytes"),
        "hash: stdout={stdout} stderr={stderr}"
    );
    assert!(stdout.trim().is_empty(), "hash produced a digest: {stdout}");
}
