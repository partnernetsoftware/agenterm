use std::sync::LazyLock;

static WORKFLOW: LazyLock<String> = LazyLock::new(|| {
    include_str!("../.github/workflows/performance-experiment.yml").replace("\r\n", "\n")
});
static SAMPLES: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/performance-samples.qjs").replace("\r\n", "\n"));
static SUMMARY: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/performance-summary.qjs").replace("\r\n", "\n"));

#[test]
fn experiment_is_manual_read_only_and_exact_source_bound() {
    assert!(WORKFLOW.contains("workflow_dispatch:"));
    assert!(!WORKFLOW.contains("\n  push:"));
    assert!(!WORKFLOW.contains("\n  pull_request:"));
    assert!(WORKFLOW.contains("permissions:\n  contents: read"));
    assert!(!WORKFLOW.contains("contents: write"));
    assert!(!WORKFLOW.contains("secrets."));
    assert!(WORKFLOW.contains("ref: ${{ inputs.source_sha }}"));
    assert!(WORKFLOW.contains("[[ \"$(git rev-parse HEAD)\" == \"$SOURCE_SHA\" ]]"));
}

#[test]
fn experiment_uses_three_equal_samples_and_one_configured_trial_switch() {
    assert!(!WORKFLOW.contains("matrix:\n        sample:"));
    assert!(WORKFLOW.contains("vars.AGENTERM_WINDOWS_EXPERIMENT_RUNNER"));
    assert!(WORKFLOW.contains("test -n \"$TRIAL_RUNNER\""));
    assert!(WORKFLOW.contains("'windows-latest'"));
    assert!(!WORKFLOW.contains("runs-on: ${{ inputs."));
    assert!(!WORKFLOW.contains("continue-on-error: ${{"));
    // The driver is the main PE's engine-neutral task route
    // (`agenterm cli script task run ...`); the rh route it used until
    // 2026-08-29 is retired with that engine.
    assert!(WORKFLOW.contains("Build experiment driver"));
    assert!(WORKFLOW.contains("agenterm.exe\" cli script task run"));
    assert!(
        WORKFLOW.contains("cli script task run performance-samples --manifest agenterm.tasks.json")
    );
    assert!(WORKFLOW.contains("cli script task run performance-summary"));
    assert!(!WORKFLOW.contains(" rh task run"));
    let compatibility_cli = ["agenterm", "rhai"].join("-");
    assert!(!WORKFLOW.contains(&compatibility_cli));
    assert!(!WORKFLOW.contains("shell: pwsh"));
}

#[test]
fn cache_strategies_are_isolated_fail_safe_and_observable() {
    assert!(
        WORKFLOW.contains("options:\n          - target\n          - sccache\n          - none")
    );
    assert!(
        WORKFLOW
            .contains("mozilla-actions/sccache-action@fc920bf0ec8de6ee65d409111f7ec508035751ba")
    );
    assert!(WORKFLOW.contains("CARGO_INCREMENTAL:"));
    assert!(WORKFLOW.contains("RUSTC_WRAPPER:"));
    assert!(
        WORKFLOW
            .contains("SCCACHE_GHA_VERSION: perf-${{ github.run_id }}-${{ github.run_attempt }}")
    );
    assert!(!WORKFLOW.contains("uses: actions/cache/"));
    assert!(WORKFLOW.contains("task run performance-summary"));
    assert!(WORKFLOW.contains("performance-summary.json"));
}

#[test]
fn samples_own_the_cold_tree_they_create() {
    assert!(SAMPLES.contains("AGENTERM_BOOTSTRAP_CACHE_ROOT"));
    assert!(SAMPLES.contains("target/qualification/performance-bootstrap-cache-"));
    assert!(SAMPLES.contains("\"cargo\", [\"clean\"]"));
    assert!(!SAMPLES.contains("rh.join(evidence, \"bootstrap-cache-"));

    let check = include_str!("../scripts/qjs/check.qjs");
    assert!(check.contains("path.join(quick_native_bin_root, \"agenterm\")"));
    assert!(check.contains("if (unix_bootstrap !== 0)"));
}

#[test]
fn experiment_outputs_never_mark_the_tree_dirty() {
    let ignore = include_str!("../.gitignore");
    assert!(ignore.contains("/performance-evidence/"));
    assert!(ignore.contains("/sccache-*.json"));
}

#[test]
fn sccache_stats_failures_keep_their_diagnostic() {
    assert!(!SAMPLES.contains("\"sccache-\" + sample_tag + \".stderr\""));
    assert!(!SAMPLES.contains("command_stdout_file(\n      \"sccache\""));
    assert!(SAMPLES.contains("\"--show-stats\", \"--stats-format\", \"json\""));
    assert!(SAMPLES.contains("performance_samples_sccache_stats:"));
    assert!(SAMPLES.contains("rh.atomic_write(stats_path, stats.stdout)"));
    assert!(SAMPLES.contains("stats.stderr.trim()"));
}

#[test]
fn summary_requires_one_ordered_experiment_run() {
    assert!(SAMPLES.contains("timing_record.experiment_run_id = run_id"));
    assert!(SAMPLES.contains("rh.atomic_write(timing,"));
    assert!(SUMMARY.contains("performance_summary_run_identity:"));
    assert!(SUMMARY.contains("experiment_run_id === run_id"));
    assert!(SUMMARY.contains("performance_summary_sample_clock:"));
    assert!(SUMMARY.contains("performance_summary_sample_order:"));
    assert!(SUMMARY.contains("previous_completed <= completed"));
    assert!(SUMMARY.contains("experiment_run_id: experiment_run_id"));
    assert!(WORKFLOW.contains("@if errorlevel 1 exit /b 1"));
    assert!(WORKFLOW.contains("cli script task run performance-summary"));
}

#[test]
fn experiment_runs_quick_only_and_cannot_publish_or_claim_qualification() {
    for forbidden in [
        "--release",
        "--include-stress",
        "gh release",
        "git tag",
        "candidate-aggregate",
        "candidate-verify",
        "package-client-release",
    ] {
        assert!(
            !WORKFLOW.contains(forbidden),
            "forbidden experiment behavior: {forbidden}"
        );
    }
    assert!(WORKFLOW.contains("Aggregate typed experiment evidence"));
    assert!(WORKFLOW.contains("retention-days: 14"));
    assert!(WORKFLOW.contains("if-no-files-found: warn"));
}
