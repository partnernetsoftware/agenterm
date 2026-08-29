const BOOTSTRAP: &str = include_str!("../scripts/bootstrap.cmd");
const UNIX_BOOTSTRAP: &str = include_str!("../scripts/bootstrap.sh");
// `release_cleanup_reclaims_both_repo_targets_after_staging` read
// `scripts/rh/build.rh` and asserted the shape of its release-cleanup
// branch. That script left with the rh engine on 2026-08-29
// (partnernetsoftware/rh); the build task is dark until its .qjs port
// lands, and the assertion belongs to that port.

#[test]
fn bootstrap_worker_never_executes_from_a_repo_cargo_target() {
    assert!(BOOTSTRAP.contains("AGENTERM_BOOTSTRAP_CACHE_ROOT"));
    assert!(BOOTSTRAP.contains("%LOCALAPPDATA%\\AgenTerm\\build-cache"));
    assert!(BOOTSTRAP.contains("%TEMP%\\AgenTerm-build-cache"));
    assert!(BOOTSTRAP.contains("AGENTERM_BOOTSTRAP_DIR=%AGENTERM_BOOTSTRAP_CACHE_DIR%\\task-"));
    assert!(!BOOTSTRAP.contains("AGENTERM_BOOTSTRAP_DIR=%AGENTERM_BOOTSTRAP_TARGET%\\task-"));
}

#[test]
fn bootstrap_exposes_one_stable_cross_platform_rustc_wrapper_path() {
    assert!(BOOTSTRAP.contains("set \"AGENTERM_BOOTSTRAP_CACHE_WORKER="));
    assert!(UNIX_BOOTSTRAP.contains("AGENTERM_BOOTSTRAP_CACHE_WORKER=\"$CACHE_WORKER\""));
    assert!(
        UNIX_BOOTSTRAP.contains("export AGENTERM_BOOTSTRAP_WORKER AGENTERM_BOOTSTRAP_CACHE_WORKER")
    );
}

#[test]
fn bootstrap_builds_caches_and_executes_only_the_script_worker() {
    assert!(
        BOOTSTRAP
            .contains("AGENTERM_BOOTSTRAP_SOURCE=%AGENTERM_BOOTSTRAP_TARGET%\\debug\\agenterm.exe")
    );
    assert!(
        BOOTSTRAP.contains(
            "AGENTERM_BOOTSTRAP_CACHE_WORKER=%AGENTERM_BOOTSTRAP_CACHE_DIR%\\agenterm.exe"
        )
    );
    assert!(BOOTSTRAP.contains("cargo build --locked --bin agenterm"));
    assert!(!BOOTSTRAP.contains("cargo build --quiet --locked --bin agenterm"));
    assert!(!BOOTSTRAP.contains("--bin agenterm-rh"));
    assert!(!BOOTSTRAP.contains("cargo build --quiet --locked --bin agenterm-rhai"));
    assert!(!BOOTSTRAP.contains("agenterm-rhai.exe"));
    assert!(BOOTSTRAP.contains("\"%AGENTERM_BOOTSTRAP_WORKER%\" cli script task run"));
    assert!(!BOOTSTRAP.contains(" rh task run"));
    assert!(!BOOTSTRAP.contains("AGENTERM_SCRIPT_BACKEND=rh"));
    assert!(!BOOTSTRAP.contains("AGENTERM_BOOTSTRAP_RH_CLI"));
    assert!(!BOOTSTRAP.contains("AGENTERM_RHAI_COMPAT_CLI"));

    assert!(UNIX_BOOTSTRAP.contains("SOURCE=\"$TARGET_ROOT/debug/agenterm\""));
    assert!(UNIX_BOOTSTRAP.contains("CACHE_WORKER=\"$CACHE_DIR/agenterm\""));
    assert!(UNIX_BOOTSTRAP.contains("WORKER=\"$BOOTSTRAP_DIR/agenterm\""));
    assert!(UNIX_BOOTSTRAP.contains("cargo build --quiet --locked --bin agenterm"));
    assert!(!UNIX_BOOTSTRAP.contains("--bin agenterm-rh"));
    assert!(!UNIX_BOOTSTRAP.contains("cargo build --quiet --locked --bin agenterm-rhai"));
    assert!(!UNIX_BOOTSTRAP.contains("agenterm-rhai\""));
    assert!(UNIX_BOOTSTRAP.contains("\"$WORKER\" cli script task run"));
    assert!(!UNIX_BOOTSTRAP.contains(" rh task run"));
    assert!(!UNIX_BOOTSTRAP.contains("AGENTERM_SCRIPT_BACKEND=rh"));
    assert!(!UNIX_BOOTSTRAP.contains("AGENTERM_BOOTSTRAP_RH_CLI"));
    assert!(!UNIX_BOOTSTRAP.contains("AGENTERM_RHAI_COMPAT_CLI"));
}
