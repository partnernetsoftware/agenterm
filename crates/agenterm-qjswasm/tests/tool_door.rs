//! The `tool.*` door, from the product face: a `.qjs` script reaching the
//! filesystem, a child process and the environment through a real engine --
//! and a sandbox engine refusing the same script, by name, at load time.
//!
//! # What this file locks
//!
//! 1. **Opt-in.** `compile_qjs` does not know the door exists;
//!    `Engine::new()` refuses bytes that import it; only
//!    `compile_qjs_tool` / `Engine::with_tool_door` open it.
//! 2. **Zero cost when unmentioned.** A script that names no tool function
//!    compiles to byte-identical wasm through both entry points.
//! 3. **Each family works end to end**, through the real compiler, load gate
//!    and door: fs, process, env, plus the shared `tool_result` fetch.
//! 4. **Every call is on the receipt.** `Outcome::tool_calls` names each
//!    operation reached, in order, and is empty for a sandbox slot.
//! 5. **The budget applies.** A file over `max_bridge_result_bytes` is a
//!    refusal (status 1, bounded diagnostic), never a prefix.
//!
//! Nothing here asserts "it compiled" where running it was possible.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use agenterm_qjswasm::{
    Budget, Engine, Guest, JsValue, Outcome, QjswasmError, Value, compile_qjs, compile_qjs_tool,
    validate_wasm, validate_wasm_tool_with,
};

// =========================================================================
// Harness
// =========================================================================

/// A fresh directory per test, under the OS temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "agenterm-qjswasm-tool-{}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst),
            tag
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A path as a JS string literal.
fn js(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).expect("a path is a JSON string")
}

#[track_caller]
fn run_tool(source: &str) -> Outcome {
    Engine::with_tool_door(Budget::default())
        .run_once(Guest::Qjs(source), None, "main", &[])
        .unwrap_or_else(|e| panic!("{source}\n--> {e}"))
}

#[track_caller]
fn string_of(out: &Outcome) -> String {
    match out.values.as_slice() {
        [Value::Js(JsValue::Str(s))] => s.clone(),
        other => panic!("returned {other:?}, wanted one JS string"),
    }
}

#[track_caller]
fn number_of(out: &Outcome) -> f64 {
    match out.values.as_slice() {
        [Value::Js(JsValue::Number(n))] => *n,
        other => panic!("returned {other:?}, wanted one JS number"),
    }
}

// =========================================================================
// fs
// =========================================================================

/// Write, exists, metadata, read back, remove, exists again -- one script,
/// one receipt.
#[test]
fn fs_round_trip_write_read_metadata_remove() {
    let dir = Scratch::new("fs");
    let file = dir.path("note.txt");
    let out = run_tool(&format!(
        r#"
        let p = {p};
        if (fs_exists(p) !== 0) {{ return "exists before write"; }}
        if (fs_write(p, "hello, door") !== 0) {{ return "write: " + tool_result(); }}
        if (fs_exists(p) !== 1) {{ return "missing after write"; }}
        if (fs_metadata(p) !== 0) {{ return "metadata: " + tool_result(); }}
        let m = JSON.parse(tool_result());
        if (!m.is_file || m.is_dir || m.len !== 11) {{ return "metadata wrong: " + tool_result(); }}
        if (fs_read_to_string(p) !== 0) {{ return "read: " + tool_result(); }}
        let text = tool_result();
        if (fs_remove_file(p) !== 0) {{ return "remove: " + tool_result(); }}
        if (fs_exists(p) !== 0) {{ return "exists after remove"; }}
        return text;
        "#,
        p = js(&file)
    ));
    assert_eq!(string_of(&out), "hello, door");
    assert!(!file.exists(), "the script removed it");
    assert_eq!(
        out.tool_calls,
        [
            "tool.fs.exists",
            "tool.fs.write",
            "tool.fs.exists",
            "tool.fs.metadata",
            "tool.fs.read_to_string",
            "tool.fs.remove_file",
            "tool.fs.exists",
        ],
        "every operation is on the receipt, in order, one entry per call"
    );
}

#[test]
fn fs_create_dir_all_then_read_dir_is_sorted_json() {
    let dir = Scratch::new("readdir");
    let nested = dir.path("a").join("b").join("c");
    let out = run_tool(&format!(
        r#"
        let d = {d};
        if (fs_create_dir_all(d) !== 0) {{ return "mkdir: " + tool_result(); }}
        fs_write(d + "/zeta.txt", "z");
        fs_write(d + "/alpha.txt", "a");
        fs_create_dir_all(d + "/mid");
        if (fs_read_dir(d) !== 0) {{ return "read_dir: " + tool_result(); }}
        let entries = JSON.parse(tool_result());
        let names = "";
        for (let i = 0; i < entries.length; i = i + 1) {{
            names = names + entries[i].name + (entries[i].is_dir ? "/" : "") + ";";
        }}
        return names;
        "#,
        d = js(&nested)
    ));
    assert_eq!(string_of(&out), "alpha.txt;mid/;zeta.txt;");
    assert!(nested.join("mid").is_dir());
}

#[test]
fn fs_tree_summary_is_exact_bounded_and_small_across_a_nested_tree() {
    let dir = Scratch::new("tree-summary");
    let root = dir.path("target");
    let deps = root.join("debug").join("deps");
    std::fs::create_dir_all(&deps).expect("nested fixture");
    std::fs::write(root.join("root.bin"), "x").expect("root fixture");
    std::fs::write(root.join("debug").join("debug.bin"), "abc").expect("debug fixture");
    std::fs::write(deps.join("dependency.bin"), "12345").expect("deps fixture");

    let out = run_tool(&format!(
        r#"
        if (fs_tree_summary({root}, 5) !== 0) {{ return "summary: " + tool_result(); }}
        return tool_result();
        "#,
        root = js(&root)
    ));
    let report: serde_json::Value =
        serde_json::from_str(&string_of(&out)).expect("tree summary JSON");
    assert_eq!(report["complete"], true);
    assert_eq!(report["entries"], 5);
    assert_eq!(report["files"], 3);
    assert_eq!(report["bytes"], 9);
    assert_eq!(report["profiles"][0]["name"], "(root)");
    assert_eq!(report["profiles"][0]["files"], 1);
    assert_eq!(report["profiles"][1]["name"], "debug");
    assert_eq!(report["profiles"][1]["files"], 2);

    let bounded = run_tool(&format!(
        r#"
        let status = fs_tree_summary({root}, 4);
        return status + ":" + tool_result();
        "#,
        root = js(&root)
    ));
    assert!(
        string_of(&bounded).contains("tree summary entry limit 4 exceeded"),
        "{bounded:?}"
    );
}

/// A missing file is status 1 with a readable diagnostic, not a trap and not
/// an empty string.
#[test]
fn fs_read_of_a_missing_file_is_status_one_with_a_diagnostic() {
    let dir = Scratch::new("missing");
    let out = run_tool(&format!(
        r#"
        let status = fs_read_to_string({p});
        return status + ":" + tool_result();
        "#,
        p = js(&dir.path("nope.txt"))
    ));
    let got = string_of(&out);
    assert!(got.starts_with("1:fs.read_to_string"), "{got}");
    assert!(got.contains("nope.txt"), "names the path: {got}");
}

/// A file over `max_bridge_result_bytes` is refused with the door's own
/// bounded message -- the script never sees a prefix of it.
#[test]
fn fs_read_over_the_result_cap_is_a_refusal_not_a_prefix() {
    let dir = Scratch::new("cap");
    let big = dir.path("big.txt");
    std::fs::write(&big, "x".repeat(64)).unwrap();
    let mut engine = Engine::with_tool_door(Budget {
        max_bridge_result_bytes: 16,
        ..Budget::default()
    });
    let out = engine
        .run_once(
            Guest::Qjs(&format!(
                "let s = fs_read_to_string({p}); return s + \":\" + tool_result();",
                p = js(&big)
            )),
            None,
            "main",
            &[],
        )
        .expect("a refusal is a normal result");
    assert_eq!(
        string_of(&out),
        "1:tool: result exceeds the slot's max_bridge_result_bytes"
    );
}

// =========================================================================
// process
// =========================================================================

#[test]
fn process_configured_child_probe() {
    if std::env::var_os("AGENTERM_QJS_CONTAINED_PROBE").is_none() {
        return;
    }
    let mut stdin = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin).expect("read probe stdin");
    println!(
        "configured:{stdin}:{}:{}",
        std::env::var_os("AGENTERM_QJS_REMOVED").is_none(),
        std::env::current_dir()
            .expect("probe current directory")
            .display()
    );
    eprint!("configured-stderr");
}

