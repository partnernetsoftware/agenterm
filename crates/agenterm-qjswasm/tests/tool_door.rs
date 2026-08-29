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
        .run_once(agenterm_qjswasm::Guest::CompiledQjs(&wasm), None, "main", &[])
        .expect("runs");
    let got = match out.values.first() {
        Some(agenterm_qjswasm::Value::Js(agenterm_qjswasm::JsValue::Str(s))) => s.clone(),
        other => panic!("expected a string, got {other:?}"),
    };
    // sha256("abc"), the vector every implementation is checked against.
    assert_eq!(got, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
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
        .run_once(agenterm_qjswasm::Guest::CompiledQjs(&wasm), None, "main", &[])
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
        .run_once(agenterm_qjswasm::Guest::CompiledQjs(&wasm), None, "main", &[])
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
        .run_once(agenterm_qjswasm::Guest::CompiledQjs(&wasm), None, "main", &[])
        .expect("runs");
    let got = match out.values.first() {
        Some(agenterm_qjswasm::Value::Js(agenterm_qjswasm::JsValue::Str(s))) => s.clone(),
        other => panic!("expected a string, got {other:?}"),
    };
    assert_eq!(got, "present|unset");
}
