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
static ARTIFACTS: LazyLock<serde_json::Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../scripts/artifacts.json"))
        .expect("scripts/artifacts.json must remain valid JSON")
});
static BUILD_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/build.qjs").replace("\r\n", "\n"));
static CHECK_QJS: LazyLock<String> =
    LazyLock::new(|| include_str!("../scripts/qjs/check.qjs").replace("\r\n", "\n"));
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
fn windows_cu_budget_is_the_governing_two_mib_control_cli_budget() {
    const CONTROL_CLI_BUDGET: u64 = 2 * 1024 * 1024;
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

#[test]
fn target_inventory_keeps_default_host_ops_and_matches_its_outer_timeout() {
    let budget = &TASKS["contracts"]["target-report"]["budget"];
    assert_eq!(budget["timeout_ms"], 120_000);
    assert!(budget.get("max_host_operations").is_none());
    assert!(CHECK_QJS.contains("task(worker, repo, \"target-report\", 120000, [], no_env, 0)"));
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
        CHECK_QJS.contains("direct_task(\n      bootstrap_worker, repo, \"prd-alignment\", 120000")
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
fn powershell_launcher_test_is_an_explicit_terminal_compatibility_subcourt() {
    assert!(CHECK_QJS.contains("\"--skip\", \"powershell_waits_for_explicit_agenterm_exe\""));
    assert!(CHECK_QJS.contains("function cargo_unit_powershell_compat_spec(environment)"));
    assert!(
        CHECK_QJS.contains("powershell_waits_for_explicit_agenterm_exe\", \"--\", \"--exact\"")
    );
    assert!(CHECK_QJS.contains("], 120000, environment, 2);"));
    assert!(CHECK_QJS.contains("cargo_unit_powershell_compat_spec(build_environment)"));
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
        "const fixture_source = csc_path(rh.join(repo, \"examples/csharp/agenterm_uia_fixture.cs\"));"
    ));
    assert!(CU_WINDOWS_SMOKE_QJS.contains(
        "const fixture_executable = csc_path(rh.join(run_directory, \"agenterm-uia-fixture.exe\"));"
    ));
    assert!(CU_WINDOWS_FIXTURE_CS.contains("[STAThread]"));
    assert!(CU_WINDOWS_FIXTURE_CS.contains("public static void Main()"));
    assert!(CU_WINDOWS_FIXTURE_CS.contains("Run();"));
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
    assert!(CANDIDATE.contains("git merge-base --is-ancestor"));
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
        .and_then(|(_, tail)| tail.split_once("\nfunction run_gate_two("))
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
    assert!(CANDIDATE.contains("needs: [build, runtime]"));

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
    assert!(runtime.contains("candidate-part-${{ matrix.platform_id }}"));
    assert!(runtime.contains("Scan final Windows Candidate bytes with Defender"));
    assert!(runtime.contains("Prove Linux bundle closure in package-free Ubuntu"));
    assert!(!runtime.contains("actions/checkout"));
    assert!(!runtime.contains("cargo "));
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
    assert!(!CANDIDATE.contains(".agenterm-rhai.bin"));
    assert!(!CANDIDATE.contains("scripts/rhai/check.rhai"));
    assert!(!CANDIDATE.contains("scripts/rhai/fresh-clone-rehearsal.rhai"));
    assert!(CANDIDATE.contains("name: release-candidate-${{ github.run_id }}"));
    assert!(CANDIDATE.contains("retention-days: 14"));
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
    assert!(PROMOTION.contains("environment: release"));
    assert!(PROMOTION.contains("contents: write"));
    assert!(PROMOTION.contains("repos/$GITHUB_REPOSITORY/git/refs"));
    assert!(PROMOTION.contains("--verify-tag"));
    assert!(PROMOTION.contains("Recovering exact unpublished draft"));
    assert!(PROMOTION.contains("agenterm-promotion-identity"));
    assert!(PROMOTION.contains("task run promotion-identity"));
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
