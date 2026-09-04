//! `agenterm cli script run FILE` must pick the engine from the file's
//! extension, and this test exercises the **product path** to say so.
//!
//! # Why the product path and not `from_entry_path`
//!
//! `ScriptBackend::from_entry_path` was correct, tested, and documented from
//! the day it was written, and had **zero callers in production code** until
//! 2026-08-28. One of the tests covering it is named
//! `lua_task_entry_backend_selection` and carries the comment "Verify
//! path-based backend selection" -- while verifying nothing but the pure
//! function. Meanwhile `agenterm cli script run t.qjs` answered with *rh's*
//! parse error for a JavaScript file, and `.lua` did the same.
//!
//! So the acceptance criterion written into PRD 02.36 §接下来 04 before the
//! repair started was explicit: **a test must assert the product path, or the
//! repair reproduces the defect it repairs.** This file is that criterion.
//! It spawns the real binary and reads what came out.
//!
//! # What actually broke
//!
//! Two places, and neither was a missing call:
//!
//! 1. `worker_supervisor::script_backend_environment` materialised `"rh"` into
//!    the worker's environment when the parent had none. An eagerly-set
//!    default is indistinguishable from a user's explicit choice, so the
//!    "explicit beats extension" rule matched every time and the extension was
//!    never consulted.
//! 2. Every engine's `check`/`execute` re-read the environment through
//!    `enabled()` after the dispatcher had already chosen. Once the dispatcher
//!    gained a second input, the two disagreed and the engine refused work it
//!    had just been handed.
//!
//! Both are the same shape: a decision with more than one home.

use std::process::Command;

const AGENTERM_BIN: &str = env!("CARGO_BIN_EXE_agenterm");

/// The supervisor caps live workers at `GLOBAL_CONCURRENCY_LIMIT` (8) for
/// the whole machine, and cargo runs this file's tests on one thread per
/// core. Past eight tests each spawning a CLI, the ninth is refused with
/// `host_concurrency_limit` and fails for a reason that has nothing to do
/// with routing -- which is what the eleventh test here did. One CLI at a
/// time; the file still finishes in seconds.
static CLI_SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn cli_slot() -> std::sync::MutexGuard<'static, ()> {
    CLI_SLOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Run the CLI with `AGENTERM_SCRIPT_BACKEND` either removed or set, and
