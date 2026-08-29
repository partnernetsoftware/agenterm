use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::json;

// `BUILD_TASK` (`scripts/rh/build.rh`), `run_prune`, `run_prune_with_manifest`
// and the tests that drove the `prune-target-incremental` task left with the
// rh engine on 2026-08-29 (partnernetsoftware/rh). What remains here is the
// incremental rustc wrapper itself -- Rust, reached through the main PE --
// which needs no script to be exercised. The prune task is dark until its
// .qjs port lands; the four `#[ignore]`d prune tests and
// `fingerprint_generations_keep_newest_two_per_crate` went with the script.
const INCREMENTAL_WRAPPER_SOURCE: &str = include_str!("../src/incremental_wrapper.rs");

fn fixture_root(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agenterm-target-prune-{name}-{}-{nonce}",
        std::process::id()
    ))
}

fn base36(mut value: u128) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut encoded = Vec::new();
    loop {
        encoded.push(DIGITS[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    encoded.reverse();
    String::from_utf8(encoded).expect("base36 is ASCII")
}

fn old_timestamp(seconds_ago: u128) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_micros();
    base36(now - seconds_ago * 1_000_000)
}

fn initialize_fixture(name: &str) -> PathBuf {
    let root = fixture_root(name);
    fs::create_dir_all(root.join("target/debug/incremental")).expect("create target fixture");
    fs::write(root.join("target/debug/.cargo-lock"), b"").expect("create Cargo lock");
    let initialized = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["init", "--quiet"])
        .output()
        .expect("initialize fixture repository");
    assert!(initialized.status.success());
    root
}

fn session(unit: &Path, timestamp: &str, random: &str, hash: &str) -> PathBuf {
    let path = unit.join(format!("s-{timestamp}-{random}-{hash}"));
    fs::create_dir_all(&path).expect("create session");
    fs::write(path.join("dep-graph.bin"), vec![b'x'; 4096]).expect("write session payload");
    path
}

fn session_lock(unit: &Path, timestamp: &str, random: &str) -> PathBuf {
    let path = unit.join(format!("s-{timestamp}-{random}.lock"));
    fs::write(&path, b"").expect("create session lock");
    path
}

fn open_locked(path: &Path) -> File {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .expect("open lock fixture");
    file.lock().expect("lock fixture");
    file
}

#[test]
fn wrapper_source_owns_the_cargo_lock_barrier_and_exact_touch_evidence() {
    let cargo_lock = INCREMENTAL_WRAPPER_SOURCE
        .find("let cargo_lock_observed = cargo_lock_is_held")
        .expect("Cargo lock observation");
    let before = INCREMENTAL_WRAPPER_SOURCE
        .find("let snapshot = snapshot_incremental_roots")
        .expect("before snapshot");
    let touch = INCREMENTAL_WRAPPER_SOURCE
        .find("touch.roots.insert(touched_name)")
        .expect("exact touched root record");
    // The forward is now three statements (builder + headless config +
    // status) so wrapped rustc never flashes a console when the wrapper is
    // the GUI-subsystem agenterm PE — assert the anchor of that chain.
    let compiler = INCREMENTAL_WRAPPER_SOURCE
        .find("let mut rustc = Command::new(compiler)")
        .expect("transparent compiler forward");
    assert!(INCREMENTAL_WRAPPER_SOURCE.contains("rustc.args(rustc_arguments)"));
    assert!(INCREMENTAL_WRAPPER_SOURCE.contains("match rustc.status()"));
    assert!(cargo_lock < before && before < touch && touch < compiler);
    assert!(INCREMENTAL_WRAPPER_SOURCE.contains("barrier.lock()?"));
    assert!(INCREMENTAL_WRAPPER_SOURCE.contains("full-tree-metadata-v1"));
    assert!(
        INCREMENTAL_WRAPPER_SOURCE
            .contains("!incremental_poison_present(&state.join(\"invalid\"))")
    );
    assert!(INCREMENTAL_WRAPPER_SOURCE.contains("crate::is_direct_directory(&entry.path())"));
    assert!(INCREMENTAL_WRAPPER_SOURCE.contains("crate::is_direct_file(path)"));
}