#[test]
fn every_qjs_child_entry_uses_contained_launch_with_configured_stdio() {
    let scratch = Scratch::new("contained-command");
    let command_stderr = scratch.path("command.stderr");
    let spawn_stdout = scratch.path("spawn.stdout");
    let executable = std::env::current_exe().expect("resolve tool-door test executable");
    let base = format!(
        r#"{{
            program: {program},
            args: ["--exact", "process_configured_child_probe", "--nocapture"],
            current_dir: {cwd},
            env: {{ AGENTERM_QJS_CONTAINED_PROBE: "1", AGENTERM_QJS_REMOVED: "present" }},
            env_remove: ["AGENTERM_QJS_REMOVED"],
            stdin_text: "from-stdin",
            timeout_ms: 10000
        }}"#,
        program = js(&executable),
        cwd = js(&scratch.0),
    );
    let source = format!(
        r#"
        const commandSpec = {base};
        commandSpec.stderr_path = {command_stderr};
        if (process_command(JSON.stringify(commandSpec)) !== 0) {{ return "command:" + tool_result(); }}
        const command = JSON.parse(tool_result());

        const spawnSpec = {base};
        spawnSpec.stdout_path = {spawn_stdout};
        const handle = process_spawn(JSON.stringify(spawnSpec));
        if (handle < 0) {{ return "spawn:" + tool_result(); }}
        if (process_wait(handle, 10000) !== 0) {{ return "wait:" + tool_result(); }}
        const spawned = JSON.parse(tool_result());

        const statusSpec = {base};
        const status = process_status(JSON.stringify(statusSpec));
        return "" + command.success + "|" + command.stderr + "|"
            + (command.stdout.indexOf("configured:from-stdin:true:") >= 0) + "|"
            + spawned.success + "|" + spawned.stdout + "|"
            + (spawned.stderr.indexOf("configured-stderr") >= 0) + "|" + status;
        "#,
        command_stderr = js(&command_stderr),
        spawn_stdout = js(&spawn_stdout),
    );
    let out = run_tool(&source);
    assert_eq!(string_of(&out), "true||true|true||true|0", "{out:?}");
    assert_eq!(
        std::fs::read_to_string(command_stderr).expect("command stderr redirect"),
        "configured-stderr"
    );
    assert!(
        std::fs::read_to_string(spawn_stdout)
            .expect("spawn stdout redirect")
            .contains("configured:from-stdin:true:"),
        "spawn stdout redirect omitted the configured child output"
    );
}

