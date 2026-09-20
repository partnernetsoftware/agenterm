use std::sync::LazyLock;

static CANDIDATE: LazyLock<String> =
    LazyLock::new(|| include_str!("../.github/workflows/candidate.yml").replace("\r\n", "\n"));
static PROMOTION: LazyLock<String> =
    LazyLock::new(|| include_str!("../.github/workflows/release.yml").replace("\r\n", "\n"));
static INTEGRITY: LazyLock<String> = LazyLock::new(|| {
    include_str!("../.github/workflows/release-integrity.yml").replace("\r\n", "\n")
});
static RELEASE_POLICY: LazyLock<String> =
    LazyLock::new(|| include_str!("../release-policy.json").replace("\r\n", "\n"));
static WINDOWS_SIGNING_SCRIPT: LazyLock<String> = LazyLock::new(|| {
    include_str!("../scripts/windows-signing-candidate.ps1").replace("\r\n", "\n")
});
static CANDIDATE_AGGREGATE_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/candidate-aggregate.qjs").replace("\r\n", "\n"));
static RELEASE_CANDIDATE_QJS: LazyLock<String> = LazyLock::new(|| {
    include_str!("../scripts/qjs/lib/release_candidate.qjs").replace("\r\n", "\n")
});
static CU_RETIREMENT_CELL_QJS: LazyLock<String> = LazyLock::new(|| {
    include_str!("../scripts/qjs/cu-retirement-cell-smoke.qjs").replace("\r\n", "\n")
});
static ARTIFACT_VERIFICATION_QJS: LazyLock<String> = LazyLock::new(|| {
    include_str!("../scripts/qjs/artifact-verification.qjs").replace("\r\n", "\n")
});
static ARTIFACTS: LazyLock<serde_json::Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../scripts/artifacts.json"))
        .expect("scripts/artifacts.json must remain valid JSON")
});
static SIX_CELL_RUNNERS: LazyLock<serde_json::Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../scripts/six-cell-runners.json"))
        .expect("scripts/six-cell-runners.json must remain valid JSON")
});
static SIX_CELL_QUALIFY_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/six-cell-qualify.qjs").replace("\r\n", "\n"));
static BUILD_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/build.qjs").replace("\r\n", "\n"));
static BUILD_ALL_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/build-all.qjs").replace("\r\n", "\n"));
static ARTIFACT_FILES_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/lib/artifact_files.qjs").replace("\r\n", "\n"));
static PACKAGE_SIX_CELL_QJS: LazyLock<String> = LazyLock::new(|| {
    include_str!("../scripts/qjs/package-six-cell-delivery.qjs").replace("\r\n", "\n")
});
static CHECK_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/check.qjs").replace("\r\n", "\n"));
static SCRIPT_ENGINE_RS: LazyLock<String> =
    LazyLock::new(|| include_str!("../src/script_engine.rs").replace("\r\n", "\n"));
static PLATFORM_THREADING_RS: LazyLock<String> = LazyLock::new(|| {
    include_str!("../crates/agenterm-platform/src/threading.rs").replace("\r\n", "\n")
});
static DOC_REDACT_CHECK: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/doc-redact-check.sh").replace("\r\n", "\n"));
static INSTALL_SH: LazyLock<String> =
    LazyLock::new(|| include_str!("../install.sh").replace("\r\n", "\n"));
static INSTALL_CU_HOTKEYS_SH: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/install-cu-hotkeys.sh").replace("\r\n", "\n"));
static NATIVE_IPC_SMOKE_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/native-ipc-smoke.qjs").replace("\r\n", "\n"));
static SCRIPT_SMOKE_HELPERS_QJS: LazyLock<String> = LazyLock::new(|| {
    include_str!("../scripts/qjs/lib/script_smoke_helpers.qjs").replace("\r\n", "\n")
});
static SCRIPT_SMOKE_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/script-qjswasm-smoke.qjs").replace("\r\n", "\n"));
static WORKBENCH_SMOKE_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/workbench-smoke.qjs").replace("\r\n", "\n"));
static WORKBENCH_COURT_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/workbench-court.qjs").replace("\r\n", "\n"));
static CONTROL_CENTER_SMOKE_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/control-center-smoke.qjs").replace("\r\n", "\n"));
static CU_WINDOWS_SMOKE_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/cu-windows-smoke.qjs").replace("\r\n", "\n"));
static CU_WINDOWS_FIXTURE_CS: &str = include_str!("../examples/csharp/agenterm_uia_fixture.cs");
static TEST_HARNESS_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/lib/test_harness.qjs").replace("\r\n", "\n"));
static DIAGNOSTIC_BUNDLE_QJS: LazyLock<String> = LazyLock::new(|| {
    include_str!("../scripts/qjs/diagnostic-bundle-selftest.qjs").replace("\r\n", "\n")
});
const WINDOWS_RELEASE_SMOKES: &[(&str, &str)] = &[
    (
        "remote-ui-smoke",
        include_str!("../scripts/qjs/remote-ui-smoke.qjs"),
    ),
    (
        "remote-ui-upgrade-smoke",
        include_str!("../scripts/qjs/remote-ui-upgrade-smoke.qjs"),
    ),
    (
        "control-center-smoke",
        include_str!("../scripts/qjs/control-center-smoke.qjs"),
    ),
    (
        "theme-smoke",
        include_str!("../scripts/qjs/theme-smoke.qjs"),
    ),
    (
        "workbench-smoke",
        include_str!("../scripts/qjs/workbench-smoke.qjs"),
    ),
];
static PREFLIGHT_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/preflight.qjs").replace("\r\n", "\n"));
static INTERNAL_VERSION_QJS: LazyLock<String> = LazyLock::new(|| {
    include_str!("../scripts/qjs/internal-version-policy.qjs").replace("\r\n", "\n")
});
static AUTOMATION_AUDIT_QJS: LazyLock<String> = LazyLock::new(|| {
    include_str!("../scripts/qjs/cross-platform-automation-audit.qjs").replace("\r\n", "\n")
});
static TASKS: LazyLock<serde_json::Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../agenterm.tasks.json"))
        .expect("agenterm.tasks.json must remain valid JSON")
});
static QUALIFICATION_GATES: LazyLock<serde_json::Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../scripts/qualification-gates.json"))
        .expect("scripts/qualification-gates.json must remain valid JSON")
});
static GIT_ATTRIBUTES: LazyLock<String> =
    LazyLock::new(|| include_str!("../.gitattributes").replace("\r\n", "\n"));

const CHECKOUT_SHA: &str = "08eba0b27e820071cde6df949e0beb9ba4906955";
const UPLOAD_SHA: &str = "ea165f8d65b6e75b540449e92b4886f43607fa02";
const DOWNLOAD_SHA: &str = "fa0a91b85d4f404e444e00e005971372dc801d16";
const CACHE_SHA: &str = "0400d5f644dc74513175e3cd8d07132dd4860809";

#[test]
fn windows_cu_eight_mib_size_reference_is_consistent() {
    const CONTROL_CLI_BUDGET: u64 = 8 * 1024 * 1024;
    let budget_for = |artifacts: &serde_json::Value| {
        artifacts
            .as_array()
            .expect("artifact list")
            .iter()
            .find(|artifact| artifact["name"] == "agenterm-cu.exe")
            .and_then(|artifact| artifact["release_budget_bytes"].as_u64())
            .expect("agenterm-cu.exe release budget")
    };

    assert_eq!(budget_for(&ARTIFACTS["executables"]), CONTROL_CLI_BUDGET);
    for arch in ["x86_64", "aarch64"] {
        let platform = ARTIFACTS["platforms"]
            .as_array()
            .expect("platform list")
            .iter()
            .find(|platform| platform["os"] == "windows" && platform["arch"] == arch)
            .unwrap_or_else(|| panic!("missing Windows platform {arch}"));
        assert_eq!(budget_for(&platform["executables"]), CONTROL_CLI_BUDGET);
    }
}

/// For v0.1.17 the owner chose size reporting over a release-blocking size
/// ceiling. Hash, source, offline behavior, and the separate agenterm.com
/// staging safety bound remain enforced.
#[test]
fn release_size_is_observed_without_blocking_v0117() {
    assert!(ARTIFACT_VERIFICATION_QJS.contains("RELEASE SIZE executable="));
    assert!(ARTIFACT_VERIFICATION_QJS.contains("RELEASE SIZE library="));
    assert!(ARTIFACT_VERIFICATION_QJS.contains("actual_bytes="));
    assert!(ARTIFACT_VERIFICATION_QJS.contains("artifact_missing:"));
    assert!(ARTIFACT_VERIFICATION_QJS.contains("artifact_library_missing:"));
    assert!(ARTIFACT_VERIFICATION_QJS.contains("artifact_empty:"));
    assert!(ARTIFACT_VERIFICATION_QJS.contains("artifact_library_empty:"));
    assert!(ARTIFACT_VERIFICATION_QJS.contains("artifact_metadata_hash:"));
    assert!(ARTIFACT_VERIFICATION_QJS.contains("artifact_library_metadata_hash:"));
    assert!(!ARTIFACT_VERIFICATION_QJS.contains("actual_bytes <= budget_bytes"));
    assert!(
        include_str!("../scripts/qjs/stage-build.qjs").contains("stage_build_agenterm_com_budget")
    );
}

#[test]
fn mcp_test_cli_path_is_runtime_relocatable() {
    let source = include_str!("mcp_stdio.rs");
    assert!(source.contains("AGENTERM_TEST_BIN_DIR"));
    assert_eq!(
        source.matches("env!(\"CARGO_BIN_EXE_agenterm\")").count(),
        1
    );
    assert!(source.contains(".unwrap_or_else(agenterm_cli)"));
}

#[test]
fn target_inventory_keeps_default_host_ops_and_matches_its_outer_timeout() {
    let budget = &TASKS["contracts"]["target-report"]["budget"];
    assert_eq!(budget["timeout_ms"], 120_000);
    assert!(budget.get("max_host_operations").is_none());
    assert!(CHECK_QJS.contains("task(worker, repo, \"target-report\", 120000, [], no_env, 0)"));
}

#[test]
fn release_preflight_uses_its_declared_task_timeout() {
    let timeout = TASKS["contracts"]["preflight"]["budget"]["timeout_ms"]
        .as_u64()
        .expect("preflight timeout");
    assert_eq!(timeout, 120_000);
    assert_eq!(
        TASKS["contracts"]["preflight"]["budget"]["max_operations"],
        1_000_000_000
    );
    assert!(CHECK_QJS.contains("bootstrap_worker, repo, \"preflight\", 120000,"));
    assert!(PREFLIGHT_QJS.contains("const output_is_absolute = rh.is_absolute(output_arg);"));
    assert!(
        include_str!("../scripts/qjs/preflight-benchmark.qjs")
            .contains("if (rh.is_absolute(output_arg))")
    );
    assert!(
        include_str!("../scripts/qjs/preflight-benchmark.qjs")
            .contains("task_args(task_manifest, repo, run_report_relative, 30000)")
    );
}