#[test]
fn both_executables_dispatch_incremental_wrapper_mode() {
    let root = initialize_fixture("wrapper-parity");
    let target = root.join("target");
    let incremental = target.join("debug/incremental");
    let invocation = "test-wrapper-parity-0001";
    let state = target.join("debug/.agenterm-incremental").join(invocation);
    // The compiler probe must be a DIFFERENT executable from the wrapper:
    // now that the wrapper IS the main agenterm PE, using agenterm as the
    // probe too would make the probe child inherit the wrapper env vars,
    // match `current_exe() == RUSTC_WRAPPER` itself, and recurse into
    // wrapper mode (a test-only artifact — real rustc is never agenterm).
    // `rustc` is guaranteed present wherever `cargo test` runs.
    let compiler_probe = Path::new("rustc");
    let direct = Command::new(compiler_probe)
        .arg("--version")
        .output()
        .expect("run direct compiler probe");

    // Exercises the main PE acting AS the `RUSTC_WRAPPER` itself:
    // `current_exe()` must equal the `RUSTC_WRAPPER` env var, and cargo's
    // real protocol invokes that single path with no injectable prefix
    // args. `src/bin/agenterm.rs::main()` probes
    // `is_incremental_rustc_wrapper_process` on raw args BEFORE any
    // subcommand dispatch (commit 82019aa9), which is exactly what makes
    // the standalone `agenterm-rh` wrapper binary retirable — this test
    // now locks that production shape.
    let executable = Path::new(env!("CARGO_BIN_EXE_agenterm"));
    let wrapped = Command::new(executable)
        .env(
            "AGENTERM_INTERNAL_RUSTC_WRAPPER",
            "agenterm-incremental-manifest-v1",
        )
        .env("AGENTERM_INCREMENTAL_INVOCATION_ID", invocation)
        .env("AGENTERM_INCREMENTAL_TARGET", &target)
        .env("AGENTERM_INCREMENTAL_ROOT", &incremental)
        .env("AGENTERM_INCREMENTAL_STATE", &state)
        .env("RUSTC_WRAPPER", executable)
        .arg(compiler_probe)
        .arg("--version")
        .output()
        .expect("run wrapper parity probe");
    assert_eq!(wrapped.status.code(), direct.status.code());
    assert_eq!(wrapped.stdout, direct.stdout);
    assert_eq!(wrapped.stderr, direct.stderr);

    fs::remove_dir_all(root).expect("remove wrapper parity fixture");
}

