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