#[test]
fn build_isolation_is_declared_and_fails_closed_before_mutation() {
    let task = TASKS["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "build")
        .expect("build task");
    let expected = serde_json::json!(["AGENTERM_BUILD_DIST_DIR", "CARGO_TARGET_DIR"]);
    assert!(task.get("env").is_none());
    assert_eq!(TASKS["contracts"]["build"]["env_allow"], expected);

    assert!(
        BUILD_QJS
            .contains("dist = isolated_build_lane(repo, \"AGENTERM_BUILD_DIST_DIR\", \"dist\")")
    );
    assert!(
        BUILD_QJS
            .contains("cargo_output = isolated_build_lane(repo, \"CARGO_TARGET_DIR\", \"target\")")
    );
    assert!(BUILD_QJS.contains("comparable_lane.startsWith(unix_prefix)"));
    assert!(BUILD_QJS.contains("!leaf.includes(\"/\")"));
    assert!(BUILD_QJS.contains("!lane_metadata.is_symlink"));
    assert!(BUILD_QJS.contains("build_isolation_path_not_repo_local_"));

    let validation = BUILD_QJS.find("isolated_build_lane(repo").unwrap();
    for mutation in [
        "rh.create_dir_all(incremental_root)",
        "task_arguments(\"stage-build\"",
        "[\"clean\", \"--target-dir\", cargo_output]",
    ] {
        assert!(validation < BUILD_QJS.find(mutation).unwrap(), "{mutation}");
    }
    assert!(BUILD_QJS.contains(
        "task_arguments(\"stage-build\", task_manifest, [repo, profile_directory, dist, profile])"
    ));
    assert!(BUILD_QJS.contains("if (profile === \"release\" && external_target === 0)"));
}

#[test]
fn full_check_prices_owned_process_sampling_without_raising_script_defaults() {
    let budget = &TASKS["contracts"]["check"]["budget"];
    assert_eq!(budget["timeout_ms"], 3_600_000);
    assert_eq!(budget["max_host_operations"], 32_768);
    assert_eq!(budget["max_operations"], 1_000_000_000);
}

#[test]
fn all_prd_alignment_lanes_honor_the_declared_120_second_contract() {
    let budget = &TASKS["contracts"]["prd-alignment"]["budget"];
    assert_eq!(budget["timeout_ms"], 120_000);
    assert!(
        CHECK_QJS.contains("direct_task(\n    bootstrap_worker, repo, \"prd-alignment\", 120000,")
    );
    assert!(CHECK_QJS.contains("task(worker, repo, \"prd-alignment\", 120000"));
    assert!(!CHECK_QJS.contains("task(worker, repo, \"prd-alignment\", 10000"));
}

#[test]
fn native_ipc_settings_paths_compare_host_separator_neutrally() {
    assert!(NATIVE_IPC_SMOKE_QJS.contains("function comparable_path(p)"));
    assert!(NATIVE_IPC_SMOKE_QJS.contains("replaceAll(\"\\\\\", \"/\")"));
    for path in [
        "native_settings_path",
        "expected_native_settings",
        "dev_settings_path",
        "dev_expected_settings",
        "override_protocol.settings_path",
        "context.settings_path",
    ] {
        assert!(
            NATIVE_IPC_SMOKE_QJS.contains(&format!("comparable_path({path})")),
            "missing separator-neutral comparison for {path}"
        );
    }
}

#[test]
fn diagnostic_bundle_path_identity_is_separator_and_dot_neutral_but_parent_exact() {
    assert!(
        DIAGNOSTIC_BUNDLE_QJS.contains("replaceAll(\"\\\\\", \"/\").toLowerCase().split(\"/\")")
    );
    assert!(DIAGNOSTIC_BUNDLE_QJS.contains("if (part === \"\" || part === \".\")"));
    assert!(DIAGNOSTIC_BUNDLE_QJS.contains("if (part === \"..\")"));
    assert!(DIAGNOSTIC_BUNDLE_QJS.contains("!normalized[normalized.length - 1].endsWith(\":\")"));
    assert!(DIAGNOSTIC_BUNDLE_QJS.contains(
        "comparable_path(path.parent(comparable_path(resolved))) === comparable_path(root)"
    ));
    assert!(
        !DIAGNOSTIC_BUNDLE_QJS
            .contains("path.parent(resolved).toLowerCase() === root.toLowerCase()")
    );
    assert!(DIAGNOSTIC_BUNDLE_QJS.contains("\"--max-operations\", \"1000000000\""));
    assert!(!DIAGNOSTIC_BUNDLE_QJS.contains("\"--max-operations\", \"100000000\"\n"));
}

#[test]
fn qualification_samples_the_exact_owned_gate_process_tree() {
    assert!(CHECK_QJS.contains("const handle = process_spawn(JSON.stringify(child_spec));"));
    assert!(CHECK_QJS.contains("const root_pid = process_pid(handle);"));
    assert!(CHECK_QJS.contains("+ sample_owned_processes(root_pid"));
    assert!(CHECK_QJS.contains("if (process_tree(root_pid) !== 0)"));
    assert!(CHECK_QJS.contains("result.observed_powershell, command_spec.allow_powershell"));
    assert!(!CHECK_QJS.contains("const process_samples = 0;"));
    assert!(CHECK_QJS.contains("function product_terminal_payload(tree, process, root_pid)"));
    assert!(CHECK_QJS.contains("if (parent_id === root_pid) { return false; }"));
    assert!(
        CHECK_QJS.contains("&& observed_terminal_powershell.length !== observed_powershell.length")
    );
}

#[test]
fn wake_court_uses_portable_raw_tcp_without_powershell_automation() {
    let wake = include_str!("../scripts/qjs/wake-smoke.qjs");
    assert!(wake.contains("\"python\", [\"-c\", python]"));
    assert!(wake.contains("socket.create_connection"));
    assert!(wake.contains("\"bash\", [\"-c\", sh]"));
    assert!(wake.contains("wake_smoke_raw_python:"));
    assert!(wake.contains("wake_smoke_raw_bash:"));
    assert!(!wake.to_ascii_lowercase().contains("\"powershell\""));
}

#[test]
fn only_owned_terminal_payload_courts_allow_powershell() {
    assert!(
        CHECK_QJS
            .contains("(id === \"remote-ui-smoke\" || id === \"native-ipc-compat-smoke\") ? 1 : 0")
    );
    assert!(
        CHECK_QJS.contains("a direct script shell-out remains\n// repository automation and fails")
    );
}

#[test]
fn windows_runtime_court_extracts_zip_with_the_python_it_already_uses() {
    let workflow = include_str!("../.github/workflows/candidate.yml");
    let windows = workflow
        .split("- name: Execute final Windows archive")
        .nth(1)
        .expect("Windows runtime court")
        .split("- name: Scan final Windows Candidate bytes with Defender")
        .next()
        .expect("Windows runtime court boundary");
    assert!(windows.contains("python -m zipfile -e \"$archive\" runtime"));
    assert!(!windows.contains("tar -xf \"$archive\""));
    assert!(windows.contains("json.load(open(sys.argv[1]"));
}

#[test]
fn candidate_seal_copies_only_closed_top_level_payload_files() {
    assert!(CANDIDATE.contains("find candidate-input -mindepth 1 -maxdepth 1 -type f"));
    assert!(CANDIDATE.contains("-exec cp -- {} candidate-output/payload/ \\;"));
    assert!(!CANDIDATE.contains("cp candidate-input/* candidate-output/payload/"));
}

#[test]
fn qualification_receipt_serializes_profile_flags_as_json_booleans() {
    let qualification = include_str!("../scripts/qjs/lib/qualification.qjs");
    let candidate = include_str!("../scripts/qjs/lib/release_candidate.qjs");
    assert!(qualification.contains("release: context.release !== 0"));
    assert!(qualification.contains("stress_included: context.stress_included !== 0"));
    assert!(candidate.contains("receipt.release === true"));
    assert!(candidate.contains("receipt.stress_included === true"));
}

#[test]
fn powershell_launcher_test_is_an_explicit_terminal_compatibility_subcourt() {
    assert!(CHECK_QJS.contains("\"--skip\", \"powershell_waits_for_explicit_agenterm_exe\""));
    assert!(CHECK_QJS.contains("function cargo_unit_powershell_compat_spec(environment)"));
    assert!(
        CHECK_QJS.contains("powershell_waits_for_explicit_agenterm_exe\", \"--\", \"--exact\"")
    );
    assert!(CHECK_QJS.contains("], 120000, environment, 2);"));
    assert!(CHECK_QJS.contains("cargo_unit_powershell_compat_spec(build_environment)"));
    let build_spec = CHECK_QJS
        .split_once("function cargo_unit_powershell_build_spec(environment) {")
        .and_then(|(_, tail)| tail.split_once("\n}"))
        .map(|(body, _)| body)
        .expect("PowerShell compatibility target must have a separate compile step");
    assert!(build_spec.contains("\"--test\", \"agenterm_cli_forwarding\", \"--no-run\""));
    assert!(build_spec.contains("], 600000, environment, 0);"));
    let build_call = CHECK_QJS
        .find("cargo_unit_powershell_build_spec(build_environment)")
        .expect("precompile step must run in the unit gate");
    let court_call = CHECK_QJS
        .find("cargo_unit_powershell_compat_spec(build_environment)")
        .expect("exact PowerShell court must run in the unit gate");
    assert!(
        build_call < court_call,
        "precompile must precede the 120-second behavior court"
    );
}

#[test]
fn release_fast_fixture_separates_cold_compilation_from_the_bounded_build() {
    let prebuild = CHECK_QJS
        .split_once("function cargo_release_fast_fixture_spec(package_name, environment) {")
        .and_then(|(_, tail)| tail.split_once("\n}"))
        .map(|(body, _)| body)
        .expect("release-fast fixture prebuild spec");
    assert!(prebuild.contains("\"build\", \"--locked\", \"--profile\", \"release-fast\""));
    assert!(prebuild.contains("\"--package\", package_name"));
    assert!(prebuild.contains("], 900000, environment, 0);"));
    let abi_prebuild = CHECK_QJS
        .split_once("function cargo_abi_release_fixture_spec(package_name, target, environment) {")
        .and_then(|(_, tail)| tail.split_once("\n}"))
        .map(|(body, _)| body)
        .expect("ABI release fixture prebuild spec");
    assert!(abi_prebuild.contains("\"--profile\", \"abi-release\", \"--target\", target"));
    assert!(abi_prebuild.contains("\"--package\", package_name"));
    assert!(abi_prebuild.contains("], 900000, environment, 0);"));

    let fixture = CHECK_QJS
        .split_once("// Cold release-fast compilation is not the 300 s artifact fixture court.")
        .and_then(|(_, tail)| tail.split_once("copy_release_fast_fixture(repo, upgrade_fixture)"))
        .map(|(body, _)| body)
        .expect("release-fast fixture gate");
    let root = fixture
        .find("cargo_release_fast_fixture_spec(\"agenterm\", build_environment)")
        .expect("prebuild the GUI package");
    let cu = fixture
        .find("cargo_release_fast_fixture_spec(\"agenterm-cu\", build_environment)")
        .expect("prebuild CU separately");
    let abi = fixture
        .find("cargo_abi_release_fixture_spec(\"agenterm-abi\", fixture_target, build_environment)")
        .expect("prebuild ABI at the build script's target");
    let provider = fixture
        .find("cargo_abi_release_fixture_spec(\"agenterm-cu-provider\", fixture_target, build_environment)")
        .expect("prebuild CU provider separately");
    let bounded = fixture
        .find("cmd_build_bat_spec(repo, \"release-fast\", 300000, build_environment)")
        .expect("retain the 300-second artifact build");
    assert!(root < cu && cu < abi && abi < provider && provider < bounded);
    assert!(fixture.contains("run_gate_specs("));
    assert!(fixture.contains("const fixture_target = host_target_triple(repo);"));
}

#[test]
fn primary_unit_spec_keeps_both_packages_and_the_explicit_skip_set() {
    let spec = CHECK_QJS
        .split_once("function cargo_unit_primary_spec(environment) {")
        .and_then(|(_, tail)| tail.split_once("\n}"))
        .map(|(body, _)| body)
        .expect("primary unit spec");
    assert!(spec.contains("\"-p\", \"agenterm\", \"-p\", \"agenterm-ui-core\""));
    let skipped = [
        "preflight_task_is_fail_closed_and_writes_reports_for_real_git_fixtures",
        "preflight_benchmark_task_measures_clean_public_worker_runs",
        "prd_alignment_task_matches_public_catalogs_and_fails_closed",
        "supply_chain_task_is_deterministic_and_covers_the_resolved_lock_graph",
        "rhai_working_context_smoke_is_private_ephemeral_and_orphan_free",
        "rhai_server_smoke_preserves_headless_authority_and_cleanup",
        "rhai_wake_smoke_preserves_concurrent_ipc_pty_and_expired_mutation",
        "rhai_startup_smoke_preserves_first_window_and_async_terminal_contract",
        "rhai_cli_smoke_preserves_public_control_ui_bridge_and_pty_contract",
        "rhai_fleet_smoke_preserves_discovery_event_launch_and_mux_contract",
        "rhai_remote_ui_smoke_preserves_replaceable_client_and_reconnect_contract",
        "rhai_script_smoke_preserves_unrestricted_runtime_and_supervisor_contract",
        "rhai_theme_smoke_preserves_native_rendering_pty_and_restart_contract",
        "rhai_workbench_smoke_preserves_physical_editing_and_compact_tree_contract",
        "rhai_diagnostic_bundles_are_bounded_private_and_orphan_free",
        "rhai_harness_cleanup_owns_only_registered_children",
        "migration_audit_rejects_operational_references_to_deleted_scripts",
        "rhai_qualification_contract_fails_closed_and_cleans_owned_scratch",
        "rhai_qualified_package_accepts_only_the_exact_receipt_bytes",
        "uses_bundled_pack",
        "uses_native_bundled_pack",
        "powershell_waits_for_explicit_agenterm_exe",
        "pack_builds",
        "native_pack_",
        "source_cache_is_stable_for_same_source",
        "script_engine_exec_parity_",
        "native_for_fixtures_qualify_with_expected_entry_values",
        "_executes_natively_without_interpreter",
        "native_pack_executes_without_interpreter",
    ];
    assert_eq!(spec.matches("\"--skip\"").count(), skipped.len());
    for name in skipped {
        assert!(
            spec.contains(&format!("\"--skip\", \"{name}\"")),
            "primary unit spec lost the explicit skip for {name}"
        );
    }
    assert!(CHECK_QJS.contains("cargo_unit_primary_spec(build_environment)"));
}

#[test]
fn jw1_host_denial_is_explicit_and_candidate_requires_independent_positive_evidence() {
    assert!(SCRIPT_ENGINE_RS.contains("error.code == \"managed_job_detach_unavailable\""));
    assert!(SCRIPT_ENGINE_RS.contains("record[\"state\"][\"code\"], \"owner_detach_unavailable\""));
    assert!(SCRIPT_ENGINE_RS.contains("EVIDENCE jw1_host_detach=BLOCKED"));
    assert!(CANDIDATE.contains("Prove JW1 causal cancellation with an independent resident owner"));
    assert!(CANDIDATE.contains("grep -q 'JW1 causal composite evidence:'"));
    assert!(!CHECK_QJS.contains("\"--skip\", \"jw1_recording_bridge_causal_composite\""));
}

#[test]
fn qjswasm_adversarial_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_qjswasm_adversarial_spec(environment)"));
    assert!(CHECK_QJS.contains("\"-p\", \"agenterm-qjswasm\""));
    assert!(CHECK_QJS.contains("\"--test\", \"door_attack\", \"--test\", \"seam_attack\""));
    assert!(CHECK_QJS.contains("cargo_unit_qjswasm_adversarial_spec(build_environment)"));
    assert!(CHECK_QJS.contains("gate = run_gate_specs(\n  context, timing, \"unit-tests\""));
}

