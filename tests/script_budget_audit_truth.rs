//! The public audit record distinguishes enforced budgets from the accepted
//! values the selected engine still names as unenforced.

#![cfg(feature = "script-qjswasm")]

use std::process::Command;

const AGENTERM_BIN: &str = env!("CARGO_BIN_EXE_agenterm");

#[test]
fn qjs_audit_credits_the_enforced_collection_budget() {
    let directory = tempfile::tempdir().expect("temporary audit directory");
    let audit_path = directory.path().join("script-audit.jsonl");
    let output = Command::new(AGENTERM_BIN)
        .args([
            "cli",
            "script",
            "eval",
            "[1, 2].length",
            "--max-collection-items",
            "2",
            "--json",
        ])
        .env("AGENTERM_SCRIPT_BACKEND", "qjswasm")
        .env("AGENTERM_SCRIPT_AUDIT_PATH", &audit_path)
        .env("AGENTERM_NO_ACTIVATE", "1")
        .output()
        .expect("agenterm CLI runs");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={stdout} stderr={stderr}"
    );
    let result: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("script result is JSON");
    assert_eq!(result["value"], 2);

    let records = std::fs::read_to_string(&audit_path).expect("public audit JSONL");
    let lines = records.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "one invocation emits one audit record");
    let record: serde_json::Value = serde_json::from_str(lines[0]).expect("audit record is JSON");
    assert_eq!(record["schema_version"], 2);
    assert_eq!(record["requested_budgets"]["collection_items"], 2);
    assert_eq!(record["effective_budgets"]["collection_items"], 2);
    assert_eq!(
        record["unenforced_budgets"],
        serde_json::json!(["expression_depth"])
    );
    assert_eq!(
        record["effective_budgets"]["host_operations"],
        record["requested_budgets"]["host_operations"]
    );
}

#[test]
fn qjs_cli_refuses_collection_limit_plus_one_by_name() {
    let directory = tempfile::tempdir().expect("temporary audit directory");
    let output = Command::new(AGENTERM_BIN)
        .args([
            "cli",
            "script",
            "eval",
            "[1, 2].length",
            "--max-collection-items",
            "1",
            "--json",
        ])
        .env("AGENTERM_SCRIPT_BACKEND", "qjswasm")
        .env(
            "AGENTERM_SCRIPT_AUDIT_PATH",
            directory.path().join("script-audit.jsonl"),
        )
        .env("AGENTERM_NO_ACTIVATE", "1")
        .output()
        .expect("agenterm CLI runs");

    assert_eq!(output.status.code(), Some(3));
    assert!(output.stderr.is_empty());
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("script refusal is JSON");
    assert_eq!(result["failure"]["category"], "limit");
    assert!(
        result["failure"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("collection_items"))
    );
}
