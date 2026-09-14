//! Public CLI evidence for bounded tool-door file reads used by release checks.

use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

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
            "agenterm-bounded-file-read-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&root).expect("unique fixture root");
        Self(root)
    }
}

impl Drop for FixtureRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_long_transcript_recovers_evidence_before_the_tail_window() {
    let root = FixtureRoot::new();
    let transcript = root.0.join("long.stdout");
    let mut bytes = b"EVIDENCE public.head.marker\n".to_vec();
    bytes.extend(std::iter::repeat_n(b'x', 600_000));
    std::fs::write(&transcript, bytes).expect("write long transcript");

    let source = format!(
        r#"const path = {};
if (fs_read_lines_starting_with_lossy(path, "EVIDENCE ", 65536, 10000) !== 0) {{
  throw "scan: " + tool_result();
}}
return tool_result();
"#,
        serde_json::to_string(&transcript.to_string_lossy()).expect("encode fixture path")
    );
    let script = root.0.join("recover-head-evidence.qjs");
    std::fs::write(&script, source).expect("write qjs fixture");

    let output = Command::new(AGENTERM_BIN)
        .args(["cli", "script", "run", "--profile", "tool"])
        .arg(&script)
        .env_remove("AGENTERM_SCRIPT_BACKEND")
        .output()
        .expect("agenterm CLI runs");
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "EVIDENCE public.head.marker"
    );
}
