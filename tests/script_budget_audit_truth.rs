//! The public audit record distinguishes enforced budgets from the accepted
//! values the selected engine still names as unenforced.

#![cfg(feature = "script-qjswasm")]

use std::process::Command;

const AGENTERM_BIN: &str = env!("CARGO_BIN_EXE_agenterm");

#[test]
fn qjs_audit_credits_every_enforced_engine_budget() {
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
    assert_eq!(record["requested_budgets"]["expression_depth"], 64);
    assert_eq!(record["effective_budgets"]["expression_depth"], 64);
    assert_eq!(record["unenforced_budgets"], serde_json::json!([]));
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

#[test]
fn qjs_cli_selects_and_audits_the_expression_depth_budget() {
    let directory = tempfile::tempdir().expect("temporary audit directory");
    let source = "1 + (2 + (3 + 4))";
    for (limit, expected_code) in [(4, 0), (3, 3)] {
        let audit_path = directory.path().join(format!("expression-{limit}.jsonl"));
        let output = Command::new(AGENTERM_BIN)
            .args([
                "cli",
                "script",
                "eval",
                source,
                "--max-expression-depth",
                &limit.to_string(),
                "--json",
            ])
            .env("AGENTERM_SCRIPT_BACKEND", "qjswasm")
            .env("AGENTERM_SCRIPT_AUDIT_PATH", &audit_path)
            .env("AGENTERM_NO_ACTIVATE", "1")
            .output()
            .expect("agenterm CLI runs");
        assert_eq!(
            output.status.code(),
            Some(expected_code),
            "limit={limit} stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if expected_code != 0 {
            let result: serde_json::Value =
                serde_json::from_slice(&output.stdout).expect("script refusal is JSON");
            assert_eq!(result["failure"]["category"], "limit");
            assert!(
                result["failure"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("expression_depth"))
            );
        }
        let records = std::fs::read_to_string(&audit_path).expect("public audit JSONL");
        let record: serde_json::Value =
            serde_json::from_str(records.lines().next().expect("one audit record"))
                .expect("audit record is JSON");
        assert_eq!(record["requested_budgets"]["expression_depth"], limit);
        assert_eq!(record["effective_budgets"]["expression_depth"], limit);
    }
}

#[test]
fn qjs_cli_rejects_expression_depth_outside_the_public_range() {
    for limit in ["0", "129"] {
        let output = Command::new(AGENTERM_BIN)
            .args([
                "cli",
                "script",
                "eval",
                "1 + 1",
                "--max-expression-depth",
                limit,
            ])
            .env("AGENTERM_SCRIPT_BACKEND", "qjswasm")
            .env("AGENTERM_NO_ACTIVATE", "1")
            .output()
            .expect("agenterm CLI runs");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr).trim(),
            "script --max-expression-depth must be from 1 to 128"
        );
    }
}

#[test]
fn qjs_task_contract_forwards_its_expression_depth_ceiling() {
    let directory = tempfile::tempdir().expect("temporary task project");
    let script_path = directory.path().join("deep.qjs");
    std::fs::write(&script_path, "return 1 + (2 + (3 + 4));\n").expect("task source");
    let manifest_path = directory.path().join("agenterm.tasks.json");
    let manifest = serde_json::json!({
        "schema_version": 3,
        "project": {
            "id": "expression-depth-fixture",
            "version": "1.0.0",
            "requires": {
                "script_api": {"minimum": 2, "maximum": 2},
                "capabilities": ["runtime.project.named-task"]
            },
            "origin": {"kind": "repository", "id": "agenterm-test"},
            "provenance": {"producer": "agenterm-test", "revision": "fixture-1"}
        },
        "contracts": {
            "deep": {
                "inputs": ["source"],
                "outputs": ["result"],
                "budget": {
                    "timeout_ms": 10_000,
                    "max_operations": 1_000_000,
                    "max_output_bytes": 65_536,
                    "max_expression_depth": 4
                },
                "network": [],
                "evidence": ["expression-depth"]
            }
        },
        "tasks": [{"id": "deep", "entry": "deep.qjs"}]
    });
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest JSON"),
    )
    .expect("task manifest");

    let exact = Command::new(AGENTERM_BIN)
        .args(["cli", "script", "task", "run", "deep", "--manifest"])
        .arg(&manifest_path)
        .env("AGENTERM_NO_ACTIVATE", "1")
        .output()
        .expect("agenterm CLI runs");
    assert_eq!(
        exact.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&exact.stdout),
        String::from_utf8_lossy(&exact.stderr)
    );

    let tight = Command::new(AGENTERM_BIN)
        .args(["cli", "script", "task", "run", "deep", "--manifest"])
        .arg(&manifest_path)
        .args(["--max-expression-depth", "3"])
        .env("AGENTERM_NO_ACTIVATE", "1")
        .output()
        .expect("agenterm CLI runs");
    assert_eq!(tight.status.code(), Some(3));
    assert!(
        String::from_utf8_lossy(&tight.stderr).contains("expression_depth"),
        "stderr={}",
        String::from_utf8_lossy(&tight.stderr)
    );

    let over_declared = Command::new(AGENTERM_BIN)
        .args(["cli", "script", "task", "run", "deep", "--manifest"])
        .arg(&manifest_path)
        .args(["--max-expression-depth", "5"])
        .env("AGENTERM_NO_ACTIVATE", "1")
        .output()
        .expect("agenterm CLI runs");
    assert_eq!(over_declared.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&over_declared.stderr)
            .contains("task_budget_exceeded: deep --max-expression-depth requested 5, declared 4"),
        "stderr={}",
        String::from_utf8_lossy(&over_declared.stderr)
    );
}
