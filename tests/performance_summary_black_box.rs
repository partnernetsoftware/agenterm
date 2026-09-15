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
    write_timing_values(
        path,
        run_id,
        completed_at_utc,
        serde_json::Value::from(wall_ms),
        serde_json::Value::from(wall_ms),
        serde_json::Value::from(wall_ms),
    );
}

fn write_timing_values(
    path: &Path,
    run_id: &str,
    completed_at_utc: &str,
    total_wall_ms: serde_json::Value,
    task_ms: serde_json::Value,
    accounted_ms: serde_json::Value,
) {
    let record = serde_json::json!({
        "schema_version": 2,
        "kind": "agenterm-quality-timing",
        "lane": "quick",
        "profile": "quick",
        "status": "passed",
        "total_wall_ms": total_wall_ms,
        "wall_time": {
            "state": "partial",
            "task_ms": task_ms,
            "accounted_ms": accounted_ms
        },
        "bootstrap": {
            "schema_version": 1,
            "kind": "agenterm-bootstrap-timing",
            "state": "measured",
            "reason": null,
            "setup_ms": 0
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

fn number_stats() -> serde_json::Value {
    serde_json::json!({
        "stats": {
            "compile_requests": 1,
            "cache_hits": 0,
            "cache_misses": 1,
            "cache_read_errors": 0,
            "cache_write_errors": 0
        }
    })
}

fn write_stats(path: &Path, stats: &serde_json::Value) {
    fs::write(
        path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(stats).expect("serialize stats")
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
        write_stats(path, &number_stats());
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

    let map_stats = serde_json::json!({
        "stats": {
            "compile_requests": 1,
            "cache_hits": { "Rust": 0 },
            "cache_misses": { "Rust": 1 },
            "cache_read_errors": 0,
            "cache_write_errors": 0
        }
    });
    write_stats(&stats[2], &map_stats);
    let valid_sccache = run_summary(
        &root.0.join("valid-sccache.json"),
        "sccache",
        [&timings[0], &timings[1], &timings[2]],
        [&stats[0], &stats[1], &stats[2]],
    );
    assert!(
        valid_sccache.status.success(),
        "{}",
        diagnostic(&valid_sccache)
    );

    let missing_errors = serde_json::json!({
        "stats": { "compile_requests": 1, "cache_hits": 0, "cache_misses": 1 }
    });
    write_stats(&stats[0], &missing_errors);
    let missing_errors_result = run_summary(
        &root.0.join("missing-errors.json"),
        "sccache",
        [&timings[0], &timings[1], &timings[2]],
        [&stats[0], &stats[1], &stats[2]],
    );
    assert!(!missing_errors_result.status.success());
    assert!(
        diagnostic(&missing_errors_result)
            .contains("performance_summary_sccache_counter:1:cache_errors")
    );

    let string_hits = serde_json::json!({
        "stats": {
            "compile_requests": 1,
            "cache_hits": "5",
            "cache_misses": 1,
            "cache_read_errors": 0,
            "cache_write_errors": 0
        }
    });
    write_stats(&stats[0], &string_hits);
    let string_hits_result = run_summary(
        &root.0.join("string-hits.json"),
        "sccache",
        [&timings[0], &timings[1], &timings[2]],
        [&stats[0], &stats[1], &stats[2]],
    );
    assert!(!string_hits_result.status.success());
    assert!(
        diagnostic(&string_hits_result)
            .contains("performance_summary_sccache_counter:1:cache_hits")
    );

    let missing_rust = serde_json::json!({
        "stats": {
            "compile_requests": 1,
            "cache_hits": { "C": 1 },
            "cache_misses": 1,
            "cache_read_errors": 0,
            "cache_write_errors": 0
        }
    });
    write_stats(&stats[0], &missing_rust);
    let missing_rust_result = run_summary(
        &root.0.join("missing-rust.json"),
        "sccache",
        [&timings[0], &timings[1], &timings[2]],
        [&stats[0], &stats[1], &stats[2]],
    );
    assert!(!missing_rust_result.status.success());
    assert!(
        diagnostic(&missing_rust_result)
            .contains("performance_summary_sccache_counter:1:cache_hits")
    );

    for (index, path) in timings.iter().enumerate() {
        let text = format!("{}", 100 + index);
        write_timing_values(
            path,
            "perf-fixture-A",
            &format!("2026-09-16T00:00:0{index}Z"),
            serde_json::Value::from(text.clone()),
            serde_json::Value::from(text.clone()),
            serde_json::Value::from(text),
        );
    }
    let string_durations = run_summary(
        &root.0.join("string-durations.json"),
        "target",
        [&timings[0], &timings[1], &timings[2]],
        [&stats[0], &stats[1], &stats[2]],
    );
    assert!(!string_durations.status.success());
    assert!(diagnostic(&string_durations).contains("performance_summary_sample_schema:1"));

    for (index, path) in timings.iter().enumerate() {
        write_timing(
            path,
            "perf-fixture-A",
            &format!("2026-09-16T00:00:0{index}Z"),
            0,
        );
    }
    let zero_path = root.0.join("zero-durations.json");
    let zero_durations = run_summary(
        &zero_path,
        "target",
        [&timings[0], &timings[1], &timings[2]],
        [&stats[0], &stats[1], &stats[2]],
    );
    assert!(
        zero_durations.status.success(),
        "{}",
        diagnostic(&zero_durations)
    );
    let zero_report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(zero_path).expect("read zero summary"))
            .expect("parse zero summary");
    assert_eq!(zero_report["warm_speedup_percent"], 0);

    let mut missing_bootstrap: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&timings[0]).expect("read timing without bootstrap fixture"),
    )
    .expect("parse timing without bootstrap fixture");
    missing_bootstrap
        .as_object_mut()
        .expect("timing object")
        .remove("bootstrap");
    fs::write(
        &timings[0],
        format!(
            "{}\n",
            serde_json::to_string_pretty(&missing_bootstrap)
                .expect("serialize timing without bootstrap")
        ),
    )
    .expect("write timing without bootstrap fixture");
    let missing_bootstrap_result = run_summary(
        &root.0.join("missing-bootstrap.json"),
        "target",
        [&timings[0], &timings[1], &timings[2]],
        [&stats[0], &stats[1], &stats[2]],
    );
    assert!(!missing_bootstrap_result.status.success());
    assert!(
        diagnostic(&missing_bootstrap_result).contains("performance_summary_sample_bootstrap:1")
    );

    write_timing(&timings[0], "perf-fixture-A", "2026-09-16T00:00:00Z", 0);
    let mut inconsistent: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&timings[0]).expect("read inconsistent timing fixture"),
    )
    .expect("parse inconsistent timing fixture");
    inconsistent["bootstrap"]["setup_ms"] = serde_json::Value::from(50);
    fs::write(
        &timings[0],
        format!(
            "{}\n",
            serde_json::to_string_pretty(&inconsistent).expect("serialize inconsistent timing")
        ),
    )
    .expect("write inconsistent timing fixture");
    let inconsistent_result = run_summary(
        &root.0.join("inconsistent-wall-time.json"),
        "target",
        [&timings[0], &timings[1], &timings[2]],
        [&stats[0], &stats[1], &stats[2]],
    );
    assert!(!inconsistent_result.status.success());
    assert!(diagnostic(&inconsistent_result).contains("performance_summary_sample_wall_time:1"));

    write_timing(&timings[0], "perf-fixture-A", "2026-09-16T00:00:00Z", 0);
    for path in [&timings[1], &timings[2]] {
        let mut unavailable: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(path).expect("read unavailable timing fixture"),
        )
        .expect("parse unavailable timing fixture");
        unavailable["wall_time"]["state"] = serde_json::Value::from("unavailable");
        unavailable["bootstrap"] = serde_json::json!({
            "state": "unavailable",
            "reason": "direct_worker_invocation",
            "setup_ms": null
        });
        fs::write(
            path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&unavailable).expect("serialize unavailable timing")
            ),
        )
        .expect("write unavailable timing fixture");
    }
    let mixed_states = run_summary(
        &root.0.join("mixed-bootstrap-states.json"),
        "target",
        [&timings[0], &timings[1], &timings[2]],
        [&stats[0], &stats[1], &stats[2]],
    );
    assert!(
        mixed_states.status.success(),
        "{}",
        diagnostic(&mixed_states)
    );
}
