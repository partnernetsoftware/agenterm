//! Resident process owner for one native managed job.
//!
//! The public launcher is responsible for reserving `StartIntent`, creating a
//! private inherited pipe, and writing exactly one bounded launch document to
//! it. Command arguments and environment values therefore never appear in the
//! resident owner's argv or environment. This module deliberately exposes no
//! public job verb and no shell or PTY compatibility layer.

use std::{
    collections::{HashSet, VecDeque},
    io::{self, Read},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use agenterm_platform::{
    contained_process::{ContainedChild, ContainedChildOutput, ContainedHeadlessCommand},
    process::{ProcessExit, start_identity},
};
use serde::{Deserialize, Serialize};

use crate::managed_job_store::{
    ExactProcessIdentity, ManagedJobHandle, ManagedJobStore, ResidentOwnerIdentity,
};

const LAUNCH_SCHEMA_VERSION: u32 = 1;
const LAUNCH_MAX_BYTES: usize = 64 * 1024;
const COMMAND_PARTS_MAX: usize = 256;
const ENVIRONMENT_ENTRIES_MAX: usize = 256;
const OUTPUT_CAPACITY_MIN: usize = 4 * 1024;
const OUTPUT_CAPACITY_MAX: usize = 2 * 1024 * 1024;
const OUTPUT_PAGE_MAX: usize = 64 * 1024;
const WAIT_POLL: Duration = Duration::from_millis(10);
const DRAIN_SETTLE_WAIT: Duration = Duration::from_secs(1);
const CLEANUP_WAIT: Duration = Duration::from_secs(5);

/// One same-host launch document sent over a private inherited byte stream.
///
/// Do not derive `Debug`: program arguments and environment values may contain
/// secrets. The document is closed-schema and bounded before deserialization.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManagedJobLaunch {
    pub schema_version: u32,
    pub state_path: PathBuf,
    pub handle: ManagedJobHandle,
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub current_directory: Option<PathBuf>,
    pub environment: Vec<ManagedJobEnvironment>,
    /// Aggregate retained bytes across stdout and stderr.
    pub output_capacity_bytes: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManagedJobEnvironment {
    pub name: String,
    /// `None` removes an inherited variable; `Some` sets it.
    pub value: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManagedJobOwnerError {
    pub code: &'static str,
}

impl ManagedJobOwnerError {
    const fn new(code: &'static str) -> Self {
        Self { code }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedJobTerminal {
    Exited(i32),
    Signaled(u16),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManagedJobRunReport {
    pub terminal: ManagedJobTerminal,
    pub stdout: OutputSnapshot,
    pub stderr: OutputSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutputSnapshot {
    pub earliest_cursor: u64,
    pub current_cursor: u64,
    pub retained: Vec<u8>,
    pub finalized: bool,
    pub read_error: Option<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(dead_code, reason = "used by the next resident IPC slice")]
pub(crate) struct OutputPage {
    pub cursor: u64,
    pub next_cursor: u64,
    pub current_cursor: u64,
    pub bytes: Vec<u8>,
    pub finalized: bool,
    pub read_error: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code, reason = "used by the next resident IPC slice")]
pub(crate) enum OutputCursorError {
    RetentionGap { earliest_cursor: u64 },
    FutureCursor { current_cursor: u64 },
    PageLimit,
}

#[derive(Debug)]
struct CursorRing {
    capacity: usize,
    bytes: VecDeque<u8>,
    current_cursor: u64,
    finalized: bool,
    read_error: Option<&'static str>,
}

impl CursorRing {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            bytes: VecDeque::with_capacity(capacity),
            current_cursor: 0,
            finalized: false,
            read_error: None,
        }
    }

    fn append(&mut self, incoming: &[u8]) -> io::Result<()> {
        let incoming_len = u64::try_from(incoming.len())
            .map_err(|_| io::Error::other("managed-job output cursor overflow"))?;
        self.current_cursor = self
            .current_cursor
            .checked_add(incoming_len)
            .ok_or_else(|| io::Error::other("managed-job output cursor overflow"))?;
        if incoming.len() >= self.capacity {
            self.bytes.clear();
            self.bytes
                .extend(incoming[incoming.len() - self.capacity..].iter().copied());
            return Ok(());
        }
        let overflow = self
            .bytes
            .len()
            .saturating_add(incoming.len())
            .saturating_sub(self.capacity);
        self.bytes.drain(..overflow);
        self.bytes.extend(incoming.iter().copied());
        Ok(())
    }

    fn finish(&mut self, read_error: Option<&'static str>) {
        self.finalized = true;
        self.read_error = read_error;
    }

    fn earliest_cursor(&self) -> u64 {
        self.current_cursor
            .saturating_sub(u64::try_from(self.bytes.len()).unwrap_or(u64::MAX))
    }

    fn snapshot(&self) -> OutputSnapshot {
        OutputSnapshot {
            earliest_cursor: self.earliest_cursor(),
            current_cursor: self.current_cursor,
            retained: self.bytes.iter().copied().collect(),
            finalized: self.finalized,
            read_error: self.read_error,
        }
    }

    #[allow(dead_code, reason = "used by the next resident IPC slice")]
    fn page(&self, cursor: u64, limit: usize) -> Result<OutputPage, OutputCursorError> {
        if !(1..=OUTPUT_PAGE_MAX).contains(&limit) {
            return Err(OutputCursorError::PageLimit);
        }
        let earliest = self.earliest_cursor();
        if cursor < earliest {
            return Err(OutputCursorError::RetentionGap {
                earliest_cursor: earliest,
            });
        }
        if cursor > self.current_cursor {
            return Err(OutputCursorError::FutureCursor {
                current_cursor: self.current_cursor,
            });
        }
        let offset = usize::try_from(cursor - earliest).unwrap_or(self.bytes.len());
        let bytes: Vec<u8> = self
            .bytes
            .iter()
            .skip(offset)
            .take(limit)
            .copied()
            .collect();
        let next_cursor = cursor + u64::try_from(bytes.len()).unwrap_or(0);
        Ok(OutputPage {
            cursor,
            next_cursor,
            current_cursor: self.current_cursor,
            bytes,
            finalized: self.finalized && next_cursor == self.current_cursor,
            read_error: self.read_error,
        })
    }
}

type SharedRing = Arc<Mutex<CursorRing>>;

/// A contained child plus the two drain owners that keep its pipes live.
///
/// Later IPC wiring can retain this object and expose `output_page`; the
/// current internal entry point simply drives it to completion.
pub(crate) struct ResidentJobOwner {
    store: ManagedJobStore,
    handle: ManagedJobHandle,
    owner: ResidentOwnerIdentity,
    process: ExactProcessIdentity,
    child: Option<ContainedChild>,
    stdout: SharedRing,
    stderr: SharedRing,
    stdout_drain: Option<JoinHandle<()>>,
    stderr_drain: Option<JoinHandle<()>>,
    finished: bool,
}

impl ResidentJobOwner {
    #[allow(dead_code, reason = "used by the next resident IPC slice")]
    pub(crate) fn output_page(
        &self,
        stderr: bool,
        cursor: u64,
        limit: usize,
    ) -> Result<OutputPage, OutputCursorError> {
        let ring = if stderr { &self.stderr } else { &self.stdout };
        lock_ring(ring).page(cursor, limit)
    }

    pub(crate) fn try_finish(
        &mut self,
    ) -> Result<Option<ManagedJobRunReport>, ManagedJobOwnerError> {
        let exit = match self
            .child
            .as_mut()
            .ok_or_else(|| ManagedJobOwnerError::new("managed_job_owner_finished"))?
            .try_wait()
        {
            Ok(Some(exit)) => exit,
            Ok(None) => return Ok(None),
            Err(_) => {
                return Err(ManagedJobOwnerError::new(
                    "managed_job_process_state_unknown",
                ));
            }
        };

        // Root exit does not prove descendants closed inherited pipe handles.
        // Close the native containment owner before joining either reader.
        self.child
            .as_mut()
            .expect("checked above")
            .terminate_and_wait(CLEANUP_WAIT)
            .map_err(|_| ManagedJobOwnerError::new("managed_job_cleanup_unknown"))?;
        self.settle_drains()?;
        self.join_drains();

        let terminal = terminal_from_exit(exit)?;
        let now = now_utc_ms()?;
        match terminal {
            ManagedJobTerminal::Exited(exit_code) => {
                self.store
                    .mark_exited(&self.handle, &self.owner, &self.process, exit_code, now)
            }
            ManagedJobTerminal::Signaled(signal) => {
                self.store
                    .mark_signaled(&self.handle, &self.owner, &self.process, signal, now)
            }
        }
        .map_err(|_| ManagedJobOwnerError::new("managed_job_terminal_publish_failed"))?;

        self.finished = true;
        self.child.take();
        Ok(Some(ManagedJobRunReport {
            terminal,
            stdout: lock_ring(&self.stdout).snapshot(),
            stderr: lock_ring(&self.stderr).snapshot(),
        }))
    }

    pub(crate) fn run_to_completion(mut self) -> Result<ManagedJobRunReport, ManagedJobOwnerError> {
        loop {
            if let Some(report) = self.try_finish()? {
                return Ok(report);
            }
            thread::sleep(WAIT_POLL);
        }
    }

    fn join_drains(&mut self) {
        join_drain(&self.stdout, self.stdout_drain.take());
        join_drain(&self.stderr, self.stderr_drain.take());
    }

    fn settle_drains(&self) -> Result<(), ManagedJobOwnerError> {
        let deadline = std::time::Instant::now() + DRAIN_SETTLE_WAIT;
        while !self.drains_finalized() && std::time::Instant::now() < deadline {
            thread::sleep(WAIT_POLL);
        }
        if self.drains_finalized() {
            Ok(())
        } else {
            Err(ManagedJobOwnerError::new(
                "managed_job_output_completion_unknown",
            ))
        }
    }

    fn drains_finalized(&self) -> bool {
        lock_ring(&self.stdout).finalized && lock_ring(&self.stderr).finalized
    }
}

impl Drop for ResidentJobOwner {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let cleanup_known = self
            .child
            .as_mut()
            .is_none_or(|child| child.terminate_and_wait(CLEANUP_WAIT).is_ok());
        if cleanup_known && self.settle_drains().is_ok() {
            self.join_drains();
        } else {
            // Dropping JoinHandle detaches the readers. A failed containment
            // cleanup must not turn owner shutdown into an unbounded join on
            // pipe handles that an unknown descendant may still own.
            self.stdout_drain.take();
            self.stderr_drain.take();
        }
        // Deliberately do not invent a terminal state here. If cleanup or wait
        // lost the native result, the durable Running record is reconciled as
        // orphaned_uncertain only after this exact owner identity is observed
        // dead. Unknown is never converted to success or stale.
    }
}

/// Read and validate one sealed launch, claim its intent, then spawn contained.
pub(crate) fn start_owner_from_reader(
    reader: impl Read,
) -> Result<ResidentJobOwner, ManagedJobOwnerError> {
    let launch = read_launch(reader)?;
    let store = ManagedJobStore::open_at(&launch.state_path)
        .map_err(|_| ManagedJobOwnerError::new("managed_job_store_unavailable"))?;
    let owner = ResidentOwnerIdentity {
        pid: std::process::id(),
        start_identity: start_identity(std::process::id())
            .map_err(|_| ManagedJobOwnerError::new("managed_job_owner_identity_unknown"))?,
    };
    store
        .claim_starting(&launch.handle, owner.clone(), now_utc_ms()?)
        .map_err(|_| ManagedJobOwnerError::new("managed_job_intent_claim_failed"))?;

    let command = build_contained_command(&launch);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            let _ = store.mark_start_failed(
                &launch.handle,
                &owner,
                "contained_spawn_failed",
                now_utc_ms()?,
            );
            return Err(ManagedJobOwnerError::new("managed_job_spawn_failed"));
        }
    };

    let stdout = Arc::new(Mutex::new(CursorRing::new(
        launch.output_capacity_bytes.div_ceil(2),
    )));
    let stderr = Arc::new(Mutex::new(CursorRing::new(
        launch.output_capacity_bytes / 2,
    )));
    let stdout_stream = child.take_stdout();
    let stderr_stream = child.take_stderr();
    let (Some(stdout_stream), Some(stderr_stream)) = (stdout_stream, stderr_stream) else {
        let cleanup = child.terminate_and_wait(CLEANUP_WAIT);
        if cleanup.is_ok() {
            let _ = store.mark_start_failed(
                &launch.handle,
                &owner,
                "capture_stream_unavailable",
                now_utc_ms()?,
            );
        }
        return Err(ManagedJobOwnerError::new(
            "managed_job_capture_stream_unavailable",
        ));
    };
    let stdout_drain = match spawn_drain("managed-job-stdout", stdout_stream, Arc::clone(&stdout)) {
        Ok(drain) => drain,
        Err(error) => {
            let cleanup = child.terminate_and_wait(CLEANUP_WAIT);
            if cleanup.is_ok() {
                let _ = store.mark_start_failed(
                    &launch.handle,
                    &owner,
                    "output_drain_unavailable",
                    now_utc_ms()?,
                );
            }
            return Err(error);
        }
    };
    let stderr_drain = match spawn_drain("managed-job-stderr", stderr_stream, Arc::clone(&stderr)) {
        Ok(drain) => drain,
        Err(error) => {
            let cleanup = child.terminate_and_wait(CLEANUP_WAIT);
            if cleanup.is_ok() {
                let _ = stdout_drain.join();
                let _ = store.mark_start_failed(
                    &launch.handle,
                    &owner,
                    "output_drain_unavailable",
                    now_utc_ms()?,
                );
            }
            return Err(error);
        }
    };

    let process = ExactProcessIdentity {
        pid: child.id(),
        start_identity: match start_identity(child.id()) {
            Ok(identity) => identity,
            Err(_) => {
                let cleanup = child.terminate_and_wait(CLEANUP_WAIT);
                if cleanup.is_ok() {
                    let _ = stdout_drain.join();
                    let _ = stderr_drain.join();
                    let _ = store.mark_start_failed(
                        &launch.handle,
                        &owner,
                        "child_identity_unavailable",
                        now_utc_ms()?,
                    );
                }
                return Err(ManagedJobOwnerError::new(
                    "managed_job_child_identity_unknown",
                ));
            }
        },
    };
    if store
        .mark_running(&launch.handle, &owner, process.clone(), now_utc_ms()?)
        .is_err()
    {
        if child.terminate_and_wait(CLEANUP_WAIT).is_ok() {
            let _ = stdout_drain.join();
            let _ = stderr_drain.join();
        }
        return Err(ManagedJobOwnerError::new(
            "managed_job_running_publish_failed",
        ));
    }

    Ok(ResidentJobOwner {
        store,
        handle: launch.handle,
        owner,
        process,
        child: Some(child),
        stdout,
        stderr,
        stdout_drain: Some(stdout_drain),
        stderr_drain: Some(stderr_drain),
        finished: false,
    })
}