/// return `(stdout, stderr, exit code)`.
fn run_script(path: &std::path::Path, backend: Option<&str>) -> (String, String, i32) {
    let mut command = Command::new(AGENTERM_BIN);
    command.args(["cli", "script", "run"]).arg(path);
    match backend {
        Some(value) => command.env("AGENTERM_SCRIPT_BACKEND", value),
        None => command.env_remove("AGENTERM_SCRIPT_BACKEND"),
    };
    let output = command.output().expect("the CLI binary runs");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

fn write(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("fixture is writable");
    path
}

/// A `.qjs` file with no environment variable set runs on qjswasm.
///
/// The program is chosen to be unambiguous about *which* engine ran it: it
/// uses a template literal, an arrow function, `Array.prototype.map` and
/// `.length` -- four things rh's parser rejects outright. A wrong engine
/// cannot accidentally produce `sum=3`.
#[cfg(feature = "script-qjswasm")]
#[test]
fn a_qjs_entry_runs_on_qjswasm_without_being_told_to() {
    let _slot = cli_slot();
    let dir = std::env::temp_dir().join(format!("agenterm-route-qjs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = write(
        &dir,
        "t.qjs",
        "const xs = [1,2,3];\nreturn `sum=${xs.map(x => x * 2).length}`;\n",
    );

    let (stdout, stderr, code) = run_script(&path, None);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("sum=3"),
        "a `.qjs` entry must reach qjswasm with no environment variable; \
         got stdout={stdout} stderr={stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Source defects have one public failure class whether the compiler rejects
/// syntax or the running engine rejects a method outside its subset. Calling
/// either `configuration` sends the author to repair the invocation instead
/// of the script and disagrees with the bounded check-many route.
#[cfg(feature = "script-qjswasm")]
#[test]
fn qjs_source_failures_are_script_failures_through_the_public_cli() {
    let _slot = cli_slot();
    let dir =
        std::env::temp_dir().join(format!("agenterm-qjs-failure-class-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");

    for (name, source) in [
        ("syntax.qjs", "return @;"),
        ("method.qjs", "return \"x\".trimStart();"),
    ] {
        let path = write(&dir, name, source);
        let (stdout, stderr, code) = run_script(&path, None);
        assert_eq!(code, 1, "{name}: stdout={stdout} stderr={stderr}");
        assert!(
            stderr.contains("\"exit_class\":\"script\""),
            "{name}: stdout={stdout} stderr={stderr}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// An explicit backend beats the extension, in the direction that matters.
///
/// This is the half that keeps the repair from being the same defect pointed
/// the other way: someone who states a backend must get it, even for a file
/// whose extension says otherwise. `rh` is an engine that has left this
/// repository, so stating it must be *refused by name* -- a silent reroute
/// to qjswasm would succeed, and until 2026-08-29 the reverse silence was
/// the product's default.
#[test]
fn an_explicit_backend_beats_the_extension() {
    let _slot = cli_slot();
    let dir = std::env::temp_dir().join(format!("agenterm-route-explicit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = write(
        &dir,
        "t.qjs",
        "const xs = [1,2,3];\nreturn xs.map(x => x);\n",
    );

    let (stdout, stderr, code) = run_script(&path, Some("rh"));
    assert_ne!(
        code, 0,
        "rh is not here to run this program, so an explicit rh must fail rather \
         than be quietly rerouted; stdout={stdout} stderr={stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("partnernetsoftware/rh"),
        "the refusal must name the engine the caller asked for and say where \
         it went; got {combined}"
    );
    assert!(
        !combined.contains("sum=") && !combined.contains("[1,2,3]"),
        "nothing may have run; got {combined}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// With no backend stated and an extension nothing routes, the answer is a
/// refusal that names the entry -- not rh's parse error, which is what a
/// `.rh` file got here until the engine left, and not a run on whichever
/// engine happens to be compiled in.
#[test]
fn an_unrouted_entry_is_refused_by_name() {
    let _slot = cli_slot();
    let dir = std::env::temp_dir().join(format!("agenterm-route-unrouted-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = write(&dir, "t.rh", "fn entry() { 42 }\n");

    let (stdout, stderr, code) = run_script(&path, None);
    assert_ne!(code, 0, "stdout={stdout} stderr={stderr}");
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("t.rh") && combined.contains(".qjs is the script language now"),
        "the refusal must name the entry and the language that replaced the \
         default; got {combined}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The routing is general, not a `.qjs` special case.
#[cfg(feature = "script-lua")]
#[test]
fn a_lua_entry_runs_on_lua_without_being_told_to() {
    let _slot = cli_slot();
    let dir = std::env::temp_dir().join(format!("agenterm-route-lua-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = write(&dir, "t.lua", "return 1+2\n");

    let (stdout, stderr, code) = run_script(&path, None);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains('3'),
        "a `.lua` entry must reach lua with no environment variable; \
         got stdout={stdout} stderr={stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The verbs that take a file and produce an **artifact** route the same way.
///
/// `run`/`check`/`eval` were fixed first because they are how a script is
/// actually run, and the fix reached only them: `pack`, `qualify`,
/// `run-smoke` and `hash` still asked the environment alone. That left
/// `agenterm cli script hash t.qjs` answering with rh's *source* digest for a
/// JavaScript file -- and hash is the quietest verb to be wrong in, because
/// the wrong engine still prints a plausible hex string.
///
/// The two labels are what makes this assertable without hard-coding a digest.
/// qjswasm hashes the compiled `.wasm` and says `wasm`; every other engine
/// hashes the text and says `source`. They are both correct answers to
/// different questions, which is why the label travels with the digest.
#[cfg(feature = "script-qjswasm")]
#[test]
fn hash_routes_by_extension_and_still_yields_to_an_explicit_backend() {
    let _slot = cli_slot();
    let dir = std::env::temp_dir().join(format!("agenterm-route-hash-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = write(&dir, "t.qjs", "return 1 + 2;\n");

    let mut by_extension = Command::new(AGENTERM_BIN);
    by_extension.args(["cli", "script", "hash"]).arg(&path);
    by_extension.env_remove("AGENTERM_SCRIPT_BACKEND");
    let out = by_extension.output().expect("the CLI binary runs");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains("  wasm  "),
        "a `.qjs` entry must be hashed by the engine that compiles it, which \
         labels its digest `wasm`; got {text}"
    );

    let mut explicit = Command::new(AGENTERM_BIN);
    explicit.args(["cli", "script", "hash"]).arg(&path);
    explicit.env("AGENTERM_SCRIPT_BACKEND", "rh");
    let out = explicit.output().expect("the CLI binary runs");
    let text = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_ne!(
        out.status.code(),
        Some(0),
        "an explicit backend must still win here"
    );
    assert!(
        text.contains("partnernetsoftware/rh"),
        "an explicit backend must still win here, and rh's answer is now the \
         refusal that says where it went; got {text}"
    );
    assert!(
        out.stdout.is_empty(),
        "no digest may be printed for a refused engine"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `pack build` too, which is the verb that would otherwise refuse outright.
///
/// Before this it reached rh (the default then), whose deployable shape was
/// a directory rather than one file of bytes, so the answer was a paragraph
/// explaining that rh cannot build an artifact through this verb -- correct
/// about rh, and about an engine the caller never asked for.
#[cfg(feature = "script-qjswasm")]
#[test]
fn pack_build_routes_by_extension() {
    let _slot = cli_slot();
    let dir = std::env::temp_dir().join(format!("agenterm-route-pack-{}", std::process::id()));
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&out_dir).expect("temp dir");
    let path = write(&dir, "t.qjs", "return 1 + 2;\n");

    let mut command = Command::new(AGENTERM_BIN);
    command
        .args(["cli", "script", "pack", "build"])
        .arg(&path)
        .arg("--dir")
        .arg(&out_dir);
    command.env_remove("AGENTERM_SCRIPT_BACKEND");
    let out = command.output().expect("the CLI binary runs");
    assert!(
        out.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out_dir.join("t.wasm").exists(),
        "the compiler-backed engine's pack shape is one self-contained `.wasm`"
    );

    // And the artifact runs by its extension, with no environment variable:
    // `.wasm` is qjswasm's compiled form (A5, decided 2026-08-30).
    let mut run = Command::new(AGENTERM_BIN);
    run.args(["cli", "script", "run"])
        .arg(out_dir.join("t.wasm"))
        .env_remove("AGENTERM_SCRIPT_BACKEND");
    let out = run.output().expect("the CLI binary runs");
    assert!(
        out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "3",
        "the packed artifact answers what the source answered; stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `.qjs` file on disk can import the library beside it, through the CLI.
///
/// This is the whole point of the module milestone, asserted where a user
/// would meet it. `scripts/qjs/lib/fleet.qjs` binds all 29 fleet operations
/// and had no consumers, because the only way to reach it was to paste its
/// text in front of a script from Rust. A file that says `import * as lib from
/// "lib/fleet"` and runs is the difference.
///
/// Specifiers resolve under the invocation's project root, which defaults to
/// the entry file's own directory -- so a script beside a `lib/` needs nothing
/// configured, which is ECMA-262's relative-to-the-importer shape in practice.
#[cfg(feature = "script-qjswasm")]
#[test]
fn a_qjs_file_on_disk_can_import_the_library_beside_it() {
    let _slot = cli_slot();
    let dir = std::env::temp_dir().join(format!("agenterm-import-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("lib")).expect("temp dir");
    std::fs::write(
        dir.join("lib/greet.qjs"),
        "export function hello(who) { return \"hi \" + who; }\n",
    )
    .expect("fixture is writable");
    let entry = write(
        &dir,
        "main.qjs",
        "import * as g from \"lib/greet\";\nreturn g.hello(\"world\");\n",
    );

    let (stdout, stderr, code) = run_script(&entry, None);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("hi world"),
        "the imported function must run; got stdout={stdout} stderr={stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A specifier that leaves the project root is refused.
///
/// Checked on the **canonical** path, so `../`, a symlink and an absolute path
/// are all one case rather than three textual ones to get right separately.
#[cfg(feature = "script-qjswasm")]
#[test]
fn a_specifier_that_escapes_the_project_root_is_refused() {
    let _slot = cli_slot();
    let dir = std::env::temp_dir().join(format!("agenterm-escape-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("inner")).expect("temp dir");
    std::fs::write(dir.join("outside.qjs"), "export const secret = 1;\n").expect("fixture");
    let entry = write(
        &dir.join("inner"),
        "main.qjs",
        "import * as o from \"../outside\";\nreturn o.secret;\n",
    );

    let (stdout, stderr, code) = run_script(&entry, None);
    assert_ne!(code, 0, "stdout={stdout} stderr={stderr}");
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("../outside"),
        "the refusal must name the specifier it would not follow; got {combined}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `--profile tool` is the only way a script reaches the machine, and it
/// reaches it through the real CLI.
///
/// The `tool.*` door landed in the crate first (`4a7f0ec3`) with the CLI
/// deliberately unwired. This is the wiring, asserted from the user's side:
/// the same script, with and without the profile, and the two answers that
/// have to differ. The door is two-pass like the fleet door -- a call returns
/// a status and `tool_result()` carries the bytes -- which is why the script
/// reads the way it does.
#[cfg(feature = "script-qjswasm")]
#[test]
fn a_tool_script_reaches_the_machine_only_under_the_tool_profile() {
    let _slot = cli_slot();
    let dir = std::env::temp_dir().join(format!("agenterm-tool-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("in.txt"), "hello from disk").expect("fixture");
    let entry = write(
        &dir,
        "tool.qjs",
        "const status = fs_read_to_string(\"in.txt\");\n\
         if (status !== 0) { return \"status:\" + status; }\n\
         return \"read:\" + tool_result() + \"|missing:\" + fs_exists(\"nope.txt\");\n",
    );

    let mut with = Command::new(AGENTERM_BIN);
    with.args(["cli", "script", "run", "--profile", "tool"])
        .arg(&entry);
    with.current_dir(&dir).env_remove("AGENTERM_SCRIPT_BACKEND");
    let out = with.output().expect("the CLI binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("read:hello from disk|missing:0"),
        "with --profile tool the script must read the file; stdout={stdout} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let mut without = Command::new(AGENTERM_BIN);
    without.args(["cli", "script", "run"]).arg(&entry);
    without
        .current_dir(&dir)
        .env_remove("AGENTERM_SCRIPT_BACKEND");
    let out = without.output().expect("the CLI binary runs");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // Which `fs_*` name the sandbox trips on first is resolution order, not
    // contract; that it names *a* tool function and lists only the three
    // sandbox imports is.
    assert!(
        !out.status.success()
            && combined.contains("no host function named `fs_")
            && combined.contains("`print`, `fleet_call` and `fleet_result`"),
        "without the profile the sandbox must refuse by name; got {combined}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The first task script moved from rh, run the way CI will run it.
///
/// `validate-artifact-manifest.qjs` imports `lib/artifact_manifest.qjs`
/// (13 of the 71 rh scripts imported the rh original), takes its argument
/// through the tool door -- `arg_count()` / `arg(0)` -- because the engine
/// face cannot carry a string into a guest, and validates the real
/// `scripts/artifacts.json`. The count it returns is the manifest's: four
/// executables and one library.
///
/// A manifest with a bad name must fail by that name. The check that catches
/// it is written without character access, because this engine has neither
/// `s[i]` nor `split("")`; the library's comment says how.
#[cfg(feature = "script-qjswasm")]
#[test]
fn the_first_migrated_task_script_validates_the_real_manifest() {
    let _slot = cli_slot();
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = repo.join("scripts/qjs/validate-artifact-manifest.qjs");
    let manifest = repo.join("scripts/artifacts.json");

    let mut ok = Command::new(AGENTERM_BIN);
    ok.args(["cli", "script", "run", "--profile", "tool"])
        .arg(&script)
        .arg("--")
        .arg(&manifest)
        .env_remove("AGENTERM_SCRIPT_BACKEND");
    let out = ok.output().expect("the CLI binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.trim() == "5",
        "the real manifest has 4 executables + 1 library; stdout={stdout} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let dir = std::env::temp_dir().join(format!("agenterm-manifest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut bad: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest).expect("read")).expect("json");
    bad["executables"][0]["name"] = serde_json::json!("agenterm-x1.exe");
    let bad_path = dir.join("bad.json");
    std::fs::write(&bad_path, bad.to_string()).expect("fixture");

    let mut refused = Command::new(AGENTERM_BIN);
    refused
        .args(["cli", "script", "run", "--profile", "tool"])
        .arg(&script)
        .arg("--")
        .arg(&bad_path)
        .env_remove("AGENTERM_SCRIPT_BACKEND");
    let out = refused.output().expect("the CLI binary runs");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // The reason, not just the fact: since tinyvm 25fcf02 a thrown String is
    // readable by the host, and this is the line every migrated gate script
    // exists to print when it fails.
    assert!(
        !out.status.success()
            && combined.contains("artifact_manifest_name_invalid:agenterm-x1.exe"),
        "a digit in the role must be refused by name; got {combined}"
    );
    // And the class: the script threw, so the failure is the script's, not
    // the invocation's. Before the engine error carried a category every
    // backend failure said `configuration`.
    assert!(
        combined.contains("\"exit_class\":\"script\""),
        "an uncaught throw is a script failure; got {combined}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `--max-operations` reaches the guest as its step ceiling, and running out
/// is a `limit`, not a `configuration` error. Until 2026-08-29 the flag was
/// validated, audited and then ignored: the engine ran under its own 16M
/// default whatever the CLI said. The loop below costs ~100 steps per
/// iteration under the V1 boxed representation, so 1000 iterations need
/// ~100k steps: a 1000-step budget must refuse it and the default must not.
/// What a script printed before it failed reaches stdout, ahead of the
/// failure on stderr. Until 2026-08-29 a throw or a budget limit discarded
/// it, so a gate's STEP lines were lost on exactly the runs that mattered;
/// every wave-2 migration group reported it.
#[cfg(feature = "script-qjswasm")]
#[test]
fn what_a_script_printed_before_it_failed_reaches_stdout() {
    let _slot = cli_slot();
    let dir = std::env::temp_dir().join(format!("agenterm-stdout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let script = write(
        &dir,
        "steps.qjs",
        "print(\"STEP 1\");\nprint(\"STEP 2\");\nthrow \"boom\";\n",
    );
    let mut run = Command::new(AGENTERM_BIN);
    run.args(["cli", "script", "run"])
        .arg(&script)
        .env_remove("AGENTERM_SCRIPT_BACKEND");
    let out = run.output().expect("the CLI binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a throw fails the run");
    assert_eq!(
        stdout, "STEP 1\nSTEP 2\n",
        "stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains("nothing caught it: boom") && !stderr.contains("STEP"),
        "the failure is on stderr, the steps are not; stderr={stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--project-root DIR` widens where imports may reach; it must not replace
/// the entry's own directory. Wave 2 found every entry beside a `lib/` lost
/// its imports the moment a root was named.
#[cfg(feature = "script-qjswasm")]
#[test]
fn a_project_root_widens_resolution_and_does_not_replace_the_entry_directory() {
    let _slot = cli_slot();
    let dir = std::env::temp_dir().join(format!("agenterm-root-{}", std::process::id()));
    let other = dir.join("elsewhere");
    std::fs::create_dir_all(dir.join("lib")).expect("temp dir");
    std::fs::create_dir_all(&other).expect("temp dir");
    write(
        &dir.join("lib"),
        "x.qjs",
        "export function answer() { return 41 + 1; }\n",
    );
    let entry = write(
        &dir,
        "entry.qjs",
        "import * as x from \"lib/x\";\nprint(\"\" + x.answer());\n",
    );
    let mut run = Command::new(AGENTERM_BIN);
    run.args(["cli", "script", "run", "--project-root"])
        .arg(&other)
        .arg(&entry)
        .env_remove("AGENTERM_SCRIPT_BACKEND");
    let out = run.output().expect("the CLI binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.trim() == "42",
        "the entry's own lib/ must resolve under a foreign root; stdout={stdout} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "script-qjswasm")]
#[test]
fn the_operations_budget_reaches_the_guest_and_exhaustion_is_a_limit() {
    let _slot = cli_slot();
    let dir = std::env::temp_dir().join(format!("agenterm-steps-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let script = write(
        &dir,
        "busy.qjs",
        "let n = 0;\nfor (let i = 0; i < 1000; i = i + 1) { n = n + 1; }\nprint(\"done \" + n);\n",
    );

    let mut starved = Command::new(AGENTERM_BIN);
    starved
        .args(["cli", "script", "run", "--max-operations", "1000"])
        .arg(&script)
        .env_remove("AGENTERM_SCRIPT_BACKEND");
    let out = starved.output().expect("the CLI binary runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success()
            && stderr.contains("budget exhausted: max_steps")
            && stderr.contains("\"exit_class\":\"limit\""),
        "1000 steps cannot run a 1000-iteration loop, and that is a limit; got {stderr}"
    );

    let mut fed = Command::new(AGENTERM_BIN);
    fed.args(["cli", "script", "run"])
        .arg(&script)
        .env_remove("AGENTERM_SCRIPT_BACKEND");
    let out = fed.output().expect("the CLI binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.trim() == "done 1000",
        "the default budget runs it; stdout={stdout} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The guest heap is a bump allocator with no collector, and tinyvm's own
/// default of 256 pages (16 MiB) stopped a GUI journey on 2026-08-30. A
/// `.qjs` invocation gets 1024 pages: twenty-four live 1 MiB strings fit, and
/// `AGENTERM_QJS_MAX_MEMORY_PAGES` turns the ceiling down to the old one,
/// where the same script is a `limit` refusal naming `max_memory_pages`.
#[cfg(feature = "script-qjswasm")]
#[test]
fn the_guest_heap_ceiling_is_1024_pages_and_the_env_knob_moves_it() {
    let _slot = cli_slot();
    let dir = std::env::temp_dir().join(format!("agenterm-heap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let script = write(
        &dir,
        "heap.qjs",
        "let s = \"0123456789012345\";\nfor (let i = 0; i < 16; i = i + 1) { s = s + s; }\nlet keep = [];\nfor (let k = 0; k < 24; k = k + 1) { keep.push(s + k); }\nprint(\"done \" + keep.length);\n",
    );

    let mut roomy = Command::new(AGENTERM_BIN);
    roomy
        .args([
            "cli",
            "script",
            "run",
            "--max-operations",
            "1000000000",
            "--timeout-ms",
            "120000",
        ])
        .arg(&script)
        .env_remove("AGENTERM_SCRIPT_BACKEND")
        .env_remove("AGENTERM_QJS_MAX_MEMORY_PAGES");
    let out = roomy.output().expect("the CLI binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.trim() == "done 24",
        "twenty-four live megabytes fit in 1024 pages; stdout={stdout} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let mut cramped = Command::new(AGENTERM_BIN);
    cramped
        .args([
            "cli",
            "script",
            "run",
            "--max-operations",
            "1000000000",
            "--timeout-ms",
            "120000",
        ])
        .arg(&script)
        .env_remove("AGENTERM_SCRIPT_BACKEND")
        .env("AGENTERM_QJS_MAX_MEMORY_PAGES", "256");
    let out = cramped.output().expect("the CLI binary runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success()
            && stderr.contains("budget exhausted: max_memory_pages")
            && stderr.contains("\"exit_class\":\"limit\""),
        "256 pages cannot hold them, and that is a limit; got {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
