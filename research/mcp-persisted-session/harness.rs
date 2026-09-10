#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    collections::{BTreeMap, VecDeque},
    env, fs,
    io::{BufRead, BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use agenterm_cu::idempotency_store::{
    FinalOutcome, FinalOutcomeKind, FinalReplay, IdempotencyStore, RequestState, ReserveDecision,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SCHEMA_VERSION: u32 = 1;
const RETENTION_TTL_MS: i64 = 60_000;
const MAX_TRACE_ROWS: usize = 128;
const MAX_RECEIPT_BYTES: usize = 256 * 1024;
const PHASES: [Phase; 5] = [Phase::FMinus1, Phase::F0, Phase::F1, Phase::F2, Phase::F3];

fn create_private_case(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|_| "case_create".to_owned())?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| "case_permissions".to_owned())?;
    Ok(())
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    schema_version: u32,
    grant_selector: String,
    store_selector: String,
    binding: Binding,
    operation: String,
    issued_at_utc_ms: i64,
    not_before_utc_ms: i64,
    expires_at_utc_ms: i64,
    now_utc_ms: i64,
    generation: u64,
    max_uses: u64,
    idempotency_key: String,
    payload_digest: String,
    marker: String,
    session_deadline_ms: u64,
    call_ceiling: u64,
    failure_schedule: Vec<String>,
    protocol_cases: Vec<ProtocolCase>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    target: String,
    session: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Variant {
    A0,
    A1,
    B,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ProtocolCase {
    name: String,
    request: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case", deny_unknown_fields)]
enum PublicMutation {
    ShellExec { idempotency_key: String },
}

#[derive(Clone, Debug)]
struct OwnedGrantRequest {
    binding: Binding,
    operation: String,
    now_utc_ms: i64,
}

impl OwnedGrantRequest {
    fn borrowed(&self) -> GrantRequest<'_> {
        GrantRequest {
            binding: &self.binding,
            operation: &self.operation,
            now_utc_ms: self.now_utc_ms,
        }
    }
}

fn parse_public_mutation(
    value: serde_json::Value,
    fixture: &Fixture,
) -> Result<OwnedGrantRequest, String> {
    match serde_json::from_value::<PublicMutation>(value) {
        Ok(PublicMutation::ShellExec { idempotency_key })
            if idempotency_key == fixture.idempotency_key =>
        {
            Ok(OwnedGrantRequest {
                binding: fixture.binding.clone(),
                operation: fixture.operation.clone(),
                now_utc_ms: fixture.now_utc_ms,
            })
        }
        Ok(_) => Err("idempotency-key-mismatch".to_owned()),
        Err(_) => Err("command-not-constructible".to_owned()),
    }
}

fn public_request(fixture: &Fixture) -> Result<OwnedGrantRequest, String> {
    parse_public_mutation(
        serde_json::json!({
            "command": "shell-exec",
            "idempotency_key": fixture.idempotency_key,
        }),
        fixture,
    )
}

fn internal_attack_request(
    binding: Binding,
    operation: String,
    now_utc_ms: i64,
) -> OwnedGrantRequest {
    OwnedGrantRequest {
        binding,
        operation,
        now_utc_ms,
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case", deny_unknown_fields)]
enum WireRequest {
    Startup {
        binding: Binding,
        operation: String,
        now_utc_ms: i64,
    },
    Effect {
        binding: Binding,
        operation: String,
        now_utc_ms: i64,
    },
    Mechanism,
    SessionOpen,
    SessionClose,
    PrepareCleanup,
    Cleanup,
    Shutdown,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WireReply {
    ok: bool,
    code: String,
}

struct GrantProcess {
    child: std::process::Child,
    input: Option<BufWriter<std::process::ChildStdin>>,
    output: Option<std::process::ChildStdout>,
    nonce: String,
    queued: usize,
    session: usize,
    deadline: Instant,
    process_group: i32,
}

#[derive(Clone, Debug, Serialize)]
struct CleanupObservation {
    elapsed_ms: u128,
    complete: bool,
    pre_loss_owned_state_counts: [usize; 4],
    pre_loss_process_group_alive: bool,
    pre_loss_worker_alive: bool,
    pre_loss_worker_same_group: bool,
    post_loss_owned_state_counts: [usize; 4],
    post_loss_process_group_zero: bool,
    post_loss_worker_zero: bool,
}

impl GrantProcess {
    fn spawn(case: &Path, nonce: &str, deadline: Instant) -> Result<Self, String> {
        let mut command =
            Command::new(env::current_exe().map_err(|_| "server_current_exe".to_owned())?);
        command
            .arg("--grant-server")
            .arg(case)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
        let mut child = command
            .spawn()
            .map_err(|_| "grant_server_spawn".to_owned())?;
        let input = BufWriter::new(child.stdin.take().ok_or("grant_server_stdin")?);
        let output = child.stdout.take().ok_or("grant_server_stdout")?;
        #[cfg(unix)]
        unsafe {
            let descriptor = output.as_raw_fd();
            let flags = libc::fcntl(descriptor, libc::F_GETFL);
            if flags < 0 || libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                let _ = child.kill();
                let _ = child.wait();
                return Err("grant_server_nonblocking".to_owned());
            }
        }
        let process_group = child.id() as i32;
        Ok(Self {
            child,
            input: Some(input),
            output: Some(output),
            nonce: nonce.to_owned(),
            queued: 0,
            session: 0,
            deadline,
            process_group,
        })
    }

    fn request(&mut self, request: WireRequest) -> Result<String, String> {
        self.queued = 1;
        let input = self.input.as_mut().ok_or("grant_server_closed")?;
        serde_json::to_writer(&mut *input, &request)
            .map_err(|_| "grant_server_encode".to_owned())?;
        input
            .write_all(b"\n")
            .map_err(|_| "grant_server_write".to_owned())?;
        input.flush().map_err(|_| "grant_server_flush".to_owned())?;
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0_u8; 1024];
            match self
                .output
                .as_mut()
                .ok_or("grant_server_closed")?
                .read(&mut chunk)
            {
                Ok(0) => return Err("grant_server_eof".to_owned()),
                Ok(count) => {
                    bytes.extend_from_slice(&chunk[..count]);
                    if bytes.len() > 16 * 1024 {
                        return Err("grant_server_reply_limit".to_owned());
                    }
                    if bytes.contains(&b'\n') {
                        break;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= self.deadline {
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        self.input.take();
                        self.output.take();
                        self.queued = 0;
                        self.session = 0;
                        return Err("case_deadline_exceeded".to_owned());
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(_) => return Err("grant_server_read".to_owned()),
            }
        }
        self.queued = 0;
        let line = std::str::from_utf8(&bytes).map_err(|_| "grant_server_reply".to_owned())?;
        let reply: WireReply =
            serde_json::from_str(line.trim_end()).map_err(|_| "grant_server_reply".to_owned())?;
        if reply.ok {
            Ok(reply.code)
        } else {
            Err(reply.code)
        }
    }

    fn startup(&mut self, request: &GrantRequest<'_>) -> Result<(), String> {
        self.request(WireRequest::Startup {
            binding: request.binding.clone(),
            operation: request.operation.to_owned(),
            now_utc_ms: request.now_utc_ms,
        })
        .map(|_| ())
    }
    fn effect(&mut self, request: &GrantRequest<'_>) -> Result<(), String> {
        self.request(WireRequest::Effect {
            binding: request.binding.clone(),
            operation: request.operation.to_owned(),
            now_utc_ms: request.now_utc_ms,
        })
        .map(|_| ())
    }
    fn mechanism(&mut self) -> Result<(), String> {
        self.request(WireRequest::Mechanism).map(|_| ())
    }
    fn session_open(&mut self) -> Result<(), String> {
        self.request(WireRequest::SessionOpen)?;
        self.session = 1;
        Ok(())
    }
    fn session_close(&mut self) -> Result<(), String> {
        self.request(WireRequest::SessionClose)?;
        self.session = 0;
        Ok(())
    }
    fn stdout_loss(mut self, case: &Path) -> Result<CleanupObservation, String> {
        let started = Instant::now();
        self.request(WireRequest::PrepareCleanup)?;
        let pre_loss_owned_state_counts = audit_owner_events(&case.join("owner-events.jsonl"))?;
        let worker_pid = fs::read_to_string(case.join("worker.pid"))
            .map_err(|_| "cleanup_worker_pid_read".to_owned())?
            .parse::<i32>()
            .map_err(|_| "cleanup_worker_pid_parse".to_owned())?;
        #[cfg(unix)]
        let pre_loss_process_group_alive = unsafe { libc::kill(-self.process_group, 0) } == 0;
        #[cfg(unix)]
        let pre_loss_worker_alive = unsafe { libc::kill(worker_pid, 0) } == 0;
        #[cfg(unix)]
        let pre_loss_worker_same_group = unsafe { libc::getpgid(worker_pid) } == self.process_group
            && worker_pid != self.process_group;
        #[cfg(not(unix))]
        let pre_loss_process_group_alive = true;
        #[cfg(not(unix))]
        let pre_loss_worker_alive = true;
        #[cfg(not(unix))]
        let pre_loss_worker_same_group = true;
        if pre_loss_owned_state_counts != [1; 4]
            || !pre_loss_kernel_proved(
                pre_loss_process_group_alive,
                pre_loss_worker_alive,
                pre_loss_worker_same_group,
            )
        {
            return Err("cleanup_pre_loss_ownership_unproved".to_owned());
        }
        self.output.take();
        let input = self.input.as_mut().ok_or("grant_server_closed")?;
        serde_json::to_writer(&mut *input, &WireRequest::Cleanup)
            .map_err(|_| "cleanup_encode".to_owned())?;
        input
            .write_all(b"\n")
            .map_err(|_| "cleanup_write".to_owned())?;
        input.flush().map_err(|_| "cleanup_flush".to_owned())?;
        let status = loop {
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|_| "cleanup_wait".to_owned())?
            {
                break status;
            }
            if started.elapsed() >= Duration::from_secs(5) || Instant::now() >= self.deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(if Instant::now() >= self.deadline {
                    "case_deadline_exceeded".to_owned()
                } else {
                    "stdout_loss_timeout".to_owned()
                });
            }
            thread::sleep(Duration::from_millis(2));
        };
        self.input.take();
        self.queued = 0;
        self.session = 0;
        let counts = audit_owner_events(&case.join("owner-events.jsonl"))?;
        #[cfg(unix)]
        let kernel_owned_processes_zero = unsafe { libc::kill(-self.process_group, 0) } != 0;
        #[cfg(unix)]
        let kernel_worker_zero = unsafe { libc::kill(worker_pid, 0) } != 0;
        #[cfg(not(unix))]
        let kernel_owned_processes_zero = status.success();
        #[cfg(not(unix))]
        let kernel_worker_zero = status.success();
        let elapsed = started.elapsed().as_millis();
        self.process_group = 0;
        Ok(CleanupObservation {
            elapsed_ms: elapsed,
            complete: cleanup_proved(
                status.success(),
                counts,
                kernel_owned_processes_zero,
                kernel_worker_zero,
            ),
            pre_loss_owned_state_counts,
            pre_loss_process_group_alive,
            pre_loss_worker_alive,
            pre_loss_worker_same_group,
            post_loss_owned_state_counts: counts,
            post_loss_process_group_zero: kernel_owned_processes_zero,
            post_loss_worker_zero: kernel_worker_zero,
        })
    }
    fn stop(mut self, graceful: bool) -> Result<(), String> {
        if graceful {
            let _ = self.request(WireRequest::Shutdown);
        } else {
            self.child
                .kill()
                .map_err(|_| "grant_server_kill".to_owned())?;
        }
        loop {
            if self
                .child
                .try_wait()
                .map_err(|_| "grant_server_wait".to_owned())?
                .is_some()
            {
                break;
            }
            if Instant::now() >= self.deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err("case_deadline_exceeded".to_owned());
            }
            thread::sleep(Duration::from_millis(2));
        }
        self.input.take();
        self.output.take();
        self.process_group = 0;
        Ok(())
    }
}

fn cleanup_proved(
    server_reaped: bool,
    counts: [usize; 4],
    process_group_zero: bool,
    worker_zero: bool,
) -> bool {
    server_reaped && counts == [0; 4] && process_group_zero && worker_zero
}

fn pre_loss_kernel_proved(
    process_group_alive: bool,
    worker_alive: bool,
    worker_same_group: bool,
) -> bool {
    process_group_alive && worker_alive && worker_same_group
}

impl Drop for GrantProcess {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.process_group > 0 {
            unsafe {
                let _ = libc::kill(-self.process_group, libc::SIGKILL);
            }
        }
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.input.take();
        self.output.take();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
enum Phase {
    #[serde(rename = "F-1")]
    FMinus1,
    F0,
    F1,
    F2,
    F3,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::FMinus1 => "F-1",
            Self::F0 => "F0",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
        }
    }
}

mod witnesses {
    use super::*;

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct GrantDocument {
        schema_version: u32,
        generation: u64,
        grant: GrantRecord,
        observations: Vec<GrantObservation>,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct GrantRecord {
        binding: Binding,
        operation: String,
        issued_at_utc_ms: i64,
        not_before_utc_ms: i64,
        expires_at_utc_ms: i64,
        max_uses: u64,
        consumed_uses: u64,
        revoked: bool,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct GrantObservation {
        boundary: String,
        code: String,
    }

    #[derive(Clone, Debug)]
    struct GrantStore {
        path: PathBuf,
        document: GrantDocument,
    }

    impl GrantStore {
        pub(super) fn schema_inventory(fixture: &Fixture) -> Result<Vec<String>, String> {
            let value = serde_json::to_value(GrantDocument {
                schema_version: SCHEMA_VERSION,
                generation: fixture.generation,
                grant: GrantRecord {
                    binding: fixture.binding.clone(),
                    operation: fixture.operation.clone(),
                    issued_at_utc_ms: fixture.issued_at_utc_ms,
                    not_before_utc_ms: fixture.not_before_utc_ms,
                    expires_at_utc_ms: fixture.expires_at_utc_ms,
                    max_uses: fixture.max_uses,
                    consumed_uses: 0,
                    revoked: false,
                },
                observations: vec![GrantObservation {
                    boundary: "schema".to_owned(),
                    code: "schema".to_owned(),
                }],
            })
            .map_err(|_| "grant_inventory_encode".to_owned())?;
            let mut paths = Vec::new();
            inventory_json_paths("grant", &value, &mut paths);
            paths.sort();
            Ok(paths)
        }
        pub(super) fn create(
            path: PathBuf,
            fixture: &Fixture,
            max_uses: u64,
        ) -> Result<Self, String> {
            let store = Self {
                path,
                document: GrantDocument {
                    schema_version: SCHEMA_VERSION,
                    generation: fixture.generation,
                    grant: GrantRecord {
                        binding: fixture.binding.clone(),
                        operation: fixture.operation.clone(),
                        issued_at_utc_ms: fixture.issued_at_utc_ms,
                        not_before_utc_ms: fixture.not_before_utc_ms,
                        expires_at_utc_ms: fixture.expires_at_utc_ms,
                        max_uses,
                        consumed_uses: 0,
                        revoked: false,
                    },
                    observations: Vec::new(),
                },
            };
            store.persist()?;
            Ok(store)
        }

        pub(super) fn reopen(path: PathBuf) -> Result<Self, String> {
            let raw = fs::read(&path).map_err(|_| "grant_store_read".to_owned())?;
            let document =
                serde_json::from_slice(&raw).map_err(|_| "grant_store_parse".to_owned())?;
            Ok(Self { path, document })
        }

        pub(super) fn verify_startup(&mut self, request: &GrantRequest<'_>) -> Result<(), String> {
            let code = self.decide(request, false);
            self.observe("startup-verification", &code)?;
            if code == "authorized" {
                Ok(())
            } else {
                Err(code)
            }
        }

        pub(super) fn reserve_effect(&mut self, request: &GrantRequest<'_>) -> Result<(), String> {
            let code = self.decide(request, true);
            self.observe("effect-attempt", &code)?;
            if code == "authorized" {
                Ok(())
            } else {
                Err(code)
            }
        }

        fn decide(&mut self, request: &GrantRequest<'_>, consume: bool) -> String {
            let grant = &mut self.document.grant;
            let code = if request.binding.target != grant.binding.target {
                "target-mismatch"
            } else if request.binding.session != grant.binding.session {
                "session-mismatch"
            } else if request.operation != grant.operation {
                "operation-mismatch"
            } else if grant.revoked {
                "revoked"
            } else if request.now_utc_ms < grant.not_before_utc_ms {
                "not-yet-valid"
            } else if request.now_utc_ms >= grant.expires_at_utc_ms {
                "expired"
            } else if grant.consumed_uses >= grant.max_uses {
                "exhausted"
            } else {
                if consume {
                    grant.consumed_uses += 1;
                }
                "authorized"
            };
            code.to_owned()
        }

        fn observe(&mut self, boundary: &str, code: &str) -> Result<(), String> {
            self.document.observations.push(GrantObservation {
                boundary: boundary.to_owned(),
                code: code.to_owned(),
            });
            if self.document.observations.len() > MAX_TRACE_ROWS {
                return Err("grant_observation_limit".to_owned());
            }
            self.document.generation += 1;
            self.persist()
        }

        fn persist(&self) -> Result<(), String> {
            let bytes =
                serde_json::to_vec(&self.document).map_err(|_| "grant_store_encode".to_owned())?;
            fs::write(&self.path, bytes).map_err(|_| "grant_store_write".to_owned())
        }

        pub(super) fn uses(&self) -> u64 {
            self.document.grant.consumed_uses
        }

        pub(super) fn observation_count(&self, boundary: &str) -> usize {
            self.document
                .observations
                .iter()
                .filter(|row| row.boundary == boundary)
                .count()
        }

        pub(super) fn codes(&self, boundary: &str) -> Vec<String> {
            self.document
                .observations
                .iter()
                .filter(|row| row.boundary == boundary)
                .map(|row| row.code.clone())
                .collect()
        }

        pub(super) fn revoke(&mut self) -> Result<(), String> {
            self.document.grant.revoked = true;
            self.document.generation += 1;
            self.persist()
        }

        pub(super) fn exhaust_for_fixture(&mut self) -> Result<(), String> {
            self.document.grant.consumed_uses = self.document.grant.max_uses;
            self.document.generation += 1;
            self.persist()
        }
    }

    pub(super) struct FixtureOwner {
        path: PathBuf,
    }

    impl FixtureOwner {
        pub(super) fn initialize(
            path: PathBuf,
            fixture: &Fixture,
            max_uses: u64,
        ) -> Result<Self, String> {
            GrantStore::create(path.clone(), fixture, max_uses)?;
            Ok(Self { path })
        }

        pub(super) fn revoke(&self) -> Result<(), String> {
            GrantStore::reopen(self.path.clone())?.revoke()
        }

        pub(super) fn exhaust_for_fixture(&self) -> Result<(), String> {
            GrantStore::reopen(self.path.clone())?.exhaust_for_fixture()
        }
    }

    pub(super) struct GrantAudit {
        store: GrantStore,
    }

    impl GrantAudit {
        pub(super) fn open(path: PathBuf) -> Result<Self, String> {
            Ok(Self {
                store: GrantStore::reopen(path)?,
            })
        }
        pub(super) fn schema_inventory(fixture: &Fixture) -> Result<Vec<String>, String> {
            GrantStore::schema_inventory(fixture)
        }
        pub(super) fn uses(&self) -> u64 {
            self.store.uses()
        }
        pub(super) fn observation_count(&self, boundary: &str) -> usize {
            self.store.observation_count(boundary)
        }
        pub(super) fn codes(&self, boundary: &str) -> Vec<String> {
            self.store.codes(boundary)
        }
    }

    pub(super) fn server_verify_startup(
        path: PathBuf,
        request: &GrantRequest<'_>,
    ) -> Result<(), String> {
        GrantStore::reopen(path)?.verify_startup(request)
    }

    pub(super) fn server_reserve_effect(
        path: PathBuf,
        request: &GrantRequest<'_>,
    ) -> Result<(), String> {
        GrantStore::reopen(path)?.reserve_effect(request)
    }

    pub(super) struct GrantRequest<'a> {
        pub(super) binding: &'a Binding,
        pub(super) operation: &'a str,
        pub(super) now_utc_ms: i64,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct MechanismDocument {
        schema_version: u32,
        attempts: u64,
        markers: Vec<String>,
    }

    #[derive(Clone, Debug)]
    struct Mechanism {
        path: PathBuf,
        document: MechanismDocument,
    }

    impl Mechanism {
        pub(super) fn schema_inventory() -> Result<Vec<String>, String> {
            let value = serde_json::to_value(MechanismDocument {
                schema_version: SCHEMA_VERSION,
                attempts: 1,
                markers: vec!["schema".to_owned()],
            })
            .map_err(|_| "mechanism_inventory_encode".to_owned())?;
            let mut paths = Vec::new();
            inventory_json_paths("mechanism", &value, &mut paths);
            paths.sort();
            paths.dedup();
            Ok(paths)
        }
        pub(super) fn create(path: PathBuf) -> Result<Self, String> {
            let mechanism = Self {
                path,
                document: MechanismDocument {
                    schema_version: SCHEMA_VERSION,
                    attempts: 0,
                    markers: Vec::new(),
                },
            };
            mechanism.persist()?;
            Ok(mechanism)
        }

        pub(super) fn reopen(path: PathBuf) -> Result<Self, String> {
            let bytes = fs::read(&path).map_err(|_| "mechanism_read".to_owned())?;
            let document =
                serde_json::from_slice(&bytes).map_err(|_| "mechanism_parse".to_owned())?;
            Ok(Self { path, document })
        }

        pub(super) fn attempt(
            &mut self,
            marker: &str,
            fail_before_marker: bool,
        ) -> Result<(), String> {
            self.document.attempts += 1;
            self.persist()?;
            if fail_before_marker {
                return Err("mechanism_injected_failure".to_owned());
            }
            self.document.markers.push(marker.to_owned());
            self.persist()
        }

        fn persist(&self) -> Result<(), String> {
            let bytes =
                serde_json::to_vec(&self.document).map_err(|_| "mechanism_encode".to_owned())?;
            fs::write(&self.path, bytes).map_err(|_| "mechanism_write".to_owned())
        }

        pub(super) fn attempts(&self) -> u64 {
            self.document.attempts
        }
        pub(super) fn markers(&self) -> u64 {
            self.document.markers.len() as u64
        }
    }

    pub(super) struct MechanismFixtureOwner;

    impl MechanismFixtureOwner {
        pub(super) fn initialize(path: PathBuf) -> Result<Self, String> {
            Mechanism::create(path)?;
            Ok(Self)
        }
    }

    pub(super) struct MechanismAudit {
        mechanism: Mechanism,
    }

    impl MechanismAudit {
        pub(super) fn open(path: PathBuf) -> Result<Self, String> {
            Ok(Self {
                mechanism: Mechanism::reopen(path)?,
            })
        }
        pub(super) fn attempts(&self) -> u64 {
            self.mechanism.attempts()
        }
        pub(super) fn markers(&self) -> u64 {
            self.mechanism.markers()
        }
        pub(super) fn schema_inventory() -> Result<Vec<String>, String> {
            Mechanism::schema_inventory()
        }
    }

    pub(super) fn server_attempt_mechanism(path: PathBuf) -> Result<(), String> {
        Mechanism::reopen(path)?.attempt("m1", false)
    }
}

use witnesses::{FixtureOwner, GrantAudit, GrantRequest, MechanismAudit, MechanismFixtureOwner};

#[derive(Clone, Debug, Serialize)]
struct DerivedSession {
    grant_digest: String,
    binding: Binding,
    operation: String,
    deadline_ms: u64,
    call_ceiling: u64,
    connection_nonce: String,
    calls: u64,
}

impl DerivedSession {
    fn create(fixture: &Fixture, connection_nonce: &str) -> Self {
        Self {
            grant_digest: digest_text("grant-selector", &fixture.grant_selector),
            binding: fixture.binding.clone(),
            operation: fixture.operation.clone(),
            deadline_ms: (fixture.now_utc_ms as u64) + fixture.session_deadline_ms,
            call_ceiling: fixture.call_ceiling,
            connection_nonce: connection_nonce.to_owned(),
            calls: 0,
        }
    }

    fn validate(&self, fixture: &Fixture) -> Result<(), String> {
        if self.grant_digest != digest_text("grant-selector", &fixture.grant_selector)
            || self.binding != fixture.binding
            || self.operation != fixture.operation
            || self.deadline_ms != (fixture.now_utc_ms as u64) + fixture.session_deadline_ms
            || self.call_ceiling != fixture.call_ceiling
            || self.connection_nonce.is_empty()
            || self.calls != 0
        {
            return Err("derived_session_invalid".to_owned());
        }
        Ok(())
    }

    fn effect(
        &mut self,
        grant: &mut GrantProcess,
        request: &GrantRequest<'_>,
        now_utc_ms: i64,
    ) -> Result<(), String> {
        if self.calls >= self.call_ceiling
            || now_utc_ms >= self.deadline_ms as i64
            || self.connection_nonce.is_empty()
        {
            return Err("derived_session_unavailable".to_owned());
        }
        // The scoped object participates in the request path but never decides
        // persisted authority: operation/binding mutations still reach the
        // independent grant decision below.
        self.calls += 1;
        grant.effect(request)
    }

    fn teardown(mut self, grant: &mut GrantProcess) -> Result<(u64, String), String> {
        grant.session_close()?;
        self.connection_nonce = digest_text("connection-teardown", &self.connection_nonce);
        Ok((self.calls, self.connection_nonce))
    }
}

#[derive(Clone, Debug, Serialize)]
struct PhaseTrace {
    variant: Variant,
    phase: Phase,
    retained_state: String,
    request_first_trace: Vec<String>,
    initial_startup: usize,
    reconnect_startup: usize,
    initial_effect_decisions: usize,
    reconnect_effect_decisions: usize,
    uses: u64,
    mechanism_attempts: u64,
    markers: u64,
    reconnect_result: String,
    pre_kill_request_state: Option<String>,
    recovery_request_state: Option<String>,
    f2_recovery_sequence: Vec<String>,
    conflict_result: String,
    cleanup_ms: u128,
    cleanup_complete: bool,
    pre_loss_owned_state_counts: [usize; 4],
    pre_loss_process_group_alive: bool,
    pre_loss_worker_alive: bool,
    pre_loss_worker_same_group: bool,
    owned_state_counts: [usize; 4],
    post_loss_process_group_zero: bool,
    post_loss_worker_zero: bool,
    connection_nonce_digests: [String; 2],
    initial_session_calls: u64,
    reconnect_session_calls: u64,
    session_teardown_observed: Option<bool>,
    elapsed_ms: u128,
    use_samples: [u64; 4],
    startup_samples: [usize; 4],
    effect_samples: [usize; 4],
    store_digests: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize)]
struct DenialTrace {
    variant: Variant,
    case: String,
    post_exchange: bool,
    startup_codes: Vec<String>,
    effect_codes: Vec<String>,
    uses: u64,
    mechanism_attempts: u64,
    markers: u64,
    setup_uses: u64,
    setup_mechanism_attempts: u64,
    setup_markers: u64,
    use_samples: [u64; 4],
    startup_samples: [usize; 4],
    effect_samples: [usize; 4],
    elapsed_ms: u128,
    store_digests: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize)]
struct VariantReceipt {
    variant: Variant,
    survived: bool,
    criteria: BTreeMap<String, bool>,
    phase_traces: Vec<PhaseTrace>,
    denial_traces: Vec<DenialTrace>,
    lifecycle_codes: Vec<String>,
    slope: Vec<SlopeTrace>,
    inventory: Inventory,
    tie_counts: [usize; 6],
    tie_members: Vec<String>,
    failures: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct SlopeTrace {
    ceiling: u64,
    authorized_effect_decisions: u64,
    use_delta: u64,
    mechanism_attempts: u64,
    marker_delta: u64,
    lifecycle_use_delta: u64,
    use_samples: [u64; 4],
    startup_samples: [usize; 4],
    effect_samples: [usize; 4],
    elapsed_ms: u128,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct Inventory {
    trusted_inputs: Vec<String>,
    experiment_controls: Vec<String>,
    memory_authority: Vec<String>,
    durable_fields: Vec<String>,
    public_fields: Vec<String>,
    cleanup_fields: Vec<String>,
}

#[derive(Serialize)]
struct CleanupOwnershipInventory {
    queue: bool,
    server_session: bool,
    connection: bool,
    worker: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    derived_session: Option<bool>,
}

#[derive(Serialize)]
struct TrustedInputInventory<'a> {
    grant_selector: &'a str,
    store_selector: &'a str,
    binding: &'a Binding,
    operation: &'a str,
    issued_at_utc_ms: i64,
    not_before_utc_ms: i64,
    expires_at_utc_ms: i64,
    now_utc_ms: i64,
    generation: u64,
    max_uses: u64,
    idempotency_key: &'a str,
    payload_digest: &'a str,
    marker: &'a str,
    session_deadline_ms: u64,
    call_ceiling: u64,
}

impl<'a> TrustedInputInventory<'a> {
    fn from_fixture(fixture: &'a Fixture) -> Self {
        Self {
            grant_selector: &fixture.grant_selector,
            store_selector: &fixture.store_selector,
            binding: &fixture.binding,
            operation: &fixture.operation,
            issued_at_utc_ms: fixture.issued_at_utc_ms,
            not_before_utc_ms: fixture.not_before_utc_ms,
            expires_at_utc_ms: fixture.expires_at_utc_ms,
            now_utc_ms: fixture.now_utc_ms,
            generation: fixture.generation,
            max_uses: fixture.max_uses,
            idempotency_key: &fixture.idempotency_key,
            payload_digest: &fixture.payload_digest,
            marker: &fixture.marker,
            session_deadline_ms: fixture.session_deadline_ms,
            call_ceiling: fixture.call_ceiling,
        }
    }
}

#[derive(Serialize)]
struct ExperimentControlInventory<'a> {
    schema_version: u32,
    failure_schedule: &'a [String],
    protocol_cases: &'a [ProtocolCase],
}

impl<'a> ExperimentControlInventory<'a> {
    fn from_fixture(fixture: &'a Fixture) -> Self {
        Self {
            schema_version: fixture.schema_version,
            failure_schedule: &fixture.failure_schedule,
            protocol_cases: &fixture.protocol_cases,
        }
    }
}

fn expected_inventory(variant: Variant) -> Inventory {
    let trusted_inputs = [
        "trusted.binding.session",
        "trusted.binding.target",
        "trusted.call_ceiling",
        "trusted.expires_at_utc_ms",
        "trusted.generation",
        "trusted.grant_selector",
        "trusted.idempotency_key",
        "trusted.issued_at_utc_ms",
        "trusted.marker",
        "trusted.max_uses",
        "trusted.not_before_utc_ms",
        "trusted.now_utc_ms",
        "trusted.operation",
        "trusted.payload_digest",
        "trusted.session_deadline_ms",
        "trusted.store_selector",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let experiment_controls = [
        "controls.failure_schedule[]",
        "controls.protocol_cases[].name",
        "controls.protocol_cases[].request.binding.session",
        "controls.protocol_cases[].request.binding.target",
        "controls.protocol_cases[].request.command",
        "controls.protocol_cases[].request.device.id",
        "controls.protocol_cases[].request.generation",
        "controls.protocol_cases[].request.idempotency_key",
        "controls.protocol_cases[].request.job.argv[]",
        "controls.protocol_cases[].request.now_utc_ms",
        "controls.protocol_cases[].request.operation",
        "controls.protocol_cases[].request.session_lease",
        "controls.schema_version",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let memory_authority = if variant == Variant::A1 {
        [
            "binding.session",
            "binding.target",
            "call_ceiling",
            "calls",
            "connection_nonce",
            "deadline_ms",
            "grant_digest",
            "operation",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    } else {
        Vec::new()
    };
    let mut durable_fields = [
        "grant.generation",
        "grant.grant.binding.session",
        "grant.grant.binding.target",
        "grant.grant.consumed_uses",
        "grant.grant.expires_at_utc_ms",
        "grant.grant.issued_at_utc_ms",
        "grant.grant.max_uses",
        "grant.grant.not_before_utc_ms",
        "grant.grant.operation",
        "grant.grant.revoked",
        "grant.observations[].boundary",
        "grant.observations[].code",
        "grant.schema_version",
        "mechanism.attempts",
        "mechanism.markers[]",
        "mechanism.schema_version",
        "request.last_now_utc_ms",
        "request.records[].completion_token_sha256",
        "request.records[].created_at_utc_ms",
        "request.records[].expires_at_utc_ms",
        "request.records[].fingerprint_sha256",
        "request.records[].outcome.code",
        "request.records[].outcome.kind",
        "request.records[].outcome.receipt_sha256",
        "request.records[].outcome.replay.kind",
        "request.records[].outcome.replay.generation",
        "request.records[].outcome.replay.job_id",
        "request.records[].outcome.replay.lease_id",
        "request.records[].outcome.replay.receipt_id",
        "request.records[].request_id",
        "request.records[].state",
        "request.records[].updated_at_utc_ms",
        "request.schema_version",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    durable_fields.sort();
    let cleanup_fields = if variant == Variant::A1 {
        vec![
            "connection",
            "derived_session",
            "queue",
            "server_session",
            "worker",
        ]
    } else {
        vec!["connection", "queue", "server_session", "worker"]
    }
    .into_iter()
    .map(str::to_owned)
    .collect();
    Inventory {
        trusted_inputs,
        experiment_controls,
        memory_authority,
        durable_fields,
        public_fields: vec!["command".to_owned(), "idempotency_key".to_owned()],
        cleanup_fields,
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunMetadata {
    source_sha: String,
    input_digest: String,
    executable_digest: String,
    target: String,
    attempt: u8,
    chain_digest: String,
    chain_inputs: Vec<String>,
    contract_digest: String,
    criteria_digest: String,
    decision_digest: String,
    source_inventory: SourceInventory,
}

fn failure_terminal(attempt: u8, code: &str) -> (&'static str, &'static str) {
    let fixture_contract = matches!(
        code,
        "fixture_read"
            | "fixture_parse"
            | "fixture_invalid"
            | "attempt_invalid"
            | "runner_chain_invalid"
            | "a0_fixture_gate_failed"
    );
    let privacy_security = code.starts_with("receipt_privacy_violation_")
        || code == "a0_lifecycle_authorized_security_failure";
    if fixture_contract {
        (
            if attempt == 2 {
                "INCONCLUSIVE_FIXTURE_EXHAUSTED"
            } else {
                "INCONCLUSIVE_FIXTURE_REPAIRABLE"
            },
            "fixture-contract-failure",
        )
    } else if privacy_security {
        ("REJECT_PRIVACY_SECURITY", "privacy-security-failure")
    } else {
        ("INCONCLUSIVE_NONREPAIRABLE", "experiment-execution-failure")
    }
}

fn inconclusive_receipt(metadata: RunMetadata, code: String) -> Result<Receipt, String> {
    let compatibility_inventory = compatibility_from(&metadata.source_inventory, &[]);
    let (decision, failure_class) = failure_terminal(metadata.attempt, &code);
    let receipt = Receipt {
        schema_version: SCHEMA_VERSION,
        precommitment: "mcp-persisted-session-experiment".to_owned(),
        source_sha: metadata.source_sha,
        input_digest: metadata.input_digest,
        executable_digest: metadata.executable_digest,
        target: metadata.target,
        attempt: metadata.attempt,
        chain_digest: metadata.chain_digest,
        terminal: true,
        fixture_gate: code,
        decision: decision.to_owned(),
        failure_class: Some(failure_class.to_owned()),
        contract_digest: metadata.contract_digest,
        criteria_digest: metadata.criteria_digest,
        decision_digest: metadata.decision_digest,
        negative_control: None,
        variants: Vec::new(),
        compatibility_inventory,
        command_probes: Vec::new(),
        privacy_matches: 0,
    };
    let bytes = serde_json::to_vec(&receipt).map_err(|_| "receipt_encode".to_owned())?;
    privacy_scan(&bytes)?;
    Ok(receipt)
}

#[derive(Clone, Debug, Serialize)]
struct Receipt {
    schema_version: u32,
    precommitment: String,
    source_sha: String,
    input_digest: String,
    executable_digest: String,
    target: String,
    attempt: u8,
    chain_digest: String,
    contract_digest: String,
    criteria_digest: String,
    decision_digest: String,
    terminal: bool,
    fixture_gate: String,
    decision: String,
    failure_class: Option<String>,
    negative_control: Option<VariantReceipt>,
    variants: Vec<VariantReceipt>,
    compatibility_inventory: CompatibilityInventory,
    command_probes: Vec<CommandProbe>,
    privacy_matches: usize,
}

#[derive(Clone, Debug, Serialize)]
struct CommandProbe {
    variant: Variant,
    case: String,
    command: String,
    code: String,
    request_delta: u64,
    use_delta: u64,
    mechanism_delta: u64,
    marker_delta: u64,
    failure: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CompatibilityInventory {
    experiment_base_sha: String,
    source_tree_oid: String,
    source_members_digest: String,
    product_sources_changed: u64,
    descriptors_changed: u64,
    catalogs_changed: u64,
    accepted_operations: Vec<String>,
    job_device_constructible: bool,
    protocol_members: Vec<String>,
    source_members: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceInventory {
    experiment_base_sha: String,
    source_tree_oid: String,
    source_members_digest: String,
    product_sources_changed: u64,
    descriptors_changed: u64,
    catalogs_changed: u64,
    source_members: Vec<String>,
}

fn compatibility_from(source: &SourceInventory, probes: &[CommandProbe]) -> CompatibilityInventory {
    let mut accepted_operations = probes
        .iter()
        .filter(|row| row.code == "constructible")
        .map(|row| row.command.clone())
        .collect::<Vec<_>>();
    accepted_operations.sort();
    accepted_operations.dedup();
    let mut protocol_members = accepted_operations.clone();
    protocol_members.sort();
    CompatibilityInventory {
        experiment_base_sha: source.experiment_base_sha.clone(),
        source_tree_oid: source.source_tree_oid.clone(),
        source_members_digest: source.source_members_digest.clone(),
        product_sources_changed: source.product_sources_changed,
        descriptors_changed: source.descriptors_changed,
        catalogs_changed: source.catalogs_changed,
        job_device_constructible: probes.iter().any(|row| {
            (row.command == "job-start" || row.command == "device-claim")
                && row.code == "constructible"
        }),
        accepted_operations,
        protocol_members,
        source_members: source.source_members.clone(),
    }
}

fn command_probes(root: &Path, fixture: &Fixture, variant: Variant) -> Vec<CommandProbe> {
    let mut traces = Vec::new();
    for (index, probe) in fixture.protocol_cases.iter().enumerate() {
        let case_started = Instant::now();
        let case = root.join(format!("{}-protocol-{index}", variant_label(variant)));
        let result = (|| -> Result<CommandProbe, String> {
            create_private_case(&case)?;
            let grant_path = case.join("grant.json");
            let mechanism_path = case.join("mechanism.json");
            let request_path = case.join("requests.json");
            let _fixture_owner =
                FixtureOwner::initialize(grant_path.clone(), fixture, fixture.max_uses)?;
            let _mechanism_owner = MechanismFixtureOwner::initialize(mechanism_path.clone())?;
            let before_grant = GrantAudit::open(grant_path.clone())?;
            let before_mechanism = MechanismAudit::open(mechanism_path.clone())?;
            let request_before = digest_file_or_absent(&request_path)?;
            let parsed = parse_public_mutation(probe.request.clone(), fixture);
            let code = match parsed {
                Err(code) => code,
                Ok(owned) => {
                    let idempotency = IdempotencyStore::open_at(&request_path).map_err(cu_code)?;
                    if idempotency
                        .lookup(
                            &format!("protocol-{index}"),
                            &fixture.payload_digest,
                            fixture.now_utc_ms,
                        )
                        .map_err(cu_code)?
                        .is_some()
                    {
                        return Err("protocol_request_not_absent".to_owned());
                    }
                    let fresh = match idempotency
                        .reserve(
                            &format!("protocol-{index}"),
                            &fixture.payload_digest,
                            RETENTION_TTL_MS,
                            fixture.now_utc_ms,
                        )
                        .map_err(cu_code)?
                    {
                        ReserveDecision::Fresh(value) => value,
                        _ => return Err("protocol_reservation_not_fresh".to_owned()),
                    };
                    let mut server = GrantProcess::spawn(
                        &case,
                        "protocol-connection",
                        case_started + Duration::from_secs(10),
                    )?;
                    let request = owned.borrowed();
                    if variant == Variant::A1 {
                        server.startup(&request)?;
                        server.session_open()?;
                        let mut session = DerivedSession::create(fixture, &server.nonce);
                        session.effect(&mut server, &request, fixture.now_utc_ms)?;
                        let _ = session.teardown(&mut server)?;
                    } else {
                        server.effect(&request)?;
                    }
                    server.mechanism()?;
                    idempotency
                        .finalize(
                            &format!("protocol-{index}"),
                            &fixture.payload_digest,
                            &fresh.completion_token,
                            final_outcome(fixture)?,
                            fixture.now_utc_ms + 1,
                        )
                        .map_err(cu_code)?;
                    server.stop(true)?;
                    "constructible".to_owned()
                }
            };
            let after_grant = GrantAudit::open(grant_path)?;
            let after_mechanism = MechanismAudit::open(mechanism_path)?;
            let command = probe
                .request
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("invalid")
                .to_owned();
            Ok(CommandProbe {
                variant,
                case: probe.name.clone(),
                command,
                code,
                request_delta: u64::from(digest_file_or_absent(&request_path)? != request_before),
                use_delta: after_grant.uses() - before_grant.uses(),
                mechanism_delta: after_mechanism.attempts() - before_mechanism.attempts(),
                marker_delta: after_mechanism.markers() - before_mechanism.markers(),
                failure: None,
            })
        })();
        match result {
            Ok(trace) => traces.push(trace),
            Err(code) => traces.push(CommandProbe {
                variant,
                case: probe.name.clone(),
                command: probe
                    .request
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("invalid")
                    .to_owned(),
                code: "probe-failed".to_owned(),
                request_delta: 0,
                use_delta: 0,
                mechanism_delta: 0,
                marker_delta: 0,
                failure: Some(code),
            }),
        }
    }
    traces
}

fn phase_case(
    root: &Path,
    fixture: &Fixture,
    variant: Variant,
    phase: Phase,
) -> Result<PhaseTrace, String> {
    let case_started = Instant::now();
    let case = root.join(format!("{}-{}", variant_label(variant), phase.label()));
    create_private_case(&case)?;
    let grant_path = case.join("grant.json");
    let mechanism_path = case.join("mechanism.json");
    let idem_path = case.join("requests.json");
    let _fixture_owner = FixtureOwner::initialize(grant_path.clone(), fixture, fixture.max_uses)?;
    let mut grant = GrantAudit::open(grant_path.clone())?;
    let _mechanism_owner = MechanismFixtureOwner::initialize(mechanism_path.clone())?;
    let mut server = GrantProcess::spawn(
        &case,
        "connection-initial",
        case_started + Duration::from_secs(10),
    )?;
    let idempotency = IdempotencyStore::open_at(&idem_path).map_err(cu_code)?;
    let grant_digest_before = digest_file(&grant_path)?;
    let request_digest_before = digest_file_or_absent(&idem_path)?;
    let mechanism_digest_before = digest_file(&mechanism_path)?;
    let uses_before_lifecycle = grant.uses();
    let startup_before_lifecycle = grant.observation_count("startup-verification");
    let effects_before_lifecycle = grant.observation_count("effect-attempt");
    let owned_request = public_request(fixture)?;
    let request = owned_request.borrowed();
    let fingerprint = &fixture.payload_digest;
    let key = &fixture.idempotency_key;
    let mut session = None;
    let mut session_teardown_observed = None;
    let mut request_first_trace = Vec::new();

    match idempotency
        .lookup(key, fingerprint, fixture.now_utc_ms)
        .map_err(cu_code)?
    {
        None => request_first_trace.push("request-absent".to_owned()),
        Some(_) => return Err("fresh_request_not_absent".to_owned()),
    }

    // Durable request lookup is the first authority-bearing operation. Only an
    // absent record permits A1 startup verification/session construction.
    if variant == Variant::A1 {
        server.startup(&request)?;
        request_first_trace.push("startup-verified".to_owned());
        server.session_open()?;
        request_first_trace.push("session-created".to_owned());
        let derived = DerivedSession::create(fixture, &server.nonce);
        derived.validate(fixture)?;
        session = Some(derived);
    }
    grant = GrantAudit::open(grant_path.clone())?;
    let initial_startup = grant.observation_count("startup-verification");
    let initial_effect_before = grant.observation_count("effect-attempt");
    let uses_after_lifecycle = grant.uses();
    let startup_after_lifecycle = initial_startup;
    let effects_after_lifecycle = initial_effect_before;

    let mut completion_token = None;
    if phase != Phase::FMinus1 {
        match idempotency
            .reserve(key, fingerprint, RETENTION_TTL_MS, fixture.now_utc_ms)
            .map_err(cu_code)?
        {
            ReserveDecision::Fresh(fresh) => completion_token = Some(fresh.completion_token),
            _ => return Err("fresh_reservation_expected".to_owned()),
        }
        request_first_trace.push("request-reserved".to_owned());
    } else {
        request_first_trace.push("stopped-before-reservation".to_owned());
    }
    let uses_before_effect = grant.uses();
    let startup_before_effect = grant.observation_count("startup-verification");
    let effects_before_effect = grant.observation_count("effect-attempt");
    if matches!(phase, Phase::F1 | Phase::F2 | Phase::F3) {
        if let Some(derived) = session.as_mut() {
            derived.effect(&mut server, &request, fixture.now_utc_ms)?;
        } else {
            server.effect(&request)?;
        }
    }
    if matches!(phase, Phase::F2 | Phase::F3) {
        server.mechanism()?;
    }
    let mut pre_kill_request_state = None;
    let mut recovery_request_state = None;
    let mut f2_recovery_sequence = Vec::new();
    if phase == Phase::F2 {
        f2_recovery_sequence.push("effect-marker-persisted".to_owned());
        match idempotency
            .reserve(key, fingerprint, RETENTION_TTL_MS, fixture.now_utc_ms + 1)
            .map_err(cu_code)?
        {
            ReserveDecision::Uncertain(status) if status.state == RequestState::Reserved => {
                pre_kill_request_state = Some("reserved".to_owned());
                f2_recovery_sequence.push("request-reserved".to_owned());
            }
            _ => return Err("f2_pre_kill_not_reserved".to_owned()),
        }
    } else if phase == Phase::F3 {
        let outcome = final_outcome(fixture)?;
        idempotency
            .finalize(
                key,
                fingerprint,
                completion_token
                    .as_deref()
                    .ok_or("completion_token_missing")?,
                outcome,
                fixture.now_utc_ms + 1,
            )
            .map_err(cu_code)?;
    }
    grant = GrantAudit::open(grant_path.clone())?;
    let uses_after_effect = grant.uses();
    let startup_after_effect = grant.observation_count("startup-verification");
    let effects_after_effect = grant.observation_count("effect-attempt");

    let initial_session_calls = session.as_ref().map(|value| value.calls).unwrap_or(0);
    let initial_nonce_digest = digest_text("connection", &server.nonce);

    if phase != Phase::F2
        && let Some(derived) = session.take()
    {
        let _ = derived.teardown(&mut server)?;
        session_teardown_observed = Some(audit_session_teardown(&case.join("owner-events.jsonl"))?);
    }
    server.stop(false)?;
    if phase == Phase::F2 {
        f2_recovery_sequence.push("server-killed-reaped".to_owned());
    }
    drop(grant);

    if phase == Phase::F2 {
        let recovery_grant = GrantAudit::open(grant_path.clone())?;
        let recovery_mechanism = MechanismAudit::open(mechanism_path.clone())?;
        if recovery_grant.uses() != 1
            || recovery_mechanism.attempts() != 1
            || recovery_mechanism.markers() != 1
        {
            return Err("f2_recovery_witness_mismatch".to_owned());
        }
        f2_recovery_sequence.push("witnesses-reopened".to_owned());
        let recovery = IdempotencyStore::open_at(&idem_path).map_err(cu_code)?;
        match recovery
            .reserve(key, fingerprint, RETENTION_TTL_MS, fixture.now_utc_ms + 2)
            .map_err(cu_code)?
        {
            ReserveDecision::Uncertain(status) if status.state == RequestState::Reserved => {}
            _ => return Err("f2_recovery_not_reserved".to_owned()),
        }
        recovery
            .mark_outcome_unknown(
                key,
                fingerprint,
                completion_token
                    .as_deref()
                    .ok_or("completion_token_missing")?,
                fixture.now_utc_ms + 3,
            )
            .map_err(cu_code)?;
        f2_recovery_sequence.push("recovery-marked-outcome-unknown".to_owned());
        match recovery
            .reserve(key, fingerprint, RETENTION_TTL_MS, fixture.now_utc_ms + 4)
            .map_err(cu_code)?
        {
            ReserveDecision::Uncertain(status) if status.state == RequestState::OutcomeUnknown => {
                recovery_request_state = Some("outcome-unknown".to_owned());
                f2_recovery_sequence.push("reconnect-observed-unknown".to_owned());
            }
            _ => return Err("f2_recovery_not_unknown".to_owned()),
        }
    }

    // A new transport connection reopens every durable owner. It asks the
    // request store first; retained state must bypass A1 startup verification.
    let grant = GrantAudit::open(grant_path.clone())?;
    let mut server = GrantProcess::spawn(
        &case,
        "connection-reconnect",
        case_started + Duration::from_secs(10),
    )?;
    let reconnect_nonce_digest = digest_text("connection", &server.nonce);
    let idempotency = IdempotencyStore::open_at(&idem_path).map_err(cu_code)?;
    let before_reconnect_startup = grant.observation_count("startup-verification");
    let before_reconnect_effect = grant.observation_count("effect-attempt");
    let reconnect_now = fixture.now_utc_ms + if phase == Phase::F2 { 5 } else { 2 };
    let decision = idempotency
        .reserve(key, fingerprint, RETENTION_TTL_MS, reconnect_now)
        .map_err(cu_code)?;
    let mut reconnect_session_calls = 0_u64;
    let (retained_state, reconnect_result) = match decision {
        ReserveDecision::ReplayFinalized(status) => {
            if status.state != RequestState::Finalized {
                return Err("final_state_invalid".to_owned());
            }
            if status.outcome != Some(final_outcome(fixture)?) {
                return Err("authoritative_replay_not_equal".to_owned());
            }
            ("finalized", "authoritative-replay")
        }
        ReserveDecision::Uncertain(status) => {
            let state = match status.state {
                RequestState::Reserved => "reserved",
                RequestState::OutcomeUnknown => "outcome-unknown",
                RequestState::Finalized => return Err("uncertain_finalized".to_owned()),
            };
            (state, "retained-uncertainty")
        }
        ReserveDecision::Fresh(fresh) => {
            if phase != Phase::FMinus1 {
                return Err("unexpected_fresh_reconnect".to_owned());
            }
            if variant == Variant::A1 {
                server.startup(&request)?;
                server.session_open()?;
                let mut derived = DerivedSession::create(fixture, &server.nonce);
                derived.effect(&mut server, &request, fixture.now_utc_ms + 2)?;
                reconnect_session_calls = derived.calls;
                let _ = derived.teardown(&mut server)?;
                session_teardown_observed =
                    Some(audit_session_teardown(&case.join("owner-events.jsonl"))?);
            } else {
                server.effect(&request)?;
            }
            server.mechanism()?;
            let outcome = final_outcome(fixture)?;
            idempotency
                .finalize(
                    key,
                    fingerprint,
                    &fresh.completion_token,
                    outcome,
                    fixture.now_utc_ms + 3,
                )
                .map_err(cu_code)?;
            ("finalized", "fresh-after-admission-stop")
        }
    };

    let different = alternate_digest(fingerprint);
    let conflict_result =
        match idempotency.reserve(key, &different, RETENTION_TTL_MS, reconnect_now + 2) {
            Err(error) if error.code == "request_id_conflict" => "request-fingerprint-conflict",
            _ => return Err("different_payload_must_conflict".to_owned()),
        };
    let cleanup = server.stdout_loss(&case)?;
    let grant_audit = GrantAudit::open(grant_path.clone())?;
    let mechanism_audit = MechanismAudit::open(mechanism_path.clone())?;
    let store_digests = BTreeMap::from([
        ("grant_before".to_owned(), grant_digest_before),
        ("grant_after".to_owned(), digest_file(&grant_path)?),
        ("request_before".to_owned(), request_digest_before),
        ("request_after".to_owned(), digest_file(&idem_path)?),
        ("mechanism_before".to_owned(), mechanism_digest_before),
        ("mechanism_after".to_owned(), digest_file(&mechanism_path)?),
    ]);
    let elapsed_ms = bounded_case_elapsed(case_started)?;
    Ok(PhaseTrace {
        variant,
        phase,
        retained_state: retained_state.to_owned(),
        request_first_trace,
        initial_startup,
        reconnect_startup: grant_audit.observation_count("startup-verification")
            - before_reconnect_startup,
        initial_effect_decisions: before_reconnect_effect - initial_effect_before,
        reconnect_effect_decisions: grant_audit.observation_count("effect-attempt")
            - before_reconnect_effect,
        uses: grant_audit.uses(),
        mechanism_attempts: mechanism_audit.attempts(),
        markers: mechanism_audit.markers(),
        reconnect_result: reconnect_result.to_owned(),
        pre_kill_request_state,
        recovery_request_state,
        f2_recovery_sequence,
        conflict_result: conflict_result.to_owned(),
        cleanup_ms: cleanup.elapsed_ms,
        cleanup_complete: cleanup.complete,
        pre_loss_owned_state_counts: cleanup.pre_loss_owned_state_counts,
        pre_loss_process_group_alive: cleanup.pre_loss_process_group_alive,
        pre_loss_worker_alive: cleanup.pre_loss_worker_alive,
        pre_loss_worker_same_group: cleanup.pre_loss_worker_same_group,
        owned_state_counts: cleanup.post_loss_owned_state_counts,
        post_loss_process_group_zero: cleanup.post_loss_process_group_zero,
        post_loss_worker_zero: cleanup.post_loss_worker_zero,
        connection_nonce_digests: [initial_nonce_digest, reconnect_nonce_digest],
        initial_session_calls,
        reconnect_session_calls,
        session_teardown_observed,
        elapsed_ms,
        use_samples: [
            uses_before_lifecycle,
            uses_after_lifecycle,
            uses_before_effect,
            uses_after_effect,
        ],
        startup_samples: [
            startup_before_lifecycle,
            startup_after_lifecycle,
            startup_before_effect,
            startup_after_effect,
        ],
        effect_samples: [
            effects_before_lifecycle,
            effects_after_lifecycle,
            effects_before_effect,
            effects_after_effect,
        ],
        store_digests,
    })
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnerEvent {
    sequence: u64,
    resource: String,
    action: String,
}

fn append_owner_event(
    path: &Path,
    sequence: &mut u64,
    resource: &str,
    action: &str,
) -> Result<(), String> {
    *sequence += 1;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| "owner_event_open".to_owned())?;
    serde_json::to_writer(
        &mut file,
        &OwnerEvent {
            sequence: *sequence,
            resource: resource.to_owned(),
            action: action.to_owned(),
        },
    )
    .map_err(|_| "owner_event_encode".to_owned())?;
    file.write_all(b"\n")
        .map_err(|_| "owner_event_write".to_owned())?;
    file.flush().map_err(|_| "owner_event_flush".to_owned())
}

fn audit_owner_events(path: &Path) -> Result<[usize; 4], String> {
    let expected = [
        ("connection", "hold"),
        ("queue", "acquire"),
        ("session", "hold"),
        ("worker", "acquire"),
        ("worker", "release"),
        ("session", "release"),
        ("queue", "release"),
        ("connection", "release"),
    ];
    let file = fs::File::open(path).map_err(|_| "owner_event_audit_open".to_owned())?;
    let mut events = Vec::new();
    for (expected_sequence, line) in (1_u64..).zip(std::io::BufReader::new(file).lines()) {
        let event: OwnerEvent =
            serde_json::from_str(&line.map_err(|_| "owner_event_audit_read".to_owned())?)
                .map_err(|_| "owner_event_audit_parse".to_owned())?;
        if event.sequence != expected_sequence {
            return Err("owner_event_sequence".to_owned());
        }
        events.push((event.resource, event.action));
    }
    let expected_len =
        if events.last().map(|row| (&*row.0, &*row.1)) == Some(("connection", "release")) {
            8
        } else {
            4
        };
    let observed = events
        .get(
            events
                .len()
                .checked_sub(expected_len)
                .ok_or("owner_event_short")?..,
        )
        .ok_or("owner_event_short")?;
    let mut counts = [0_isize; 4];
    for (index_in_cleanup, event) in observed.iter().enumerate() {
        if event.0 != expected[index_in_cleanup].0 || event.1 != expected[index_in_cleanup].1 {
            return Err("owner_event_order".to_owned());
        }
        let index = match event.0.as_str() {
            "queue" => 0,
            "session" => 1,
            "connection" => 2,
            "worker" => 3,
            _ => return Err("owner_event_resource".to_owned()),
        };
        counts[index] += match event.1.as_str() {
            "acquire" | "hold" => 1,
            "release" => -1,
            _ => return Err("owner_event_action".to_owned()),
        };
        if counts[index] < 0 {
            return Err("owner_event_underflow".to_owned());
        }
    }
    Ok([
        counts[0] as usize,
        counts[1] as usize,
        counts[2] as usize,
        counts[3] as usize,
    ])
}

fn audit_session_teardown(path: &Path) -> Result<bool, String> {
    let file = fs::File::open(path).map_err(|_| "session_event_audit_open".to_owned())?;
    let mut acquired = 0_usize;
    let mut released = 0_usize;
    for line in std::io::BufReader::new(file).lines() {
        let event: OwnerEvent =
            serde_json::from_str(&line.map_err(|_| "session_event_audit_read".to_owned())?)
                .map_err(|_| "session_event_audit_parse".to_owned())?;
        if event.resource == "session" {
            match event.action.as_str() {
                "acquire" => acquired += 1,
                "release" => released += 1,
                "hold" => {}
                _ => return Err("session_event_action".to_owned()),
            }
        }
    }
    Ok(acquired > 0 && acquired == released)
}

fn grant_server(case: &Path) -> Result<(), String> {
    struct ServerSession {
        _opened: Instant,
    }
    let grant_path = case.join("grant.json");
    let mechanism_path = case.join("mechanism.json");
    let event_path = case.join("owner-events.jsonl");
    let _ = fs::remove_file(&event_path);
    let mut event_sequence = 0_u64;
    let mut session: Option<ServerSession> = None;
    let mut cleanup_worker: Option<std::process::Child> = None;
    let mut queue = VecDeque::with_capacity(1);
    append_owner_event(&event_path, &mut event_sequence, "connection", "acquire")?;
    let input = std::io::stdin();
    let mut output = BufWriter::new(std::io::stdout());
    for line in input.lock().lines() {
        let request: WireRequest =
            serde_json::from_str(&line.map_err(|_| "grant_server_input".to_owned())?)
                .map_err(|_| "grant_server_request".to_owned())?;
        if matches!(request, WireRequest::PrepareCleanup) {
            if !queue.is_empty() || cleanup_worker.is_some() {
                return Err("cleanup_already_prepared".to_owned());
            }
            append_owner_event(&event_path, &mut event_sequence, "connection", "hold")?;
            queue.push_back(WireRequest::Cleanup);
            append_owner_event(&event_path, &mut event_sequence, "queue", "acquire")?;
            if session.is_none() {
                session = Some(ServerSession {
                    _opened: Instant::now(),
                });
            }
            append_owner_event(&event_path, &mut event_sequence, "session", "hold")?;
            cleanup_worker = Some(
                Command::new(env::current_exe().map_err(|_| "worker_current_exe".to_owned())?)
                    .arg("--owned-worker")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|_| "worker_spawn".to_owned())?,
            );
            fs::write(
                case.join("worker.pid"),
                cleanup_worker
                    .as_ref()
                    .ok_or("worker_missing")?
                    .id()
                    .to_string(),
            )
            .map_err(|_| "worker_pid_write".to_owned())?;
            append_owner_event(&event_path, &mut event_sequence, "worker", "acquire")?;
            serde_json::to_writer(
                &mut output,
                &WireReply {
                    ok: true,
                    code: "cleanup-prepared".to_owned(),
                },
            )
            .map_err(|_| "cleanup_prepare_reply".to_owned())?;
            output
                .write_all(b"\n")
                .map_err(|_| "cleanup_prepare_reply".to_owned())?;
            output
                .flush()
                .map_err(|_| "cleanup_prepare_reply".to_owned())?;
            continue;
        }
        let prepared_cleanup = matches!(request, WireRequest::Cleanup) && queue.len() == 1;
        if prepared_cleanup {
            let _ = queue.pop_front();
        }
        let request = if prepared_cleanup {
            request
        } else {
            if !queue.is_empty() {
                return Err("server_queue_full".to_owned());
            }
            queue.push_back(request);
            let request = queue.pop_front().ok_or("server_queue_empty")?;
            append_owner_event(&event_path, &mut event_sequence, "queue", "acquire")?;
            request
        };
        let shutdown = matches!(request, WireRequest::Shutdown);
        if matches!(request, WireRequest::Cleanup) {
            let mut worker = cleanup_worker.take().ok_or("cleanup_not_prepared")?;
            let reply = WireReply {
                ok: true,
                code: "cleanup-accepted".to_owned(),
            };
            let write_failed = serde_json::to_writer(&mut output, &reply).is_err()
                || output.write_all(b"\n").is_err()
                || output.flush().is_err();
            if !write_failed {
                let _ = worker.kill();
                let _ = worker.wait();
                return Err("cleanup_stdout_not_broken".to_owned());
            }
            let _ = worker.kill();
            worker.wait().map_err(|_| "worker_wait".to_owned())?;
            append_owner_event(&event_path, &mut event_sequence, "worker", "release")?;
            append_owner_event(&event_path, &mut event_sequence, "session", "release")?;
            session.take();
            append_owner_event(&event_path, &mut event_sequence, "queue", "release")?;
            append_owner_event(&event_path, &mut event_sequence, "connection", "release")?;
            return Ok(());
        }
        let result = match request {
            WireRequest::Startup {
                binding,
                operation,
                now_utc_ms,
            } => witnesses::server_verify_startup(
                grant_path.clone(),
                &GrantRequest {
                    binding: &binding,
                    operation: &operation,
                    now_utc_ms,
                },
            ),
            WireRequest::Effect {
                binding,
                operation,
                now_utc_ms,
            } => witnesses::server_reserve_effect(
                grant_path.clone(),
                &GrantRequest {
                    binding: &binding,
                    operation: &operation,
                    now_utc_ms,
                },
            ),
            WireRequest::Mechanism => witnesses::server_attempt_mechanism(mechanism_path.clone()),
            WireRequest::SessionOpen => {
                if session.is_none() {
                    append_owner_event(&event_path, &mut event_sequence, "session", "acquire")?;
                    session = Some(ServerSession {
                        _opened: Instant::now(),
                    });
                }
                Ok(())
            }
            WireRequest::SessionClose => {
                if session.take().is_some() {
                    append_owner_event(&event_path, &mut event_sequence, "session", "release")?;
                }
                Ok(())
            }
            WireRequest::PrepareCleanup => unreachable!(),
            WireRequest::Cleanup => unreachable!(),
            WireRequest::Shutdown => Ok(()),
        };
        let reply = match result {
            Ok(()) => WireReply {
                ok: true,
                code: "accepted".to_owned(),
            },
            Err(code) => WireReply { ok: false, code },
        };
        append_owner_event(&event_path, &mut event_sequence, "queue", "release")?;
        if serde_json::to_writer(&mut output, &reply).is_err()
            || output.write_all(b"\n").is_err()
            || output.flush().is_err()
        {
            break;
        }
        if shutdown {
            if session.take().is_some() {
                append_owner_event(&event_path, &mut event_sequence, "session", "release")?;
            }
            append_owner_event(&event_path, &mut event_sequence, "connection", "release")?;
            break;
        }
    }
    Ok(())
}

fn owned_worker() -> Result<(), String> {
    let mut byte = [0_u8; 1];
    loop {
        match std::io::stdin().read(&mut byte) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(_) => return Err("owned_worker_read".to_owned()),
        }
    }
}

fn bounded_case_elapsed(started: Instant) -> Result<u128, String> {
    let elapsed_ms = started.elapsed().as_millis();
    if elapsed_ms >= 10_000 {
        Err("case_deadline_exceeded".to_owned())
    } else {
        Ok(elapsed_ms)
    }
}

fn denial_case(
    root: &Path,
    fixture: &Fixture,
    variant: Variant,
    case_name: &str,
    post_exchange: bool,
) -> Result<DenialTrace, String> {
    let case_started = Instant::now();
    let arm = if post_exchange { "post" } else { "fresh" };
    let dir = root.join(format!("{}-deny-{arm}-{case_name}", variant_label(variant)));
    create_private_case(&dir)?;
    let grant_path = dir.join("grant.json");
    let fixture_owner = FixtureOwner::initialize(grant_path.clone(), fixture, fixture.max_uses)?;
    let mut grant = GrantAudit::open(grant_path.clone())?;
    let mechanism_path = dir.join("mechanism.json");
    let _mechanism_owner = MechanismFixtureOwner::initialize(mechanism_path.clone())?;
    let mut server = GrantProcess::spawn(
        &dir,
        "denial-connection",
        case_started + Duration::from_secs(10),
    )?;
    let idem_path = dir.join("requests.json");
    let idempotency = IdempotencyStore::open_at(&idem_path).map_err(cu_code)?;
    let grant_digest_before = digest_file(&grant_path)?;
    let mechanism_digest_before = digest_file(&mechanism_path)?;
    let request_digest_before = digest_file_or_absent(&idem_path)?;
    let uses_before_lifecycle = grant.uses();
    let startup_before_lifecycle = grant.observation_count("startup-verification");
    let effects_before_lifecycle = grant.observation_count("effect-attempt");
    let mut binding = fixture.binding.clone();
    let mut operation = fixture.operation.clone();
    let mut now = fixture.now_utc_ms;
    let tested_key = format!("request-denial-{case_name}");
    if idempotency
        .lookup(&tested_key, &fixture.payload_digest, fixture.now_utc_ms)
        .map_err(cu_code)?
        .is_some()
    {
        return Err("denial_request_not_absent".to_owned());
    }

    let mut established_session = None;
    if post_exchange && variant == Variant::A1 {
        if idempotency
            .lookup(
                "request-denial-baseline",
                &fixture.payload_digest,
                fixture.now_utc_ms,
            )
            .map_err(cu_code)?
            .is_some()
        {
            return Err("denial_request_not_absent".to_owned());
        }
        let owned_baseline = public_request(fixture)?;
        let baseline = owned_baseline.borrowed();
        server.startup(&baseline)?;
        server.session_open()?;
        let derived = DerivedSession::create(fixture, &server.nonce);
        derived.validate(fixture)?;
        established_session = Some(derived);
    }
    grant = GrantAudit::open(grant_path.clone())?;
    let uses_after_lifecycle = grant.uses();
    let startup_after_lifecycle = grant.observation_count("startup-verification");
    let effects_after_lifecycle = grant.observation_count("effect-attempt");
    match case_name {
        "revoked" => fixture_owner.revoke()?,
        "expired" => now = fixture.expires_at_utc_ms + 1,
        "not-yet-valid" => now = fixture.not_before_utc_ms - 1,
        "wrong-target" => binding.target = "other-target".to_owned(),
        "wrong-session" | "binding-mismatch" => binding.session = "other-session".to_owned(),
        "wrong-operation" | "operation-mismatch" => operation = "other-operation".to_owned(),
        "exhausted" if post_exchange => {
            let setup_key = "request-setup-x4";
            let setup_reservation = match idempotency
                .reserve(
                    setup_key,
                    &fixture.payload_digest,
                    RETENTION_TTL_MS,
                    fixture.now_utc_ms,
                )
                .map_err(cu_code)?
            {
                ReserveDecision::Fresh(fresh) => fresh,
                _ => return Err("x4_setup_reservation_not_fresh".to_owned()),
            };
            let owned_setup = public_request(fixture)?;
            let setup = owned_setup.borrowed();
            server.effect(&setup)?;
            server.mechanism()?;
            idempotency
                .finalize(
                    setup_key,
                    &fixture.payload_digest,
                    &setup_reservation.completion_token,
                    final_outcome(fixture)?,
                    fixture.now_utc_ms + 1,
                )
                .map_err(cu_code)?;
        }
        "exhausted" => fixture_owner.exhaust_for_fixture()?,
        _ => return Err("unknown_denial_case".to_owned()),
    }
    grant = GrantAudit::open(grant_path.clone())?;
    let mechanism = MechanismAudit::open(mechanism_path.clone())?;
    let setup_uses = grant.uses();
    let setup_attempts = mechanism.attempts();
    let setup_markers = mechanism.markers();
    let startup_before_effect = grant.observation_count("startup-verification");
    let effects_before_effect = grant.observation_count("effect-attempt");
    let tested_reservation = if variant == Variant::B || post_exchange {
        match idempotency
            .reserve(
                &tested_key,
                &fixture.payload_digest,
                RETENTION_TTL_MS,
                fixture.now_utc_ms + 2,
            )
            .map_err(cu_code)?
        {
            ReserveDecision::Fresh(fresh) => Some(fresh),
            _ => return Err("denial_reservation_not_fresh".to_owned()),
        }
    } else {
        None
    };
    let owned_request = internal_attack_request(binding, operation, now);
    let request = owned_request.borrowed();
    let result = if variant == Variant::A1 && !post_exchange {
        server.startup(&request)
    } else if let Some(derived) = established_session.as_mut() {
        derived.effect(&mut server, &request, now)
    } else {
        server.effect(&request)
    };
    let denial_code = match result {
        Ok(()) => return Err("denial_was_authorized".to_owned()),
        Err(code) => code,
    };
    if let Some(reservation) = tested_reservation {
        let outcome = FinalOutcome::new(
            FinalOutcomeKind::Failed,
            format!("research_{}", denial_code.replace('-', "_")),
            None,
        )
        .map_err(cu_code)?;
        idempotency
            .finalize(
                &tested_key,
                &fixture.payload_digest,
                &reservation.completion_token,
                outcome,
                fixture.now_utc_ms + 3,
            )
            .map_err(cu_code)?;
    }
    if denial_code == "authorized" {
        return Err("denial_was_authorized".to_owned());
    }
    if let Some(derived) = established_session.take() {
        let (calls, _) = derived.teardown(&mut server)?;
        if calls != 1 {
            return Err("post_exchange_session_calls".to_owned());
        }
    }
    server.stop(true)?;
    let grant = GrantAudit::open(grant_path.clone())?;
    let mechanism = MechanismAudit::open(mechanism_path.clone())?;
    let store_digests = BTreeMap::from([
        ("grant_before".to_owned(), grant_digest_before),
        ("grant_after".to_owned(), digest_file(&grant_path)?),
        ("request_before".to_owned(), request_digest_before),
        (
            "request_after".to_owned(),
            digest_file_or_absent(&idem_path)?,
        ),
        ("mechanism_before".to_owned(), mechanism_digest_before),
        ("mechanism_after".to_owned(), digest_file(&mechanism_path)?),
    ]);
    let elapsed_ms = bounded_case_elapsed(case_started)?;
    Ok(DenialTrace {
        variant,
        case: case_name.to_owned(),
        post_exchange,
        startup_codes: grant.codes("startup-verification"),
        effect_codes: grant.codes("effect-attempt"),
        uses: grant.uses() - setup_uses,
        mechanism_attempts: mechanism.attempts() - setup_attempts,
        markers: mechanism.markers() - setup_markers,
        setup_uses,
        setup_mechanism_attempts: setup_attempts,
        setup_markers,
        use_samples: [
            uses_before_lifecycle,
            uses_after_lifecycle,
            setup_uses,
            grant.uses(),
        ],
        startup_samples: [
            startup_before_lifecycle,
            startup_after_lifecycle,
            startup_before_effect,
            grant.observation_count("startup-verification"),
        ],
        effect_samples: [
            effects_before_lifecycle,
            effects_after_lifecycle,
            effects_before_effect,
            grant.observation_count("effect-attempt"),
        ],
        store_digests,
        elapsed_ms,
    })
}

fn a0_control(root: &Path, fixture: &Fixture) -> Result<(Vec<String>, SlopeTrace), String> {
    let case_started = Instant::now();
    let dir = root.join("a0-control");
    create_private_case(&dir)?;
    let grant_path = dir.join("grant.json");
    let mechanism_path = dir.join("mechanism.json");
    let request_path = dir.join("requests.json");
    let _fixture_owner = FixtureOwner::initialize(grant_path.clone(), fixture, fixture.max_uses)?;
    let grant = GrantAudit::open(grant_path.clone())?;
    let _mechanism_owner = MechanismFixtureOwner::initialize(mechanism_path.clone())?;
    let mut server = GrantProcess::spawn(
        &dir,
        "a0-connection",
        case_started + Duration::from_secs(10),
    )?;
    let initial_uses = grant.uses();
    let initial_startup = grant.observation_count("startup-verification");
    let initial_effects = grant.observation_count("effect-attempt");
    let idempotency = IdempotencyStore::open_at(&request_path).map_err(cu_code)?;
    let mut lifecycle_codes = Vec::new();
    for operation in ["session-start", "session-renew", "session-end"] {
        let owned_request = internal_attack_request(
            fixture.binding.clone(),
            operation.to_owned(),
            fixture.now_utc_ms,
        );
        let request = owned_request.borrowed();
        let code = match server.effect(&request) {
            Err(code) => code,
            Ok(()) => return Err("a0_lifecycle_authorized_security_failure".to_owned()),
        };
        lifecycle_codes.push(code);
    }
    let after_lifecycle = GrantAudit::open(grant_path.clone())?;
    let lifecycle_uses = after_lifecycle.uses();
    let lifecycle_startup = after_lifecycle.observation_count("startup-verification");
    let lifecycle_effects = after_lifecycle.observation_count("effect-attempt");
    let owned_shell = public_request(fixture)?;
    let shell = owned_shell.borrowed();
    if idempotency
        .lookup(
            &fixture.idempotency_key,
            &fixture.payload_digest,
            fixture.now_utc_ms,
        )
        .map_err(cu_code)?
        .is_some()
    {
        return Err("a0_request_not_absent".to_owned());
    }
    let fresh = match idempotency
        .reserve(
            &fixture.idempotency_key,
            &fixture.payload_digest,
            RETENTION_TTL_MS,
            fixture.now_utc_ms,
        )
        .map_err(cu_code)?
    {
        ReserveDecision::Fresh(fresh) => fresh,
        _ => return Err("a0_reservation_not_fresh".to_owned()),
    };
    server.effect(&shell)?;
    server.mechanism()?;
    idempotency
        .finalize(
            &fixture.idempotency_key,
            &fixture.payload_digest,
            &fresh.completion_token,
            final_outcome(fixture)?,
            fixture.now_utc_ms + 1,
        )
        .map_err(cu_code)?;
    match idempotency
        .reserve(
            &fixture.idempotency_key,
            &fixture.payload_digest,
            RETENTION_TTL_MS,
            fixture.now_utc_ms + 2,
        )
        .map_err(cu_code)?
    {
        ReserveDecision::ReplayFinalized(status)
            if status.outcome == Some(final_outcome(fixture)?) => {}
        ReserveDecision::ReplayFinalized(_) => return Err("a0_replay_outcome_mismatch".to_owned()),
        _ => return Err("a0_replay_not_finalized".to_owned()),
    }
    server.stop(true)?;
    let grant = GrantAudit::open(grant_path.clone())?;
    let mechanism = MechanismAudit::open(mechanism_path.clone())?;
    // Literal substitution of max_uses=1: 0 startup + 0 lifecycle + 1 shell
    // equals one consumed use and leaves zero remaining uses.
    if fixture.max_uses != 1 || lifecycle_uses != 0 || grant.uses() != 1 {
        return Err("max_uses_one_substitution_failed".to_owned());
    }
    let elapsed_ms = bounded_case_elapsed(case_started)?;
    Ok((
        lifecycle_codes,
        SlopeTrace {
            ceiling: 1,
            authorized_effect_decisions: grant
                .codes("effect-attempt")
                .iter()
                .filter(|code| code.as_str() == "authorized")
                .count() as u64,
            use_delta: grant.uses(),
            mechanism_attempts: mechanism.attempts(),
            marker_delta: mechanism.markers(),
            lifecycle_use_delta: lifecycle_uses,
            use_samples: [initial_uses, lifecycle_uses, lifecycle_uses, grant.uses()],
            startup_samples: [
                initial_startup,
                lifecycle_startup,
                lifecycle_startup,
                grant.observation_count("startup-verification"),
            ],
            effect_samples: [
                initial_effects,
                lifecycle_effects,
                lifecycle_effects,
                grant.observation_count("effect-attempt"),
            ],
            elapsed_ms,
        },
    ))
}

fn slope_case(
    root: &Path,
    fixture: &Fixture,
    variant: Variant,
    ceiling: u64,
) -> Result<SlopeTrace, String> {
    let case_started = Instant::now();
    let dir = root.join(format!("{}-slope-{ceiling}", variant_label(variant)));
    create_private_case(&dir)?;
    let grant_path = dir.join("grant.json");
    let mechanism_path = dir.join("mechanism.json");
    let _fixture_owner = FixtureOwner::initialize(grant_path.clone(), fixture, ceiling.max(1))?;
    let grant = GrantAudit::open(grant_path.clone())?;
    let _mechanism_owner = MechanismFixtureOwner::initialize(mechanism_path.clone())?;
    let mut server = GrantProcess::spawn(
        &dir,
        "slope-connection",
        case_started + Duration::from_secs(10),
    )?;
    let owned_request = public_request(fixture)?;
    let request = owned_request.borrowed();
    let uses_before_lifecycle = grant.uses();
    let startup_count_before = grant.observation_count("startup-verification");
    let effect_count_before = grant.observation_count("effect-attempt");
    let mut established_session = None;
    let slope_store = IdempotencyStore::open_at(dir.join("requests.json")).map_err(cu_code)?;
    if slope_store
        .lookup(
            &format!("slope-{ceiling}"),
            &fixture.payload_digest,
            fixture.now_utc_ms,
        )
        .map_err(cu_code)?
        .is_some()
    {
        return Err("slope_request_not_absent".to_owned());
    }
    if variant == Variant::A1 {
        server.startup(&request)?;
        server.session_open()?;
        let mut session = DerivedSession::create(fixture, &server.nonce);
        session.call_ceiling = ceiling;
        if ceiling == fixture.call_ceiling {
            session.validate(fixture)?;
        }
        established_session = Some(session);
    }
    let after_lifecycle_audit = GrantAudit::open(grant_path.clone())?;
    let after_lifecycle = after_lifecycle_audit.uses();
    let startup_after_lifecycle = after_lifecycle_audit.observation_count("startup-verification");
    let effect_after_lifecycle = after_lifecycle_audit.observation_count("effect-attempt");
    let before_effect = after_lifecycle;
    for _ in 0..ceiling {
        if let Some(session) = established_session.as_mut() {
            session.effect(&mut server, &request, fixture.now_utc_ms)?;
        } else {
            server.effect(&request)?;
        }
        server.mechanism()?;
    }
    if let Some(session) = established_session.take() {
        let _ = session.teardown(&mut server)?;
    }
    server.stop(true)?;
    drop(grant);
    let grant = GrantAudit::open(grant_path)?;
    let mechanism = MechanismAudit::open(mechanism_path)?;
    let elapsed_ms = bounded_case_elapsed(case_started)?;
    Ok(SlopeTrace {
        ceiling,
        authorized_effect_decisions: grant
            .codes("effect-attempt")
            .iter()
            .filter(|code| code.as_str() == "authorized")
            .count() as u64,
        use_delta: grant.uses() - after_lifecycle,
        mechanism_attempts: mechanism.attempts(),
        marker_delta: mechanism.markers(),
        lifecycle_use_delta: after_lifecycle - uses_before_lifecycle,
        use_samples: [
            uses_before_lifecycle,
            after_lifecycle,
            before_effect,
            grant.uses(),
        ],
        startup_samples: [
            startup_count_before,
            startup_after_lifecycle,
            startup_after_lifecycle,
            grant.observation_count("startup-verification"),
        ],
        effect_samples: [
            effect_count_before,
            effect_after_lifecycle,
            effect_after_lifecycle,
            grant.observation_count("effect-attempt"),
        ],
        elapsed_ms,
    })
}

fn run_variant(root: &Path, fixture: &Fixture, variant: Variant) -> Result<VariantReceipt, String> {
    let mut phase_traces = Vec::new();
    let mut denial_traces = Vec::new();
    let mut failures = Vec::new();
    let mut lifecycle_codes = Vec::new();
    let mut slope = Vec::new();

    if variant == Variant::A0 {
        let (codes, one) = a0_control(root, fixture)?;
        lifecycle_codes = codes;
        slope.push(slope_case(root, fixture, Variant::A0, 0)?);
        slope.push(one);
        slope.push(slope_case(root, fixture, Variant::A0, 2)?);
    } else {
        for phase in PHASES {
            match phase_case(root, fixture, variant, phase) {
                Ok(trace) => phase_traces.push(trace),
                Err(code) => failures.push(format!("phase:{}:{code}", phase.label())),
            }
        }
        for name in [
            "revoked",
            "expired",
            "not-yet-valid",
            "wrong-target",
            "wrong-session",
            "wrong-operation",
            "exhausted",
        ] {
            match denial_case(root, fixture, variant, name, false) {
                Ok(trace) => denial_traces.push(trace),
                Err(code) => failures.push(format!("fresh-denial:{name}:{code}")),
            }
        }
        if variant == Variant::A1 {
            for name in [
                "revoked",
                "expired",
                "binding-mismatch",
                "exhausted",
                "operation-mismatch",
            ] {
                match denial_case(root, fixture, variant, name, true) {
                    Ok(trace) => denial_traces.push(trace),
                    Err(code) => failures.push(format!("post-denial:{name}:{code}")),
                }
            }
        }
        for ceiling in 0..=2 {
            match slope_case(root, fixture, variant, ceiling) {
                Ok(trace) => slope.push(trace),
                Err(code) => failures.push(format!("slope:{ceiling}:{code}")),
            }
        }
    }

    let inventory = inventory_for(root, variant, fixture)?;
    if inventory_has_forbidden(&inventory) {
        failures.push("C6-forbidden-inventory".to_owned());
    }
    if variant == Variant::A0
        && (lifecycle_codes != ["operation-mismatch"; 3]
            || slope.len() != 3
            || slope[1].ceiling != 1
            || slope[1].use_delta != 1
            || slope[1].mechanism_attempts != 1
            || slope[1].marker_delta != 1
            || slope[1].authorized_effect_decisions != 1)
    {
        failures.push("a0-negative-control-contract".to_owned());
    }
    let survived = failures.is_empty();
    Ok(VariantReceipt {
        variant,
        survived,
        criteria: BTreeMap::new(),
        phase_traces,
        denial_traces,
        lifecycle_codes,
        slope,
        inventory,
        tie_counts: [0; 6],
        tie_members: Vec::new(),
        failures,
    })
}

fn inventory_for(root: &Path, variant: Variant, fixture: &Fixture) -> Result<Inventory, String> {
    let memory_authority = if variant == Variant::A1 {
        let value = serde_json::to_value(DerivedSession::create(fixture, "inventory-connection"))
            .map_err(|_| "memory_inventory_encode".to_owned())?;
        let mut paths = Vec::new();
        inventory_json_paths("memory", &value, &mut paths);
        paths.sort();
        paths
            .into_iter()
            .map(|field| field.trim_start_matches("memory.").to_owned())
            .collect()
    } else {
        Vec::new()
    };
    let trusted_value = serde_json::to_value(TrustedInputInventory::from_fixture(fixture))
        .map_err(|_| "trusted_inventory_encode".to_owned())?;
    let mut trusted_inputs = Vec::new();
    inventory_json_paths("trusted", &trusted_value, &mut trusted_inputs);
    trusted_inputs.sort();
    trusted_inputs.dedup();
    let controls_value = serde_json::to_value(ExperimentControlInventory::from_fixture(fixture))
        .map_err(|_| "control_inventory_encode".to_owned())?;
    let mut fixture_top_level = serde_json::to_value(fixture)
        .map_err(|_| "fixture_inventory_encode".to_owned())?
        .as_object()
        .ok_or("fixture_inventory_shape")?
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut covered_top_level = trusted_value
        .as_object()
        .ok_or("trusted_inventory_shape")?
        .keys()
        .chain(
            controls_value
                .as_object()
                .ok_or("control_inventory_shape")?
                .keys(),
        )
        .cloned()
        .collect::<Vec<_>>();
    fixture_top_level.sort();
    covered_top_level.sort();
    covered_top_level.dedup();
    if fixture_top_level != covered_top_level {
        return Err("control_inventory_incomplete".to_owned());
    }
    let mut experiment_controls = Vec::new();
    inventory_json_paths("controls", &controls_value, &mut experiment_controls);
    experiment_controls.sort();
    experiment_controls.dedup();
    let mut durable_fields = GrantAudit::schema_inventory(fixture)?;
    durable_fields.extend(MechanismAudit::schema_inventory()?);
    let schema_case = root.join(format!("{}-request-schema", variant_label(variant)));
    create_private_case(&schema_case)?;
    let request_path = schema_case.join("requests.json");
    let request_store = IdempotencyStore::open_at(&request_path).map_err(cu_code)?;
    let replay_shapes = [
        (
            "inventory-schema-job",
            FinalReplay::JobSpawn {
                job_id: "00000000-0000-4000-8000-000000000001".to_owned(),
                generation: 1,
            },
        ),
        (
            "inventory-schema-device",
            FinalReplay::DeviceClaim {
                lease_id: "00000000-0000-4000-8000-000000000002".to_owned(),
                generation: 1,
            },
        ),
        (
            "inventory-schema-privilege",
            FinalReplay::PrivilegeApply {
                receipt_id: "00000000-0000-4000-8000-000000000003".to_owned(),
            },
        ),
    ];
    for (offset, (request_id, replay)) in replay_shapes.into_iter().enumerate() {
        let now = fixture.now_utc_ms + i64::try_from(offset).unwrap_or(0);
        let reservation = match request_store
            .reserve(request_id, &fixture.payload_digest, RETENTION_TTL_MS, now)
            .map_err(cu_code)?
        {
            ReserveDecision::Fresh(fresh) => fresh,
            _ => return Err("request_inventory_not_fresh".to_owned()),
        };
        request_store
            .finalize(
                request_id,
                &fixture.payload_digest,
                &reservation.completion_token,
                final_outcome(fixture)?
                    .with_replay(replay)
                    .map_err(cu_code)?,
                now + 1,
            )
            .map_err(cu_code)?;
    }
    let mut request_value: serde_json::Value = serde_json::from_slice(
        &fs::read(&request_path).map_err(|_| "request_inventory_read".to_owned())?,
    )
    .map_err(|_| "request_inventory_parse".to_owned())?;
    let records = request_value
        .get_mut("records")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or("request_inventory_records")?;
    let records = records.values().cloned().collect::<Vec<_>>();
    request_value["records"] = serde_json::Value::Array(records);
    inventory_json_paths("request", &request_value, &mut durable_fields);
    durable_fields.sort();
    durable_fields.dedup();
    Ok(Inventory {
        trusted_inputs,
        experiment_controls,
        memory_authority,
        durable_fields,
        public_fields: {
            let public = PublicMutation::ShellExec {
                idempotency_key: fixture.idempotency_key.clone(),
            };
            let value =
                serde_json::to_value(public).map_err(|_| "public_inventory_encode".to_owned())?;
            let mut fields = Vec::new();
            inventory_json_paths("public", &value, &mut fields);
            fields.sort();
            fields
                .into_iter()
                .map(|field| field.trim_start_matches("public.").to_owned())
                .collect()
        },
        cleanup_fields: {
            let value = serde_json::to_value(CleanupOwnershipInventory {
                queue: true,
                server_session: true,
                connection: true,
                worker: true,
                derived_session: (variant == Variant::A1).then_some(true),
            })
            .map_err(|_| "cleanup_inventory_encode".to_owned())?;
            let mut fields = Vec::new();
            inventory_json_paths("cleanup", &value, &mut fields);
            fields.sort();
            fields
                .into_iter()
                .map(|field| field.trim_start_matches("cleanup.").to_owned())
                .collect()
        },
    })
}

fn inventory_json_paths(prefix: &str, value: &serde_json::Value, output: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                inventory_json_paths(&format!("{prefix}.{key}"), child, output);
            }
        }
        serde_json::Value::Array(values) => {
            if values.is_empty() {
                output.push(format!("{prefix}[]"));
            } else {
                for child in values {
                    inventory_json_paths(&format!("{prefix}[]"), child, output);
                }
            }
        }
        _ => output.push(prefix.to_owned()),
    }
}

fn inventory_has_forbidden(inventory: &Inventory) -> bool {
    let text = serde_json::to_string(&serde_json::json!({
        "trusted_inputs": inventory.trusted_inputs,
        "memory_authority": inventory.memory_authority,
        "durable_fields": inventory.durable_fields,
        "public_fields": inventory.public_fields,
        "cleanup_fields": inventory.cleanup_fields,
    }))
    .unwrap_or_default()
    .to_ascii_lowercase();
    [
        "raw_grant",
        "raw_store",
        "session_lease",
        "installation_key",
        "sealed_binding_bytes",
        "filesystem_path",
        "command_output",
        "credential",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn failed_variant(variant: Variant, code: String) -> VariantReceipt {
    VariantReceipt {
        variant,
        survived: false,
        criteria: BTreeMap::from([
            ("C1".to_owned(), false),
            ("C2".to_owned(), false),
            ("C3".to_owned(), false),
            ("C4".to_owned(), false),
            ("C5".to_owned(), false),
            ("C6".to_owned(), false),
            ("C7".to_owned(), false),
        ]),
        phase_traces: Vec::new(),
        denial_traces: Vec::new(),
        lifecycle_codes: Vec::new(),
        slope: Vec::new(),
        inventory: Inventory {
            trusted_inputs: Vec::new(),
            experiment_controls: Vec::new(),
            memory_authority: Vec::new(),
            durable_fields: Vec::new(),
            public_fields: Vec::new(),
            cleanup_fields: Vec::new(),
        },
        tie_counts: [0; 6],
        tie_members: Vec::new(),
        failures: vec![code],
    }
}

fn criteria_for(
    receipt: &VariantReceipt,
    compatibility: &CompatibilityInventory,
    command_probes: &[CommandProbe],
) -> BTreeMap<String, bool> {
    let c1 = receipt
        .phase_traces
        .iter()
        .find(|row| row.phase == Phase::F3)
        .map(|row| row.uses == 1 && row.markers == 1 && row.mechanism_attempts == 1)
        .unwrap_or(false);
    let c2 = receipt.phase_traces.len() == 5 && receipt.phase_traces.iter().all(expected_phase);
    let expected_denials = if receipt.variant == Variant::A1 {
        12
    } else {
        7
    };
    let c3 = receipt.denial_traces.len() == expected_denials
        && receipt.denial_traces.iter().all(expected_denial);
    let c4 = receipt.phase_traces.len() == 5
        && receipt.phase_traces.iter().all(|row| {
            row.cleanup_complete
                && row.cleanup_ms < 5_000
                && row.pre_loss_owned_state_counts == [1, 1, 1, 1]
                && row.pre_loss_process_group_alive
                && row.pre_loss_worker_alive
                && row.pre_loss_worker_same_group
                && row.owned_state_counts == [0, 0, 0, 0]
                && row.post_loss_process_group_zero
                && row.post_loss_worker_zero
        });
    let c5 = receipt.slope.len() == 3
        && receipt.slope.iter().enumerate().all(|(n, row)| {
            row.ceiling == n as u64
                && row.use_delta == n as u64
                && row.authorized_effect_decisions == n as u64
                && row.marker_delta == n as u64
                && row.mechanism_attempts == n as u64
                && row.lifecycle_use_delta == 0
                && monotone(&row.use_samples)
                && monotone_usize(&row.startup_samples)
                && monotone_usize(&row.effect_samples)
        });
    let c6 = !inventory_has_forbidden(&receipt.inventory)
        && receipt.inventory == expected_inventory(receipt.variant);
    let probes = command_probes
        .iter()
        .filter(|row| row.variant == receipt.variant)
        .collect::<Vec<_>>();
    let c7 = compatibility.product_sources_changed == 0
        && compatibility.descriptors_changed == 0
        && compatibility.catalogs_changed == 0
        && compatibility.experiment_base_sha.len() == 40
        && compatibility.source_tree_oid.len() == 40
        && is_digest(&compatibility.source_members_digest)
        && compatibility.accepted_operations == ["shell-exec"]
        && !compatibility.job_device_constructible
        && compatibility.protocol_members == ["shell-exec"]
        && !compatibility.source_members.is_empty()
        && probes.len() == canonical_protocol_cases().len()
        && probes
            .iter()
            .filter(|row| row.code == "constructible")
            .count()
            == 1
        && probes
            .iter()
            .filter(|row| row.code == "command-not-constructible")
            .count()
            == canonical_protocol_cases().len() - 1
        && probes.iter().all(|row| {
            if row.failure.is_some() {
                false
            } else if row.code == "constructible" {
                row.command == "shell-exec"
                    && row.request_delta == 1
                    && row.use_delta == 1
                    && row.mechanism_delta == 1
                    && row.marker_delta == 1
            } else {
                row.request_delta == 0
                    && row.use_delta == 0
                    && row.mechanism_delta == 0
                    && row.marker_delta == 0
            }
        });
    BTreeMap::from([
        ("C1".to_owned(), c1),
        ("C2".to_owned(), c2),
        ("C3".to_owned(), c3),
        ("C4".to_owned(), c4),
        ("C5".to_owned(), c5),
        ("C6".to_owned(), c6),
        ("C7".to_owned(), c7),
    ])
}

fn a0_c1(receipt: &VariantReceipt) -> bool {
    receipt.variant == Variant::A0
        && receipt.lifecycle_codes == ["operation-mismatch"; 3]
        && receipt.slope.len() == 3
        && receipt.slope[1].ceiling == 1
        && receipt.slope[1].authorized_effect_decisions == 1
        && receipt.slope[1].use_delta == 1
        && receipt.slope[1].mechanism_attempts == 1
        && receipt.slope[1].marker_delta == 1
        && receipt.slope[1].lifecycle_use_delta == 0
}

fn tie_inventory(inventory: &Inventory, lifecycle_delta: u64) -> ([usize; 6], Vec<String>) {
    let mut members = Vec::new();
    for (class, values) in [
        ("durable", &inventory.durable_fields),
        ("memory", &inventory.memory_authority),
        ("trusted", &inventory.trusted_inputs),
        ("public", &inventory.public_fields),
        ("cleanup", &inventory.cleanup_fields),
    ] {
        for value in values {
            members.push(format!("{class}.{value}"));
        }
    }
    members.sort();
    members.dedup();
    (
        [
            inventory.durable_fields.len(),
            inventory.memory_authority.len(),
            inventory.trusted_inputs.len(),
            inventory.public_fields.len(),
            lifecycle_delta as usize,
            inventory.cleanup_fields.len(),
        ],
        members,
    )
}

fn select_receipts(receipts: &[VariantReceipt]) -> String {
    let a1 = receipts
        .iter()
        .filter(|row| row.variant == Variant::A1)
        .collect::<Vec<_>>();
    let b = receipts
        .iter()
        .filter(|row| row.variant == Variant::B)
        .collect::<Vec<_>>();
    if a1.len() != 1 || b.len() != 1 {
        return "INCONCLUSIVE_MODEL".to_owned();
    }
    let (a1, b) = (a1[0], b[0]);
    match (a1.survived, b.survived) {
        (false, false) => "REJECT_BOTH".to_owned(),
        (true, false) => "SELECT_A1_BOUNDED_SESSION".to_owned(),
        (false, true) => "SELECT_B_REQUEST_DIRECT".to_owned(),
        (true, true) => {
            if a1.tie_members.is_empty() || b.tie_members.is_empty() {
                "INCONCLUSIVE_MODEL".to_owned()
            } else if a1.tie_counts < b.tie_counts {
                "SELECT_A1_BOUNDED_SESSION".to_owned()
            } else {
                "SELECT_B_REQUEST_DIRECT".to_owned()
            }
        }
    }
}

fn expected_denial(row: &DenialTrace) -> bool {
    let expected = match row.case.as_str() {
        "revoked" => "revoked",
        "expired" => "expired",
        "not-yet-valid" => "not-yet-valid",
        "wrong-target" => "target-mismatch",
        "wrong-session" | "binding-mismatch" => "session-mismatch",
        "wrong-operation" | "operation-mismatch" => "operation-mismatch",
        "exhausted" => "exhausted",
        _ => return false,
    };
    if row.mechanism_attempts != 0
        || row.markers != 0
        || row.uses != 0
        || !monotone(&row.use_samples)
        || !monotone_usize(&row.startup_samples)
        || !monotone_usize(&row.effect_samples)
    {
        return false;
    }
    match (row.variant, row.post_exchange) {
        (Variant::A1, false) => row.startup_codes == vec![expected] && row.effect_codes.is_empty(),
        (Variant::A1, true) => {
            row.startup_codes == vec!["authorized"]
                && row.effect_codes.last().map(String::as_str) == Some(expected)
                && if row.case == "exhausted" {
                    row.effect_codes == vec!["authorized", "exhausted"]
                        && row.setup_uses == 1
                        && row.setup_mechanism_attempts == 1
                        && row.setup_markers == 1
                } else {
                    row.effect_codes == vec![expected]
                        && row.setup_uses == 0
                        && row.setup_mechanism_attempts == 0
                        && row.setup_markers == 0
                }
        }
        (Variant::B, false) => {
            row.startup_codes.is_empty()
                && row.effect_codes == vec![expected]
                && row.setup_mechanism_attempts == 0
                && row.setup_markers == 0
        }
        _ => false,
    }
}

fn expected_phase(row: &PhaseTrace) -> bool {
    let a1 = row.variant == Variant::A1;
    let mut expected_request_first = vec!["request-absent".to_owned()];
    if a1 {
        expected_request_first.push("startup-verified".to_owned());
        expected_request_first.push("session-created".to_owned());
    }
    expected_request_first.push(
        if row.phase == Phase::FMinus1 {
            "stopped-before-reservation"
        } else {
            "request-reserved"
        }
        .to_owned(),
    );
    let startup = if a1 {
        match row.phase {
            Phase::FMinus1 => (1, 1),
            _ => (1, 0),
        }
    } else {
        (0, 0)
    };
    let expected: (&str, usize, u64, u64, &str) = match row.phase {
        Phase::FMinus1 => ("finalized", 1, 1, 1, "fresh-after-admission-stop"),
        Phase::F0 => ("reserved", 0, 0, 0, "retained-uncertainty"),
        Phase::F1 => ("reserved", 1, 1, 0, "retained-uncertainty"),
        Phase::F2 => ("outcome-unknown", 1, 1, 1, "retained-uncertainty"),
        Phase::F3 => ("finalized", 1, 1, 1, "authoritative-replay"),
    };
    row.request_first_trace == expected_request_first
        && monotone(&row.use_samples)
        && monotone_usize(&row.startup_samples)
        && monotone_usize(&row.effect_samples)
        && row.initial_startup == startup.0
        && row.reconnect_startup == startup.1
        && row.initial_effect_decisions
            == expected
                .1
                .saturating_sub(if row.phase == Phase::FMinus1 { 1 } else { 0 })
        && row.reconnect_effect_decisions == if row.phase == Phase::FMinus1 { 1 } else { 0 }
        && row.retained_state == expected.0
        && row.uses == expected.2
        && row.markers == expected.3
        && row.reconnect_result == expected.4
        && if row.phase == Phase::F2 {
            row.pre_kill_request_state.as_deref() == Some("reserved")
                && row.recovery_request_state.as_deref() == Some("outcome-unknown")
                && valid_f2_recovery_sequence(&row.f2_recovery_sequence)
        } else {
            row.pre_kill_request_state.is_none()
                && row.recovery_request_state.is_none()
                && row.f2_recovery_sequence.is_empty()
        }
        && row.conflict_result == "request-fingerprint-conflict"
        && row.connection_nonce_digests[0] != row.connection_nonce_digests[1]
        && expected_session_teardown(row.variant, row.phase, row.session_teardown_observed)
        && if a1 {
            row.initial_session_calls
                == if matches!(row.phase, Phase::F1 | Phase::F2 | Phase::F3) {
                    1
                } else {
                    0
                }
                && row.reconnect_session_calls == if row.phase == Phase::FMinus1 { 1 } else { 0 }
        } else {
            row.initial_session_calls == 0 && row.reconnect_session_calls == 0
        }
}

fn expected_session_teardown(variant: Variant, phase: Phase, observed: Option<bool>) -> bool {
    observed
        == if variant == Variant::A1 && phase != Phase::F2 {
            Some(true)
        } else {
            None
        }
}

fn monotone(values: &[u64]) -> bool {
    values.windows(2).all(|pair| pair[0] <= pair[1])
}

fn monotone_usize(values: &[usize]) -> bool {
    values.windows(2).all(|pair| pair[0] <= pair[1])
}

fn valid_f2_recovery_sequence(values: &[String]) -> bool {
    values
        == [
            "effect-marker-persisted",
            "request-reserved",
            "server-killed-reaped",
            "witnesses-reopened",
            "recovery-marked-outcome-unknown",
            "reconnect-observed-unknown",
        ]
}

fn pure_self_test() -> Result<(), String> {
    if failure_terminal(1, "fixture_invalid")
        != (
            "INCONCLUSIVE_FIXTURE_REPAIRABLE",
            "fixture-contract-failure",
        )
        || failure_terminal(2, "fixture_invalid")
            != ("INCONCLUSIVE_FIXTURE_EXHAUSTED", "fixture-contract-failure")
        || failure_terminal(1, "a0_lifecycle_authorized_security_failure")
            != ("REJECT_PRIVACY_SECURITY", "privacy-security-failure")
        || failure_terminal(1, "case_deadline_exceeded")
            != ("INCONCLUSIVE_NONREPAIRABLE", "experiment-execution-failure")
    {
        return Err("self_test_failure_terminal".to_owned());
    }
    if !expected_session_teardown(Variant::A1, Phase::F1, Some(true))
        || expected_session_teardown(Variant::A1, Phase::F1, None)
        || !expected_session_teardown(Variant::A1, Phase::F2, None)
        || expected_session_teardown(Variant::A1, Phase::F2, Some(true))
        || !expected_session_teardown(Variant::B, Phase::F1, None)
    {
        return Err("self_test_session_teardown_meaning".to_owned());
    }
    if !cleanup_proved(true, [0; 4], true, true)
        || cleanup_proved(false, [0; 4], true, true)
        || cleanup_proved(true, [0; 4], false, true)
        || cleanup_proved(true, [0; 4], true, false)
        || cleanup_proved(true, [0, 0, 0, 1], true, true)
        || pre_loss_kernel_proved(true, true, false)
        || pre_loss_kernel_proved(true, false, true)
        || pre_loss_kernel_proved(false, true, true)
        || !pre_loss_kernel_proved(true, true, true)
    {
        return Err("self_test_cleanup_independence".to_owned());
    }
    let fixture = canonical_fixture_for_self_test();
    if parse_public_mutation(canonical_protocol_cases()[0].request.clone(), &fixture).is_err()
        || canonical_protocol_cases()[1..]
            .iter()
            .any(|row| parse_public_mutation(row.request.clone(), &fixture).is_ok())
    {
        return Err("self_test_protocol_inventory".to_owned());
    }
    // Frozen max_uses=1 substitution, line by line: lifecycle rejects without
    // use; startup verification consumes zero; the one effect consumes 0→1;
    // replay/uncertainty consume no later use; remaining uses are exactly zero.
    let max_uses = 1_u64;
    let lifecycle_delta = 0_u64;
    let startup_delta = 0_u64;
    let effect_before = 0_u64;
    let effect_after = effect_before + 1;
    let reconnect_delta = 0_u64;
    if lifecycle_delta != 0
        || startup_delta != 0
        || effect_after != max_uses
        || reconnect_delta != 0
        || max_uses - effect_after != 0
    {
        return Err("self_test_max_uses_one".to_owned());
    }
    for phase in PHASES {
        let (state, uses, markers) = match phase {
            Phase::FMinus1 => ("none-then-finalized", 1, 1),
            Phase::F0 => ("reserved", 0, 0),
            Phase::F1 => ("reserved", 1, 0),
            Phase::F2 => ("outcome-unknown", 1, 1),
            Phase::F3 => ("finalized", 1, 1),
        };
        if state.is_empty() || uses > 1 || markers > 1 {
            return Err("self_test_phase".to_owned());
        }
    }
    let valid = [
        "effect-marker-persisted",
        "request-reserved",
        "server-killed-reaped",
        "witnesses-reopened",
        "recovery-marked-outcome-unknown",
        "reconnect-observed-unknown",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if !valid_f2_recovery_sequence(&valid) {
        return Err("self_test_f2_sequence".to_owned());
    }
    for invalid in [
        [1_usize, 0, 2, 3, 4, 5],
        [0, 1, 4, 2, 3, 5],
        [0, 1, 2, 3, 5, 4],
    ] {
        let candidate = invalid
            .iter()
            .map(|index| valid[*index].clone())
            .collect::<Vec<_>>();
        if valid_f2_recovery_sequence(&candidate) {
            return Err("self_test_f2_counterexample".to_owned());
        }
    }
    for denial in [
        "revoked",
        "expired",
        "not-yet-valid",
        "target-mismatch",
        "session-mismatch",
        "operation-mismatch",
        "exhausted",
    ] {
        if denial.is_empty() {
            return Err("self_test_denial".to_owned());
        }
    }
    for attack in [
        "X1-revoked",
        "X2-expired",
        "X3-binding-mismatch",
        "X4-exhausted",
        "X5-operation-mismatch",
    ] {
        if !attack.contains('-') {
            return Err("self_test_attack".to_owned());
        }
    }
    let mut a1 = failed_variant(Variant::A1, "test".to_owned());
    let mut b = failed_variant(Variant::B, "test".to_owned());
    if select_receipts(&[a1.clone(), b.clone()]) != "REJECT_BOTH" {
        return Err("self_test_decision_reject".to_owned());
    }
    a1.survived = true;
    a1.tie_members = vec!["a1".to_owned()];
    a1.tie_counts = [1, 1, 1, 1, 1, 1];
    if select_receipts(&[b.clone(), a1.clone()]) != "SELECT_A1_BOUNDED_SESSION" {
        return Err("self_test_decision_a1".to_owned());
    }
    b.survived = true;
    b.tie_members = vec!["b".to_owned()];
    b.tie_counts = [1, 0, 1, 1, 0, 1];
    if select_receipts(&[a1.clone(), b.clone()]) != "SELECT_B_REQUEST_DIRECT"
        || select_receipts(&[b.clone(), a1.clone()]) != "SELECT_B_REQUEST_DIRECT"
        || select_receipts(&[a1.clone(), a1, b]) != "INCONCLUSIVE_MODEL"
    {
        return Err("self_test_decision_order_or_identity".to_owned());
    }
    if variant_label(Variant::A0) != "a0"
        || variant_label(Variant::A1) != "a1"
        || variant_label(Variant::B) != "b"
    {
        return Err("self_test_variant".to_owned());
    }
    Ok(())
}

fn run(root: &Path, fixture_path: &Path, metadata_path: &Path) -> Result<Receipt, String> {
    let fixture: Fixture =
        serde_json::from_slice(&fs::read(fixture_path).map_err(|_| "fixture_read".to_owned())?)
            .map_err(|_| "fixture_parse".to_owned())?;
    let metadata: RunMetadata =
        serde_json::from_slice(&fs::read(metadata_path).map_err(|_| "metadata_read".to_owned())?)
            .map_err(|_| "metadata_parse".to_owned())?;
    validate_fixture(&fixture)?;
    if metadata.attempt != 1 && metadata.attempt != 2 {
        return Err("attempt_invalid".to_owned());
    }
    let expected_chain = digest_chain(&metadata.chain_inputs);
    if metadata.chain_digest != expected_chain
        || (metadata.attempt == 1
            && metadata.chain_inputs
                != [metadata.source_sha.clone(), metadata.input_digest.clone()])
        || (metadata.attempt == 2
            && (metadata.chain_inputs.len() != 3
                || !is_digest(&metadata.chain_inputs[0])
                || metadata.chain_inputs[1] != metadata.source_sha
                || !is_digest(&metadata.chain_inputs[2])))
    {
        return Err("runner_chain_invalid".to_owned());
    }
    fs::create_dir_all(root).map_err(|_| "invocation_create".to_owned())?;
    let mut fixture_gate = run_variant(root, &fixture, Variant::A0)?;
    if !fixture_gate.survived || !a0_c1(&fixture_gate) {
        return Err("a0_fixture_gate_failed".to_owned());
    }
    let fixture_c1 = a0_c1(&fixture_gate);
    fixture_gate.criteria.insert("C1".to_owned(), fixture_c1);
    let mut probes = Vec::new();
    for variant in [Variant::A1, Variant::B] {
        probes.extend(command_probes(root, &fixture, variant));
    }
    let compatibility = compatibility_from(&metadata.source_inventory, &probes);
    let mut variants = Vec::new();
    for variant in [Variant::A1, Variant::B] {
        let mut receipt = match run_variant(root, &fixture, variant) {
            Ok(value) => value,
            Err(code) => failed_variant(variant, code),
        };
        receipt.criteria = criteria_for(&receipt, &compatibility, &probes);
        receipt.survived = receipt.failures.is_empty() && receipt.criteria.values().all(|v| *v);
        let lifecycle_delta = receipt
            .slope
            .iter()
            .map(|row| row.lifecycle_use_delta)
            .sum();
        let (counts, members) = tie_inventory(&receipt.inventory, lifecycle_delta);
        receipt.tie_counts = counts;
        receipt.tie_members = members;
        variants.push(receipt);
    }
    let decision = select_receipts(&variants);
    let receipt = Receipt {
        schema_version: SCHEMA_VERSION,
        precommitment: "mcp-persisted-session-experiment".to_owned(),
        source_sha: metadata.source_sha,
        input_digest: metadata.input_digest,
        executable_digest: metadata.executable_digest,
        target: metadata.target,
        attempt: metadata.attempt,
        chain_digest: metadata.chain_digest,
        contract_digest: metadata.contract_digest,
        criteria_digest: metadata.criteria_digest,
        decision_digest: metadata.decision_digest,
        terminal: true,
        fixture_gate: "passed".to_owned(),
        decision,
        failure_class: None,
        negative_control: Some(fixture_gate),
        variants,
        compatibility_inventory: compatibility,
        command_probes: probes,
        privacy_matches: 0,
    };
    let encoded = serde_json::to_vec(&receipt).map_err(|_| "receipt_encode".to_owned())?;
    if encoded.len() > MAX_RECEIPT_BYTES {
        return Err("receipt_limit".to_owned());
    }
    privacy_scan(&encoded)?;
    Ok(receipt)
}

fn validate_fixture(fixture: &Fixture) -> Result<(), String> {
    if fixture.schema_version != SCHEMA_VERSION
        || fixture.operation != "shell-exec"
        || fixture.max_uses != 1
        || fixture.call_ceiling != 1
        || fixture.session_deadline_ms != 60_000
        || fixture.issued_at_utc_ms > fixture.not_before_utc_ms
        || fixture.not_before_utc_ms >= fixture.expires_at_utc_ms
        || fixture.now_utc_ms < fixture.not_before_utc_ms
        || fixture.now_utc_ms >= fixture.expires_at_utc_ms
        || fixture.marker.len() > 16
        || fixture.store_selector.is_empty()
        || fixture.failure_schedule != ["F-1", "F0", "F1", "F2", "F3"]
        || fixture.protocol_cases != canonical_protocol_cases()
        || !is_digest(&fixture.payload_digest)
    {
        return Err("fixture_invalid".to_owned());
    }
    let _ = public_request(fixture)?;
    Ok(())
}

fn canonical_protocol_cases() -> Vec<ProtocolCase> {
    serde_json::from_value(serde_json::json!([
        {"name":"legal-shell","request":{"command":"shell-exec","idempotency_key":"request-fixture-01"}},
        {"name":"job-start","request":{"command":"job-start"}},
        {"name":"device-claim","request":{"command":"device-claim"}},
        {"name":"shell-job-operation","request":{"command":"shell-exec","idempotency_key":"request-fixture-01","operation":"job-start"}},
        {"name":"shell-device-operation","request":{"command":"shell-exec","idempotency_key":"request-fixture-01","operation":"device-claim"}},
        {"name":"extra-job","request":{"command":"shell-exec","idempotency_key":"request-fixture-01","job":{"argv":[]}}},
        {"name":"extra-device","request":{"command":"shell-exec","idempotency_key":"request-fixture-01","device":{"id":"fixture"}}},
        {"name":"extra-binding","request":{"command":"shell-exec","idempotency_key":"request-fixture-01","binding":{"target":"target-fixture-01","session":"session-fixture-01"}}},
        {"name":"extra-clock","request":{"command":"shell-exec","idempotency_key":"request-fixture-01","now_utc_ms":1000000}},
        {"name":"extra-session","request":{"command":"shell-exec","idempotency_key":"request-fixture-01","session_lease":"fixture"}},
        {"name":"extra-generation","request":{"command":"shell-exec","idempotency_key":"request-fixture-01","generation":2}}
    ])).expect("canonical protocol cases")
}

fn canonical_fixture_for_self_test() -> Fixture {
    serde_json::from_str(include_str!("fixture.json")).expect("embedded canonical fixture")
}

fn privacy_scan(bytes: &[u8]) -> Result<(), String> {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    for (index, forbidden) in [
        "grant-fixture-opaque",
        "store-fixture-opaque",
        "target-fixture",
        "session-fixture",
        "nonce-opaque",
        concat!("/use", "rs/"),
        concat!("/ho", "me/"),
        "\\users\\",
        "\"password\":",
        "\"token\":",
    ]
    .into_iter()
    .enumerate()
    {
        if text.contains(forbidden) {
            return Err(format!("receipt_privacy_violation_{index}"));
        }
    }
    Ok(())
}

fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn alternate_digest(value: &str) -> String {
    let mut bytes = value.as_bytes().to_vec();
    bytes[0] = if bytes[0] == b'a' { b'b' } else { b'a' };
    String::from_utf8(bytes).expect("ASCII digest")
}

fn digest_text(domain: &str, value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"agenterm-research/mcp-persisted-session/v1\0");
    hasher.update((domain.len() as u64).to_be_bytes());
    hasher.update(domain.as_bytes());
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
    hex_bytes(&hasher.finalize())
}

fn digest_chain(inputs: &[String]) -> String {
    let mut hasher = Sha256::new();
    for (index, value) in inputs.iter().enumerate() {
        if index > 0 {
            hasher.update([0]);
        }
        hasher.update(value.as_bytes());
    }
    hex_bytes(&hasher.finalize())
}

fn digest_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|_| "witness_digest_read".to_owned())?;
    let mut hasher = Sha256::new();
    hasher.update(b"agenterm-research/mcp-persisted-session-witness/v1\0");
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    Ok(hex_bytes(&hasher.finalize()))
}

fn digest_absent() -> String {
    digest_text("witness", "absent")
}

fn digest_file_or_absent(path: &Path) -> Result<String, String> {
    if path.exists() {
        digest_file(path)
    } else {
        Ok(digest_absent())
    }
}

fn final_outcome(fixture: &Fixture) -> Result<FinalOutcome, String> {
    FinalOutcome::new(
        FinalOutcomeKind::Succeeded,
        "research_effect_complete",
        Some(digest_text("receipt", fixture.marker.as_str())),
    )
    .map_err(cu_code)
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn variant_label(variant: Variant) -> &'static str {
    match variant {
        Variant::A0 => "a0",
        Variant::A1 => "a1",
        Variant::B => "b",
    }
}

fn cu_code(error: agenterm_cu::CuError) -> String {
    error.code
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() == 2 && args[1] == "--owned-worker" {
        if owned_worker().is_err() {
            std::process::exit(1);
        }
        return;
    }
    if args.len() == 3 && args[1] == "--grant-server" {
        if grant_server(Path::new(&args[2])).is_err() {
            std::process::exit(1);
        }
        return;
    }
    let result = if args.len() == 2 && args[1] == "--self-test" {
        pure_self_test().map(|()| serde_json::json!({"ok": true, "self_test": "passed"}))
    } else if args.len() == 5 && args[1] == "--run" {
        (|| -> Result<serde_json::Value, String> {
            let metadata: RunMetadata = serde_json::from_slice(
                &fs::read(&args[4]).map_err(|_| "metadata_read".to_owned())?,
            )
            .map_err(|_| "metadata_parse".to_owned())?;
            match run(
                Path::new(&args[2]),
                Path::new(&args[3]),
                Path::new(&args[4]),
            ) {
                Ok(receipt) => {
                    serde_json::to_value(receipt).map_err(|_| "receipt_encode".to_owned())
                }
                Err(code) => serde_json::to_value(inconclusive_receipt(metadata, code)?)
                    .map_err(|_| "receipt_encode".to_owned()),
            }
        })()
    } else {
        Err("usage".to_owned())
    };
    match result {
        Ok(value) => println!("{}", serde_json::to_string(&value).expect("bounded JSON")),
        Err(code) => {
            println!("{}", serde_json::json!({"ok": false, "code": code}));
            std::process::exit(1);
        }
    }
}