#[test]
fn qjswasm_tool_door_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_qjswasm_tool_door_spec(environment)"));
    assert!(CHECK_QJS.contains("\"-p\", \"agenterm-qjswasm\", \"--test\", \"tool_door\""));
    assert!(CHECK_QJS.contains("cargo_unit_qjswasm_tool_door_spec(build_environment)"));
    assert!(CHECK_QJS.contains("gate = run_gate_specs(\n  context, timing, \"unit-tests\""));
}

#[test]
fn qjswasm_guest_seam_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_qjswasm_guest_spec(environment)"));
    assert!(CHECK_QJS.contains("\"-p\", \"agenterm-qjswasm\", \"--test\", \"qjs_guest\""));
    assert!(CHECK_QJS.contains("cargo_unit_qjswasm_guest_spec(build_environment)"));
}

#[test]
fn qjswasm_product_door_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_qjswasm_door_spec(environment)"));
    assert!(CHECK_QJS.contains("\"-p\", \"agenterm-qjswasm\", \"--test\", \"qjs_door\""));
    assert!(CHECK_QJS.contains("cargo_unit_qjswasm_door_spec(build_environment)"));
}

#[test]
fn qjswasm_host_door_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_qjswasm_host_door_spec(environment)"));
    assert!(CHECK_QJS.contains("\"-p\", \"agenterm-qjswasm\", \"--test\", \"host_door\""));
    assert!(CHECK_QJS.contains("cargo_unit_qjswasm_host_door_spec(build_environment)"));
}

#[test]
fn qjswasm_internal_mechanism_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_qjswasm_lib_spec(environment)"));
    assert!(
        CHECK_QJS.contains(
            "\"test\", \"--quiet\", \"--locked\", \"-p\", \"agenterm-qjswasm\", \"--lib\""
        )
    );
    assert!(CHECK_QJS.contains("cargo_unit_qjswasm_lib_spec(build_environment)"));
}

#[test]
fn qjswasm_native_door_tests_are_an_explicit_full_gate_subcourt() {
    let spec = CHECK_QJS
        .split_once("function cargo_unit_qjswasm_native_door_spec(environment) {")
        .and_then(|(_, tail)| tail.split_once("\n}"))
        .map(|(body, _)| body)
        .expect("qjswasm native-door unit spec");
    assert!(spec.contains("\"-p\", \"agenterm-qjswasm\""));
    assert!(spec.contains("\"--test\", \"native_door_schema\""));
    assert!(spec.contains("\"--test\", \"native_door\""));
    assert!(CHECK_QJS.contains("cargo_unit_qjswasm_native_door_spec(build_environment)"));
}

#[test]
fn qjswasm_acu_door_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_qjswasm_acu_door_spec(environment)"));
    assert!(CHECK_QJS.contains("\"-p\", \"agenterm-qjswasm\", \"--test\", \"acu_door\""));
    assert!(CHECK_QJS.contains("cargo_unit_qjswasm_acu_door_spec(build_environment)"));
}

#[test]
fn qjswasm_syntax_contract_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_qjswasm_syntax_contract_spec(environment)"));
    assert!(CHECK_QJS.contains("\"-p\", \"agenterm-qjswasm\", \"--test\", \"syntax_contract\""));
    assert!(CHECK_QJS.contains("cargo_unit_qjswasm_syntax_contract_spec(build_environment)"));
}

#[test]
fn qjswasm_slot_contract_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_qjswasm_slot_contracts_spec(environment)"));
    assert!(CHECK_QJS.contains(
        "\"--test\", \"allocation_probe\", \"--test\", \"isolation\", \"--test\", \"wasm_slot\""
    ));
    assert!(CHECK_QJS.contains("cargo_unit_qjswasm_slot_contracts_spec(build_environment)"));
}

#[test]
fn vnc_pure_contract_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_vnc_pure_contracts_spec(environment)"));
    assert!(
        CHECK_QJS.contains("\"--test\", \"ard\", \"--test\", \"des\", \"--test\", \"framebuffer\"")
    );
    assert!(CHECK_QJS.contains("cargo_unit_vnc_pure_contracts_spec(build_environment)"));
}

#[test]
fn dyn_mechanism_contracts_are_an_explicit_full_gate_subcourt() {
    let spec = CHECK_QJS
        .split_once("function cargo_unit_dyn_spec(environment) {")
        .and_then(|(_, tail)| tail.split_once("\n}"))
        .map(|(body, _)| body)
        .expect("dyn unit spec");
    assert!(spec.contains("\"-p\", \"agenterm-dyn\""));
    assert!(!spec.contains("--all-features"));
    assert!(!spec.contains("--features"));
    assert!(!spec.contains("--lib"));
    assert!(!spec.contains("--test"));
    assert!(CHECK_QJS.contains("cargo_unit_dyn_spec(build_environment)"));
}

#[test]
fn cu_provider_abi_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_cu_provider_spec(environment)"));
    assert!(
        CHECK_QJS
            .contains("\"--profile\", \"abi-dev\", \"-p\", \"agenterm-cu-provider\", \"--lib\",\n    \"--test\", \"exports_set\"")
    );
    assert!(CHECK_QJS.contains("cargo_unit_cu_provider_spec(build_environment)"));
}

#[test]
fn script_common_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_script_common_spec(environment)"));
    assert!(CHECK_QJS.contains("\"-p\", \"agenterm-script-common\", \"--lib\""));
    assert!(CHECK_QJS.contains("cargo_unit_script_common_spec(build_environment)"));
}

#[test]
fn chassis_default_feature_tests_are_an_explicit_full_gate_subcourt() {
    let spec = CHECK_QJS
        .split_once("function cargo_unit_chassis_spec(environment) {")
        .and_then(|(_, tail)| tail.split_once("\n}"))
        .map(|(body, _)| body)
        .expect("chassis unit spec");
    assert!(spec.contains("\"-p\", \"agenterm-chassis\""));
    assert!(!spec.contains("--all-features"));
    assert!(!spec.contains("--features"));
    assert!(CHECK_QJS.contains("cargo_unit_chassis_spec(build_environment)"));
}

#[test]
fn platform_unconditional_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_platform_unconditional_spec(environment)"));
    assert!(CHECK_QJS.contains(
        "\"-p\", \"agenterm-platform\", \"--lib\",\n    \"--\", \"--skip\", \"threading::\""
    ));
    assert!(CHECK_QJS.contains("cargo_unit_platform_unconditional_spec(build_environment)"));
    assert!(PLATFORM_THREADING_RS.contains("fn detached_task_runs_with_the_requested_os_name"));
    assert!(PLATFORM_THREADING_RS.contains("fn panic_is_contained_and_unwinds_the_detached_task"));
}

#[test]
fn abi_pure_contracts_are_an_explicit_full_gate_subcourt() {
    let spec = CHECK_QJS
        .split_once("function cargo_unit_abi_pure_contracts_spec(environment) {")
        .and_then(|(_, tail)| tail.split_once("\n}"))
        .map(|(body, _)| body)
        .expect("ABI pure-contract unit spec");
    assert!(spec.contains("\"-p\", \"agenterm-abi\", \"--features\", \"allow-abort-profile\""));
    assert!(spec.contains(
        "\"--lib\", \"--test\", \"exports_set\", \"--test\", \"capability_enum_gate\", \"--test\", \"pkgconfig_libs\""
    ));
    for forbidden in [
        "dylib_load",
        "null_sweep",
        "c_consumer",
        "desktop_host_contract",
    ] {
        assert!(
            !spec.contains(forbidden),
            "ABI pure spec selected {forbidden}"
        );
    }
    assert!(CHECK_QJS.contains("cargo_unit_abi_pure_contracts_spec(build_environment)"));
}

#[test]
fn grouped_gates_refuse_an_empty_command_set() {
    assert!(CHECK_QJS.contains("command_specs.length > 0"));
    assert!(CHECK_QJS.contains("qualification_gate_specs_empty:"));
}

#[test]
fn sql_engine_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_sql_spec(environment)"));
    assert!(CHECK_QJS.contains("\"test\", \"--quiet\", \"--locked\", \"-p\", \"agenterm-sql\""));
    assert!(CHECK_QJS.contains("cargo_unit_sql_spec(build_environment)"));
    assert!(CHECK_QJS.contains("gate = run_gate_specs(\n  context, timing, \"unit-tests\""));
}

#[test]
fn lua_engine_tests_are_an_explicit_full_gate_subcourt() {
    assert!(CHECK_QJS.contains("function cargo_unit_lua_spec(environment)"));
    assert!(CHECK_QJS.contains("\"test\", \"--quiet\", \"--locked\", \"-p\", \"agenterm-lua\""));
    assert!(CHECK_QJS.contains("cargo_unit_lua_spec(build_environment)"));
    assert!(CHECK_QJS.contains("gate = run_gate_specs(\n  context, timing, \"unit-tests\""));
}

#[test]
fn windows_release_smokes_have_no_live_qjs_migration_gap() {
    for (name, source) in WINDOWS_RELEASE_SMOKES {
        for gap in [
            "remote_ui_gap:",
            "remote_upgrade_gap:",
            "gap:rh_image_inspect_png_unavailable",
            "gap:process_window_key_unavailable",
            "qjs_gap:",
        ] {
            assert!(!source.contains(gap), "{name} still contains {gap}");
        }
    }

    let cu_entry = CHECK_QJS
        .split("if (id === \"cu-windows-smoke\")")
        .nth(1)
        .and_then(|source| source.split("throw \"check_task_unknown:").next())
        .expect("Windows CU catalog entry");
    assert!(cu_entry.contains("args: [repo, path.join(repo, \"dist/agenterm-cu.exe\")]"));
    assert!(!cu_entry.contains("dist/agenterm.dll"));
    assert!(!cu_entry.contains("agenterm_plain_window.c"));
    assert!(CU_WINDOWS_SMOKE_QJS.contains(
        "const fixture_template = rh.join(repo, \"examples/csharp/agenterm_uia_fixture.cs\");"
    ));
    assert!(
        CU_WINDOWS_SMOKE_QJS.contains(
            "const fixture_source = rh.join(run_directory, \"agenterm-uia-fixture.cs\");"
        )
    );
    assert!(CU_WINDOWS_SMOKE_QJS.contains("csc_path(fixture_source)"));
    assert!(CU_WINDOWS_SMOKE_QJS.contains("program: windows_inbox(\"ping.exe\")"));
    assert!(CU_WINDOWS_SMOKE_QJS.contains("args: [\"-n\", \"3\", \"127.0.0.1\"]"));
    assert!(CU_WINDOWS_SMOKE_QJS.contains("if (short_child >= 0 && !short_reaped)"));
    assert!(CU_WINDOWS_FIXTURE_CS.contains("[STAThread]"));
    assert!(CU_WINDOWS_FIXTURE_CS.contains("public static void Main(string[] args)"));
    assert!(CU_WINDOWS_FIXTURE_CS.contains("Run(args.Length == 1 ? args[0] : null);"));
}

#[test]
fn control_center_smoke_distinguishes_json_null_from_missing_fields() {
    assert_eq!(
        CONTROL_CENTER_SMOKE_QJS
            .matches("connected_server === null")
            .count(),
        3
    );
    assert!(!CONTROL_CENTER_SMOKE_QJS.contains("connected_server === undefined"));
    assert!(CONTROL_CENTER_SMOKE_QJS.contains("live_snapshot.server_reason === null"));
    assert!(!CONTROL_CENTER_SMOKE_QJS.contains("live_snapshot.server_reason === undefined"));
}

#[test]
fn control_center_children_use_the_doors_cross_platform_pid() {
    let start = TEST_HARNESS_QJS
        .split("export function start_child")
        .nth(1)
        .and_then(|source| source.split("export function child_state").next())
        .expect("start_child body");
    assert!(start.contains("const handle = start(spec);"));
    assert!(start.contains("const pid = process_pid(handle);"));
    assert!(!start.contains("program: \"sh\""));
    assert!(!start.contains("echo $$"));
}

#[test]
fn test_harness_journal_serializes_once_and_appends_without_quadratic_rewrite() {
    assert!(TEST_HARNESS_QJS.contains(
        "door(fs_append(context.command_journal_path, record_text + \"\\n\"), \"fs_append command journal\")"
    ));
    assert_eq!(
        TEST_HARNESS_QJS.matches("JSON.stringify(record)").count(),
        1
    );
    assert_eq!(
        TEST_HARNESS_QJS
            .matches("publish_command_record(context, record);")
            .count(),
        4
    );
    assert!(!TEST_HARNESS_QJS.contains("journal + JSON.stringify(record)"));
    assert!(!TEST_HARNESS_QJS.contains("JSON.stringify([record])"));
}

