//! Public `.wasm` execution crosses the framed worker boundary. Native calls
//! may crash or block because `dlsym` cannot validate the guest declaration;
//! these courts prove that the CLI process reports those outcomes and reaps
//! the independently supervised worker instead of sharing its fate.

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

#[cfg(unix)]
use std::time::{Duration, Instant};

const AGENTERM_BIN: &str = env!("CARGO_BIN_EXE_agenterm");
static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct FixtureRoot(PathBuf);

impl FixtureRoot {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "agenterm-native-artifact-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&root).expect("unique fixture root");
        Self(root)
    }

    fn wasm(&self, name: &str, wat_source: &str) -> PathBuf {
        let path = self.0.join(format!("{name}.wasm"));
        std::fs::write(
            &path,
            wat::parse_str(wat_source).expect("fixture WAT compiles"),
        )
        .expect("write fixture wasm");
        path
    }

    fn qjs(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(format!("{name}.qjs"));
        std::fs::write(&path, source).expect("write fixture qjs");
        path
    }
}

impl Drop for FixtureRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_plain(path: &Path, timeout_ms: u64) -> Output {
    Command::new(AGENTERM_BIN)
        .args([
            "cli",
            "script",
            "run",
            "--wasm-convention",
            "plain",
            "--timeout-ms",
            &timeout_ms.to_string(),
        ])
        .arg(path)
        .env_remove("AGENTERM_SCRIPT_BACKEND")
        .output()
        .expect("agenterm CLI runs")
}

fn run_plain_with_args(
    path: &Path,
    timeout_ms: u64,
    entry_args: &[&str],
    script_args: &[&str],
) -> Output {
    let mut command = Command::new(AGENTERM_BIN);
    command.args([
        "cli",
        "script",
        "run",
        "--wasm-convention",
        "plain",
        "--profile",
        "tool",
        "--timeout-ms",
        &timeout_ms.to_string(),
    ]);
    for argument in entry_args {
        command.args(["--wasm-entry-arg", argument]);
    }
    command.arg(path);
    if !script_args.is_empty() {
        command.arg("--").args(script_args);
    }
    command
        .env_remove("AGENTERM_SCRIPT_BACKEND")
        .output()
        .expect("agenterm CLI runs")
}

#[test]
fn typed_plain_wasm_entry_arguments_cross_the_public_worker_wire() {
    let root = FixtureRoot::new();
    let path = root.wasm(
        "typed-entry-arguments",
        r#"(module
            (func (export "main") (param i32 i64 f32 f64) (result i32)
                local.get 0
                i32.const -7
                i32.eq
                local.get 1
                i64.const 9000000000
                i64.eq
                i32.and
                local.get 2
                i32.reinterpret_f32
                i32.const -2147483648
                i32.eq
                i32.and
                local.get 3
                local.get 3
                f64.ne
                i32.and))"#,
    );
    let output = run_plain_with_args(
        &path,
        2_000,
        &["i32:-7", "i64:9000000000", "f32:-0", "f64:NaN"],
        &[],
    );
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1");
}

#[test]
fn plain_wasm_tool_arguments_remain_a_separate_string_channel() {
    let root = FixtureRoot::new();
    let path = root.wasm(
        "tool-argument-count",
        r#"(module
            (import "tool" "arg_count" (func $arg_count (result i32)))
            (func (export "main") (result i32)
                call $arg_count))"#,
    );
    let output = run_plain_with_args(&path, 2_000, &[], &["7", "8"]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "2");
}

#[test]
fn script_hash_matches_qualification_receipts_and_plain_wasm_bytes() {
    let root = FixtureRoot::new();
    let source = root.qjs("qualified-hash", "return 42;");
    let output_dir = root.0.join("qualified");
    let qualify = Command::new(AGENTERM_BIN)
        .args(["cli", "script", "qualify"])
        .arg(&source)
        .args(["--dir"])
        .arg(&output_dir)
        .env_remove("AGENTERM_SCRIPT_BACKEND")
        .output()
        .expect("qualification runs");
    assert!(
        qualify.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&qualify.stdout),
        String::from_utf8_lossy(&qualify.stderr)
    );

    let receipt: serde_json::Value = serde_json::from_slice(
        &std::fs::read(output_dir.join("receipt.json")).expect("read receipt"),
    )
    .expect("receipt is JSON");
    let artifact = output_dir.join("qualified-hash.wasm");
    let digest = receipt["artifact_sha256"]
        .as_str()
        .expect("receipt carries artifact digest");
    let hash = Command::new(AGENTERM_BIN)
        .args(["cli", "script", "hash"])
        .arg(&artifact)
        .env_remove("AGENTERM_SCRIPT_BACKEND")
        .output()
        .expect("artifact hash runs");
    assert!(
        hash.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&hash.stdout),
        String::from_utf8_lossy(&hash.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&hash.stdout).trim(),
        format!("{digest}  wasm  {}", artifact.display())
    );

    let plain = root.wasm(
        "plain-hash",
        r#"(module (func (export "main") (result i32) i32.const 7))"#,
    );
    let plain_bytes = std::fs::read(&plain).expect("read plain wasm");
    let plain_digest = agenterm_script_common::hex::sha256_hex(&plain_bytes);
    let hash = Command::new(AGENTERM_BIN)
        .args(["cli", "script", "hash"])
        .arg(&plain)
        .env_remove("AGENTERM_SCRIPT_BACKEND")
        .output()
        .expect("plain artifact hash runs");
    assert!(
        hash.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&hash.stdout),
        String::from_utf8_lossy(&hash.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&hash.stdout).trim(),
        format!("{plain_digest}  wasm  {}", plain.display())
    );
}

