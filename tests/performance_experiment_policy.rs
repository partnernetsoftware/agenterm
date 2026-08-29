use std::sync::LazyLock;

static WORKFLOW: LazyLock<String> = LazyLock::new(|| {
    include_str!("../.github/workflows/performance-experiment.yml").replace("\r\n", "\n")
});
// `SAMPLES` used to be `scripts/rh/performance-samples.rh`, and
// `TASK_MANIFEST` was asserted to carry the two experiment tasks. The script
// left with the rh engine on 2026-08-29 (partnernetsoftware/rh) and the tasks
// are dark until their .qjs ports land; the assertions on the script's body
// go with it, and the workflow-side assertions below are what remain.

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