#[test]
fn control_center_projection_owner_outlives_short_command_helpers() {
    let title_waiter = CONTROL_CENTER_SMOKE_QJS
        .split("function wait_for_window_title")
        .nth(1)
        .and_then(|source| source.split("function wait_for_exit").next())
        .expect("window title waiter body");
    assert!(CONTROL_CENTER_SMOKE_QJS.contains("spec.timeout_ms = 15000;"));
    assert!(CONTROL_CENTER_SMOKE_QJS.contains("spec.timeout_ms = 90000;"));
    assert_eq!(
        CONTROL_CENTER_SMOKE_QJS
            .matches("configured_owner(context, control_center")
            .count(),
        3
    );
    assert!(
        CONTROL_CENTER_SMOKE_QJS
            .contains("_early_exit:exit=\" + output.exit_code + \":\" + output.stderr")
    );
    assert!(title_waiter.contains("const deadline = rh.now_ms() + 10000;"));
    assert!(title_waiter.contains("_title_timeout:\" + last_observation"));
    assert!(title_waiter.contains("const facts = platform_facts(child);"));
    assert!(title_waiter.contains("facts.top_level_window_present"));
    assert!(!title_waiter.contains("[\"screenshot\", \"--output\", \"NUL\""));
    assert!(!title_waiter.contains("document.rendered_snapshot.window_title"));
    assert!(!title_waiter.contains("attempt < 400"));

    let projection_waiter = CONTROL_CENTER_SMOKE_QJS
        .split("function wait_for_native_projection")
        .nth(1)
        .and_then(|source| source.split("function state_file_facts").next())
        .expect("native projection waiter body");
    assert!(projection_waiter.contains("[\"snapshot\", \"--json\"]"));
    assert!(projection_waiter.contains("const facts = platform_facts(child);"));
    assert!(
        projection_waiter.contains("[\"screenshot\", \"--output\", output_path_text, \"--json\"]")
    );
    assert!(projection_waiter.contains("document.capture_strategy === \"direct-native-window\""));
    assert!(projection_waiter.contains("source: \"semantic-snapshot+native-window-facts\""));
    assert!(!projection_waiter.contains("document.rendered_snapshot"));
}

#[test]
fn long_ui_smokes_price_their_bump_heaps_without_raising_the_default() {
    let check = include_str!("../scripts/qjs/check.qjs");
    let engine = include_str!("../src/script_engine.rs");
    assert!(
        check.contains("AGENTERM_QJS_MAX_MEMORY_PAGES: \"4096\"")
            && check.contains("id === \"remote-ui-smoke\" || id === \"workbench-smoke\""),
        "the two measured long GUI courts must opt into their 256 MiB heaps"
    );
    assert!(
        engine.contains("pub(crate) const QJS_MAX_MEMORY_PAGES: usize = 1024;"),
        "ordinary qjswasm invocations must retain the 64 MiB default"
    );
}

#[test]
fn workbench_render_waits_reuse_the_snapshot_that_satisfied_the_wait() {
    assert!(WORKBENCH_SMOKE_QJS.contains("return snapshot;"));
    assert!(
        !WORKBENCH_SMOKE_QJS
            .contains("wait_tab_render(context, cli, root);\n  snapshot = json_cli")
    );
    assert!(!WORKBENCH_SMOKE_QJS.contains(
        "wait_tab_label_render(context, cli, target);\n    const width_snapshot = json_cli"
    ));
}

#[test]
fn workbench_court_splits_one_public_gate_without_dropping_evidence() {
    let task = TASKS["tasks"]
        .as_array()
        .expect("task catalog array")
        .iter()
        .find(|task| task["id"] == "workbench-smoke")
        .expect("workbench-smoke task");
    assert_eq!(task["entry"], "scripts/qjs/workbench-court.qjs");
    assert_eq!(
        TASKS["contracts"]["workbench-smoke"]["budget"]["timeout_ms"],
        1_000_000
    );

    let gate = QUALIFICATION_GATES["required_gates"]
        .as_array()
        .expect("qualification gate array")
        .iter()
        .find(|gate| gate["id"] == "workbench-smoke")
        .expect("workbench-smoke qualification gate");
    assert_eq!(gate["suite"], "workbench-court");
    assert!(CHECK_QJS.contains("return { entry: \"workbench-court\", args: [repo"));

    assert!(WORKBENCH_COURT_QJS.contains(
        "for (const phase of [\"editing\", \"scroll\", \"width-180\", \"width-250\", \"width-480\"])"
    ));
    assert!(WORKBENCH_COURT_QJS.contains("scripts/qjs/workbench-smoke.qjs"));
    assert!(WORKBENCH_COURT_QJS.contains("args: ["));
    assert!(!WORKBENCH_COURT_QJS.contains("arguments:"));
    assert!(WORKBENCH_COURT_QJS.contains("AGENTERM_QJS_MAX_MEMORY_PAGES: \"4096\""));
    assert!(WORKBENCH_COURT_QJS.contains("stdout_path: stdout_path"));
    assert!(WORKBENCH_COURT_QJS.contains("const line_text = line.trim();"));
    assert!(WORKBENCH_COURT_QJS.contains("\"--max-operations\", \"1000000000\""));
    assert!(WORKBENCH_COURT_QJS.contains("\"--timeout-ms\", \"120000\""));
    assert!(WORKBENCH_COURT_QJS.contains("timeout_ms: 180000"));
    assert!(CHECK_QJS.contains("return 1000000;"));
    for evidence in [
        "ux.workbench-inline-edit",
        "ux.workbench-compact-tree",
        "ux.workbench-proxy-archived",
    ] {
        assert!(WORKBENCH_COURT_QJS.contains(evidence));
    }
    assert!(WORKBENCH_SMOKE_QJS.contains("if (phase === \"editing\")"));
    assert!(WORKBENCH_SMOKE_QJS.contains("if (phase === \"scroll\")"));
    for phase in ["width-180", "width-250", "width-480"] {
        assert!(WORKBENCH_SMOKE_QJS.contains(&format!("phase === \"{phase}\"")));
    }
    assert!(
        WORKBENCH_SMOKE_QJS.contains("\"new-window\", \"-d\", \"-n\", child_name, \"--parent\"")
    );
    assert!(WORKBENCH_SMOKE_QJS.contains("[\"set-tab-note\", \"-t\", target, child_note]"));
    assert!(WORKBENCH_SMOKE_QJS.contains("[\"ui-action\", \"select-tab\", \"-t\", target]"));
    assert!(WORKBENCH_SMOKE_QJS.contains("tab_render.node.x >= 0"));
    assert!(WORKBENCH_SMOKE_QJS.contains("tab_render.node.y >= 0"));
    assert!(!WORKBENCH_SMOKE_QJS.contains("tab_render.node.width"));
    assert!(WORKBENCH_SMOKE_QJS.contains("set_tab_editor_text(gui_child, child_name, child_note)"));
    assert!(WORKBENCH_SMOKE_QJS.contains("expected: REPO GUI_EXE CLI_EXE --phase PHASE"));
    assert!(!WORKBENCH_SMOKE_QJS.contains("phase = \"all\""));
}

#[test]
fn fleet_stress_prices_its_intentional_client_fanout_at_the_owning_court() {
    let budget = &TASKS["contracts"]["fleet-smoke"]["budget"];
    assert_eq!(budget["max_host_operations"], 16_384);
    assert!(CHECK_QJS.contains("entry === \"fleet-smoke\""));
    assert!(CHECK_QJS.contains("arguments_list.push(\"16384\")"));
}

#[test]
fn script_api_catalog_streams_to_a_run_owned_file_before_guest_parsing() {
    assert!(SCRIPT_SMOKE_HELPERS_QJS.contains("spec.stdout_path = spool_path"));
    assert!(!SCRIPT_SMOKE_HELPERS_QJS.contains("rh.atomic_write(spool_path, output.stdout)"));
}

#[test]
fn script_smoke_executes_its_declared_complete_catalog_string_budget() {
    let budget = &TASKS["contracts"]["script-smoke"]["budget"];
    assert_eq!(budget["max_string_bytes"], 8_388_608);
    assert!(CHECK_QJS.contains("return { entry: \"script-qjswasm-smoke\", args: [repo"));
    assert!(CHECK_QJS.contains("entry === \"script-qjswasm-smoke\""));
    assert!(CHECK_QJS.contains("arguments_list.push(\"8388608\")"));
    assert!(SCRIPT_SMOKE_QJS.contains("\"--max-string-bytes\", \"8388608\", \"--\""));
}

#[test]
fn script_process_court_uses_the_qjs_tool_door_not_a_retired_rhai_child() {
    assert!(SCRIPT_SMOKE_QJS.contains("process_command(JSON.stringify(spec))"));
    assert!(SCRIPT_SMOKE_QJS.contains("AGENTERM_SCRIPT_BACKEND: \"qjswasm\""));
    assert!(SCRIPT_SMOKE_QJS.contains("script.qjs-tool-process"));
    assert!(!SCRIPT_SMOKE_QJS.contains("process.rh"));
    assert!(!SCRIPT_SMOKE_QJS.contains("rh::task"));
    assert!(!SCRIPT_SMOKE_QJS.contains("std::process::command"));
    assert!(!SCRIPT_SMOKE_QJS.contains("script.rh-"));
}

#[test]
fn script_release_court_emits_only_active_qjswasm_evidence() {
    let task = TASKS["tasks"]
        .as_array()
        .expect("task catalog array")
        .iter()
        .find(|task| task["id"] == "script-smoke")
        .expect("script-smoke task");
    assert_eq!(task["entry"], "scripts/qjs/script-qjswasm-smoke.qjs");

    let gate = QUALIFICATION_GATES["required_gates"]
        .as_array()
        .expect("qualification gate array")
        .iter()
        .find(|gate| gate["id"] == "script-smoke")
        .expect("script-smoke qualification gate");
    assert_eq!(gate["suite"], "script-qjswasm-smoke");
    assert!(CHECK_QJS.contains("typeof gate.suite === \"string\""));
    assert!(CHECK_QJS.contains("evidence_list_spec(worker, repo, suite_id)"));
    let evidence = gate["evidence"].as_array().expect("script evidence array");
    assert_eq!(evidence.len(), 7);
    assert!(evidence.iter().all(|id| {
        let id = id.as_str().expect("evidence id");
        SCRIPT_SMOKE_QJS.contains(&format!("\"{id}\""))
            && !id.starts_with("script.rh-")
            && id != "script.http"
            && id != "script.modules-tasks"
    }));
}

#[test]
fn remote_ui_selection_checks_cell_ownership_not_global_event_stasis() {
    let smoke = include_str!("../scripts/qjs/remote-ui-smoke.qjs");
    let start = smoke
        .find("const selection_armed =")
        .expect("selection ownership court exists");
    let end = smoke[start..]
        .find("const selection_completed =")
        .map(|offset| start + offset)
        .expect("selection court has a completion boundary");
    let court = &smoke[start..end];
    assert!(
        court.contains("terminal_interaction.selection.selection")
            && court.contains("terminal_interaction.selection.selection.start"),
        "the court must compare the selected terminal cells and drag anchor"
    );
    assert!(
        !court.contains("selection_armed.event_position")
            && !court.contains("selection_prepared.event_position")
            && !court.contains("selection_dragging.event_position"),
        "send-keys advances the server journal; root event_position cannot be a selection anchor"
    );
}

#[test]
fn remote_ui_paste_enablement_has_a_known_clipboard_precondition() {
    let smoke = include_str!("../scripts/qjs/remote-ui-smoke.qjs");
    let seed = smoke
        .find("write_clipboard_text(\"REMOTE_COPY_SENTINEL\")")
        .expect("selection court seeds the clipboard");
    let snapshot = smoke[seed..]
        .find("const selection_completed =")
        .map(|offset| seed + offset)
        .expect("selection completion snapshot follows the seed");
    let paste_gate = smoke[snapshot..]
        .find("selection_completed.system_menu.paste.enabled")
        .map(|offset| snapshot + offset)
        .expect("selection court checks paste enablement");
    assert!(seed < snapshot && snapshot < paste_gate);
    assert_eq!(
        smoke
            .matches("write_clipboard_text(\"REMOTE_COPY_SENTINEL\")")
            .count(),
        1,
        "copy must consume the same known sentinel instead of masking the precondition"
    );
}

#[test]
fn candidate_is_manual_exact_sha_and_has_no_publish_authority() {
    assert!(CANDIDATE.contains("name: Release Candidate"));
    assert!(CANDIDATE.contains("workflow_dispatch:"));
    assert!(CANDIDATE.contains("source_sha:"));
    assert!(!CANDIDATE.contains("\n  push:"));
    assert!(CANDIDATE.contains("actions: read\n  contents: read"));
    assert!(!CANDIDATE.contains("contents: write"));
    assert!(CANDIDATE.contains("[[ \"$SOURCE_SHA\" =~ ^[0-9a-f]{40}$ ]]"));
    assert!(CANDIDATE.contains("[[ \"$GITHUB_SHA\" == \"$SOURCE_SHA\" ]]"));
    assert!(CANDIDATE.contains("[[ \"$(git rev-parse origin/main)\" == \"$SOURCE_SHA\" ]]"));
    assert!(!CANDIDATE.contains("workflows/$workflow/runs?head_sha=$SOURCE_SHA"));
    assert!(!CANDIDATE.contains("for workflow in ci-agenterm.yml"));
    assert!(CANDIDATE.contains("name: Verify exact current main source"));
    assert!(CANDIDATE.contains("ref: ${{ inputs.source_sha }}"));
    assert!(CANDIDATE.contains("AGENTERM_CANDIDATE_SOURCE_SHA: ${{ inputs.source_sha }}"));
    assert!(CANDIDATE.contains("git switch -C main \"%SOURCE_SHA%\""));
}

#[test]
fn candidate_scans_the_full_tracked_public_text_before_building() {
    let preflight = CANDIDATE
        .split_once("  preflight:\n")
        .and_then(|(_, tail)| tail.split_once("\n  build:\n"))
        .map(|(preflight, _)| preflight)
        .expect("one preflight job before build");
    assert!(preflight.contains("name: Scan tracked public text for disclosures"));
    assert!(preflight.contains("run: ./scripts/doc-redact-check.sh"));
    assert!(DOC_REDACT_CHECK.contains("'*.js' '*.json' '*.qjs')"));
}