#[test]
fn rustc_wrapper_snapshots_under_cargo_lock_and_finalizes_exact_touch_manifest() {
    let root = initialize_fixture("producer-wrapper");
    let target = root.join("target");
    let incremental = target.join("debug/incremental");
    let stale = incremental.join("agenterm-stale");
    fs::create_dir(&stale).expect("create stale root");
    let timestamp = old_timestamp(120);
    session(&stale, &timestamp, "one", "hash");
    session_lock(&stale, &timestamp, "one");

    let invocation = "test-producer-invocation-0001";
    let state = target.join("debug/.agenterm-incremental").join(invocation);
    fs::create_dir_all(&state).expect("create producer state");
    let touched = incremental.join("probe-root");
    let executable = Path::new(env!("CARGO_BIN_EXE_agenterm"));
    // Distinct from the wrapper executable — see
    // both_executables_dispatch_incremental_wrapper_mode for why the probe
    // must not be agenterm itself now that agenterm IS the wrapper.
    let compiler_probe = Path::new("rustc");
    let compiler_arguments = [
        "--crate-name".to_owned(),
        "agenterm_incremental_probe".to_owned(),
        "-C".to_owned(),
        format!("incremental={}", touched.display()),
    ];
    let direct = Command::new(compiler_probe)
        .args(&compiler_arguments)
        .output()
        .expect("run direct compiler probe");
    let cargo_lock = open_locked(&target.join("debug/.cargo-lock"));
    let wrapped = Command::new(executable)
        .env(
            "AGENTERM_INTERNAL_RUSTC_WRAPPER",
            "agenterm-incremental-manifest-v1",
        )
        .env("AGENTERM_INCREMENTAL_INVOCATION_ID", invocation)
        .env("AGENTERM_INCREMENTAL_TARGET", &target)
        .env("AGENTERM_INCREMENTAL_ROOT", &incremental)
        .env("AGENTERM_INCREMENTAL_STATE", &state)
        .env("RUSTC_WRAPPER", executable)
        .arg(compiler_probe)
        .args(&compiler_arguments)
        .output()
        .expect("run rustc wrapper probe");
    assert_eq!(wrapped.status.code(), direct.status.code());
    assert_eq!(wrapped.stdout, direct.stdout);
    assert_eq!(wrapped.stderr, direct.stderr);
    let before: serde_json::Value =
        serde_json::from_slice(&fs::read(state.join("before.json")).expect("read before snapshot"))
            .expect("parse before snapshot");
    let touch: serde_json::Value =
        serde_json::from_slice(&fs::read(state.join("touch.json")).expect("read touch state"))
            .expect("parse touch state");
    assert_eq!(before["cargo_lock_observed"], true);
    assert_eq!(touch["rustc_invocations"], 1);
    assert_eq!(touch["roots"], json!(["probe-root"]));
    drop(cargo_lock);

    let manifest = state.join("manifest.json");
    let finalized = Command::new(executable)
        .env(
            "AGENTERM_INTERNAL_RUSTC_WRAPPER",
            "agenterm-incremental-manifest-v1",
        )
        .env("RUSTC_WRAPPER", executable)
        .arg("--internal-incremental-finalize")
        .arg(&state)
        .arg(&target)
        .arg(&manifest)
        .arg(invocation)
        .output()
        .expect("finalize producer manifest");
    assert!(finalized.status.success());
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).expect("read producer manifest"))
            .expect("parse producer manifest");
    assert_eq!(document["snapshot_complete"], true);
    assert_eq!(document["rustc_invocations"], 1);
    let stale_entry = document["roots"]
        .as_array()
        .expect("manifest roots")
        .iter()
        .find(|entry| entry["name"] == "agenterm-stale")
        .expect("stale root manifest entry");
    assert_eq!(stale_entry["touched"], false);
    assert_eq!(
        stale_entry["before_identity"],
        stale_entry["after_identity"]
    );

    fs::remove_dir_all(root).expect("remove producer wrapper fixture");
}

#[test]
fn hot_build_without_a_real_incremental_rustc_cannot_authorize_roots() {
    let root = initialize_fixture("producer-hot-build");
    let target = root.join("target");
    let invocation = "test-producer-hot-build-0001";
    let state = target.join("debug/.agenterm-incremental").join(invocation);
    fs::create_dir_all(&state).expect("create empty producer state");
    let manifest = state.join("manifest.json");
    let executable = Path::new(env!("CARGO_BIN_EXE_agenterm"));
    let finalized = Command::new(executable)
        .env(
            "AGENTERM_INTERNAL_RUSTC_WRAPPER",
            "agenterm-incremental-manifest-v1",
        )
        .env("RUSTC_WRAPPER", executable)
        .arg("--internal-incremental-finalize")
        .arg(&state)
        .arg(&target)
        .arg(&manifest)
        .arg(invocation)
        .output()
        .expect("finalize hot build manifest");
    assert!(finalized.status.success());
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).expect("read hot build manifest"))
            .expect("parse hot build manifest");
    assert_eq!(document["snapshot_complete"], false);
    assert_eq!(document["rustc_invocations"], 0);
    assert_eq!(document["roots"], json!([]));

    fs::remove_dir_all(root).expect("remove hot-build fixture");
}