/// Current synchronous internal entry point. A later detached-owner command
/// can pass its inherited pipe here without placing launch data in argv/env.
pub(crate) fn run_owner(reader: impl Read) -> Result<ManagedJobRunReport, ManagedJobOwnerError> {
    start_owner_from_reader(reader)?.run_to_completion()
}

fn read_launch(mut reader: impl Read) -> Result<ManagedJobLaunch, ManagedJobOwnerError> {
    let mut bytes = Vec::with_capacity(LAUNCH_MAX_BYTES.min(4096));
    reader
        .by_ref()
        .take((LAUNCH_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ManagedJobOwnerError::new("managed_job_launch_read_failed"))?;
    if bytes.len() > LAUNCH_MAX_BYTES {
        return Err(ManagedJobOwnerError::new("managed_job_launch_too_large"));
    }
    let launch: ManagedJobLaunch = serde_json::from_slice(&bytes)
        .map_err(|_| ManagedJobOwnerError::new("managed_job_launch_invalid"))?;
    validate_launch(&launch)?;
    Ok(launch)
}

fn validate_launch(launch: &ManagedJobLaunch) -> Result<(), ManagedJobOwnerError> {
    if launch.schema_version != LAUNCH_SCHEMA_VERSION
        || !launch.state_path.is_absolute()
        || !launch.program.is_absolute()
        || launch.arguments.len() > COMMAND_PARTS_MAX
        || launch.environment.len() > ENVIRONMENT_ENTRIES_MAX
        || !(OUTPUT_CAPACITY_MIN..=OUTPUT_CAPACITY_MAX).contains(&launch.output_capacity_bytes)
        || launch
            .current_directory
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
    {
        return Err(ManagedJobOwnerError::new("managed_job_launch_invalid"));
    }
    if launch.program.as_os_str().is_empty()
        || launch.program.as_os_str().as_encoded_bytes().contains(&0)
        || launch
            .arguments
            .iter()
            .any(|value| value.as_bytes().contains(&0))
        || launch
            .current_directory
            .as_ref()
            .is_some_and(|path| path.as_os_str().as_encoded_bytes().contains(&0))
    {
        return Err(ManagedJobOwnerError::new("managed_job_launch_invalid"));
    }
    let mut names = HashSet::with_capacity(launch.environment.len());
    for entry in &launch.environment {
        if entry.name.is_empty()
            || entry.name.as_bytes().contains(&0)
            || entry.name.as_bytes().contains(&b'=')
            || entry
                .value
                .as_ref()
                .is_some_and(|value| value.as_bytes().contains(&0))
            || !names.insert(entry.name.as_str())
        {
            return Err(ManagedJobOwnerError::new("managed_job_launch_invalid"));
        }
    }
    Ok(())
}

fn build_contained_command(launch: &ManagedJobLaunch) -> ContainedHeadlessCommand {
    let mut command = ContainedHeadlessCommand::new(&launch.program);
    command.args(&launch.arguments).capture_output();
    if let Some(directory) = &launch.current_directory {
        command.current_dir(directory);
    }
    for entry in &launch.environment {
        match &entry.value {
            Some(value) => {
                command.env(&entry.name, value);
            }
            None => {
                command.env_remove(&entry.name);
            }
        }
    }
    command
}

fn spawn_drain(
    name: &'static str,
    mut stream: ContainedChildOutput,
    ring: SharedRing,
) -> Result<JoinHandle<()>, ManagedJobOwnerError> {
    thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let mut buffer = [0_u8; 16 * 1024];
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) => {
                        lock_ring(&ring).finish(None);
                        return;
                    }
                    Ok(count) => {
                        if lock_ring(&ring).append(&buffer[..count]).is_err() {
                            lock_ring(&ring).finish(Some("managed_job_output_cursor_overflow"));
                            return;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => {
                        lock_ring(&ring).finish(Some("managed_job_output_read_failed"));
                        return;
                    }
                }
            }
        })
        .map_err(|_| ManagedJobOwnerError::new("managed_job_output_drain_unavailable"))
}

