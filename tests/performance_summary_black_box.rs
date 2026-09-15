use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

struct FixtureRoot(PathBuf);

impl FixtureRoot {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agenterm-performance-summary-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create performance summary fixture root");
        Self(path)
    }
}

impl Drop for FixtureRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_timing(path: &Path, run_id: &str, completed_at_utc: &str, wall_ms: u64) {
    let record = serde_json::json!({
        "schema_version": 2,
        "kind": "agenterm-quality-timing",
        "lane": "quick",
        "profile": "quick",
        "status": "passed",
        "total_wall_ms": wall_ms,
        "wall_time": {
            "state": "partial",
            "task_ms": wall_ms,
            "accounted_ms": wall_ms
        },
        "source": { "commit": "0123456789012345678901234567890123456789" },
        "workload": { "fingerprint": "fixture-workload" },
        "experiment_run_id": run_id,
        "completed_at_utc": completed_at_utc
    });
    fs::write(
        path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&record).expect("serialize timing")
        ),
    )
    .expect("write timing fixture");
}

fn write_stats(path: &Path) {
    let stats = serde_json::json!({
        "stats": {
            "compile_requests": 1,
            "cache_hits": 0,
            "cache_misses": 1,
            "cache_read_errors": 0,
            "cache_write_errors": 0
        }
    });
    fs::write(
        path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&stats).expect("serialize stats")
        ),
    )
    .expect("write stats fixture");
}

fn run_summary(
    output_path: &Path,
    strategy: &str,
    timings: [&Path; 3],
    stats: [&Path; 3],
) -> Output {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("agenterm.tasks.json");
    let mut command = Command::new(env!("CARGO_BIN_EXE_agenterm"));
    command
        .args([
            "cli",
            "script",
            "task",
            "run",
            "performance-summary",
            "--manifest",
        ])
        .arg(manifest)
        .arg("--")
        .arg(output_path)
        .arg("standard")
        .arg(strategy);
    for timing in timings {
        command.arg(timing);
    }
    for sample_stats in stats {
        command.arg(sample_stats);
    }
    command
        .env_remove("GITHUB_STEP_SUMMARY")
        .output()
        .expect("run performance-summary task")
}

fn diagnostic(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn performance_summary_accepts_one_run_and_rejects_misattributed_samples() {
    let root = FixtureRoot::new();
    let timings = [
        root.0.join("timing-1.json"),
        root.0.join("timing-2.json"),
        root.0.join("timing-3.json"),
    ];
    let stats = [
        root.0.join("stats-1.json"),
        root.0.join("stats-2.json"),
        root.0.join("stats-3.json"),
    ];
    for (index, path) in timings.iter().enumerate() {
        write_timing(
            path,
            "perf-fixture-A",
            &format!("2026-09-16T00:00:0{index}Z"),
            100 + index as u64,
        );
    }
    for path in &stats {
        write_stats(path);
    }

    let valid = run_summary(
        &root.0.join("valid.json"),
        "target",
        [&timings[0], &timings[1], &timings[2]],
        [&stats[0], &stats[1], &stats[2]],
    );
    assert!(valid.status.success(), "{}", diagnostic(&valid));
    assert!(root.0.join("valid.json").is_file());

    write_timing(&timings[1], "perf-fixture-B", "2026-09-16T00:00:01Z", 101);
    let mixed = run_summary(
        &root.0.join("mixed.json"),
        "target",
        [&timings[0], &timings[1], &timings[2]],
        [&stats[0], &stats[1], &stats[2]],
    );
    assert!(!mixed.status.success());
    assert!(diagnostic(&mixed).contains("performance_summary_run_identity:"));

    write_timing(&timings[1], "perf-fixture-A", "2026-09-15T23:59:59Z", 101);
    let reversed = run_summary(
        &root.0.join("reversed.json"),
        "target",
        [&timings[0], &timings[1], &timings[2]],
        [&stats[0], &stats[1], &stats[2]],
    );
    assert!(!reversed.status.success());
    assert!(diagnostic(&reversed).contains("performance_summary_sample_order:"));

    write_timing(&timings[1], "perf-fixture-A", "2026-09-16T00:00:01Z", 101);
    let duplicate_stats = run_summary(
        &root.0.join("duplicate-stats.json"),
        "sccache",
        [&timings[0], &timings[1], &timings[2]],
        [&stats[0], &stats[0], &stats[2]],
    );
    assert!(!duplicate_stats.status.success());
    assert!(diagnostic(&duplicate_stats).contains("performance_summary_sccache_path_duplicate:"));
}