/// `process.status` does not read the child's output, and it used to spawn
/// with pipes and drop them at once: a child that printed anything died of
/// SIGPIPE and the door answered `-1`. Wave-1 measured it on
/// `agenterm-cc --help` (command: exit 0, status: -1). Null pipes now: a
/// chatty child exits with its own status.
/// `fs.metadata` carries a modification time. rh's `target-report` needs
/// oldest/newest write and an age; without this field it was the one
/// wave-1 script the door could not carry.
#[test]
fn fs_metadata_reports_a_modification_time() {
    let dir = std::env::temp_dir().join(format!("agenterm-mtime-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("touched.txt");
    std::fs::write(&file, "x").expect("fixture");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64;
    let source = format!(
        r#"
        if (fs_metadata("{}") !== 0) {{ return "metadata: " + tool_result(); }}
        let m = JSON.parse(tool_result());
        return "" + m.modified_ms;
        "#,
        file.display()
    );
    let out = run_tool(&source);
    let got: u64 = string_of(&out)
        .parse()
        .unwrap_or_else(|_| panic!("not a number: {out:?}"));
    assert!(
        got.abs_diff(now_ms) < 60_000,
        "modified_ms {got} should be within a minute of now {now_ms}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A handle answers its child's OS pid before and after the wait, and a
/// second wait replays the first answer. rh's `child.id` backed ~40 identity
/// checks in the smoke scripts, and rh let a script `wait_with_output` a child
/// it had already reaped; wave 2 met "handle N was already waited" in every
/// group that waited its own server.
#[cfg(unix)]
#[test]
fn process_pid_is_stable_across_the_wait_and_a_second_wait_replays() {
    let out = run_tool(
        r#"
        let h = process_spawn(JSON.stringify({ program: "sh", args: ["-c", "echo hi; exit 4"] }));
        let pid = process_pid(h);
        let a = process_wait(h, 5000); let first = tool_result();
        let b = process_wait(h, 5000); let second = tool_result();
        return "" + (pid > 0) + "|" + (process_pid(h) === pid) + "|" + a + "|" + b + "|"
            + (first === second) + "|" + JSON.parse(first).exit_code + "|" + JSON.parse(second).stdout;
        "#,
    );
    assert_eq!(string_of(&out), "true|true|0|0|true|4|hi\n", "{out:?}");
}

#[cfg(unix)]
#[test]
fn process_spawn_refuses_before_the_thirty_third_native_child() {
    let scratch = Scratch::new("child-handle-limit");
    let marker = scratch.path("forbidden-spawn");
    let out = run_tool(&format!(
        r#"
        const handles = [];
        const spec = JSON.stringify({{ program: "sh", args: ["-c", "exit 0"] }});
        for (let i = 0; i < 32; i = i + 1) {{
            const h = process_spawn(spec);
            if (h < 0) {{ return "early:" + i + ":" + tool_result(); }}
            handles.push(h);
        }}
        const refused = process_spawn(JSON.stringify({{
            program: "/usr/bin/touch", args: [{marker}]
        }}));
        const refusal = tool_result();
        let cleaned = 0;
        for (const h of handles) {{
            if (process_wait(h, 5000) === 0) {{ cleaned = cleaned + 1; }}
        }}
        return "" + refused + "|" + refusal + "|" + cleaned + "|" + fs_exists({marker});
        "#,
        marker = js(&marker),
    ));
    assert_eq!(
        string_of(&out),
        "-1|process.spawn: child handle limit 32 reached|32|0",
        "{out:?}"
    );
    assert_eq!(
        out.tool_calls
            .iter()
            .filter(|call| call.as_str() == "tool.process.spawn")
            .count(),
        33
    );
}

#[test]
fn process_list_and_tree_contain_the_tool_host_identity() {
    let out = run_tool(
        r#"
        const me = process_id();
        if (process_list() !== 0) { throw tool_result(); }
        const processes = JSON.parse(tool_result());
        let found = false;
        for (const process of processes) {
            if (process.id === me && process.parent_id >= 0 && process.executable_name !== "") {
                found = true;
            }
        }
        if (process_tree(me) !== 0) { throw tool_result(); }
        const tree = JSON.parse(tool_result());
        let tree_found = false;
        for (const process of tree) {
            if (process.id === me && process.executable_name !== "") { tree_found = true; }
        }
        return "" + found + "|" + tree_found;
        "#,
    );
    assert_eq!(string_of(&out), "true|true", "{out:?}");
    assert!(
        out.tool_calls
            .iter()
            .any(|call| call == "tool.process.list"),
        "{:?}",
        out.tool_calls
    );
    assert!(
        out.tool_calls
            .iter()
            .any(|call| call == "tool.process.tree"),
        "{:?}",
        out.tool_calls
    );
}

#[test]
fn process_kill_pid_refuses_a_negative_id_before_touching_the_host() {
    let out = run_tool(
        r#"
        let status = process_kill_pid(-1);
        return status + ":" + tool_result();
        "#,
    );
    assert!(
        string_of(&out).contains("process.kill_pid: pid is negative"),
        "{out:?}"
    );
    assert_eq!(out.tool_calls, ["tool.process.kill_pid"]);
}

/// A running child's output can be read as it arrives -- rh's
/// `child.stdout.read(4096, 2s)` minus the blocking -- and the wait still
/// answers the whole capture afterwards. Drains run from spawn, so a chatty
/// server never blocks on a pipe nobody reads.
#[cfg(unix)]
#[test]
fn process_read_hands_out_output_as_it_arrives_and_wait_still_has_all_of_it() {
    let out = run_tool(
        r#"
        let h = process_spawn(JSON.stringify({ program: "sh", args: ["-c", "echo a; sleep 0.4; echo b"] }));
        let first = "";
        for (let i = 0; i < 40; i = i + 1) {
            if (process_read(h, 4096) !== 0) { return "read: " + tool_result(); }
            let r = JSON.parse(tool_result());
            first = first + r.stdout;
            if (first.length > 0) { break; }
            time_sleep_ms(25);
        }
        if (process_wait(h, 5000) !== 0) { return "wait: " + tool_result(); }
        let w = JSON.parse(tool_result());
        return first + "|" + w.stdout + "|" + w.exit_code;
        "#,
    );
    assert_eq!(string_of(&out), "a\n|a\nb\n|0", "{out:?}");
}

/// Negative robustness limits are refusals, never spellings for an unbounded
/// wait or capture. Refusal must leave the owned child handle usable so a
/// corrected caller can still clean it up.
#[cfg(unix)]
#[test]
fn process_wait_and_read_reject_negative_limits_without_consuming_the_child() {
    let out = run_tool(
        r#"
        let h = process_spawn(JSON.stringify({ program: "sh", args: ["-c", "sleep 0.05; echo done"] }));
        let wait_status = process_wait(h, -1);
        let wait_error = tool_result();
        let read_status = process_read(h, -1);
        let read_error = tool_result();
        if (process_wait(h, 5000) !== 0) { return "cleanup: " + tool_result(); }
        let result = JSON.parse(tool_result());
        return "" + wait_status + "|" + wait_error + "|" + read_status + "|"
            + read_error + "|" + result.exit_code + "|" + result.stdout;
        "#,
    );
    assert_eq!(
        string_of(&out),
        "1|process.wait: timeout_ms is negative|1|process.read: max_bytes is negative|0|done\n",
        "{out:?}"
    );
}

/// A spawn spec's `timeout_ms` is a deadline: past it the child is killed,
/// `state` says exited, and the wait reports `timed_out`. rh gave `.start()`
/// children a 15/22 s cap; the door used to ignore the field.
#[cfg(unix)]
#[test]
fn a_spawned_child_past_its_timeout_is_killed_and_the_wait_says_so() {
    let out = run_tool(
        r#"
        let h = process_spawn(JSON.stringify({ program: "sleep", args: ["30"], timeout_ms: 100 }));
        time_sleep_ms(250);
        if (process_state(h) !== 0) { return "state: " + tool_result(); }
        let state = tool_result();
        if (process_wait(h, 5000) !== 0) { return "wait: " + tool_result(); }
        let w = JSON.parse(tool_result());
        return state + "|" + w.timed_out + "|" + w.exit_code;
        "#,
    );
    assert_eq!(string_of(&out), "exited|true|null", "{out:?}");
}

/// `stdout_path` / `stderr_path` send a stream to a file instead of the
/// bounded capture: a 6 MB cargo log was a thrown refusal at the 1 MiB
/// bridge cap (wave 3). rh had `stdout_file`/`stderr_file`.
/// An advisory exclusive lock: the second taker is told -1 while the first
/// holds it, and gets it after the release. prune-target-incremental's
/// `.cargo-lock` pre-flight and its hold-while-removing protocol need this.
#[cfg(unix)]
#[test]
fn fs_try_lock_exclusive_refuses_a_second_taker_until_unlock() {
    let dir = std::env::temp_dir().join(format!("agenterm-lock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("held.lock");
    let source = format!(
        r#"
        let a = fs_try_lock_exclusive("{0}");
        let b = fs_try_lock_exclusive("{0}");
        if (fs_unlock(a) !== 0) {{ return "unlock: " + tool_result(); }}
        // flock is tied to the open file description, and a child forked by
        // a neighbouring test inherits our descriptor for the instant before
        // its exec closes it (O_CLOEXEC); so the release can lag a fork by a
        // few milliseconds. The door is a *try*: poll, as a script would.
        let c = -1;
        for (let i = 0; i < 40 && c < 0; i = i + 1) {{
            c = fs_try_lock_exclusive("{0}");
            if (c < 0) {{ time_sleep_ms(25); }}
        }}
        return (a >= 0) + "|" + b + "|" + (c >= 0);
        "#,
        file.display()
    );
    let out = run_tool(&source);
    assert_eq!(string_of(&out), "true|-1|true", "{out:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn direct_operation_diagnostics_obey_the_result_cap() {
    let mut engine = Engine::with_tool_door(Budget {
        max_bridge_result_bytes: 8,
        ..Budget::default()
    });
    let out = engine
        .run_once(
            Guest::Qjs("let status = fs_unlock(-1); return status + ':' + tool_result();"),
            None,
            "main",
            &[],
        )
        .expect("a capped direct-operation refusal is a normal result");
    assert_eq!(
        string_of(&out),
        "-1:tool: result exceeds the slot's max_bridge_result_bytes"
    );
}

#[test]
fn fs_try_lock_exclusive_refuses_before_creating_the_thirty_third_file() {
    let scratch = Scratch::new("lock-handle-limit");
    let stem = js(&scratch.path("held-"));
    let forbidden = scratch.path("held-32.lock");
    let forbidden_js = js(&forbidden);
    let out = run_tool(&format!(
        r#"
        const handles = [];
        for (let i = 0; i < 32; i = i + 1) {{
            const h = fs_try_lock_exclusive({stem} + i + ".lock");
            if (h < 0) {{ return "early:" + i + ":" + tool_result(); }}
            handles.push(h);
        }}
        const refused = fs_try_lock_exclusive({forbidden_js});
        const refusal = tool_result();
        const created = fs_exists({forbidden_js});
        let unlocked = 0;
        for (const h of handles) {{
            if (fs_unlock(h) === 0) {{ unlocked = unlocked + 1; }}
        }}
        return "" + refused + "|" + refusal + "|" + created + "|" + unlocked;
        "#
    ));
    assert_eq!(
        string_of(&out),
        "-1|fs.try_lock_exclusive: lock handle limit 32 reached|0|32",
        "{out:?}"
    );
    assert!(
        !forbidden.exists(),
        "the refused lock must not create its path"
    );
}

#[test]
fn unlocked_lock_handles_remain_tombstones_across_engine_calls() {
    let scratch = Scratch::new("lock-handle-tombstones");
    let path = js(&scratch.path("reused.lock"));
    let source = format!(
        r#"
        let h = -1;
        for (let attempt = 0; attempt < 40 && h < 0; attempt = attempt + 1) {{
            h = fs_try_lock_exclusive({path});
            if (h < 0) {{
                const error = tool_result();
                if (error !== "") {{ return "lock:" + error; }}
                time_sleep_ms(25);
            }}
        }}
        if (h < 0) {{ return "lock:contention"; }}
        const unlocked = fs_unlock(h);
        return "" + h + "|" + unlocked;
        "#
    );
    let wasm = compile_qjs_tool(&source).expect("compiles");
    let mut engine = Engine::with_tool_door(Budget::default());
    let slot = engine
        .spawn(Guest::CompiledQjs(&wasm), None)
        .expect("loads");

    for expected_handle in 0..32 {
        let out = engine.call(slot, "main", &[]).expect("lock then unlock");
        assert_eq!(
            string_of(&out),
            format!("{expected_handle}|0"),
            "call {expected_handle}"
        );
    }

    let refused = engine.call(slot, "main", &[]).expect("typed refusal");
    assert_eq!(
        string_of(&refused),
        "lock:fs.try_lock_exclusive: lock handle limit 32 reached"
    );
}

/// `fs.append` writes at the end without reading the file back: the harness
/// journal was read whole, concatenated and rewritten per record.
#[test]
fn fs_append_adds_to_the_end_and_creates_the_file() {
    let dir = std::env::temp_dir().join(format!("agenterm-append-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("journal.jsonl");
    let source = format!(
        r#"
        if (fs_append("{0}", "one\n") !== 0) {{ return "append: " + tool_result(); }}
        if (fs_append("{0}", "two\n") !== 0) {{ return "append: " + tool_result(); }}
        if (fs_read_to_string("{0}") !== 0) {{ return "read: " + tool_result(); }}
        return tool_result();
        "#,
        file.display()
    );
    let out = run_tool(&source);
    assert_eq!(string_of(&out), "one\ntwo\n", "{out:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `process.command_stdout` parks the child's stdout itself: no envelope to
/// parse for the common case. A failure still answers the envelope, so the
/// exit code and stderr are not lost.
#[cfg(unix)]
#[test]
fn process_command_stdout_parks_the_text_and_a_failure_keeps_the_envelope() {
    let out = run_tool(
        r#"
        let ok = JSON.stringify({ program: "sh", args: ["-c", "printf hello"], timeout_ms: 10000 });
        let a = process_command_stdout(ok); let text = tool_result();
        let bad = JSON.stringify({ program: "sh", args: ["-c", "printf oops 1>&2; exit 3"], timeout_ms: 10000 });
        let b = process_command_stdout(bad); let env = JSON.parse(tool_result());
        return a + "|" + text + "|" + b + "|" + env.exit_code + "|" + env.stderr;
        "#,
    );
    assert_eq!(string_of(&out), "0|hello|1|3|oops", "{out:?}");
}

#[cfg(unix)]
#[test]
fn process_command_can_send_its_streams_to_files() {
    let dir = std::env::temp_dir().join(format!("agenterm-redirect-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let out_path = dir.join("out.txt");
    let err_path = dir.join("err.txt");
    let source = format!(
        r#"
        let spec = JSON.stringify({{
            program: "sh",
            args: ["-c", "printf out; printf err 1>&2; exit 0"],
            stdout_path: "{}",
            stderr_path: "{}",
            timeout_ms: 10000
        }});
        if (process_command(spec) !== 0) {{ return "command: " + tool_result(); }}
        let r = JSON.parse(tool_result());
        return r.exit_code + "|" + r.stdout + "|" + r.stderr;
        "#,
        out_path.display(),
        err_path.display()
    );
    let out = run_tool(&source);
    assert_eq!(string_of(&out), "0||", "{out:?}");
    assert_eq!(
        std::fs::read_to_string(&out_path).expect("stdout file"),
        "out"
    );
    assert_eq!(
        std::fs::read_to_string(&err_path).expect("stderr file"),
        "err"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn process_status_of_a_chatty_child_is_its_own_exit_code() {
    let out = run_tool(
        r#"
        let spec = JSON.stringify({
            program: "sh",
            args: ["-c", "yes | head -c 300000; exit 3"],
            timeout_ms: 10000
        });
        return "" + process_status(spec);
        "#,
    );
    assert_eq!(string_of(&out), "3", "{out:?}");
}

#[cfg(unix)]
#[test]
fn process_command_captures_stdout_stderr_and_the_exit_code() {
    let out = run_tool(
        r#"
        let spec = JSON.stringify({
            program: "sh",
            args: ["-c", "printf out; printf err 1>&2; exit 3"],
            timeout_ms: 10000
        });
        if (process_command(spec) !== 0) { return "command: " + tool_result(); }
        let r = JSON.parse(tool_result());
        return r.stdout + "|" + r.stderr + "|" + r.exit_code + "|" + r.success + "|" + r.timed_out;
        "#,
    );
    assert_eq!(string_of(&out), "out|err|3|false|false");
    assert_eq!(out.tool_calls, ["tool.process.command"]);
}

/// Captured child output is allowed to be partial only when the result says
/// which stream lost bytes. The raw-stdout convenience cannot carry those
/// flags, so it refuses the same partial answer instead of returning a prefix.
#[cfg(unix)]
#[test]
fn process_capture_limit_is_explicit_for_envelopes_and_raw_stdout() {
    let budget = Budget {
        max_bridge_result_bytes: 512,
        ..Budget::default()
    };
    let out = Engine::with_tool_door(budget)
        .run_once(
            Guest::Qjs(
                r#"
                let spec = JSON.stringify({
                    program: "sh",
                    args: ["-c", "yes x | head -c 4096"],
                    timeout_ms: 10000
                });
                if (process_command(spec) !== 0) { return "command:" + tool_result(); }
                let first = JSON.parse(tool_result());
                let raw_status = process_command_stdout(spec);
                let second = JSON.parse(tool_result());
                return "" + first.success + "|" + first.stdout_truncated + "|"
                    + first.stderr_truncated + "|" + (first.stdout.length < 4096) + "|"
                    + raw_status + "|" + second.stdout_truncated;
                "#,
            ),
            None,
            "main",
            &[],
        )
        .expect("bounded process capture");
    assert_eq!(string_of(&out), "true|true|false|true|1|true");
}

#[cfg(unix)]
#[test]
fn process_read_and_wait_report_a_drained_stream_that_hit_its_cap() {
    let budget = Budget {
        max_bridge_result_bytes: 512,
        ..Budget::default()
    };
    let out = Engine::with_tool_door(budget)
        .run_once(
            Guest::Qjs(
                r#"
                let h = process_spawn(JSON.stringify({
                    program: "sh",
                    args: ["-c", "yes x | head -c 4096"],
                    timeout_ms: 10000
                }));
                time_sleep_ms(50);
                if (process_read(h, 64) !== 0) { return "read:" + tool_result(); }
                let read = JSON.parse(tool_result());
                if (process_wait(h, 5000) !== 0) { return "wait:" + tool_result(); }
                let waited = JSON.parse(tool_result());
                return "" + read.stdout_truncated + "|" + read.stderr_truncated + "|"
                    + (read.stdout.length <= 64) + "|" + waited.stdout_truncated;
                "#,
            ),
            None,
            "main",
            &[],
        )
        .expect("bounded incremental process capture");
    assert_eq!(string_of(&out), "true|false|true|true");
}

#[cfg(unix)]
#[test]
fn process_command_honours_current_dir_env_and_stdin() {
    let dir = Scratch::new("cwd");
    let out = run_tool(&format!(
        r#"
        let spec = JSON.stringify({{
            program: "sh",
            args: ["-c", "cat; printf ' '; pwd; printf \"$DOOR_VAR\""],
            current_dir: {d},
            env: {{ DOOR_VAR: "from-env" }},
            stdin_text: "from-stdin",
            timeout_ms: 10000
        }});
        if (process_command(spec) !== 0) {{ return "command: " + tool_result(); }}
        return JSON.parse(tool_result()).stdout;
        "#,
        d = js(&dir.0)
    ));
    let got = string_of(&out);
    assert!(got.starts_with("from-stdin "), "{got}");
    assert!(got.ends_with("from-env"), "{got}");
    let cwd = dir.0.canonicalize().unwrap();
    assert!(
        got.contains(&*cwd.to_string_lossy()),
        "cwd {} in {got}",
        cwd.display()
    );
}

#[cfg(windows)]
#[test]
fn process_command_captures_stdout_and_the_exit_code() {
    let out = run_tool(
        r#"
        let spec = JSON.stringify({ program: "cmd", args: ["/c", "echo out& exit 3"], timeout_ms: 10000 });
        if (process_command(spec) !== 0) { return "command: " + tool_result(); }
        let r = JSON.parse(tool_result());
        return r.exit_code + "|" + r.success;
        "#,
    );
    assert_eq!(string_of(&out), "3|false");
}

/// A program that does not exist is status 1 with a readable diagnostic; a
/// misspelled spec field is refused rather than ignored.
#[test]
fn process_command_refuses_a_bad_spec_and_names_a_missing_program() {
    let out = run_tool(
        r#"
        let a = process_command("{\"program\":\"x\",\"timeout\":1}");
        let first = a + ":" + tool_result();
        let b = process_command(JSON.stringify({ program: "agenterm-no-such-program-zz", timeout_ms: 1000 }));
        return first + "\n" + b + ":" + tool_result();
        "#,
    );
    let got = string_of(&out);
    let (first, second) = got.split_once('\n').unwrap();
    assert!(
        first.starts_with("1:process.command: the spec is not valid"),
        "{first}"
    );
    assert!(first.contains("timeout"), "names the field: {first}");
    assert!(
        second.starts_with("1:process.command: spawning `agenterm-no-such-program-zz`"),
        "{second}"
    );
    assert_eq!(
        out.tool_calls,
        ["tool.process.command", "tool.process.command"]
    );
}

#[test]
fn process_id_is_this_process() {
    let out = run_tool("return process_id();");
    assert_eq!(number_of(&out), f64::from(std::process::id()));
    assert_eq!(out.tool_calls, ["tool.process.id"]);
}

// =========================================================================
// env
// =========================================================================

#[test]
fn env_get_has_and_cwd() {
    // `PATH` is set in every environment a test runs in; the other name is
    // not set anywhere.
    let out = run_tool(
        r#"
        let a = env_has("PATH");
        let b = env_has("AGENTERM_QJSWASM_TOOL_DOOR_SURELY_UNSET");
        let status = env_get("PATH");
        let path = tool_result();
        let unset = env_get("AGENTERM_QJSWASM_TOOL_DOOR_SURELY_UNSET");
        let diag = tool_result();
        let c = env_cwd();
        let cwd = tool_result();
        return a + "|" + b + "|" + status + "|" + path + "|" + unset + "|" + diag + "|" + c + "|" + cwd;
        "#,
    );
    let got = string_of(&out);
    let parts: Vec<&str> = got.split('|').collect();
    assert_eq!(parts[0], "1");
    assert_eq!(parts[1], "0");
    assert_eq!(parts[2], "0");
    assert_eq!(parts[3], std::env::var("PATH").unwrap());
    assert_eq!(parts[4], "1");
    assert_eq!(
        parts[5],
        "env.get: `AGENTERM_QJSWASM_TOOL_DOOR_SURELY_UNSET` is not set"
    );
    assert_eq!(parts[6], "0");
    assert_eq!(parts[7], std::env::current_dir().unwrap().to_string_lossy());
    assert_eq!(
        out.tool_calls,
        [
            "tool.env.has",
            "tool.env.has",
            "tool.env.get",
            "tool.env.get",
            "tool.env.cwd"
        ]
    );
}

// =========================================================================
// The two doors together
// =========================================================================

/// A tool engine still has the fleet door: `print` and `fleet_call` work as
/// they do in a sandbox, and their pending buffer is not the tool door's.
#[test]
fn a_tool_script_still_has_the_fleet_door_and_the_two_buffers_are_separate() {
    let dir = Scratch::new("both");
    let file = dir.path("f.txt");
    std::fs::write(&file, "from-file").unwrap();
    let bridge: agenterm_qjswasm::FleetBridgeFn =
        std::sync::Arc::new(|op: &str, _params: &str| Ok(format!("fleet:{op}")));
    let out = Engine::with_tool_door(Budget::default())
        .run_once(
            Guest::Qjs(&format!(
                r#"
                print("hi");
                fs_read_to_string({p});
                fleet_call("tabs.list", "{{}}");
                return tool_result() + "|" + fleet_result();
                "#,
                p = js(&file)
            )),
            Some(bridge),
            "main",
            &[],
        )
        .expect("runs");
    assert_eq!(string_of(&out), "from-file|fleet:tabs.list");
    assert_eq!(out.stdout, "hi\n");
    assert_eq!(out.tool_calls, ["tool.fs.read_to_string"]);
}

// =========================================================================
// Opt-in: the sandbox never has it
// =========================================================================

/// `compile_qjs` does not know the tool door exists: a tool name is an
/// undeclared name, refused with the capability diagnostic, and the list of
/// what *is* offered does not mention it.
#[test]
fn the_sandbox_compiler_refuses_a_tool_name() {
    let err = compile_qjs("return fs_exists(\"/\");").expect_err("not a sandbox name");
    assert!(err.message.starts_with("this engine "), "{err}");
    assert!(err.message.contains("fs_exists"), "{err}");
    assert!(
        !err.message.contains("tool_result"),
        "the sandbox does not advertise the door: {err}"
    );
    compile_qjs_tool("return fs_exists(\"/\");").expect("the tool compiler accepts it");
}

/// Tool bytes in a sandbox engine are refused at load -- naming the import and
/// the constructor that would have opened the door -- and never run.
#[test]
fn a_sandbox_engine_refuses_tool_bytes_at_load_by_name() {
    let wasm = compile_qjs_tool("return fs_exists(\"/\");").expect("compiles");
    let err = Engine::new()
        .spawn(Guest::CompiledQjs(&wasm), None)
        .expect_err("a sandbox slot has no tool door");
    match &err {
        QjswasmError::Door(message) => {
            assert!(message.contains("tool.fs.exists"), "{message}");
            assert!(message.contains("with_tool_door"), "{message}");
        }
        other => panic!("expected a Door refusal, got {other:?}"),
    }
    // The sandbox check agrees with the sandbox engine, and the tool check
    // agrees with the tool engine.
    validate_wasm(&wasm).expect_err("the sandbox gate refuses it too");
    validate_wasm_tool_with(&wasm, &Budget::default()).expect("the tool gate accepts it");
    Engine::with_tool_door(Budget::default())
        .spawn(Guest::CompiledQjs(&wasm), None)
        .expect("the tool engine binds it");
}

/// `Guest::Qjs` in a sandbox engine is compiled with the sandbox compiler, so
/// a tool name is a compile refusal there -- and the same source runs in a
/// tool engine.
#[test]
fn a_sandbox_engine_compiles_source_without_the_tool_door() {
    let source = "return fs_exists(\"/\");";
    match Engine::new().run_once(Guest::Qjs(source), None, "main", &[]) {
        Err(QjswasmError::Compile(e)) => assert!(e.message.contains("fs_exists"), "{e}"),
        other => panic!("expected a compile refusal, got {other:?}"),
    }
    let out = run_tool(source);
    assert_eq!(number_of(&out), 1.0, "`/` exists");
}

#[test]
fn has_tool_door_says_which_engine_this_is() {
    assert!(!Engine::new().has_tool_door());
    assert!(!Engine::with_budget(Budget::default()).has_tool_door());
    assert!(Engine::with_tool_door(Budget::default()).has_tool_door());
}

// =========================================================================
// Zero cost when unmentioned
// =========================================================================

/// The declaration costs nothing until a script mentions it: a script that
/// names no tool function compiles to byte-identical wasm through both entry
/// points, and imports nothing.
#[test]
fn a_script_that_mentions_no_tool_name_compiles_byte_identical_through_both_entry_points() {
    for source in [
        "return 1;",
        "print(\"x\"); return 0;",
        "print(\"x\"); let s = fleet_call(\"o\", \"p\"); return fleet_result();",
        "let n = 0; for (let i = 0; i < 10; i = i + 1) { n = n + i; } return n;",
    ] {
        let sandbox = compile_qjs(source).unwrap();
        let tool = compile_qjs_tool(source).unwrap();
        assert_eq!(
            sandbox, tool,
            "{source}: the tool declarations changed the bytes"
        );
    }
    assert_eq!(
        imports(&compile_qjs_tool("return 1;").unwrap()),
        Vec::<String>::new()
    );
}

/// Only the tool functions a script mentions become imports, after the fleet
/// door's, and `tool_result` brings its length pass with it.
#[test]
fn only_the_tool_functions_a_script_mentions_are_imported() {
    assert_eq!(
        imports(&compile_qjs_tool("return fs_exists(\"/\");").unwrap()),
        vec!["tool.fs.exists(i32, i32) -> i32"]
    );
    assert_eq!(
        imports(
            &compile_qjs_tool("print(\"x\"); fs_write(\"a\", \"b\"); return tool_result();")
                .unwrap()
        ),
        vec![
            "agenterm.print(i32, i32) -> ()",
            "tool.fs.write(i32, i32, i32, i32) -> i32",
            "tool.result_len() -> i32",
            "tool.result(i32, i32) -> i32",
        ]
    );
    // The length pass is not a name a script may write.
    compile_qjs_tool("return result_len();").expect_err("not script-visible");
}

/// A sandbox slot's receipt line is empty: there is no door to reach.
#[test]
fn a_sandbox_outcome_has_no_tool_calls() {
    let out = Engine::new()
        .run_once(Guest::Qjs("print(\"x\"); return 1;"), None, "main", &[])
        .unwrap();
    assert!(out.tool_calls.is_empty());
}

// =========================================================================
// A minimal wasm import decoder (the same one `tests/qjs_door.rs` carries)
// =========================================================================

fn imports(wasm: &[u8]) -> Vec<String> {
    let mut at = 8;
    let mut types: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut out = Vec::new();
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let size = uleb(wasm, &mut at) as usize;
        let end = at + size;
        match id {
            1 => {
                let n = uleb(wasm, &mut at);
                for _ in 0..n {
                    assert_eq!(wasm[at], 0x60);
                    at += 1;
                    let params = valtypes(wasm, &mut at);
                    let results = valtypes(wasm, &mut at);
                    types.push((params, results));
                }
            }
            2 => {
                let n = uleb(wasm, &mut at);
                for _ in 0..n {
                    let module = name(wasm, &mut at);
                    let field = name(wasm, &mut at);
                    let kind = wasm[at];
                    at += 1;
                    assert_eq!(kind, 0);
                    let (params, results) = &types[uleb(wasm, &mut at) as usize];
                    out.push(format!(
                        "{module}.{field}({}) -> {}",
                        render(params),
                        if results.is_empty() {
                            "()".to_string()
                        } else {
                            render(results)
                        }
                    ));
                }
            }
            _ => {}
        }
        at = end;
    }
    out
}

fn render(types: &[u8]) -> String {
    types
        .iter()
        .map(|t| match t {
            0x7F => "i32",
            0x7E => "i64",
            0x7D => "f32",
            0x7C => "f64",
            _ => "?",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn valtypes(wasm: &[u8], at: &mut usize) -> Vec<u8> {
    let n = uleb(wasm, at);
    let types = wasm[*at..*at + n as usize].to_vec();
    *at += n as usize;
    types
}

fn name(wasm: &[u8], at: &mut usize) -> String {
    let n = uleb(wasm, at) as usize;
    let s = String::from_utf8(wasm[*at..*at + n].to_vec()).unwrap();
    *at += n;
    s
}

fn uleb(wasm: &[u8], at: &mut usize) -> u64 {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = wasm[*at];
        *at += 1;
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

/// `crypto.sha256_file` answers lower hex, 64 chars, of the file's bytes --
/// the fingerprint the build-identity library moved from rh needs for
/// `Cargo.lock` and the artifact manifest.
#[test]
fn sha256_file_fingerprints_the_bytes_on_disk() {
    let dir = std::env::temp_dir().join(format!("qjswasm-sha-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("x.bin");
    std::fs::write(&file, b"abc").expect("fixture");
    let path = file.to_string_lossy().replace('\\', "\\\\");
    let source = format!(
        "if (crypto_sha256_file(\"{path}\") !== 0) {{ return \"err:\" + tool_result(); }} return tool_result();"
    );
    let mut engine = agenterm_qjswasm::Engine::with_tool_door(agenterm_qjswasm::Budget::default());
    let wasm = agenterm_qjswasm::compile_qjs_tool(&source).expect("compiles");
    let out = engine
        .run_once(
            agenterm_qjswasm::Guest::CompiledQjs(&wasm),
            None,
            "main",
            &[],
        )
        .expect("runs");
    let got = match out.values.first() {
        Some(agenterm_qjswasm::Value::Js(agenterm_qjswasm::JsValue::Str(s))) => s.clone(),
        other => panic!("expected a string, got {other:?}"),
    };
    // sha256("abc"), the vector every implementation is checked against.
    assert_eq!(
        got,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A long-lived child: spawn, observe it running, kill it, wait, and read
/// what it wrote -- the shape 29 of the 71 rh scripts have, and the one
/// `process.command` cannot express.
#[test]
fn a_spawned_child_can_be_watched_killed_and_waited() {
    let source = r#"
        const spec = JSON.stringify({ program: "sh", args: ["-c", "echo started; sleep 30"] });
        const h = process_spawn(spec);
        if (h < 0) { return "spawn:" + tool_result(); }
        time_sleep_ms(200);
        if (process_state(h) !== 0) { return "state:" + tool_result(); }
        const before = tool_result();
        process_kill(h);
        if (process_wait(h, 5000) !== 0) { return "wait:" + tool_result(); }
        const out = JSON.parse(tool_result());
        return before + "|" + out.stdout.trim() + "|" + out.success;
    "#;
    let mut engine = agenterm_qjswasm::Engine::with_tool_door(agenterm_qjswasm::Budget::default());
    let wasm = agenterm_qjswasm::compile_qjs_tool(source).expect("compiles");
    let out = engine
        .run_once(
            agenterm_qjswasm::Guest::CompiledQjs(&wasm),
            None,
            "main",
            &[],
        )
        .expect("runs");
    let got = match out.values.first() {
        Some(agenterm_qjswasm::Value::Js(agenterm_qjswasm::JsValue::Str(s))) => s.clone(),
        other => panic!("expected a string, got {other:?}"),
    };
    assert_eq!(got, "running|started|false");
}

/// A child the script never waited for does not outlive the slot.
///
/// `run_once` drops its slot before returning, so the reap has already
/// happened by the time control is back here -- which is the property.
///
/// The child is found by its own command line, not by `pgrep -P <us>`: under
/// the workspace run every test in this binary shares one parent PID, and
/// counting children of it counts *other tests'* processes too. The first
/// cut of this test did that and failed only under the parallel run, which
/// read as a reap bug and was a test-isolation bug.
#[test]
fn an_unwaited_child_is_killed_with_the_slot() {
    let marker = format!("30.{}", std::process::id() % 1000 + 100);
    let source = format!(
        r#"
        const spec = JSON.stringify({{ program: "sleep", args: ["{marker}"] }});
        const h = process_spawn(spec);
        if (h < 0) {{ return "spawn:" + tool_result(); }}
        time_sleep_ms(100);
        if (process_state(h) !== 0) {{ return "state:" + tool_result(); }}
        return tool_result();
    "#
    );
    let mut engine = agenterm_qjswasm::Engine::with_tool_door(agenterm_qjswasm::Budget::default());
    let wasm = agenterm_qjswasm::compile_qjs_tool(&source).expect("compiles");
    let out = engine
        .run_once(
            agenterm_qjswasm::Guest::CompiledQjs(&wasm),
            None,
            "main",
            &[],
        )
        .expect("runs");
    let got = match out.values.first() {
        Some(agenterm_qjswasm::Value::Js(agenterm_qjswasm::JsValue::Str(s))) => s.clone(),
        other => panic!("expected a string, got {other:?}"),
    };
    assert_eq!(got, "running", "the child was alive while the script ran");
    drop(engine);
    std::thread::sleep(std::time::Duration::from_millis(200));
    let survivors = std::process::Command::new("pgrep")
        .args(["-f", &format!("^sleep {marker}$")])
        .output()
        .expect("pgrep");
    let survivors = String::from_utf8_lossy(&survivors.stdout);
    assert!(
        survivors.trim().is_empty(),
        "the unwaited child must be reaped with the slot; still running: {}",
        survivors.trim()
    );
}

/// The owned unit is the process tree, not only the shell returned by spawn.
/// Dropping a slot must not leave a background grandchild running after its
/// direct child is killed.
#[cfg(unix)]
#[test]
fn an_unwaited_child_tree_is_killed_with_the_slot() {
    let dir = Scratch::new("child-tree");
    let pid_file = dir.path("grandchild.pid");
    let source = format!(
        r#"
        const h = process_spawn(JSON.stringify({{
            program: "sh",
            args: ["-c", "sleep 30 & echo $! > \"$1\"; wait", "sh", {pid_file}],
            timeout_ms: 30000
        }}));
        if (h < 0) {{ return "spawn:" + tool_result(); }}
        for (let i = 0; i < 100 && fs_exists({pid_file}) !== 1; i = i + 1) {{
            time_sleep_ms(10);
        }}
        return "" + fs_exists({pid_file});
        "#,
        pid_file = js(&pid_file)
    );
    let mut engine = Engine::with_tool_door(Budget::default());
    let wasm = compile_qjs_tool(&source).expect("compiles");
    let out = engine
        .run_once(Guest::CompiledQjs(&wasm), None, "main", &[])
        .expect("runs");
    assert_eq!(string_of(&out), "1", "grandchild identity was published");
    let grandchild: u32 = std::fs::read_to_string(&pid_file)
        .expect("grandchild pid")
        .trim()
        .parse()
        .expect("numeric grandchild pid");
    let is_alive = || {
        std::process::Command::new("kill")
            .args(["-0", &grandchild.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    // The PID file is written by the grandchild's owning shell before the
    // script returns. `run_once` reclaims its slot (and tree) before handing
    // this Outcome back, so absence here is the desired postcondition.
    drop(engine);
    for _ in 0..100 {
        if !is_alive() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("owned grandchild {grandchild} survived slot cleanup");
}

/// `env_remove` strips an inherited variable, which is not what setting it
/// to "" does: a child that tests `env_has` sees the difference.
#[test]
fn a_command_spec_can_remove_an_inherited_variable() {
    // SAFETY: test-local; the door's child inherits it and nothing else reads it.
    unsafe { std::env::set_var("QJSWASM_DOOR_PROBE", "present") };
    let source = r#"
        const keep = JSON.parse(tool_result_after(process_command(JSON.stringify({ program: "sh", args: ["-c", "echo ${QJSWASM_DOOR_PROBE:-unset}"] }))));
        const gone = JSON.parse(tool_result_after(process_command(JSON.stringify({ program: "sh", args: ["-c", "echo ${QJSWASM_DOOR_PROBE:-unset}"], env_remove: ["QJSWASM_DOOR_PROBE"] }))));
        return keep.stdout.trim() + "|" + gone.stdout.trim();
        function tool_result_after(status) { if (status !== 0) { throw tool_result(); } return tool_result(); }
    "#;
    let mut engine = agenterm_qjswasm::Engine::with_tool_door(agenterm_qjswasm::Budget::default());
    let wasm = agenterm_qjswasm::compile_qjs_tool(source).expect("compiles");
    let out = engine
        .run_once(
            agenterm_qjswasm::Guest::CompiledQjs(&wasm),
            None,
            "main",
            &[],
        )
        .expect("runs");
    let got = match out.values.first() {
        Some(agenterm_qjswasm::Value::Js(agenterm_qjswasm::JsValue::Str(s))) => s.clone(),
        other => panic!("expected a string, got {other:?}"),
    };
    assert_eq!(got, "present|unset");
}

// =========================================================================
// process.platform_facts / process.window_*: the platform crate's
// process-window contract on a spawned handle
// =========================================================================

/// The host's own view of a child's top-level window, as rh's
/// `child.platform_facts` gave it. A `sleep` has no window: the answer says
/// so with the same six fields the GUI journeys read, and nothing traps.
#[cfg(unix)]
#[test]
fn process_platform_facts_answers_the_hosts_view_of_a_windowless_child() {
    let out = run_tool(
        r#"
        let h = process_spawn(JSON.stringify({ program: "sleep", args: ["5"] }));
        let status = process_platform_facts(h);
        let facts = JSON.parse(tool_result());
        process_kill(h); process_wait(h, 5000);
        return "" + status + "|" + (typeof facts.top_level_window_supported) + "|" + facts.top_level_window_present
            + "|" + facts.top_level_window_id + "|" + (typeof facts.foreground_window_id) + "|" + facts.top_level_window_is_foreground
            + "|" + (typeof facts.top_level_window_title);
        "#,
    );
    assert_eq!(
        string_of(&out),
        "0|boolean|false|0|number|false|string",
        "{out:?}"
    );
    assert!(
        out.tool_calls
            .iter()
            .any(|c| c == "tool.process.platform_facts"),
        "{:?}",
        out.tool_calls
    );
}

/// A key the contract does not name is refused by name before any window is
/// looked up; a real key on a windowless child is the contract's typed
/// refusal, prefixed with the op so a script can tell the two apart. The
/// status is `direct`'s -1 for both; `answer`-shaped ops (control) say 1.
#[cfg(unix)]
#[test]
fn process_window_key_refuses_unknown_keys_and_windowless_children_by_name() {
    let out = run_tool(
        r#"
        let h = process_spawn(JSON.stringify({ program: "sleep", args: ["5"] }));
        let a = process_window_key(h, "Meta"); let first = tool_result();
        let b = process_window_key(h, "Escape"); let second = tool_result();
        let c = process_window_key(99, "Escape"); let third = tool_result();
        process_kill(h); process_wait(h, 5000);
        return a + "|" + first + "||" + b + "|" + second + "||" + c + "|" + third;
        "#,
    );
    let text = string_of(&out);
    let parts: Vec<&str> = text.split("||").collect();
    assert_eq!(parts.len(), 3, "{text}");
    assert!(
        parts[0]
            .starts_with("-1|process.window_key: no key named `Meta` (process_window_key_invalid)"),
        "{text}"
    );
    assert!(
        parts[1].starts_with("-1|process.window_key: ")
            && parts[1].contains("process_window_not_found"),
        "{text}"
    );
    assert!(
        parts[2].starts_with("-1|process.window_key: no child with handle 99"),
        "{text}"
    );
}

/// The pointer and control ops parse their JSON spec and refuse an unknown
/// action or op by name; a resize on a windowless child is the typed refusal.
#[cfg(unix)]
#[test]
fn process_window_pointer_control_and_resize_refuse_by_name() {
    let out = run_tool(
        r#"
        let h = process_spawn(JSON.stringify({ program: "sleep", args: ["5"] }));
        let a = process_window_pointer(h, JSON.stringify({ action: "tap", x: 1, y: 2 })); let first = tool_result();
        let b = process_window_control(h, JSON.stringify({ id: 2105, op: "hover" })); let second = tool_result();
        let c = process_window_resize(h, JSON.stringify({ width: 640, height: 480 })); let third = tool_result();
        let d = process_window_rect(h, 1); let fourth = tool_result();
        process_kill(h); process_wait(h, 5000);
        return a + "|" + first + "||" + b + "|" + second + "||" + c + "|" + third + "||" + d + "|" + fourth;
        "#,
    );
    let text = string_of(&out);
    let parts: Vec<&str> = text.split("||").collect();
    assert_eq!(parts.len(), 4, "{text}");
    assert!(
        parts[0].starts_with("-1|process.window_pointer: no action named `tap`"),
        "{text}"
    );
    assert!(
        parts[1].starts_with("1|process.window_control: no op named `hover`"),
        "{text}"
    );
    assert!(parts[2].starts_with("-1|process.window_resize: "), "{text}");
    assert!(parts[3].starts_with("1|process.window_rect: "), "{text}");
}

/// `image.inspect_png(path)`: dimensions, pixel count and mean luma of an
/// evidence screenshot, which is what the GUI journeys assert on. A 4x2 RGB
/// image, half white and half black, is 8 samples at luma 127.5; a file
/// that is not a PNG is a refusal that names the path.
#[test]
fn image_inspect_png_answers_dimensions_samples_and_mean_luma() {
    let scratch = Scratch::new("image-inspect-png");
    let path = scratch.0.join("half.png");
    {
        let file = std::fs::File::create(&path).expect("create");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 4, 2);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("header");
        let mut data = vec![255u8; 4 * 3];
        data.extend(vec![0u8; 4 * 3]);
        writer.write_image_data(&data).expect("pixels");
    }
    let not_png = scratch.0.join("not.png");
    std::fs::write(&not_png, b"hello").expect("write");
    let out = run_tool(&format!(
        r#"
        let a = image_inspect_png({p}); let facts = JSON.parse(tool_result());
        let b = image_inspect_png({q}); let refusal = tool_result();
        return "" + a + "|" + facts.width + "x" + facts.height + "|" + facts.samples + "|" + facts.luminance + "|" + b + "|" + refusal;
        "#,
        p = js(&path),
        q = js(&not_png)
    ));
    let text = string_of(&out);
    assert!(
        text.starts_with("0|4x2|8|127.5|1|image.inspect_png: `"),
        "{text}"
    );
    assert!(
        text.contains("not.png`: not a PNG this engine can read"),
        "{text}"
    );
}

// ---- the host-side bill (PRD_02_36 A1.12) ---------------------------------

/// Every `tool.*` operation is one host operation on the receipt, and the
/// cap ends the call as a budget, not a trap: the same class as running out
/// of steps, because it is the same kind of refusal.
#[test]
fn host_operations_are_counted_and_capped() {
    let out = run_tool(
        "let n = 0; let i = 0; while (i < 5) { n = n + time_now_ms(); i = i + 1; } return i;",
    );
    assert_eq!(out.host_ops, 5, "{out:?}");
    assert_eq!(out.tool_calls.len(), 5);
    assert!(out.steps > 0);

    let budget = Budget {
        max_host_ops: 3,
        ..Budget::default()
    };
    let err = Engine::with_tool_door(budget)
        .run_once(
            Guest::Qjs("let i = 0; while (i < 5) { time_now_ms(); i = i + 1; } return i;"),
            None,
            "main",
            &[],
        )
        .expect_err("the fourth operation is past the cap");
    assert!(
        matches!(err, QjswasmError::Budget("max_host_ops")),
        "got {err:?}"
    );
}

/// `host_bytes` is both directions through the door: the string arguments
/// a script sends, charged where the operation is, and the answer it parks,
/// charged when parked. Collecting an answer moves bytes already paid for,
/// and an operation that parks nothing does not re-bill whatever the
/// previous one left parked.
#[test]
fn bytes_through_the_door_are_billed_in_both_directions() {
    let scratch = Scratch::new("bill");
    let path = scratch.path("h.txt");
    std::fs::write(&path, b"hello").expect("write");
    let path_len = path.to_string_lossy().len() as u64;

    let sent_only = run_tool(&format!("return fs_exists({p});", p = js(&path)));
    assert_eq!(sent_only.host_ops, 1, "{sent_only:?}");
    assert_eq!(sent_only.host_bytes, path_len, "{sent_only:?}");

    let both = run_tool(&format!(
        "fs_read_to_string({p}); let t = tool_result(); fs_exists({p}); return t;",
        p = js(&path)
    ));
    assert_eq!(string_of(&both), "hello");
    // read, exists: collecting the answer is not an operation (it was two
    // until 2026-08-30, which doubled every journey's count).
    assert_eq!(both.host_ops, 2, "{both:?}");
    assert_eq!(both.host_bytes, 2 * path_len + 5, "{both:?}");

    let written = run_tool(&format!(
        "return fs_write({p}, \"abcdefghij\");",
        p = js(&path)
    ));
    assert_eq!(written.host_bytes, path_len + 10, "{written:?}");

    // A refusal is parked like an answer and billed like one.
    let missing = scratch.path("missing.txt");
    let refused = run_tool(&format!(
        "fs_remove_file({p}); return tool_result();",
        p = js(&missing)
    ));
    let diagnostic = string_of(&refused);
    assert!(diagnostic.contains("fs.remove_file"), "{diagnostic}");
    assert_eq!(
        refused.host_bytes,
        missing.to_string_lossy().len() as u64 + diagnostic.len() as u64,
        "{refused:?}"
    );
}

/// A secret-looking environment name is refused in the tool profile unless
/// the budget's `env_allow` lists it; either way the receipt carries the
/// name and never the value (PRD_02_36 A1.16).
#[test]
fn secret_looking_environment_names_are_refused_unless_allowed() {
    let name = "AGENTERM_QJSWASM_TEST_SECRET_TOKEN";
    // SAFETY: a process-global write, to a name no other test reads.
    unsafe { std::env::set_var(name, "hunter2") };
    let refused = run_tool(&format!(
        "let h = env_has(\"{name}\"); let g = env_get(\"{name}\"); return h + \"|\" + g + \"|\" + tool_result();"
    ));
    let text = string_of(&refused);
    assert!(
        text.starts_with("-1|1|env.get: `AGENTERM_QJSWASM_TEST_SECRET_TOKEN` denied (secret)"),
        "{text}"
    );
    assert!(!text.contains("hunter2"), "{text}");
    assert_eq!(
        refused.tool_calls,
        [
            format!("tool.env.has({name}) denied:secret"),
            format!("tool.env.get({name}) denied:secret"),
        ]
    );
    // Two operations, both refused before the host was asked; the refusal
    // sentence is parked and billed like any diagnostic.
    assert_eq!(refused.host_ops, 2, "{refused:?}");

    let budget = Budget {
        env_allow: vec![name.to_ascii_lowercase()],
        ..Budget::default()
    };
    let allowed = Engine::with_tool_door(budget)
        .run_once(
            Guest::Qjs(&format!(
                "let h = env_has(\"{name}\"); env_get(\"{name}\"); return h + \"|\" + tool_result();"
            )),
            None,
            "main",
            &[],
        )
        .expect("an allowed read answers");
    assert_eq!(string_of(&allowed), "1|hunter2");
    assert_eq!(
        allowed.tool_calls,
        [
            format!("tool.env.has({name})"),
            format!("tool.env.get({name})")
        ]
    );
    // An ordinary name is not named on the receipt.
    let plain = run_tool("env_has(\"PATH\"); return 1;");
    assert_eq!(plain.tool_calls, ["tool.env.has"]);
}

/// `Budget::fixed_clock_ms` is a clock a script can be replayed against:
/// `time.now_ms` answers the origin, moved only by the script's own
/// `sleep_ms` requests, so two runs read the same times and a poll-until
/// loop written around the clock still ends.
#[test]
fn a_fixed_clock_makes_time_replayable() {
    let budget = || Budget {
        fixed_clock_ms: Some(1_700_000_000_000),
        ..Budget::default()
    };
    let script = r#"
        time_now_ms(); let a = tool_result();
        time_sleep_ms(30);
        time_now_ms(); let b = tool_result();
        let start = Number(a); let n = 0;
        while (Number(b) - start < 100) { time_sleep_ms(10); n = n + 1; time_now_ms(); b = tool_result(); }
        return a + "|" + b + "|" + n;
    "#;
    let run = |budget: Budget| {
        let out = Engine::with_tool_door(budget)
            .run_once(Guest::Qjs(script), None, "main", &[])
            .expect("runs");
        (string_of(&out), out.waited_ms)
    };
    let (first, waited) = run(budget());
    assert_eq!(first, "1700000000000|1700000000100|7");
    assert!(waited >= 90, "the sleeps still happened: {waited}");
    let (second, _) = run(budget());
    assert_eq!(second, first, "a replay reads the same clock");
    // The wall clock does not answer the origin.
    let wall = Engine::with_tool_door(Budget::default())
        .run_once(
            Guest::Qjs("time_now_ms(); return tool_result();"),
            None,
            "main",
            &[],
        )
        .expect("runs");
    assert_ne!(string_of(&wall), "1700000000000");
}

/// Waiting is on the bill as wall-clock time, separately from steps: a
/// script that sleeps 30 ms reports at least that, and a script that only
/// computes reports none.
#[test]
fn waiting_is_billed_apart_from_computing() {
    let slept = run_tool("time_sleep_ms(30); return 1;");
    assert!(slept.waited_ms >= 25, "{slept:?}");
    assert_eq!(slept.host_ops, 1);

    let computed =
        run_tool("let s = 0; let i = 0; while (i < 1000) { s = s + i; i = i + 1; } return s;");
    assert_eq!(computed.waited_ms, 0, "{computed:?}");
    assert_eq!(computed.host_ops, 0);
    assert!(computed.steps > 1000);
    assert!(computed.heap_pages >= 1, "{computed:?}");
}

/// A call that fails keeps its bill on the engine, beside its stdout: a
/// failed wait is exactly the run whose bill matters.
#[test]
fn a_failed_call_keeps_its_bill() {
    let mut eng = Engine::with_tool_door(Budget::default());
    let err = eng
        .run_once(
            Guest::Qjs("time_sleep_ms(30); throw \"after the wait\";"),
            None,
            "main",
            &[],
        )
        .expect_err("the throw is uncaught");
    assert!(matches!(err, QjswasmError::UncaughtThrow(_)), "got {err:?}");
    let cost = eng
        .take_failed_cost()
        .expect("the run happened, so it cost");
    assert_eq!(cost.host_ops, 1, "{cost:?}");
    assert!(cost.waited_ms >= 25, "{cost:?}");
    assert!(cost.steps > 0, "{cost:?}");
    assert_eq!(eng.take_failed_cost(), None, "read once, like stdout");
}

/// A cancel set while the guest sleeps ends the call within a slice, as
/// `Cancelled` -- its own class, neither the script's doing nor a budget --
/// and the bill still says how long it actually waited.
#[test]
fn a_cancel_ends_a_sleep_within_a_slice() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let flag = Arc::new(AtomicBool::new(false));
    let budget = Budget {
        cancel: Some(Arc::clone(&flag)),
        ..Budget::default()
    };
    let setter = Arc::clone(&flag);
    let hand = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(60));
        setter.store(true, Ordering::Relaxed);
    });
    let started = std::time::Instant::now();
    let mut eng = Engine::with_tool_door(budget);
    let err = eng
        .run_once(
            Guest::Qjs("time_sleep_ms(5000); return 1;"),
            None,
            "main",
            &[],
        )
        .expect_err("the sleep is cut short");
    hand.join().expect("setter thread");
    assert!(matches!(err, QjswasmError::Cancelled), "got {err:?}");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
    let cost = eng.take_failed_cost().expect("it ran");
    assert!(cost.waited_ms >= 50 && cost.waited_ms < 1500, "{cost:?}");
}

/// A cancel already set ends the call at the first host operation, before
/// it runs.
#[test]
fn a_cancel_already_set_stops_at_the_first_operation() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    let budget = Budget {
        cancel: Some(Arc::new(AtomicBool::new(true))),
        ..Budget::default()
    };
    let err = Engine::with_tool_door(budget)
        .run_once(Guest::Qjs("time_now_ms(); return 1;"), None, "main", &[])
        .expect_err("cancelled before the first operation");
    assert!(matches!(err, QjswasmError::Cancelled), "got {err:?}");
}