#[cfg(unix)]
fn worker_pid(stderr: &[u8]) -> u32 {
    let stderr = String::from_utf8_lossy(stderr);
    let marker = "script worker ";
    let after = stderr
        .split_once(marker)
        .unwrap_or_else(|| panic!("worker pid missing from {stderr}"))
        .1;
    after
        .split(|character: char| !character.is_ascii_digit())
        .next()
        .expect("worker pid digits")
        .parse()
        .expect("worker pid is u32")
}

#[cfg(unix)]
fn assert_process_absent(pid: u32) {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .expect("ps is available to the Unix process court");
    assert!(
        String::from_utf8_lossy(&output.stdout).trim().is_empty(),
        "worker {pid} remained after the supervisor returned"
    );
}

#[test]
fn a_plain_native_artifact_runs_through_the_public_supervised_cli() {
    let root = FixtureRoot::new();
    #[cfg(unix)]
    let wat_source = include_str!("../crates/agenterm-qjswasm/tests/fixtures/native/getpid.wat");
    #[cfg(windows)]
    let wat_source = include_str!(
        "../crates/agenterm-qjswasm/tests/fixtures/native/windows_get_current_process_id.wat"
    );
    let path = root.wasm("getpid", wat_source);
    let output = run_plain(&path, 2_000);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let process_id: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("getpid result is printed as an integer");
    assert!(process_id > 0);
}

#[test]
fn a_multi_result_plain_artifact_is_refused_instead_of_silently_truncated() {
    let root = FixtureRoot::new();
    let path = root.wasm(
        "multi-result",
        r#"(module
            (func (export "main") (result i32 i64)
                i32.const 7
                i64.const 9000000000))"#,
    );
    let output = run_plain(&path, 2_000);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("qjswasm_result_not_json"), "{stderr}");
    assert!(stderr.contains("2 completion values"), "{stderr}");
}

#[test]
fn the_default_wasm_convention_preserves_compiled_qjs_artifacts() {
    let root = FixtureRoot::new();
    let path = root.0.join("compiled-qjs.wasm");
    std::fs::write(
        &path,
        agenterm_qjswasm::compile_qjs("return 1 + 2;").expect("qjs compiles"),
    )
    .expect("write compiled artifact");
    let output = Command::new(AGENTERM_BIN)
        .args(["cli", "script", "run"])
        .arg(&path)
        .env_remove("AGENTERM_SCRIPT_BACKEND")
        .output()
        .expect("agenterm CLI runs");
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "3",
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn wasm_convention_is_a_closed_value_option() {
    let root = FixtureRoot::new();
    let path = root.wasm(
        "getpid",
        include_str!("../crates/agenterm-qjswasm/tests/fixtures/native/getpid.wat"),
    );
    let unknown = Command::new(AGENTERM_BIN)
        .args(["cli", "script", "run", "--wasm-convention", "guessed"])
        .arg(&path)
        .output()
        .expect("agenterm CLI runs");
    assert_eq!(unknown.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("must be compiled-qjs or plain"));

    let missing = Command::new(AGENTERM_BIN)
        .args(["cli", "script", "run", "--wasm-convention"])
        .output()
        .expect("agenterm CLI runs");
    assert_eq!(missing.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("requires a value"),
        "stderr={}",
        String::from_utf8_lossy(&missing.stderr)
    );
}

#[cfg(unix)]
#[test]
fn a_crashing_native_declaration_becomes_a_typed_worker_crash() {
    let root = FixtureRoot::new();
    let path = root.wasm(
        "misdeclared-abort",
        include_str!("../crates/agenterm-qjswasm/tests/fixtures/native/misdeclared_abort.wat"),
    );
    let output = run_plain(&path, 2_000);
    assert_eq!(
        output.status.code(),
        Some(1),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("\"code\":\"host_worker_crash\""),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&path.to_string_lossy().to_string()),
        "{stderr}"
    );
    assert_process_absent(worker_pid(&output.stderr));
}

#[cfg(unix)]
#[test]
fn a_blocking_native_call_hits_hard_timeout_and_is_reaped() {
    let root = FixtureRoot::new();
    let path = root.wasm(
        "sleep-thirty-seconds",
        include_str!("../crates/agenterm-qjswasm/tests/fixtures/native/sleep_thirty_seconds.wat"),
    );
    let started = Instant::now();
    let output = run_plain(&path, 100);
    let elapsed = started.elapsed();
    assert_eq!(
        output.status.code(),
        Some(3),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("\"code\":\"host_hard_timeout\""),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&path.to_string_lossy().to_string()),
        "{stderr}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "hard timeout took {elapsed:?}"
    );
    assert_process_absent(worker_pid(&output.stderr));
}
