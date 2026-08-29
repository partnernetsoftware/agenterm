//! The engine-neutral framed worker route, driven as a subprocess.
//!
//! `__agenterm-internal-engine worker --framed-worker` is what every hosted
//! script invocation travels through (`worker_supervisor::SCRIPT_WORKER_ENGINE_ARGS`).
//! It was spelled `... rh --framed-worker` until the rh engine left the
//! repository on 2026-08-29, and `tests/rh_framed_worker.rs` covered it with
//! rh sources. What that file proved about the *protocol* -- a worker answers
//! each invoke frame with a result frame carrying the invocation's id -- is
//! proved here without any engine at all, because the frames below are
//! refused by name before any engine is asked. A build with no script engine
//! compiled in still has a worker, and it still answers.

use std::process::{Command, Stdio};

use agenterm::script_protocol::{
    SCRIPT_API_VERSION, SCRIPT_ENVELOPE_VERSION, SCRIPT_FRAME_VERSION, ScriptBudgets,
    ScriptExitClass, ScriptFrame, ScriptFramePayload, ScriptFrameRead, ScriptInvocation,
    ScriptOperation, ScriptProfile, ScriptResult, read_script_frame, write_script_frame,
};

fn invocation(id: &str, label: &str, source: &str) -> ScriptInvocation {
    ScriptInvocation {
        envelope_version: SCRIPT_ENVELOPE_VERSION,
        invocation_id: id.into(),
        api_version: SCRIPT_API_VERSION,
        operation: ScriptOperation::Run,
        profile: ScriptProfile::Local,
        source_label: label.into(),
        source: source.into(),
        project_root: Some(env!("CARGO_MANIFEST_DIR").into()),
        invocation_temp_root: None,
        arguments: Vec::new(),
        budgets: ScriptBudgets::default(),
        observation: None,
    }
}

/// Spawn the worker, send the frames, and collect one result per frame.
fn round_trip(frames: &[ScriptFrame], backend: Option<&str>) -> Vec<ScriptResult> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agenterm"));
    command
        .args(["__agenterm-internal-engine", "worker", "--framed-worker"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    match backend {
        Some(value) => command.env("AGENTERM_SCRIPT_BACKEND", value),
        None => command.env_remove("AGENTERM_SCRIPT_BACKEND"),
    };
    let mut child = command.spawn().expect("spawn framed worker");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        for frame in frames {
            write_script_frame(&mut stdin, frame).expect("write invoke frame");
        }
    }
    let mut stdout = child.stdout.take().expect("stdout");
    let mut results = Vec::new();
    loop {
        match read_script_frame(&mut stdout).expect("read frame") {
            ScriptFrameRead::Frame(frame) => {
                if let ScriptFramePayload::Result(result) = frame.payload {
                    results.push(result);
                }
            }
            ScriptFrameRead::Eof => break,
            ScriptFrameRead::Rejected(rejection) => panic!("frame rejected: {rejection:?}"),
        }
    }
    let status = child.wait().expect("worker exits");
    assert!(status.success(), "worker exit status {status}");
    results
}

fn invoke(frame_id: &str, invocation: ScriptInvocation) -> ScriptFrame {
    ScriptFrame {
        frame_version: SCRIPT_FRAME_VERSION,
        frame_id: frame_id.into(),
        payload: ScriptFramePayload::Invoke(invocation),
    }
}

/// An entry nothing routes is refused by name, in a result frame. This is
/// the default engine's absence made observable: until 2026-08-29 the same
/// frame ran on rh. One invocation per worker, because a framed worker
/// serves one invocation at a time and answers a second with
/// `protocol_worker_busy` -- the supervisor never sends two.
#[test]
fn framed_worker_refuses_an_unrouted_entry_by_name() {
    for (label, source) in [("task.rh", "fn entry() { 42 }"), ("stdin", "40 + 2")] {
        let results = round_trip(&[invoke("one", invocation("first", label, source))], None);
        assert_eq!(results.len(), 1, "one result per invoke frame");
        let result = &results[0];
        assert_eq!(result.invocation_id, "first");
        assert!(!result.ok);
        assert_eq!(
            result.exit_class,
            ScriptExitClass::Configuration,
            "{label}: {:?}",
            result.failure
        );
        let failure = result.failure.as_ref().expect("a typed failure");
        assert_eq!(failure.code, "script_backend_unavailable");
        assert!(
            failure.message.contains(&format!("`{label}`")),
            "{}",
            failure.message
        );
        assert!(
            failure.message.contains(".qjs is the script language now"),
            "{}",
            failure.message
        );
    }
}

/// The retired engine's name is answered with where it went, not with a
/// typo diagnostic and not with a run on something else.
#[test]
fn framed_worker_refuses_the_retired_engine_by_name() {
    for name in ["rh", "rhai"] {
        let results = round_trip(
            &[invoke("one", invocation("first", "t.qjs", "return 1;"))],
            Some(name),
        );
        assert_eq!(results.len(), 1);
        let failure = results[0].failure.as_ref().expect("a typed failure");
        assert_eq!(failure.code, "script_backend_unavailable", "{name}");
        assert!(
            failure.message.contains("partnernetsoftware/rh"),
            "{name}: {}",
            failure.message
        );
        assert!(
            !failure.message.contains("unknown"),
            "{name}: {}",
            failure.message
        );
    }
}

/// The route the supervisor spawns is the one this file drives: the two
/// spellings must not drift apart, or every hosted invocation would go
/// somewhere these tests do not look.
#[test]
fn the_worker_route_is_what_the_supervisor_spawns() {
    let output = Command::new(env!("CARGO_BIN_EXE_agenterm"))
        .args([
            "__agenterm-internal-engine",
            "worker",
            "definitely-not-a-mode",
        ])
        .output()
        .expect("run the worker entry with a bad mode");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--framed-worker"), "{stderr}");
}