#[test]
fn windows_candidate_retains_script_worker_crash_diagnostics() {
    let quality = CANDIDATE
        .split("      - name:")
        .find(|step| step.contains("Run release quality gate"))
        .expect("Windows release quality step");
    assert!(quality.contains("AGENTERM_SCRIPT_WORKER_STDERR: inherit"));
    assert!(quality.contains("agenterm-release-check.log"));
}

#[test]
fn release_cleanup_does_not_serialize_a_large_development_deps_directory() {
    assert!(BUILD_QJS.contains("[\"clean\", \"--dry-run\", \"--target-dir\", development_target]"));
    assert!(BUILD_QJS.contains("build_development_target_dry_run"));
}

#[test]
fn final_artifacts_keep_independent_cargo_feature_graphs() {
    for contract in [
        "build_agenterm_cargo",
        "build_agenterm_cu_cargo",
        "build_agenterm_abi_cargo",
        "build_agenterm_cu_provider_cargo",
        "Each final artifact is its own Cargo feature graph",
    ] {
        assert!(
            BUILD_QJS.contains(contract),
            "missing split build contract: {contract}"
        );
    }
    assert!(!BUILD_QJS.contains(
        "abi_args.push(\"agenterm-abi\");\n  abi_args.push(\"--package\");\n  abi_args.push(\"agenterm-cu-provider\");"
    ));
}

#[test]
fn qualification_recreates_owned_scratch_after_a_gate_cleans_target() {
    let execute = CHECK_QJS
        .split_once("function execute(")
        .and_then(|(_, tail)| tail.split_once("\nfunction run_gate_step("))
        .map(|(execute, _)| execute)
        .expect("check execute function");
    let child = execute
        .find("const handle = process_spawn")
        .expect("child command");
    let recreate = execute[child..]
        .find("rh.create_dir_all(scratch);")
        .expect("post-child scratch recreation");
    let publish = execute[child..]
        .find("const stdout = read_stream(stdout_path);")
        .expect("redirected stream publication");
    assert!(recreate < publish);
}

#[test]
fn retryable_smoke_finalizes_its_canonical_timing_state_once() {
    let first_attempt = CHECK_QJS
        .split_once("function run_gate_retryable_first(")
        .and_then(|(_, tail)| tail.split_once("\nfunction run_gate_specs("))
        .map(|(body, _)| body)
        .expect("retryable first-attempt helper");
    assert!(first_attempt.contains("all_output, 0)"));
    assert!(first_attempt.contains("timing_set_gate(timing, id, \"passed\""));

    let retry_loop = CHECK_QJS
        .split_once("if (skip_smoke === 0) {")
        .and_then(|(_, tail)| tail.split_once("\nif (skip_smoke !== 0)"))
        .map(|(body, _)| body)
        .expect("smoke retry loop");
    assert!(retry_loop.contains("run_gate_retryable_first("));
    assert!(retry_loop.contains("completed = run_gate(context, timing, id, id + \" (retry)\""));
}

#[test]
fn candidate_policy_is_explicit_and_runtime_courts_are_execute_only() {
    for contract in [
        "\"native_six_cell\": true",
        "\"chassis_product\": true",
        "\"experimental_ape\": false",
        "\"windows\": \"off\"",
        "\"linux\": \"off\"",
        "\"macos\": \"unsigned-preview\"",
        "\"executable_compression\": \"off\"",
        "\"windows_final_candidate_bytes\": \"required\"",
    ] {
        assert!(
            RELEASE_POLICY.contains(contract),
            "missing release policy: {contract}"
        );
    }
    assert!(CANDIDATE.contains("name: Resolve checked-in release policy"));
    assert!(
        CANDIDATE.contains("needs: [preflight, build, windows_unsigned, windows_sign, runtime]")
    );

    let runtime = CANDIDATE
        .split_once("\n  runtime:\n")
        .and_then(|(_, tail)| tail.split_once("\n  aggregate:\n"))
        .map(|(runtime, _)| runtime)
        .expect("one runtime job before aggregate");
    for runner in [
        "windows-2025",
        "windows-11-arm",
        "ubuntu-24.04",
        "ubuntu-24.04-arm",
        "macos-15",
        "macos-15-intel",
    ] {
        assert!(runtime.contains(runner), "missing runtime runner: {runner}");
    }
    for (platform, os, arch) in [
        ("windows-x86_64", "Windows", "X64"),
        ("windows-aarch64", "Windows", "ARM64"),
        ("linux-x86_64", "Linux", "X64"),
        ("linux-aarch64", "Linux", "ARM64"),
        ("macos-aarch64", "macOS", "ARM64"),
        ("macos-x86_64", "macOS", "X64"),
    ] {
        let cell = runtime
            .split_once(&format!("platform_id: {platform}\n"))
            .and_then(|(_, tail)| {
                tail.split_once("\n          - platform_id:")
                    .map(|(cell, _)| cell)
                    .or(Some(tail))
            })
            .expect("native runtime cell");
        assert!(
            cell.contains(&format!("expected_os: {os}")),
            "wrong OS guard for {platform}"
        );
        assert!(
            cell.contains(&format!("expected_arch: {arch}")),
            "wrong architecture guard for {platform}"
        );
    }
    assert!(runtime.contains("name: Guard native runner identity (no compile)"));
    assert!(runtime.contains("test \"$RUNNER_OS\" = \"$EXPECTED_RUNNER_OS\""));
    assert!(runtime.contains("test \"$RUNNER_ARCH\" = \"$EXPECTED_RUNNER_ARCH\""));
    assert!(runtime.contains("candidate-part-${{ matrix.platform_id }}"));
    assert!(runtime.contains("Scan final Windows Candidate bytes with Defender"));
    assert!(runtime.contains("name: cu-retirement-cell-smoke"));
    assert!(CANDIDATE.contains("Upload exact-source ACU runtime control"));
    assert!(CANDIDATE.contains("scripts/qjs/cu-retirement-cell-smoke.qjs"));
    assert!(CANDIDATE.contains("scripts/qjs/acu-provider-smoke.qjs"));
    assert!(CANDIDATE.contains("scripts/qjs/native-acu-composition-smoke.qjs"));
    assert!(CANDIDATE.contains("scripts/qjs/acu-mcp-provider-smoke.qjs"));
    assert!(CANDIDATE.contains("scripts/qjs/acu-power-action-provider-smoke.qjs"));
    assert!(CANDIDATE.contains("scripts/qjs/cu-setup-cli-smoke.qjs"));
    assert!(CANDIDATE.contains("scripts/qjs/cu-setup-runtime-refresh-smoke.qjs"));
    assert!(CANDIDATE.contains("scripts/qjs/lib/test_harness.qjs"));
    assert!(runtime.contains("runtime-control/cu-setup-cli-smoke.qjs"));
    assert!(runtime.contains("runtime-control/cu-setup-runtime-refresh-smoke.qjs"));
    assert!(runtime.contains("runtime-control/cu-retirement-cell-smoke.qjs"));
    assert!(runtime.contains("runtime-control/acu-mcp-provider-smoke.qjs"));
    assert!(runtime.contains("runtime-control/native-acu-composition-smoke.qjs"));
    assert!(runtime.contains("runtime-control/acu-power-action-provider-smoke.qjs"));
    assert!(runtime.contains("\"$provider\" \"$abi\""));
    for provider in [
        "runtime/agenterm-cu-provider.dll",
        "runtime/agenterm-cu-provider.so",
        "runtime/agenterm-cu-provider.dylib",
    ] {
        assert!(
            runtime.contains(provider),
            "missing ACU provider court: {provider}"
        );
    }
    for contract in [
        "schema_version: 6",
        "cu.retirement-cell.native-acu-composition",
        "AGENTERM_CU_GRANT: \"observe\"",
        "acu_provider: {",
        "abi_version: 1",
        "cu.retirement-cell.acu-provider",
        "acu.mcp-provider-capabilities",
        "acu.mcp-provider-observe",
        "cu.power-action-plan.acu-object",
    ] {
        assert!(
            CU_RETIREMENT_CELL_QJS.contains(contract),
            "missing ACU runtime receipt contract: {contract}"
        );
    }
    for contract in [
        "cell.schema_version === 6",
        "cu.retirement-cell.native-acu-composition",
        "agenterm_consumer.name === consumer_name",
        "acu_provider.name === provider_name",
        "acu_provider.abi_version === 1",
        "cu.retirement-cell.acu-provider",
        "acu.mcp-provider-capabilities",
        "acu.mcp-provider-observe",
        "cu.power-action-plan.acu-object",
    ] {
        assert!(
            RELEASE_CANDIDATE_QJS.contains(contract),
            "missing ACU runtime validator contract: {contract}"
        );
    }
    assert!(runtime.contains(
        "candidate-cu-runtime-${{ matrix.platform_id }}-${{ github.run_id }}-${{ github.run_attempt }}"
    ));
    assert!(
        runtime
            .contains("program_data_windows=\"${PROGRAMDATA:-${ProgramData:-C:\\\\ProgramData}}\"")
    );
    assert!(runtime.contains("test -n \"$mpcmd\" && test -f \"$mpcmd\""));
    assert!(runtime.contains("Prove Linux bundle closure in package-free Ubuntu"));
    assert!(!runtime.contains("actions/checkout"));
    assert!(!runtime.contains("cargo "));
}

#[test]
fn candidate_windows_signing_is_policy_selected_and_precedes_runtime() {
    let unsigned = CANDIDATE
        .split_once("\n  windows_unsigned:\n")
        .and_then(|(_, tail)| tail.split_once("\n  windows_sign:\n"))
        .map(|(body, _)| body)
        .expect("credential-free Windows finalization job");
    let signing = CANDIDATE
        .split_once("\n  windows_sign:\n")
        .and_then(|(_, tail)| tail.split_once("\n  runtime:\n"))
        .map(|(body, _)| body)
        .expect("Azure Windows signing job");

    assert!(CANDIDATE.contains("candidate-unsigned-part-${{ matrix.platform_id }}"));
    assert!(unsigned.contains("signed_windows != 'true'"));
    assert!(unsigned.contains("candidate-part-windows-x86_64"));
    assert!(unsigned.contains("candidate-part-windows-aarch64"));
    assert!(!unsigned.contains("azure/login"));
    assert!(!unsigned.contains("secrets."));
    assert!(!unsigned.contains("release-signing"));

    assert!(signing.contains("signed_windows == 'true'"));
    assert!(signing.contains("environment: release-signing"));
    assert!(signing.contains("id-token: write"));
    assert!(signing.contains("azure/login@0949e32778441b2c442592b7a0e6313466dc8f29"));
    assert!(
        signing.contains("Azure/artifact-signing-action@208f8af4bf26cf2af8597424e3cb5582801523ba")
    );
    assert!(signing.contains("files-catalog: signing-input/signing-catalog.txt"));
    assert!(signing.contains("timestamp-rfc3161: http://timestamp.acs.microsoft.com"));
    assert!(signing.contains("windows-signing-candidate.ps1 -Mode Prepare"));
    assert!(signing.contains("windows-signing-candidate.ps1 -Mode Finalize"));

    for contract in [
        "Get-AuthenticodeSignature",
        "ProductName",
        "ProductVersion",
        "archive required payload missing",
        "archive payload is not allowlisted",
        "payload_files = $actual",
        "input is already signed",
        "signing did not change bytes",
        "windows-signing-receipt.json",
        "asset_count = @($receiptAssets.Keys).Count",
    ] {
        assert!(
            WINDOWS_SIGNING_SCRIPT.contains(contract),
            "missing Windows signing script guard: {contract}"
        );
    }
    for owner in [
        CANDIDATE_AGGREGATE_QJS.as_str(),
        RELEASE_CANDIDATE_QJS.as_str(),
    ] {
        for contract in [
            "windows-signing-receipt.json",
            "candidate_windows_signing_receipt_missing",
            "candidate_windows_signing_receipt_unexpected",
            "candidate_windows_signing_provenance",
        ] {
            assert!(
                owner.contains(contract),
                "missing Candidate receipt guard: {contract}"
            );
        }
    }
    assert!(RELEASE_CANDIDATE_QJS.contains("receipt.asset_count === receipt_asset_names.length"));
    assert!(RELEASE_CANDIDATE_QJS.contains("receipt.platform_count === 2"));
}

#[test]
fn release_policy_owners_reference_living_qjs_and_parked_ci_paths() {
    for owner in [PREFLIGHT_QJS.as_str(), INTERNAL_VERSION_QJS.as_str()] {
        assert!(owner.contains("scripts/qjs/release.qjs"));
        assert!(!owner.contains("scripts/rh/release.rh"));
    }
    assert!(AUTOMATION_AUDIT_QJS.contains(".github/workflows/ci-agenterm.yml.disabled"));
    assert!(!AUTOMATION_AUDIT_QJS.contains("read_repo(\".github/workflows/ci-agenterm.yml\")"));
}

