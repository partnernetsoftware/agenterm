use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SOURCE_SHA: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
const VERSION: &str = "0.1.19";
const CELLS: &[(&str, &str)] = &[
    ("windows-x86_64", "win-x86_64"),
    ("windows-aarch64", "win-aarch64"),
    ("linux-x86_64", "lnx-x86_64"),
    ("linux-aarch64", "lnx-aarch64"),
    ("macos-x86_64", "osx-x86_64"),
    ("macos-aarch64", "osx-aarch64"),
];

fn sha256(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_json(path: &Path, value: &Value) -> Vec<u8> {
    let bytes = serde_json::to_vec_pretty(value).expect("serialize fixture JSON");
    fs::write(path, &bytes).expect("write fixture JSON");
    bytes
}

fn fixture(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    fs::create_dir_all(root).expect("create asset root");
    let log = b"pre-push: clean\n";
    fs::write(root.join("pre-push-check.log"), log).expect("write pre-push log");
    let cargo_hash = "1".repeat(64);
    let artifact_hash = "2".repeat(64);
    let policy_hash = "3".repeat(64);
    let gate_hash = "4".repeat(64);
    let sbom_hash = "5".repeat(64);
    let cells: Vec<Value> = CELLS
        .iter()
        .map(|(platform, _)| {
            json!({
                "platform_id": platform,
                "state": "PASS",
                "artifacts": [{"name": "agenterm", "bytes": 1, "sha256": "6".repeat(64)}]
            })
        })
        .collect();
    let loaders: Vec<Value> = CELLS
        .iter()
        .map(|(_, cell)| json!({"cell": cell, "bytes": 1, "sha256": "7".repeat(64)}))
        .collect();
    let pre_push = json!({
        "status": "passed",
        "name": "pre-push-check.log",
        "size": log.len(),
        "sha256": sha256(log)
    });
    let build_path = root.join("local-build-manifest.json");
    let build_bytes = write_json(
        &build_path,
        &json!({
            "schema_version": 1,
            "kind": "agenterm-local-six-cell-build",
            "source_sha": SOURCE_SHA,
            "version": VERSION,
            "profile": "release",
            "pre_push_check": pre_push.clone(),
            "source_inputs": {
                "cargo_lock_sha256": cargo_hash,
                "artifact_manifest_sha256": artifact_hash,
                "release_policy_sha256": policy_hash.clone(),
                "gate_manifest_sha256": gate_hash,
                "sbom_sha256": sbom_hash
            },
            "cells": cells,
            "chassis_loaders": loaders
        }),
    );
    let receipt_path = root.join("qualification-receipt.json");
    write_json(
        &receipt_path,
        &json!({
            "schema_version": 2,
            "product": "AgenTerm",
            "profile": "prebuilt-six-cell-execute-only",
            "release": true,
            "stress_included": false,
            "source_sha": SOURCE_SHA,
            "version": VERSION,
            "release_policy_sha256": policy_hash,
            "pre_push_check": pre_push,
            "local_build_manifest": {
                "name": "local-build-manifest.json",
                "size": build_bytes.len(),
                "sha256": sha256(&build_bytes)
            }
        }),
    );
    let gate_path = root.join("qualification-gates.json");
    fs::write(&gate_path, b"{}\n").expect("write unused schema-v2 gate fixture");
    (receipt_path, gate_path, root.to_path_buf())
}

fn validate(receipt: &Path, gate: &Path, assets: &Path) -> Output {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    Command::new(env!("CARGO_BIN_EXE_agenterm"))
        .current_dir(repo)
        .env("AGENTERM_NO_ACTIVATE", "1")
        .args([
            "cli",
            "script",
            "run",
            "--profile",
            "tool",
            "--timeout-ms",
            "60000",
            "--max-operations",
            "1000000000",
            "--project-root",
        ])
        .arg(repo)
        .arg("scripts/qjs/local-candidate-receipt-selftest.qjs")
        .arg("--")
        .arg(receipt)
        .arg(SOURCE_SHA)
        .arg(VERSION)
        .arg("1".repeat(64))
        .arg("2".repeat(64))
        .arg("4".repeat(64))
        .arg("5".repeat(64))
        .arg(gate)
        .arg(assets)
        .output()
        .expect("run schema-v2 Candidate receipt validator")
}

#[test]
fn schema_v2_receipt_passes_and_tampered_local_evidence_is_rejected() {
    let temp = tempfile::tempdir().expect("create isolated receipt fixture");
    let (receipt, gate, assets) = fixture(&temp.path().join("payload"));
    let positive = validate(&receipt, &gate, &assets);
    assert!(
        positive.status.success(),
        "valid receipt rejected: {}",
        String::from_utf8_lossy(&positive.stderr)
    );
    assert!(
        String::from_utf8_lossy(&positive.stdout)
            .contains("PASS local six-cell receipt validation")
    );

    fs::write(
        assets.join("pre-push-check.log"),
        b"modified after local check\n",
    )
    .expect("mutate pre-push evidence");
    let negative = validate(&receipt, &gate, &assets);
    assert!(
        !negative.status.success(),
        "tampered local evidence was accepted"
    );
    let diagnostic = String::from_utf8_lossy(&negative.stdout);
    let error = String::from_utf8_lossy(&negative.stderr);
    assert!(
        diagnostic.contains("candidate_pre_push_log_identity")
            || error.contains("candidate_pre_push_log_identity")
            || diagnostic.contains("candidate_pre_push_receipt")
            || error.contains("candidate_pre_push_receipt"),
        "unexpected negative-control result: stdout={diagnostic} stderr={error}"
    );
}
