#![cfg(unix)]

use std::{
    fs,
    io::{ErrorKind, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// How long the mock API waits for the requests it expects. Generous enough
/// that a slow machine is not a failure, short enough that a helper which
/// never calls it fails instead of hanging the whole suite.
const MOCK_API_TIMEOUT: Duration = Duration::from_secs(15);

fn scratch_dir(test_name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must follow the Unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "agenterm-{test_name}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&path).expect("create test scratch directory");
    path
}

fn run_script(scratch: &Path, extra_env: &[(&str, &str)]) -> Output {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut command = Command::new("bash");
    command
        .arg(repo.join("scripts/cursor_agent_chat.sh"))
        .args([
            "--from",
            "bc-11111111",
            "--to",
            "bc-22222222",
            "--dry-run",
            "bounded test message",
        ])
        .env_remove("CURSOR_API")
        .env("TMPDIR", scratch);
    for (name, value) in extra_env {
        command.env(name, value);
    }
    command.output().expect("run cursor agent chat helper")
}

#[test]
fn dry_run_removes_tracked_payload_files() {
    let scratch = scratch_dir("cursor-chat-cleanup");
    let output = run_script(&scratch, &[]);
    assert!(
        output.status.success(),
        "dry run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_dir(&scratch)
            .expect("read scratch directory")
            .count(),
        0,
        "tracked payload files must be removed on exit"
    );
    fs::remove_dir(&scratch).expect("remove empty scratch directory");
}

#[test]
fn live_resolution_removes_subshell_response_files() {
    let scratch = scratch_dir("cursor-chat-responses");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock API");
    let address = listener.local_addr().expect("read mock API address");
    // Bounded, because `accept` has no timeout and the thread is joined
    // below: a script that makes fewer requests than expected would
    // otherwise hang this test -- and with it `cargo test --workspace` --
    // forever, which reads as "still running" rather than as the failure it
    // is. The count is returned so the assertion can say what happened.
    listener
        .set_nonblocking(true)
        .expect("set mock API listener non-blocking");
    let server = thread::spawn(move || {
        let body = concat!(
            r#"{"items":["#,
            r#"{"name":"from","id":"bc-11111111","status":"ACTIVE"},"#,
            r#"{"name":"to","id":"bc-22222222","status":"ACTIVE"}"#,
            "]}"
        );
        let deadline = Instant::now() + MOCK_API_TIMEOUT;
        let mut served = 0_usize;
        while served < 2 && Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(accepted) => accepted,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
                Err(error) => panic!("accept mock API request: {error}"),
            };
            stream
                .set_nonblocking(false)
                .expect("set mock API stream blocking");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).expect("read mock API request");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write mock API response");
            served += 1;
        }
        served
    });
    let api_base = format!("http://{address}/v1");
    let output = run_script(
        &scratch,
        &[
            ("CURSOR_API", "<TOKEN>"),
            ("CURSOR_AGENT_API_BASE", api_base.as_str()),
        ],
    );
    let served = server.join().expect("mock API server completed");
    assert_eq!(
        served, 2,
        "the helper made {served} request(s) to the mock API, expected 2"
    );
    assert!(
        output.status.success(),
        "dry run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_dir(&scratch)
            .expect("read scratch directory")
            .count(),
        0,
        "subshell response files must be removed on exit"
    );
    fs::remove_dir(&scratch).expect("remove empty scratch directory");
}

#[test]
fn zero_re_resolve_interval_is_rejected_before_modulo() {
    let scratch = scratch_dir("cursor-chat-interval");
    let output = run_script(&scratch, &[("CURSOR_AGENT_CHAT_RE_RESOLVE_EVERY", "0")]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("CURSOR_AGENT_CHAT_RE_RESOLVE_EVERY must be greater than zero")
    );
    fs::remove_dir(&scratch).expect("remove empty scratch directory");
}