#[test]
fn candidate_runs_one_full_gate_and_seals_six_platform_parts_plus_chassis_product() {
    assert_eq!(
        CANDIDATE
            .matches("check.cmd --release --include-stress")
            .count(),
        1
    );
    for platform in [
        "windows-x86_64",
        "windows-aarch64",
        "linux-x86_64",
        "linux-aarch64",
        "macos-aarch64",
        "macos-x86_64",
    ] {
        assert!(
            CANDIDATE.contains(&format!("platform_id: {platform}")),
            "missing candidate cell {platform}"
        );
    }
    assert!(CANDIDATE.contains("pattern: candidate-part-*"));
    assert!(CANDIDATE.contains("merge-multiple: true"));
    assert!(CANDIDATE.contains("target/qualification/receipt.json"));
    assert!(CANDIDATE.contains("name: Stage flat candidate part"));
    assert!(CANDIDATE.contains("path: candidate-part/"));
    assert!(CANDIDATE.contains("cli script \\\n            task run candidate-aggregate"));
    assert!(!CANDIDATE.contains("candidate-aggregate.rh"));
    assert!(!CANDIDATE.contains(" rh \\"));
    assert!(CANDIDATE.contains("python3 scripts/chassis-candidate-pack.py"));
    assert!(CANDIDATE.contains("candidate-input/agenterm-$version-chassis-product.tgz"));
    assert!(CANDIDATE.contains("name: Build thin Chassis-L1 loader"));
    assert!(CANDIDATE.contains("--features loader"));
    assert!(CANDIDATE.contains("python3 scripts/chassis-stage-l1-loader.py"));
    assert!(CANDIDATE.contains("--loader target/chassis-l1-loader"));
    assert!(
        CANDIDATE.contains("task run candidate-aggregate --manifest agenterm.tasks.json -- \\")
    );
    assert!(CANDIDATE.contains("path: candidate-output/"));
    assert!(CANDIDATE.contains("Download exact-attempt ACU runtime receipts"));
    assert!(CANDIDATE.contains("scripts/qjs/cu-retirement-runtime-aggregate.qjs"));
    assert!(CANDIDATE.contains("candidate-output/evidence/cu-six-cell-runtime.json"));
    assert!(CANDIDATE_AGGREGATE_QJS.contains("candidate.validate_cu_runtime_summary"));
    assert!(RELEASE_CANDIDATE_QJS.contains("validate_cu_runtime_summary(cu_runtime"));
    assert!(!CANDIDATE.contains(".agenterm-rhai.bin"));
    assert!(!CANDIDATE.contains("scripts/rhai/check.rhai"));
    assert!(!CANDIDATE.contains("scripts/rhai/fresh-clone-rehearsal.rhai"));
    assert!(CANDIDATE.contains("name: release-candidate-${{ github.run_id }}"));
    assert!(CANDIDATE.contains("retention-days: 14"));
}

#[test]
fn staged_artifact_gate_pins_the_complete_mcp_catalog() {
    assert!(ARTIFACT_VERIFICATION_QJS.contains("mcp_capabilities.tools.length === 3"));
    for name in [
        "agenterm_wait",
        "agenterm_acu_capabilities",
        "agenterm_acu_observe",
    ] {
        assert!(
            ARTIFACT_VERIFICATION_QJS.contains(name),
            "artifact verification omitted MCP tool {name}"
        );
    }
}

#[test]
fn candidate_cargo_home_caches_are_platform_isolated_and_revision_reusable() {
    let input_hash = "${{ hashFiles('rust-toolchain.toml', 'Cargo.lock', 'Cargo.toml', 'build.rs', 'scripts/artifacts.json') }}";
    let generic_key = format!(
        "cargo-home-candidate-v2-${{{{ matrix.platform_id }}}}-${{{{ runner.os }}}}-${{{{ runner.arch }}}}-rust1.97-{input_hash}"
    );
    let windows_arm64_key = format!(
        "cargo-home-candidate-v2-windows-aarch64-${{{{ runner.os }}}}-${{{{ runner.arch }}}}-rust1.97-{input_hash}"
    );

    assert_eq!(CANDIDATE.matches(&format!("key: {generic_key}")).count(), 2);
    assert_eq!(
        CANDIDATE
            .matches(&format!("key: {windows_arm64_key}"))
            .count(),
        2
    );
    assert!(!CANDIDATE.contains("cargo-home-candidate-${{ runner.os }}"));
    assert!(!CANDIDATE.contains("cargo-home-v3-windows-aarch64"));

    for step_name in [
        "Restore candidate Cargo cache",
        "Save candidate Cargo cache",
        "Restore Windows ARM64 cargo cache",
        "Save Windows ARM64 cargo cache",
    ] {
        let step = CANDIDATE
            .split("      - name:")
            .find(|step| step.contains(step_name))
            .expect("candidate Cargo-home cache step");
        assert!(step.contains(CACHE_SHA));
        assert!(!step.contains("inputs.source_sha"));
        if step_name.starts_with("Restore") {
            assert!(step.contains("restore-keys:"));
        } else {
            assert!(!step.contains("restore-keys:"));
        }
    }

    for step_name in [
        "Restore candidate Cargo cache",
        "Save candidate Cargo cache",
    ] {
        let step = CANDIDATE
            .split("      - name:")
            .find(|step| step.contains(step_name))
            .expect("generic candidate Cargo-home cache step");
        assert!(step.contains("matrix.platform_id != 'windows-aarch64'"));
    }
}

#[test]
fn windows_candidate_target_cache_is_exact_source_and_success_only() {
    let restore = CANDIDATE
        .split("      - name:")
        .find(|step| step.contains("Restore Windows x86_64 debug and release-fast targets"))
        .expect("Windows target cache restore step");
    let save = CANDIDATE
        .split("      - name:")
        .find(|step| step.contains("Save Windows x86_64 debug and release-fast targets"))
        .expect("Windows target cache save step");

    assert!(restore.contains("cargo-target-v3-windows-x86_64-candidate-"));
    assert!(restore.contains("${{ inputs.source_sha }}"));
    assert!(!restore.contains("restore-keys:"));
    assert!(save.contains("if: success() && matrix.platform_id == 'windows-x86_64'"));
    assert!(save.contains("cargo-target-v3-windows-x86_64-candidate-"));
}

#[test]
fn promotion_is_manual_candidate_bound_and_performs_no_build_or_overwrite() {
    assert!(PROMOTION.contains("workflow_dispatch:"));
    assert!(PROMOTION.contains("candidate_run_id:"));
    assert!(PROMOTION.contains("confirmation:"));
    assert!(!PROMOTION.contains("\n  push:"));
    assert!(PROMOTION.contains(".github/workflows/candidate.yml"));
    assert!(PROMOTION.contains("workflow_dispatch"));
    assert!(PROMOTION.contains("conclusion"));
    assert!(PROMOTION.contains("head_sha"));
    assert!(PROMOTION.contains("source_sha\" != \"$GITHUB_SHA"));
    assert!(PROMOTION.contains("cannot create a tag for an older workflow-bearing commit"));
    assert!(PROMOTION.contains("publish-$tag"));
    assert!(PROMOTION.contains("task run candidate-verify"));
    assert!(!PROMOTION.contains("candidate-verify.rh"));
    assert!(!PROMOTION.contains(" rh \\"));
    // H1: pure-derive releases.json during verify + publish (not a second truth).
    assert!(PROMOTION.contains("task run build-releases-index"));
    assert!(!PROMOTION.contains("build-releases-index.rhai"));
    assert!(PROMOTION.contains("candidate/releases.json"));
    assert!(PROMOTION.contains("Derive releases.json index"));
    assert!(INTEGRITY.contains("echo releases.json"));
    assert!(INTEGRITY.contains("agenterm-releases-index"));
    assert!(INTEGRITY.contains(".source.manifest_sha256 == $manifest_sha"));
    assert!(PROMOTION.contains("(.releases[0].artifacts | length) == 7"));
    assert!(INTEGRITY.contains("(.releases[0].artifacts | length) == 7"));
    assert!(INTEGRITY.contains("workflow_dispatch:"));
    assert!(INTEGRITY.contains("promotion_run_id:"));
    assert!(INTEGRITY.contains("inputs.promotion_run_id || github.event.workflow_run.id"));
    assert!(INTEGRITY.matches("name: .checksum.name").count() == 2);
    assert!(INTEGRITY.matches("sha256: .checksum.sha256").count() == 2);
    assert!(INTEGRITY.matches("name: .provenance.name").count() == 2);
    assert!(INTEGRITY.matches("sha256: .provenance.sha256").count() == 2);
    assert!(PROMOTION.contains("environment: release"));
    assert!(PROMOTION.contains("contents: write"));
    assert!(PROMOTION.contains("repos/$GITHUB_REPOSITORY/git/refs"));
    assert!(PROMOTION.contains("--verify-tag"));
    assert!(PROMOTION.contains("Recovering exact unpublished draft"));
    assert!(PROMOTION.contains("agenterm-promotion-identity"));
    assert!(PROMOTION.contains("cli script run \\"));
    assert!(PROMOTION.contains("--profile tool scripts/qjs/promotion-identity.qjs -- \\"));
    assert!(!PROMOTION.contains("scripts/rh/promotion-identity.rh"));
    assert!(PROMOTION.contains("agenterm-promotion:v1 candidate_run_id="));
    assert!(PROMOTION.contains("body_sha256"));
    assert!(PROMOTION.contains("[[ \"$(jq -r .body <<<\"$release\")\" == \"$release_body\" ]]"));
    assert!(PROMOTION.contains("[[ \"$(jq -r .name <<<\"$release\")\" == \"AgenTerm $TAG\" ]]"));
    assert!(!PROMOTION.contains("--generate-notes"));
    assert!(PROMOTION.contains("gh api --paginate --slurp"));
    assert!(PROMOTION.contains("select(.tag_name == $wanted)"));
    assert!(PROMOTION.contains("verify_remote_assets"));
    assert!(PROMOTION.contains("gh release upload \"$TAG\" \"$file\""));
    assert!(PROMOTION.contains("sha256sum \"$remote_file\""));
    assert!(PROMOTION.contains("path: candidate/"));
    assert!(!PROMOTION.contains(".agenterm-rhai.bin"));
    for forbidden in [
        "--clobber",
        "cargo ",
        "build.bat",
        "build.sh",
        "check.cmd",
        "check.sh",
        "release.cmd",
        "release.sh",
        "task run check",
        "task run package",
        "notarytool",
        "codesign",
    ] {
        assert!(
            !PROMOTION.contains(forbidden),
            "promotion contains forbidden operation: {forbidden}"
        );
    }
    // Forbid the build/check/package orchestrators, but allow leaf tasks whose
    // ids share a prefix (e.g. build-releases-index).
    for line in PROMOTION.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("task run ") {
            let task = rest.split_whitespace().next().unwrap_or("");
            assert!(
                task != "build" && task != "check" && task != "package",
                "promotion contains forbidden operation: task run {task}"
            );
        }
    }
}

#[test]
fn promotion_identity_is_registered_for_future_candidates() {
    let tasks = include_str!("../agenterm.tasks.json");
    assert!(tasks.contains("\"promotion-identity\": {"));
    assert!(tasks.contains("\"id\": \"promotion-identity\""));
    assert!(tasks.contains("\"entry\": \"scripts/qjs/promotion-identity.qjs\""));
}

#[test]
fn workflow_actions_are_immutable_and_post_release_integrity_is_read_only() {
    for (source, sha) in [
        (CANDIDATE.as_str(), CHECKOUT_SHA),
        (CANDIDATE.as_str(), UPLOAD_SHA),
        (CANDIDATE.as_str(), DOWNLOAD_SHA),
        (PROMOTION.as_str(), CHECKOUT_SHA),
        (PROMOTION.as_str(), UPLOAD_SHA),
        (PROMOTION.as_str(), DOWNLOAD_SHA),
    ] {
        assert!(source.contains(sha), "missing pinned action SHA {sha}");
    }
    assert!(INTEGRITY.contains("permissions:\n  actions: read\n  contents: read"));
    assert!(!INTEGRITY.contains("contents: write"));
    assert!(!INTEGRITY.contains("gh release upload"));
    assert!(!INTEGRITY.contains("--clobber"));
    assert!(INTEGRITY.contains("sha256sum -c"));
    assert!(INTEGRITY.contains("verified-promotion-$PROMOTION_RUN_ID"));
    assert!(INTEGRITY.contains("candidate-manifest.json"));
    assert!(!INTEGRITY.contains("\n  push:"));
}

#[test]
fn candidate_collects_delivery_timing_evidence() {
    assert!(CANDIDATE.contains("Publish candidate delivery timing evidence"));
    assert!(CANDIDATE.contains("Upload candidate delivery timing"));
    assert!(CANDIDATE.contains("delivery-timing/candidate-delivery-timing.json"));
    assert!(CANDIDATE.contains("candidate-delivery-timing-${{ github.run_id }}"));
}

#[test]
fn promotion_collects_delivery_timing_evidence() {
    assert!(PROMOTION.contains("Publish promotion timing evidence"));
    assert!(PROMOTION.contains("Upload promotion timing evidence"));
    assert!(PROMOTION.contains("delivery-timing/release-delivery-timing.json"));
    assert!(PROMOTION.contains("release-delivery-timing-${{ github.run_id }}"));
    assert!(PROMOTION.contains("agenterm-release-timing"));
    assert!(PROMOTION.contains("\"checkout_ms\""));
    assert!(PROMOTION.contains("\"toolchain_ms\""));
    assert!(PROMOTION.contains("\"promotion_ms\""));
    assert!(PROMOTION.contains("\"tag_to_public_ms\""));
    assert!(PROMOTION.contains("\"release_published_ms\""));
    assert!(PROMOTION.contains("\"candidate_to_promotion_ms\""));
}

#[test]
fn candidate_timing_collects_checkout_and_toolchain_stages() {
    assert!(CANDIDATE.contains("\"checkout_ms\""));
    assert!(CANDIDATE.contains("\"toolchain_ms\""));
    assert!(CANDIDATE.contains("\"cache_ms\""));
    assert!(CANDIDATE.contains("\"compile_ms\""));
    assert!(CANDIDATE.contains("\"test_ms\""));
    assert!(CANDIDATE.contains("\"package_ms\""));
    assert!(CANDIDATE.contains("\"artifact_transfer_ms\""));
    assert!(CANDIDATE.contains("\"aggregate_ms\""));
}

