use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
};

use agenterm_cu::{
    Command, CuReply, RequestIdentity, TargetRef, runtime_coordinator::RuntimeCoordinator,
    worker_wire,
};

fn fixture_root() -> PathBuf {
    let random = agenterm_platform::entropy::secure_random_array::<8>().expect("fixture entropy");
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let root = std::env::temp_dir().join(format!(
        "agenterm-cu-worker-request-{}-{suffix}",
        std::process::id()
    ));
    std::fs::create_dir(&root).expect("fixture root");
    std::fs::canonicalize(root).expect("canonical fixture root")
}

fn run_worker(root: &Path, payload: &str) -> CuReply {
    let mut child = ProcessCommand::new(env!("CARGO_BIN_EXE_agenterm-cu"))
        .args(["exec", "--grant", "actuate", "--json", "-"])
        .env("AGENTERM_CU_RUNTIME_PATH", root.join("runtime.json"))
        .env("AGENTERM_CU_IDEMPOTENCY_PATH", root.join("requests.json"))
        .env("AGENTERM_CU_AUDIT_PATH", root.join("audit.jsonl"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn worker");
    child
        .stdin
        .take()
        .expect("worker stdin")
        .write_all(payload.as_bytes())
        .expect("write envelope");
    let output = child.wait_with_output().expect("worker output");
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "worker reply JSON: {error}; status={:?}; stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn effect_worker_admits_once_replays_and_binds_the_transport_scope() {
    let root = fixture_root();
    let runtime_path = root.join("runtime.json");
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    let session = RuntimeCoordinator::open_at(&runtime_path)
        .expect("runtime")
        .session_start(Some("worker fixture"), 60, now_s)
        .expect("session");
    let identity = RequestIdentity {
        request_id: "worker-request-1".to_owned(),
        session_id: session.session_id,
        session_lease: session.lease.clone(),
    };
    let command = Command::ClipboardClear {
        target: TargetRef::Current,
        apply: false,
    };
    let first_scope = worker_wire::effect_scope("vnc", &["fixture:5907"]);
    let payload = worker_wire::encode(&command, Some(&identity), Some(&first_scope))
        .expect("request envelope");

    let first = run_worker(&root, &payload);
    assert!(first.ok, "{first:?}");
    let audit_after_first = std::fs::read(root.join("audit.jsonl")).expect("audit");

    let replay = run_worker(&root, &payload);
    assert!(replay.ok, "{replay:?}");
    assert_eq!(replay.data.as_ref().unwrap()["effect"], "not_repeated");
    assert_eq!(
        std::fs::read(root.join("audit.jsonl")).expect("audit"),
        audit_after_first,
        "an exact replay must not dispatch or audit the effect again"
    );

    let changed_scope = worker_wire::effect_scope("vnc", &["fixture:5908"]);
    let changed = worker_wire::encode(&command, Some(&identity), Some(&changed_scope))
        .expect("changed envelope");
    let conflict = run_worker(&root, &changed);
    assert!(!conflict.ok);
    assert_eq!(conflict.error.as_ref().unwrap().code, "request_id_conflict");

    for state in ["runtime.json", "requests.json", "audit.jsonl"] {
        let bytes = std::fs::read(root.join(state)).expect("state file");
        assert!(
            !bytes
                .windows(session.lease.len())
                .any(|part| part == session.lease.as_bytes()),
            "lease plaintext reached {state}"
        );
    }
    std::fs::remove_dir_all(root).expect("cleanup fixture");
}

#[test]
fn effect_worker_rejects_oversized_stdin_with_its_typed_error() {
    let root = fixture_root();
    let reply = run_worker(
        &root,
        &"x".repeat(worker_wire::MAX_WORKER_REQUEST_BYTES + 1),
    );
    assert!(!reply.ok);
    assert_eq!(reply.command, "exec");
    assert_eq!(
        reply.error.as_ref().unwrap().code,
        "worker_request_too_large"
    );
    std::fs::remove_dir_all(root).expect("cleanup fixture");
}
