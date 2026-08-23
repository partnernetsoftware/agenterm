use std::sync::LazyLock;

static AGENTERM: LazyLock<String> = LazyLock::new(|| {
    include_str!("../.github/workflows/ci-agenterm.yml.disabled").replace("\r\n", "\n")
});
static CHASSIS: LazyLock<String> = LazyLock::new(|| {
    include_str!("../.github/workflows/ci-chassis.yml.disabled").replace("\r\n", "\n")
});

const CHECKOUT_SHA: &str = "08eba0b27e820071cde6df949e0beb9ba4906955";

#[test]
fn product_ci_workflows_are_independent_and_sha_pinned() {
    assert!(AGENTERM.contains("name: CI / agenterm"));
    // `agenterm-con` left for the `minicon` repository; this repo's CI must not
    // reference a package it no longer owns.
    assert!(!AGENTERM.contains("-p agenterm-con"));
    assert!(AGENTERM.contains("-p agenterm --all-targets"));
    assert!(AGENTERM.contains("./rh-check.sh"));
    assert!(CHASSIS.contains("name: CI / chassis"));
    assert!(CHASSIS.contains("-p agenterm-chassis"));
    assert!(!CHASSIS.contains("-p agenterm --"));
    assert!(!CHASSIS.contains("cargo xwin"));
    for source in [AGENTERM.as_str(), CHASSIS.as_str()] {
        assert!(source.contains(CHECKOUT_SHA));
        assert!(source.contains("persist-credentials: false"));
        assert!(source.contains("permissions:\n  contents: read"));
        assert!(source.contains("workflow_dispatch:"));
        assert!(source.contains("push:"));
        assert!(source.contains("pull_request:"));
    }
}

#[test]
fn the_workbench_covers_all_six_target_cells() {
    for target in [
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
    ] {
        assert!(AGENTERM.contains(target), "main CI misses {target}");
    }
    assert!(AGENTERM.contains("cargo xwin check --locked -p agenterm"));
    assert!(AGENTERM.contains("gcc-aarch64-linux-gnu"));
    assert!(AGENTERM.contains("runner: macos-15-intel"));
}

#[test]
fn chassis_ci_covers_six_cells_and_packs_l2_without_cargo() {
    for target in [
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
    ] {
        assert!(CHASSIS.contains(target), "chassis CI misses {target}");
    }
    assert!(CHASSIS.contains("python3 scripts/chassis-ci-pack.py"));
    assert!(CHASSIS.contains("python3 scripts/chassis-compose-product-test.py"));
    assert!(AGENTERM.contains("-p agenterm-chassis"));
    assert!(AGENTERM.contains("python3 scripts/chassis-ci-pack.py"));
    assert!(!CHASSIS.contains("-p agenterm --all-targets"));
}