#[test]
fn release_identity_inputs_have_platform_stable_line_endings() {
    for path in [
        "Cargo.lock",
        "scripts/artifacts.json",
        "scripts/qualification-gates.json",
    ] {
        assert!(
            GIT_ATTRIBUTES
                .lines()
                .any(|line| line == format!("{path} text eol=lf")),
            "release identity input lacks an LF policy: {path}"
        );
    }
}

#[test]
fn six_cell_qjs_orchestrators_use_the_live_script_front_door() {
    for source in [
        include_str!("../scripts/qjs/build-all.qjs"),
        include_str!("../scripts/qjs/six-cell-qualify.qjs"),
    ] {
        assert!(source.contains("\"cli\", \"script\", \"task\", \"run\""));
        assert!(!source.contains("\"rh\", \"task\", \"run\""));
    }
}

#[test]
fn six_cell_build_listing_names_cargo_files_and_requires_every_listed_artifact() {
    assert!(ARTIFACT_FILES_QJS.contains("export function cargo_file_name(name)"));
    assert!(ARTIFACT_FILES_QJS.contains("const source_name = cargo_file_name(name);"));
    assert!(
        ARTIFACT_FILES_QJS.contains("const destination = rh.join(destination_directory, name);")
    );
    assert!(
        !ARTIFACT_FILES_QJS
            .contains("const destination = rh.join(destination_directory, source_name);")
    );
    assert!(BUILD_QJS.contains("artifact_files.cargo_file_name(\"\" + executable.name)"));
    assert!(BUILD_QJS.contains("\"Built client artifacts [\""));
    assert!(!BUILD_QJS.contains("rh.join(profile_directory, executable.name)"));
    assert!(BUILD_ALL_QJS.contains("build_all_listed_artifact_missing:"));
    assert!(BUILD_ALL_QJS.contains("build_all_artifact_set_unmatched:"));
    assert!(BUILD_ALL_QJS.contains("artifact_files.cargo_file_name"));
    assert!(BUILD_ALL_QJS.contains("output.stdout_truncated"));
    assert!(!BUILD_ALL_QJS.contains("matches === 1"));
    assert!(!BUILD_ALL_QJS.contains("if (rh.exists(artifact_path))"));
    assert!(BUILD_ALL_QJS.contains("startsWith(\"Built client artifacts [\")"));
    assert!(BUILD_ALL_QJS.contains("rh.is_absolute(path)"));
    assert!(!BUILD_ALL_QJS.contains("startsWith(\"  /\")"));

    fn cargo_file_name(name: &str) -> &str {
        match name {
            "agenterm.com" => "agenterm-com.exe",
            other => other,
        }
    }
    let mut divergences = Vec::new();
    for platform in ARTIFACTS["platforms"]
        .as_array()
        .expect("platform list must be an array")
    {
        let os = platform["os"].as_str().expect("platform os must be text");
        for executable in platform["executables"]
            .as_array()
            .expect("executables must be an array")
        {
            let name = executable["name"]
                .as_str()
                .expect("executable name must be text");
            if cargo_file_name(name) != name {
                divergences.push((os, name));
            }
        }
        for library in platform["libraries"]
            .as_array()
            .expect("libraries must be an array")
        {
            let name = library["name"].as_str().expect("library name must be text");
            assert_eq!(cargo_file_name(name), name, "library name drifted: {name}");
        }
    }
    assert!(
        divergences
            .iter()
            .all(|&(os, name)| os == "windows" && name == "agenterm.com")
    );
    assert_eq!(
        divergences.len(),
        ARTIFACTS["platforms"]
            .as_array()
            .expect("platform list must be an array")
            .iter()
            .filter(|platform| platform["os"].as_str() == Some("windows"))
            .count(),
        "each Windows architecture has exactly one staged/Cargo name divergence"
    );
}

#[test]
fn six_cell_declared_artifact_sets_reject_partial_listings() {
    let platforms = ARTIFACTS["platforms"]
        .as_array()
        .expect("platform list must be an array");
    let mut declared = Vec::new();
    for platform in platforms {
        let label = format!(
            "{}-{}",
            platform["os"].as_str().expect("platform os must be text"),
            platform["arch"]
                .as_str()
                .expect("platform arch must be text")
        );
        let mut names = Vec::new();
        for key in ["executables", "libraries"] {
            for entry in platform[key]
                .as_array()
                .expect("declared artifacts must be an array")
            {
                let name = entry["name"].as_str().expect("artifact name must be text");
                names.push(match name {
                    "agenterm.com" => "agenterm-com.exe".to_owned(),
                    other => other.to_owned(),
                });
            }
        }
        declared.push((label, names));
    }

    for (label, names) in &declared {
        let distinct: std::collections::BTreeSet<&String> = names.iter().collect();
        assert_eq!(distinct.len(), names.len(), "{label} declares a duplicate");
    }
    for (outer, outer_names) in &declared {
        let outer_set: std::collections::BTreeSet<&String> = outer_names.iter().collect();
        for (inner, inner_names) in &declared {
            if outer == inner {
                continue;
            }
            let inner_set: std::collections::BTreeSet<&String> = inner_names.iter().collect();
            if outer_set == inner_set {
                continue;
            }
            assert!(
                !outer_set.is_subset(&inner_set),
                "{outer} is a proper subset of {inner}; a partial listing could match it"
            );
        }
    }
}

#[test]
fn six_cell_build_all_covers_every_declared_artifact_platform() {
    let start = BUILD_ALL_QJS
        .find("const cells = [")
        .expect("cells literal");
    let end = start
        + BUILD_ALL_QJS[start..]
            .find("];")
            .expect("cells literal end");
    let mut targets = Vec::new();
    let mut rest = &BUILD_ALL_QJS[start..end];
    while let Some(offset) = rest.find("target: \"") {
        rest = &rest[offset + "target: \"".len()..];
        let close = rest.find('"').expect("target triple terminator");
        targets.push(rest[..close].to_owned());
        rest = &rest[close..];
    }

    let platforms = ARTIFACTS["platforms"]
        .as_array()
        .expect("platform list must be an array");
    assert_eq!(targets.len(), platforms.len(), "one cell per platform");
    let distinct: std::collections::BTreeSet<&str> = targets.iter().map(String::as_str).collect();
    assert_eq!(distinct.len(), targets.len(), "duplicate build-all cell");
    for platform in platforms {
        let os = platform["os"].as_str().expect("platform os must be text");
        let arch = platform["arch"]
            .as_str()
            .expect("platform arch must be text");
        let token = if os == "macos" { "apple" } else { os };
        assert!(
            targets
                .iter()
                .any(|target| target.starts_with(arch) && target.contains(token)),
            "build-all must include the {os}/{arch} cell"
        );
    }
}

#[test]
fn build_qjs_marks_bounded_child_captures_as_incomplete() {
    assert!(BUILD_QJS.contains("function truncation_note(output)"));
    assert!(BUILD_QJS.contains("output.stdout_truncated === true"));
    assert!(BUILD_QJS.contains("output.stderr_truncated === true"));
    assert!(BUILD_QJS.contains("+ truncation_note(result)"));
    assert!(BUILD_QJS.contains("truncation_note(output)"));
    assert!(BUILD_QJS.contains("+ stage_truncation_note"));
}

#[test]
fn build_qjs_refuses_a_repeated_value_option_instead_of_last_one_wins() {
    assert!(BUILD_QJS.contains("function claim_option(value)"));
    assert!(BUILD_QJS.contains("build_option_duplicate:"));
    assert_eq!(
        BUILD_QJS.matches("claim_option(value);").count(),
        6,
        "one claim per value option: target, glibc, driver, os, arch and action"
    );
    assert!(BUILD_QJS.contains("build_profile_duplicate"));
    assert!(BUILD_QJS.contains("build_unknown_argument:"));
    assert!(BUILD_QJS.contains("build_option_value_missing:"));
}

#[test]
fn check_qjs_refuses_a_repeated_quick_target_instead_of_silently_changing_scope() {
    assert!(CHECK_QJS.contains("check_option_duplicate:"));
    assert!(CHECK_QJS.contains("quick_target_set"));
    assert!(CHECK_QJS.contains("check_target_value_missing"));
    assert!(CHECK_QJS.contains("check_target_only_for_quick"));
    assert!(CHECK_QJS.contains("only_gates.push(gate_id)"));
}

#[test]
fn check_qjs_refuses_a_repeated_timing_path_instead_of_redirecting_evidence() {
    assert!(CHECK_QJS.contains("check_option_duplicate:"));
    assert!(CHECK_QJS.contains("timing_set"));
    assert!(CHECK_QJS.contains("check_timing_value_missing"));
    assert!(CHECK_QJS.contains("try_remove_file(timing_path)"));
    assert!(CHECK_QJS.contains("check_unknown_argument:"));
}

#[test]
fn check_quick_probes_the_binary_the_target_asks_for() {
    assert!(CHECK_QJS.contains("path.join(quick_native_bin_root, \"agenterm\")"));
    assert!(!CHECK_QJS.contains(
        "return task(bootstrap_worker, repo, \"prd-alignment\", 120000, [], empty_environment(), 0);"
    ));
    assert!(
        include_str!("../scripts/qjs/prd-alignment.qjs").contains("prd_alignment_input_missing:")
    );
}

#[test]
fn six_cell_orchestrators_reject_surplus_arguments_instead_of_widening_scope() {
    assert!(BUILD_ALL_QJS.contains("build_all_unknown_argument:"));
    assert!(BUILD_ALL_QJS.contains("build_all_profile_duplicate"));
    assert!(!BUILD_ALL_QJS.contains("if (args.length >= 2)"));
    assert!(BUILD_QJS.contains("build_unknown_argument:"));
    assert!(SIX_CELL_QUALIFY_QJS.contains("qualify_unknown_argument:"));
    assert!(SIX_CELL_QUALIFY_QJS.contains("qualify_profile_duplicate"));

    for task_id in ["client-build-all", "six-cell-qualify"] {
        let task = TASKS["tasks"]
            .as_array()
            .expect("task list must be an array")
            .iter()
            .find(|task| task["id"].as_str() == Some(task_id))
            .unwrap_or_else(|| panic!("missing task {task_id}"));
        assert_eq!(
            task["args"],
            serde_json::json!(["."]),
            "{task_id} must declare only the repository argument"
        );
    }
}

#[test]
fn six_cell_qualify_refuses_contradictory_run_selection_instead_of_last_one_wins() {
    assert!(SIX_CELL_QUALIFY_QJS.contains("qualify_build_flag_conflict"));
    assert!(SIX_CELL_QUALIFY_QJS.contains("qualify_runners_duplicate"));
    assert!(SIX_CELL_QUALIFY_QJS.contains("qualify_profile_duplicate"));
    assert!(SIX_CELL_QUALIFY_QJS.contains("build_selector"));
    assert!(SIX_CELL_QUALIFY_QJS.contains("value === \"--no-build\""));
    assert!(SIX_CELL_QUALIFY_QJS.contains("value === \"--runners\""));
    assert!(SIX_CELL_QUALIFY_QJS.contains("build: build_state,"));
    assert!(SIX_CELL_QUALIFY_QJS.contains("runners: runners_path,"));
}

#[test]
fn six_cell_delivery_documents_the_profile_directory_it_reads() {
    assert!(!PACKAGE_SIX_CELL_QJS.contains("target/qualification/six-cell/<triple>"));
    assert!(PACKAGE_SIX_CELL_QJS.contains("target/<triple>/<leaf>/"));
    assert!(
        PACKAGE_SIX_CELL_QJS.contains("rh.join(rh.join(rh.join(repo, \"target\"), target), leaf)")
    );
    assert!(SIX_CELL_QUALIFY_QJS.contains("let leaf = profile;"));
    assert!(SIX_CELL_QUALIFY_QJS.contains("if (profile === \"dev\") { leaf = \"debug\"; }"));
    assert!(!SIX_CELL_QUALIFY_QJS.contains("profile === \"release-fast\""));
    assert!(PACKAGE_SIX_CELL_QJS.contains("if (profile === \"dev\") { return \"debug\"; }"));
    assert!(PACKAGE_SIX_CELL_QJS.contains("return profile;"));
    assert!(!PACKAGE_SIX_CELL_QJS.contains("profile === \"release-fast\""));
}

#[test]
fn six_cell_static_gate_distinguishes_windows_gui_and_cu_console_subsystems() {
    let cells = SIX_CELL_RUNNERS["cells"]
        .as_array()
        .expect("six-cell runner cells must be an array");
    let windows = cells
        .iter()
        .filter(|cell| {
            cell["target"]
                .as_str()
                .is_some_and(|target| target.contains("windows"))
        })
        .collect::<Vec<_>>();
    assert_eq!(windows.len(), 2, "both Windows ISA cells must be described");
    for cell in windows {
        assert!(
            cell["expect_file"]
                .as_str()
                .is_some_and(|expected| expected.contains("(GUI)")),
            "the AgenTerm launcher must remain a GUI-subsystem executable"
        );
        assert!(
            cell["expect_cu_file"]
                .as_str()
                .is_some_and(|expected| expected.contains("(console)")),
            "agenterm-cu must be checked as its distinct console-subsystem executable"
        );
    }

    let qualify = include_str!("../scripts/qjs/six-cell-qualify.qjs");
    assert!(qualify.contains("typeof cell.expect_cu_file === \"string\""));
    assert!(qualify.contains("describe_binary(local_cu_binary, cu_described_path)"));
}

