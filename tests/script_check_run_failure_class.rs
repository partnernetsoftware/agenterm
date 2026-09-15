//! `script check` and `script run` must classify the same source-build failure
//! the same way through the public CLI.

#![cfg(feature = "script-qjswasm")]

use std::process::{Command, Output};

const AGENTERM_BIN: &str = env!("CARGO_BIN_EXE_agenterm");

fn invoke(verb: &str, path: &std::path::Path) -> Output {
    Command::new(AGENTERM_BIN)
        .args(["cli", "script", verb])
        .arg(path)
        .env_remove("AGENTERM_SCRIPT_BACKEND")
        .output()
        .expect("the CLI binary runs")
}

fn assert_script_failure(label: &str, output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{label}: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains("\"exit_class\":\"script\""),
        "{label}: stdout={stdout} stderr={stderr}"
    );
}

#[test]
fn check_and_run_agree_on_source_build_failure_class() {
    let dir = tempfile::tempdir().expect("tempdir");
    let syntax = dir.path().join("syntax.qjs");
    let missing_import = dir.path().join("missing-import.qjs");
    std::fs::write(&syntax, "return @;\n").expect("write syntax fixture");
    std::fs::write(
        &missing_import,
        "import { value } from \"./missing.qjs\";\nreturn value;\n",
    )
    .expect("write import fixture");

    for (name, path) in [("syntax", syntax), ("missing import", missing_import)] {
        assert_script_failure(&format!("check {name}"), &invoke("check", &path));
        assert_script_failure(&format!("run {name}"), &invoke("run", &path));
    }
}
