use std::collections::BTreeSet;

const ROOT_BUILD: &str = include_str!("../build.rs");
const ABI_BUILD: &str = include_str!("../crates/agenterm-abi/build.rs");
const CANDIDATE: &str = include_str!("../.github/workflows/candidate.yml");

#[test]
fn windows_signing_allowlist_is_five_pe_files_on_both_isas() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../scripts/artifacts.json")).unwrap();
    let expected = BTreeSet::from([
        "agenterm-cc.exe",
        "agenterm-cu.exe",
        "agenterm.com",
        "agenterm.dll",
        "agenterm.exe",
    ]);

    for arch in ["x86_64", "aarch64"] {
        let cell = manifest["platforms"]
            .as_array()
            .unwrap()
            .iter()
            .find(|cell| cell["os"] == "windows" && cell["arch"] == arch)
            .unwrap_or_else(|| panic!("missing Windows {arch} artifact cell"));
        let actual = cell["executables"]
            .as_array()
            .unwrap()
            .iter()
            .chain(cell["libraries"].as_array().unwrap())
            .map(|entry| entry["name"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected, "Windows {arch} signing allowlist drift");
    }
}

#[test]
fn root_windows_resources_follow_target_and_keep_forwarder_icon_free() {
    assert!(ROOT_BUILD.contains("CARGO_CFG_TARGET_OS"));
    assert!(ROOT_BUILD.contains("CARGO_CFG_TARGET_ENV"));
    assert!(!ROOT_BUILD
        .lines()
        .any(|line| line.trim_start().starts_with("#[cfg(windows)]")));
    assert!(ROOT_BUILD.contains("ProductName"));
    assert!(ROOT_BUILD.contains("ProductVersion"));
    assert!(ROOT_BUILD.contains("OriginalFilename\", \"agenterm.com"));
    assert!(ROOT_BUILD.contains("cargo:rustc-link-arg-bin=agenterm-com={forwarder_resource}"));
    assert!(!ROOT_BUILD
        .contains("ICON_BINS: &[&str] = &[\"agenterm\", \"agenterm-cc\", \"agenterm-com\"]"));
}

#[test]
fn abi_resource_is_cdylib_only_and_cross_build_has_a_pinned_compiler() {
    assert!(ABI_BUILD.contains("CARGO_CFG_TARGET_OS"));
    assert!(ABI_BUILD.contains("ProductName"));
    assert!(ABI_BUILD.contains("ProductVersion"));
    assert!(ABI_BUILD.contains("OriginalFilename\", \"agenterm.dll"));
    assert!(ABI_BUILD.contains("cargo:rustc-cdylib-link-arg={resource_path}"));
    assert!(!ABI_BUILD.contains("cargo:rustc-link-arg={resource_path}"));

    let arm_cell = CANDIDATE
        .split("- platform_id: windows-aarch64")
        .nth(1)
        .and_then(|tail| tail.split("timeout_minutes:").next())
        .expect("Windows ARM64 Candidate matrix cell");
    assert!(arm_cell.contains("runner: ubuntu-24.04"));

    let arm_tooling = CANDIDATE
        .split("- name: Install Windows ARM64 cross-build tooling")
        .nth(1)
        .and_then(|tail| tail.split("- name: Build Windows ARM64 release").next())
        .expect("Windows ARM64 resource-tooling step");
    assert!(arm_tooling.contains("llvm-18"));
    assert!(arm_tooling.contains("RC_PATH=/usr/bin/llvm-rc-18"));
}