#[test]
fn six_cell_static_gate_keeps_one_file_oracle_without_shell_pipeline_wrappers() {
    let qualify = include_str!("../scripts/qjs/six-cell-qualify.qjs");

    assert!(qualify.contains("rh.command(\"file\", [\"-b\", path]"));
    assert!(qualify.contains("probe.success === true"));
    assert!(qualify.contains("matched.text.includes(expected)"));
    assert!(qualify.contains("cu_matched.text.includes(cu_expected)"));
    assert!(qualify.contains("rh.atomic_write(path_out, text)"));
    assert!(!qualify.contains("rh.command(\n    \"sh\","));
    assert!(!qualify.contains("\"-c\",\n      \"file -b"));
}

#[test]
fn six_cell_registry_does_not_invent_fixed_ssh_endpoints_for_lima_runners() {
    let cells = SIX_CELL_RUNNERS["cells"]
        .as_array()
        .expect("six-cell runner cells must be an array");
    let linux = cells
        .iter()
        .filter(|cell| {
            cell["target"]
                .as_str()
                .is_some_and(|target| target.contains("linux"))
        })
        .collect::<Vec<_>>();
    assert_eq!(linux.len(), 2, "both Linux ISA cells must be described");
    for cell in linux {
        assert_eq!(cell["kind"], "blocked");
        assert!(cell.get("host").is_none());
        assert!(cell.get("port").is_none());
        assert!(cell.get("identity_from_home").is_none());
        assert!(
            cell["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("Lima") && reason.contains("provisioner")),
            "the blocker must name the real runner family and its owner"
        );
    }
}

#[test]
fn user_installer_keeps_the_fixed_sibling_acu_provider_with_every_cu_symlink() {
    assert!(INSTALL_SH.contains("PROVIDER_LIBRARY=\"agenterm-cu-provider.dylib\""));
    assert!(INSTALL_SH.contains("PROVIDER_LIBRARY=\"agenterm-cu-provider.so\""));
    assert_eq!(
        INSTALL_SH
            .matches("replace_symlink \"$CURRENT_LINK/$PROVIDER_LIBRARY\"")
            .count(),
        2,
        "local-build and release installs must both publish the fixed sibling"
    );
    assert!(INSTALL_SH.contains("local build is missing ACU provider:"));
    assert!(INSTALL_SH.contains("release payload is missing $PROVIDER_LIBRARY"));
    assert_eq!(
        INSTALL_SH
            .matches("installed ACU provider symlink is unavailable:")
            .count(),
        2
    );

    let artifacts = ARTIFACTS.to_string();
    assert!(artifacts.contains("agenterm-cu-provider.dylib"));
    assert!(artifacts.contains("agenterm-cu-provider.so"));
}

#[test]
fn user_installer_publishes_cu_as_a_regular_path_executable() {
    assert!(INSTALL_SH.contains("replace_cu_executable()"));
    assert_eq!(
        INSTALL_SH
            .matches("validate_cu_destination \"$BIN_DIR/agenterm-cu\"")
            .count(),
        2,
        "both paths must reject an unmanaged destination before changing the release"
    );
    assert_eq!(
        INSTALL_SH
            .matches("replace_cu_executable \"$CURRENT_LINK/agenterm-cu\" \"$BIN_DIR/agenterm-cu\"")
            .count(),
        2,
        "local-build and release installs must both publish a regular CU executable"
    );
    assert_eq!(
        INSTALL_SH
            .matches("installed agenterm-cu is not a regular executable:")
            .count(),
        2
    );
    assert!(INSTALL_SH.contains("chmod 0755 \"$next\""));
    assert!(INSTALL_SH.contains("codesign --verify --strict \"$BIN_DIR/agenterm-cu\""));
    assert_eq!(
        INSTALL_SH
            .matches("verify_cu_abi \"$BIN_DIR/agenterm-cu\" \"$BIN_DIR/$REQUIRED_LIBRARY\"")
            .count(),
        2,
        "both installer paths must verify CU through the published PATH layout"
    );
    assert!(
        !INSTALL_SH
            .contains("replace_symlink \"$CURRENT_LINK/agenterm-cu\" \"$BIN_DIR/agenterm-cu\"")
    );
}

#[test]
fn hotkey_installer_keeps_the_provider_beside_both_cu_copies() {
    assert!(INSTALL_CU_HOTKEYS_SH.contains("-p agenterm-cu-provider"));
    assert!(
        INSTALL_CU_HOTKEYS_SH
            .contains("PROVIDER_SOURCE=\"${ROOT}/target/abi-release/agenterm-cu-provider.dylib\"")
    );
    assert_eq!(
        INSTALL_CU_HOTKEYS_SH
            .matches("cp \"${PROVIDER_SOURCE}\"")
            .count(),
        2,
        "the primary and legacy hotkey app copies both need the fixed sibling"
    );
    assert_eq!(
        INSTALL_CU_HOTKEYS_SH
            .matches("chmod 644 \"${APP}/Contents/MacOS/agenterm-cu-provider.dylib\"")
            .count(),
        1
    );
    assert_eq!(
        INSTALL_CU_HOTKEYS_SH
            .matches("chmod 644 \"${LEGACY_APP}/Contents/MacOS/agenterm-cu-provider.dylib\"")
            .count(),
        1
    );
    assert!(INSTALL_CU_HOTKEYS_SH.contains("codesign --verify --strict \"${provider}\""));
}

/// The qjs door caps a slot at 32 live child handles and a release
/// qualification spawns one child per gate step -- the unit-tests gate alone is
/// 23 -- so a waited-for slot that is never released exhausts the door part way
/// through the run. The failure then lands on whichever step happens to be the
/// 33rd, which is never the step that is actually wrong.
#[test]
fn the_qualification_releases_every_child_slot_it_spawns() {
    let spawns = CHECK_QJS.matches("process_spawn(").count();
    let releases = CHECK_QJS.matches("process_release(").count();
    assert!(
        spawns > 0,
        "the qualification no longer spawns children; retire this gate deliberately"
    );
    assert_eq!(
        releases, spawns,
        "check.qjs spawns {spawns} child slots but releases {releases}: a leaked \
         slot exhausts the door's 32-handle cap mid-run"
    );
    assert!(
        CHECK_QJS.contains("if (process_release(handle) !== 0)"),
        "the release must be checked, not fired and forgotten"
    );
}

/// The PowerShell ledger closes PowerShell as product and build automation.
/// `scripts/inspect-authenticode.ps1` sits outside it only because nothing
/// executes it: a Windows reader runs it by hand from the signing docs. That
/// premise is the whole justification for the exemption, so check it here
/// instead of trusting the comment that states it. If anything ever invokes
/// the inspector, it has become automation and belongs back under the ledger.
#[test]
fn the_exempt_authenticode_inspector_is_never_invoked_by_automation() {
    const INSPECTOR: &str = "inspect-authenticode.ps1";
    let audit = include_str!("../scripts/qjs/powershell-migration-audit.qjs").replace("\r\n", "\n");
    assert!(
        audit.contains("|| path === \"scripts/inspect-authenticode.ps1\";"),
        "the inspector must be an exact exempt path, not a pattern"
    );
    let exemption = audit
        .split_once("function is_non_automation_ps1(path) {")
        .expect("the PowerShell exemption function must keep its name")
        .1
        .split_once("\n}")
        .expect("the exemption function must be closed")
        .0;
    assert!(
        !exemption.contains("startsWith")
            && !exemption.contains("indexOf")
            && !exemption.contains("includes"),
        "the PowerShell exemption must stay an exact-path list, not a pattern match"
    );

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut invokers = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if !matches!(name.as_ref(), "target" | ".git" | "node_modules") {
                    stack.push(path);
                }
                continue;
            }
            // Documentation may tell a human to run it; automation may not.
            // The policy test that reads the file's text is not an invocation.
            let is_prose = name.ends_with(".md");
            let is_this_test = name == "release_workflow_policy.rs";
            let is_reader = name == "authenticode_inspector_policy.rs";
            let is_itself = name == INSPECTOR;
            // The ledger audit names the path because that *is* the exemption.
            let is_the_ledger_audit = name == "powershell-migration-audit.qjs";
            if is_prose || is_this_test || is_reader || is_itself || is_the_ledger_audit {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if text.contains(INSPECTOR) {
                invokers.push(
                    path.strip_prefix(root)
                        .unwrap_or(&path)
                        .display()
                        .to_string(),
                );
            }
        }
    }
    assert!(
        invokers.is_empty(),
        "the exempt Authenticode inspector is referenced by non-prose files, so it is \
         automation after all and the PowerShell ledger exemption no longer holds: {invokers:?}"
    );
}

/// The release artifact build is strictly more work than the release-fast
/// fixture roots it shares a toolchain with, so it can never legitimately be
/// given a smaller runaway budget. These budgets are guards against a hung
/// build, not contracts about how fast a hosted runner happens to be that day;
/// a runner 1.5x slower than the last one already took the artifact build from
/// 399s through a 600s ceiling once.
#[test]
fn the_release_artifact_build_is_never_budgeted_below_its_fixture_prebuild() {
    fn budget(source: &str, needle: &str) -> u64 {
        let tail = source
            .split_once(needle)
            .unwrap_or_else(|| panic!("check.qjs no longer contains {needle}"))
            .1;
        let digits: String = tail
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse().expect("a numeric budget")
    }
    let fixture = budget(
        &CHECK_QJS,
        "\"build\", \"--locked\", \"--profile\", \"release-fast\"",
    );
    let artifact = budget(&CHECK_QJS, "return release !== 0 ? ");
    assert!(
        artifact >= fixture,
        "the release artifact build is budgeted {artifact} ms but the smaller \
         release-fast fixture prebuild gets {fixture} ms"
    );
}

/// check.qjs keeps only EVIDENCE lines when it accumulates a gate's command
/// output, because that is all the qualification receipt consumes. Retaining
/// every line held each spec's whole stdout as guest strings and exhausted the
/// wasm guest's memory partway through the 23-spec unit-tests gate. If the
/// consumer ever starts keeping more than EVIDENCE lines, that upstream filter
/// silently becomes data loss, so pin both halves of the coupling together.
#[test]
fn gate_output_is_filtered_to_exactly_what_the_receipt_consumes() {
    let qualification = include_str!("../scripts/qjs/lib/qualification.qjs").replace("\r\n", "\n");
    let consumer = qualification
        .split_once("export function evidence_from_output(output) {")
        .expect("qualification.qjs must still expose evidence_from_output")
        .1
        .split_once("\n}")
        .expect("evidence_from_output must be closed")
        .0;
    assert!(
        consumer.contains("line.startsWith(\"EVIDENCE \")"),
        "the receipt consumer no longer selects on the EVIDENCE prefix, so \
         check.qjs must stop pre-filtering on it"
    );
    assert!(
        CHECK_QJS.contains("append_evidence_lines(all_output, result.stdout);"),
        "a gate's accumulated output must use the EVIDENCE-filtered appender"
    );
    // The two consumers need different filters and collapsing them breaks the
    // other one: `--list-evidence` answers bare ids with no marker prefix, so
    // filtering that path on the EVIDENCE prefix silently empties every
    // declaration and fails the run as `check_evidence_declaration_stale`.
    assert!(
        CHECK_QJS.contains("append_text_lines(evidence, result.stdout);"),
        "the --list-evidence declaration path must keep every line"
    );
    let declaration_appender = CHECK_QJS
        .split_once("function append_text_lines(target, text) {")
        .expect("append_text_lines must still exist")
        .1
        .split_once("\n}")
        .expect("append_text_lines must be closed")
        .0;
    assert!(
        !declaration_appender.contains("EVIDENCE "),
        "append_text_lines feeds the declaration comparison and must not filter \
         on the EVIDENCE prefix"
    );
}

/// Size references remain consistent across platform overrides even while
/// v0.1.17 observes rather than blocks on them. They can be compared with
/// exact Candidate measurements and reviewed before re-enabling a size court.
#[test]
fn a_release_size_budget_is_declared_consistently_for_every_platform() {
    let artifacts: serde_json::Value =
        serde_json::from_str(include_str!("../scripts/artifacts.json"))
            .expect("scripts/artifacts.json must parse");
    let mut budgets: std::collections::BTreeMap<String, std::collections::BTreeSet<u64>> =
        Default::default();
    fn collect(
        node: &serde_json::Value,
        out: &mut std::collections::BTreeMap<String, std::collections::BTreeSet<u64>>,
    ) {
        match node {
            serde_json::Value::Object(map) => {
                if let (Some(name), Some(budget)) = (
                    map.get("name").and_then(|v| v.as_str()),
                    map.get("release_budget_bytes").and_then(|v| v.as_u64()),
                ) {
                    out.entry(name.to_owned()).or_default().insert(budget);
                }
                for value in map.values() {
                    collect(value, out);
                }
            }
            serde_json::Value::Array(items) => {
                for value in items {
                    collect(value, out);
                }
            }
            _ => {}
        }
    }
    collect(&artifacts, &mut budgets);
    assert!(
        !budgets.is_empty(),
        "no release size budgets found; retire this gate deliberately"
    );
    for (name, values) in &budgets {
        assert_eq!(
            values.len(),
            1,
            "{name} declares more than one release budget across its platform \
             overrides: {values:?}"
        );
    }
}