fn join_drain(ring: &SharedRing, drain: Option<JoinHandle<()>>) {
    if drain.is_some_and(|drain| drain.join().is_err()) {
        lock_ring(ring).finish(Some("managed_job_output_drain_panicked"));
    }
}

fn lock_ring(ring: &SharedRing) -> MutexGuard<'_, CursorRing> {
    ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn terminal_from_exit(exit: ProcessExit) -> Result<ManagedJobTerminal, ManagedJobOwnerError> {
    match exit {
        ProcessExit::Code(code) => Ok(ManagedJobTerminal::Exited(code)),
        ProcessExit::Signal(signal) => u16::try_from(signal)
            .map(ManagedJobTerminal::Signaled)
            .map_err(|_| ManagedJobOwnerError::new("managed_job_process_state_unknown")),
        ProcessExit::Unavailable => Err(ManagedJobOwnerError::new(
            "managed_job_process_state_unknown",
        )),
        _ => Err(ManagedJobOwnerError::new(
            "managed_job_process_state_unknown",
        )),
    }
}

fn now_utc_ms() -> Result<i64, ManagedJobOwnerError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ManagedJobOwnerError::new("managed_job_clock_invalid"))?
        .as_millis();
    i64::try_from(millis).map_err(|_| ManagedJobOwnerError::new("managed_job_clock_invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Cursor, Write as _},
    };

    fn test_directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agenterm-managed-job-owner-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test clock")
                .as_nanos()
        ));
        fs::create_dir(&path).expect("create test directory");
        path.canonicalize().expect("canonicalize test directory")
    }

    fn launch_fixture(state_path: PathBuf, handle: ManagedJobHandle) -> ManagedJobLaunch {
        ManagedJobLaunch {
            schema_version: LAUNCH_SCHEMA_VERSION,
            state_path,
            handle,
            program: std::env::current_exe().expect("test executable"),
            arguments: vec![
                "--exact".into(),
                "managed_job_owner::tests::contained_output_probe".into(),
                "--ignored".into(),
                "--nocapture".into(),
            ],
            current_directory: None,
            environment: Vec::new(),
            output_capacity_bytes: 16 * 1024,
        }
    }

    #[test]
    #[ignore = "spawned by the owner lifecycle test"]
    fn contained_output_probe() {
        let stdout = io::stdout();
        let stderr = io::stderr();
        let mut stdout = stdout.lock();
        let mut stderr = stderr.lock();
        let stdout_block = [b'O'; 1024];
        let stderr_block = [b'E'; 1024];
        for _ in 0..256 {
            stdout.write_all(&stdout_block).expect("write stdout block");
            stderr.write_all(&stderr_block).expect("write stderr block");
        }
        stdout
            .write_all(b"STDOUT-END")
            .expect("write stdout marker");
        stderr
            .write_all(b"STDERR-END")
            .expect("write stderr marker");
    }

    #[test]
    fn cursor_ring_reports_retention_and_future_gaps() {
        let mut ring = CursorRing::new(4);
        ring.append(b"abcdef").expect("append");
        assert_eq!(ring.earliest_cursor(), 2);
        assert_eq!(
            ring.page(0, 4),
            Err(OutputCursorError::RetentionGap { earliest_cursor: 2 })
        );
        assert_eq!(
            ring.page(7, 4),
            Err(OutputCursorError::FutureCursor { current_cursor: 6 })
        );
        assert_eq!(ring.page(2, 2).expect("page").bytes, b"cd");
        ring.finish(None);
        assert!(ring.page(4, 4).expect("final page").finalized);
    }

    #[test]
    fn sealed_launch_is_bounded_closed_schema_and_rejects_duplicate_environment() {
        assert_eq!(
            read_launch(Cursor::new(vec![b'x'; LAUNCH_MAX_BYTES + 1]))
                .err()
                .expect("oversize launch")
                .code,
            "managed_job_launch_too_large"
        );
        assert_eq!(
            read_launch(Cursor::new(br#"{"schema_version":1,"unknown":true}"#))
                .err()
                .expect("unknown field")
                .code,
            "managed_job_launch_invalid"
        );

        let directory = test_directory("invalid-env");
        let store = ManagedJobStore::open_at(directory.join("jobs.json")).expect("open store");
        let record = store.reserve_start(None, 1).expect("reserve start");
        let mut launch = launch_fixture(directory.join("jobs.json"), record.handle());
        launch.environment = vec![
            ManagedJobEnvironment {
                name: "DUPLICATE".into(),
                value: Some("one".into()),
            },
            ManagedJobEnvironment {
                name: "DUPLICATE".into(),
                value: Some("two".into()),
            },
        ];
        let bytes = serde_json::to_vec(&launch).expect("serialize launch");
        assert_eq!(
            read_launch(Cursor::new(bytes))
                .err()
                .expect("duplicate environment")
                .code,
            "managed_job_launch_invalid"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn contained_child_drains_both_streams_and_publishes_exact_terminal_identity() {
        let directory = test_directory("lifecycle");
        let state_path = directory.join("jobs.json");
        let store = ManagedJobStore::open_at(&state_path).expect("open store");
        let record = store
            .reserve_start(None, now_utc_ms().expect("clock"))
            .expect("reserve");
        let launch = launch_fixture(state_path, record.handle());
        let report = run_owner(Cursor::new(
            serde_json::to_vec(&launch).expect("serialize launch"),
        ))
        .expect("run owner");
        assert_eq!(report.terminal, ManagedJobTerminal::Exited(0));
        assert!(
            report
                .stdout
                .retained
                .windows(b"STDOUT-END".len())
                .any(|window| window == b"STDOUT-END")
        );
        assert!(
            report
                .stderr
                .retained
                .windows(b"STDERR-END".len())
                .any(|window| window == b"STDERR-END")
        );
        assert!(report.stdout.finalized);
        assert!(report.stderr.finalized);
        assert!(report.stdout.current_cursor > report.stdout.retained.len() as u64);
        assert!(report.stderr.current_cursor > report.stderr.retained.len() as u64);

        let stored = store
            .get(&record.job_id)
            .expect("read store")
            .expect("job record");
        assert_eq!(
            stored.state,
            crate::managed_job_store::ManagedJobState::Exited { exit_code: 0 }
        );
        assert_eq!(stored.owner.expect("owner").pid, std::process::id());
        assert!(stored.process.expect("process").pid != 0);
        fs::remove_dir_all(directory).expect("remove test directory");
    }
}
