//! `scripts/qjs/cu-retirement-runtime-aggregate.qjs` over the directory
//! layout Candidate's aggregate downloads: one directory per
//! `candidate-cu-runtime-<platform>-<run>-<attempt>` artifact of the run.
//!
//! A rerun of failed cells leaves the cells that passed at their earlier
//! attempt, so each cell's receipt is its highest attempt present, and that
//! receipt must name the same run and attempt as the artifact carrying it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RUN: &str = "4242";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const PLATFORMS: [(&str, &str, &str); 6] = [
    ("windows-x86_64", "windows", "x86_64"),
    ("windows-aarch64", "windows", "aarch64"),
    ("linux-x86_64", "linux", "x86_64"),
    ("linux-aarch64", "linux", "aarch64"),
    ("macos-aarch64", "macos", "aarch64"),
    ("macos-x86_64", "macos", "x86_64"),
];
const EVIDENCE: [&str; 9] = [
    "cu.retirement-cell.acu-provider",
    "cu.retirement-cell.native-acu-composition",
    "acu.mcp-provider-capabilities",
    "acu.mcp-provider-observe",
    "cu.power-action-plan.acu-object",
    "cu.retirement-cell.capabilities",
    "cu.retirement-cell.runtime-status",
    "cu.setup-cli-entrypoint",
    "cu.setup-runtime-refresh-owned-job",
];

fn hash(tag: &str) -> String {
    let mut out = String::new();
    while out.len() < 64 {
        out.push_str(&format!("{:x}", tag.len() % 16));
        out.push_str("a1b2c3d4");
    }
    out.truncate(64);
    out
}

/// A receipt the shared validator accepts for `platform` at `attempt`.
fn cell(platform: &str, os: &str, arch: &str, attempt: &str) -> serde_json::Value {
    let windows = os == "windows";
    let (abi, provider) = match os {
        "windows" => ("agenterm.dll", "agenterm-cu-provider.dll"),
        "macos" => ("libagenterm.dylib", "agenterm-cu-provider.dylib"),
        _ => ("libagenterm.so", "agenterm-cu-provider.so"),
    };
    serde_json::json!({
        "schema_version": 6,
        "kind": "agenterm-cu-runtime-cell",
        "source_sha": SHA,
        "run": { "id": RUN, "attempt": attempt },
        "platform_id": platform,
        "os": os,
        "arch": arch,
        "archive_sha256": hash("archive"),
        "agenterm_launcher": { "name": if windows { "agenterm.com" } else { "agenterm" }, "sha256": hash("launcher") },
        "agenterm_consumer": { "name": if windows { "agenterm.exe" } else { "agenterm" }, "sha256": hash("consumer") },
        "agenterm_cu": { "name": if windows { "agenterm-cu.exe" } else { "agenterm-cu" }, "sha256": hash("cu") },
        "libagenterm": { "name": abi, "sha256": hash("abi") },
        "acu_provider": { "name": provider, "abi_version": 1, "sha256": hash("provider") },
        "owned_ephemeral_cleanup": "complete",
        "persistent_mutation_performed": false,
        "result": "passed",
        "evidence": EVIDENCE,
    })
}

fn write(root: &Path, platform: &str, run: &str, attempt: &str, value: &serde_json::Value) {
    let dir = root.join(format!("candidate-cu-runtime-{platform}-{run}-{attempt}"));
    fs::create_dir_all(&dir).expect("artifact dir");
    fs::write(dir.join(format!("{platform}.json")), value.to_string()).expect("receipt");
}

fn full(root: &Path, attempt: &str) {
    for (platform, os, arch) in PLATFORMS {
        write(
            root,
            platform,
            RUN,
            attempt,
            &cell(platform, os, arch, attempt),
        );
    }
}

fn aggregate(root: &Path, run_attempt: &str) -> (Output, PathBuf) {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let summary = root.with_extension("summary.json");
    let output = Command::new(env!("CARGO_BIN_EXE_agenterm"))
        .args([
            "cli",
            "script",
            "run",
            "--profile",
            "tool",
            "--timeout-ms",
            "120000",
        ])
        .args(["--max-operations", "1000000000", "--project-root"])
        .arg(repo)
        .arg(repo.join("scripts/qjs/cu-retirement-runtime-aggregate.qjs"))
        .arg("--")
        .arg(root)
        .args([SHA, RUN, run_attempt])
        .arg(&summary)
        .output()
        .expect("run aggregator");
    (output, summary)
}

fn refused(root: &Path, run_attempt: &str, code: &str) {
    let (output, _) = aggregate(root, run_attempt);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{code}: accepted");
    assert!(stderr.contains(code), "expected {code}, got {stderr}");
}

#[test]
fn six_receipts_of_one_attempt_are_summarised() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("receipts");
    full(&root, "1");
    let (output, summary) = aggregate(&root, "1");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(summary).expect("summary")).expect("json");
    assert_eq!(value["run"]["attempt"], "1");
    assert_eq!(value["cells"].as_array().expect("cells").len(), 6);
}

/// One failed cell is rerun: it has a failed attempt 1 and a passing
/// attempt 2; the other five passed at attempt 1. The consumer runs at
/// attempt 2 and takes each cell's own highest attempt.
#[test]
fn a_partial_rerun_takes_each_cells_highest_attempt() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("receipts");
    full(&root, "1");
    let mut failed = cell("windows-x86_64", "windows", "x86_64", "1");
    failed["result"] = serde_json::json!("failed");
    write(&root, "windows-x86_64", RUN, "1", &failed);
    write(
        &root,
        "windows-x86_64",
        RUN,
        "2",
        &cell("windows-x86_64", "windows", "x86_64", "2"),
    );
    let (output, summary) = aggregate(&root, "2");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(summary).expect("summary")).expect("json");
    assert_eq!(
        value["run"]["attempt"], "2",
        "the summary names the consumer attempt"
    );
    let attempts: Vec<String> = value["cells"]
        .as_array()
        .expect("cells")
        .iter()
        .map(|cell| cell["run"]["attempt"].as_str().unwrap_or("").to_owned())
        .collect();
    assert_eq!(attempts, ["2", "1", "1", "1", "1", "1"]);
}

/// The old download named receipts from the consumer's own attempt, so a
/// partial rerun delivered only the rerun cell's receipt.
#[test]
fn only_the_consumers_own_attempt_is_missing_cells() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("receipts");
    write(
        &root,
        "windows-x86_64",
        RUN,
        "2",
        &cell("windows-x86_64", "windows", "x86_64", "2"),
    );
    refused(&root, "2", "cu_runtime_receipt_missing");
}

#[test]
fn a_receipt_must_name_the_attempt_that_carried_it() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("receipts");
    full(&root, "1");
    // Carried by attempt 2, but the receipt claims attempt 1.
    write(
        &root,
        "linux-x86_64",
        RUN,
        "2",
        &cell("linux-x86_64", "linux", "x86_64", "1"),
    );
    refused(
        &root,
        "2",
        "cu_runtime_receipt_producer_mismatch:linux-x86_64",
    );
}

#[test]
fn foreign_runs_future_attempts_and_stray_files_are_refused() {
    let tmp = tempfile::tempdir().expect("tmp");

    let foreign = tmp.path().join("foreign");
    full(&foreign, "1");
    write(
        &foreign,
        "linux-x86_64",
        "9999",
        "1",
        &cell("linux-x86_64", "linux", "x86_64", "1"),
    );
    refused(&foreign, "1", "cu_runtime_receipt_foreign_run");

    let future = tmp.path().join("future");
    full(&future, "1");
    write(
        &future,
        "linux-x86_64",
        RUN,
        "3",
        &cell("linux-x86_64", "linux", "x86_64", "3"),
    );
    refused(&future, "2", "cu_runtime_receipt_attempt");

    let stray = tmp.path().join("stray");
    full(&stray, "1");
    fs::write(
        stray
            .join(format!("candidate-cu-runtime-macos-x86_64-{RUN}-1"))
            .join("extra.json"),
        "{}",
    )
    .expect("stray");
    refused(&stray, "1", "cu_runtime_receipt_set:macos-x86_64");

    let flat = tmp.path().join("flat");
    fs::create_dir_all(&flat).expect("flat");
    fs::write(flat.join("linux-x86_64.json"), "{}").expect("merged file");
    refused(&flat, "1", "cu_runtime_receipt_not_artifact");
}
