//! Resident process owner for one native managed job.
//!
//! The public launcher is responsible for reserving `StartIntent`, creating a
//! private inherited pipe, and writing exactly one bounded launch document to
//! it. Command arguments and environment values therefore never appear in the
//! resident owner's argv or environment. This module deliberately exposes no
//! public job verb and no shell or PTY compatibility layer.

use std::{
    collections::{HashSet, VecDeque},
    io::{self, Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use agenterm_platform::{
    contained_process::{
        ContainedChild, ContainedChildInput, ContainedChildOutput, ContainedHeadlessCommand,
        ContainedProcessLimits,
    },
    process::{ProcessExit, start_identity},
};
use serde::{Deserialize, Serialize};

use crate::managed_job_store::{
    ExactProcessIdentity, ManagedJobHandle, ManagedJobStore, ResidentOwnerIdentity,
};

pub(crate) const LAUNCH_SCHEMA_VERSION: u32 = 2;
const LAUNCH_MAX_BYTES: usize = 64 * 1024;
const COMMAND_PARTS_MAX: usize = 256;
const ENVIRONMENT_ENTRIES_MAX: usize = 256;
const OUTPUT_CAPACITY_MIN: usize = 4 * 1024;
const OUTPUT_CAPACITY_MAX: usize = 2 * 1024 * 1024;
const OUTPUT_PAGE_MAX: usize = 64 * 1024;
const STDIN_WRITE_MAX: usize = 64 * 1024;
const LEASE_TTL_MIN_MS: u64 = 1_000;
const LEASE_TTL_MAX_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const WAIT_MAX: Duration = Duration::from_secs(300);
const WAIT_POLL: Duration = Duration::from_millis(10);
const DRAIN_SETTLE_WAIT: Duration = Duration::from_secs(1);
const CLEANUP_WAIT: Duration = Duration::from_secs(5);
pub(crate) const RESOURCE_MEMBERS_MAX: usize = 256;
const RESOURCE_STABILITY_ATTEMPTS: usize = 6;

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
    pub limits: Option<ManagedJobProcessLimits>,
    /// Aggregate retained bytes across stdout and stderr.
    pub output_capacity_bytes: usize,
    /// Resident control lease. It is held only in memory and is never persisted.
    pub lease_ttl_ms: u64,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManagedJobProcessLimits {
    pub cpu_seconds: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub file_size_bytes: Option<u64>,
    pub open_files: Option<u64>,
    pub processes: Option<u32>,
}

impl ManagedJobProcessLimits {
    fn native(self) -> ContainedProcessLimits {
        ContainedProcessLimits {
            cpu_seconds: self.cpu_seconds,
            memory_bytes: self.memory_bytes,
            file_size_bytes: self.file_size_bytes,
            open_files: self.open_files,
            active_processes: self.processes,
        }
    }
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
    pub(crate) const fn new(code: &'static str) -> Self {
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
pub(crate) enum ResidentJobState {
    Running,
    Exited(i32),
    Signaled(u16),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResidentJobStatus {
    pub state: ResidentJobState,
    pub stdin_open: bool,
    pub lease_remaining_ms: u64,
    pub stdout_earliest_cursor: u64,
    pub stdout_current_cursor: u64,
    pub stderr_earliest_cursor: u64,
    pub stderr_current_cursor: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidentResourceMember {
    pub pid: u32,
    pub start_identity: String,
    pub cpu_time_ns: String,
    pub rss_bytes: String,
    pub page_faults_total: String,
    pub page_faults_soft: Option<String>,
    pub page_faults_hard: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidentResourceSnapshot {
    pub provider: String,
    pub breakaway_prevented: bool,
    pub membership_complete: bool,
    pub members: Vec<ResidentResourceMember>,
    pub cpu_time_ns: String,
    pub rss_bytes: String,
    pub page_faults_total: String,
    pub page_faults_soft: Option<String>,
    pub page_faults_hard: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StdinWriteError {
    Limit,
    Closed,
    Owner(ManagedJobOwnerError),
    DeliveryUncertain {
        /// Bytes known to have been accepted before the failing write or flush.
        known_written: usize,
    },
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
    stdin: Option<ContainedChildInput>,
    stdout: SharedRing,
    stderr: SharedRing,
    stdout_drain: Option<JoinHandle<()>>,
    stderr_drain: Option<JoinHandle<()>>,
    finished: bool,
    terminal_report: Option<ManagedJobRunReport>,
    lease_deadline: Instant,
}

impl ResidentJobOwner {
    pub(crate) fn status(&mut self) -> Result<ResidentJobStatus, ManagedJobOwnerError> {
        self.poll_lifecycle()?;
        let stdout = lock_ring(&self.stdout);
        let stderr = lock_ring(&self.stderr);
        let state = match self.terminal_report.as_ref().map(|report| report.terminal) {
            None => ResidentJobState::Running,
            Some(ManagedJobTerminal::Exited(code)) => ResidentJobState::Exited(code),
            Some(ManagedJobTerminal::Signaled(signal)) => ResidentJobState::Signaled(signal),
        };
        Ok(ResidentJobStatus {
            state,
            stdin_open: self.stdin.is_some(),
            lease_remaining_ms: self
                .lease_deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            stdout_earliest_cursor: stdout.earliest_cursor(),
            stdout_current_cursor: stdout.current_cursor,
            stderr_earliest_cursor: stderr.earliest_cursor(),
            stderr_current_cursor: stderr.current_cursor,
        })
    }

    pub(crate) fn resource_snapshot(
        &mut self,
        max_members: usize,
    ) -> Result<ResidentResourceSnapshot, ManagedJobOwnerError> {
        if !(1..=RESOURCE_MEMBERS_MAX).contains(&max_members) {
            return Err(ManagedJobOwnerError::new(
                "managed_job_resource_member_limit",
            ));
        }
        self.poll_lifecycle()?;
        if self.finished {
            return Err(ManagedJobOwnerError::new("managed_job_resources_terminal"));
        }
        for _ in 0..RESOURCE_STABILITY_ATTEMPTS {
            let before = self.resource_member_identities(max_members)?;
            let Some(members) = sample_resource_members(&before.identities)? else {
                continue;
            };
            let after = self.resource_member_identities(max_members)?;
            if before.provider == after.provider
                && before.breakaway_prevented == after.breakaway_prevented
                && before.identities == after.identities
            {
                return aggregate_resource_members(
                    before.provider,
                    before.breakaway_prevented,
                    members,
                );
            }
        }
        Err(ManagedJobOwnerError::new("managed_job_membership_unstable"))
    }

    fn resource_member_identities(
        &self,
        max_members: usize,
    ) -> Result<ResourceMemberIdentities, ManagedJobOwnerError> {
        let containment = self
            .child
            .as_ref()
            .ok_or_else(|| ManagedJobOwnerError::new("managed_job_resources_terminal"))?
            .containment_members(max_members)
            .map_err(|_| ManagedJobOwnerError::new("managed_job_containment_inventory_failed"))?;
        let mut identities = Vec::with_capacity(containment.process_ids.len());
        for pid in containment.process_ids {
            let identity = start_identity(pid)
                .map_err(|_| ManagedJobOwnerError::new("managed_job_member_identity_unknown"))?;
            identities.push(ExactProcessIdentity {
                pid,
                start_identity: identity,
            });
        }
        if !identities.contains(&self.process) {
            return Err(ManagedJobOwnerError::new(
                "managed_job_containment_root_missing",
            ));
        }
        identities.sort_by(|left, right| {
            left.pid
                .cmp(&right.pid)
                .then_with(|| left.start_identity.cmp(&right.start_identity))
        });
        Ok(ResourceMemberIdentities {
            provider: containment.provider.to_owned(),
            breakaway_prevented: containment.breakaway_prevented,
            identities,
        })
    }

    pub(crate) fn write_stdin(&mut self, bytes: &[u8]) -> Result<usize, StdinWriteError> {
        if bytes.len() > STDIN_WRITE_MAX {
            return Err(StdinWriteError::Limit);
        }
        self.poll_lifecycle().map_err(StdinWriteError::Owner)?;
        if self.finished {
            return Err(StdinWriteError::Closed);
        }
        let Some(mut writer) = self.stdin.take() else {
            return Err(StdinWriteError::Closed);
        };
        let known_written = write_exact_and_flush(&mut writer, bytes)
            .map_err(|known_written| StdinWriteError::DeliveryUncertain { known_written })?;
        self.stdin = Some(writer);
        Ok(known_written)
    }

    pub(crate) fn close_stdin(&mut self) -> Result<bool, ManagedJobOwnerError> {
        self.poll_lifecycle()?;
        Ok(self.stdin.take().is_some())
    }

    pub(crate) fn renew(&mut self, ttl_ms: u64) -> Result<u64, ManagedJobOwnerError> {
        validate_lease_ttl(ttl_ms)?;
        self.poll_lifecycle()?;
        if self.finished {
            return Err(ManagedJobOwnerError::new("managed_job_already_terminal"));
        }
        let ttl = Duration::from_millis(ttl_ms);
        self.lease_deadline = Instant::now()
            .checked_add(ttl)
            .ok_or_else(|| ManagedJobOwnerError::new("managed_job_lease_invalid"))?;
        Ok(ttl_ms)
    }

    pub(crate) fn wait(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<ManagedJobRunReport>, ManagedJobOwnerError> {
        if timeout > WAIT_MAX {
            return Err(ManagedJobOwnerError::new("managed_job_wait_limit"));
        }
        let deadline = Instant::now() + timeout;
        loop {
            self.poll_lifecycle()?;
            if let Some(report) = self.terminal_report.clone() {
                return Ok(Some(report));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            thread::sleep(WAIT_POLL.min(deadline.saturating_duration_since(now)));
        }
    }

    pub(crate) fn stop(&mut self) -> Result<ManagedJobRunReport, ManagedJobOwnerError> {
        if let Some(report) = self.terminal_report.clone() {
            return Ok(report);
        }
        if let Some(report) = self.try_finish()? {
            return Ok(report);
        }
        self.stdin.take();
        let _cleanup_result = self
            .child
            .as_mut()
            .ok_or_else(|| ManagedJobOwnerError::new("managed_job_process_state_unknown"))?
            .terminate_and_wait(CLEANUP_WAIT);
        let exit = self
            .child
            .as_mut()
            .ok_or_else(|| ManagedJobOwnerError::new("managed_job_process_state_unknown"))?
            .try_wait()
            .map_err(|_| ManagedJobOwnerError::new("managed_job_process_state_unknown"))?
            .ok_or_else(|| ManagedJobOwnerError::new("managed_job_process_state_unknown"))?;
        // Closing stdin can let the root exit between the pre-stop poll and
        // native tree termination. Preserve that exact observed exit and let
        // `finish_after_exit` retry containment cleanup after the root has
        // become unambiguously dead. We reach this point only after retaining
        // the native terminal result; a still-live root failed above.
        self.finish_after_exit(exit)
    }

    /// Terminal cleanup for the owning runtime session. Ordinary `stop`
    /// retains the resident owner until its output lease expires so callers
    /// can still drain output. Session teardown instead revokes that lease and
    /// requires the native IPC owner itself to disappear after replying.
    pub(crate) fn stop_and_release(&mut self) -> Result<ManagedJobRunReport, ManagedJobOwnerError> {
        let report = self.stop()?;
        self.lease_deadline = Instant::now();
        Ok(report)
    }

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
        if let Some(report) = self.terminal_report.clone() {
            return Ok(Some(report));
        }
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

        self.finish_after_exit(exit).map(Some)
    }

    fn finish_after_exit(
        &mut self,
        exit: ProcessExit,
    ) -> Result<ManagedJobRunReport, ManagedJobOwnerError> {
        self.stdin.take();
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
        let report = ManagedJobRunReport {
            terminal,
            stdout: lock_ring(&self.stdout).snapshot(),
            stderr: lock_ring(&self.stderr).snapshot(),
        };
        self.terminal_report = Some(report.clone());
        Ok(report)
    }

    #[cfg(test)]
    pub(crate) fn run_to_completion(mut self) -> Result<ManagedJobRunReport, ManagedJobOwnerError> {
        loop {
            self.poll_lifecycle()?;
            if let Some(report) = self.terminal_report.clone() {
                return Ok(report);
            }
            thread::sleep(WAIT_POLL);
        }
    }

    fn poll_lifecycle(&mut self) -> Result<(), ManagedJobOwnerError> {
        if self.finished {
            return Ok(());
        }
        if Instant::now() >= self.lease_deadline {
            self.stop().map(|_| ())
        } else {
            self.try_finish().map(|_| ())
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

struct ResourceMemberIdentities {
    provider: String,
    breakaway_prevented: bool,
    identities: Vec<ExactProcessIdentity>,
}

fn sample_resource_members(
    identities: &[ExactProcessIdentity],
) -> Result<Option<Vec<ResidentResourceMember>>, ManagedJobOwnerError> {
    let mut members = Vec::with_capacity(identities.len());
    for expected in identities {
        if start_identity(expected.pid).ok().as_deref() != Some(&expected.start_identity) {
            return Ok(None);
        }
        let metrics = match agenterm_platform::process_metrics::metrics(expected.pid) {
            Ok(metrics) => metrics,
            Err(error)
                if error.kind()
                    == agenterm_platform::process_metrics::ProcessMetricsErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(_) => {
                return Err(ManagedJobOwnerError::new(
                    "managed_job_member_metrics_failed",
                ));
            }
        };
        if start_identity(expected.pid).ok().as_deref() != Some(&expected.start_identity) {
            return Ok(None);
        }
        members.push(ResidentResourceMember {
            pid: expected.pid,
            start_identity: expected.start_identity.clone(),
            cpu_time_ns: metrics.cpu_time.as_nanos().to_string(),
            rss_bytes: metrics.resident_bytes.to_string(),
            page_faults_total: metrics.page_faults.total.to_string(),
            page_faults_soft: metrics.page_faults.soft.map(|value| value.to_string()),
            page_faults_hard: metrics.page_faults.hard.map(|value| value.to_string()),
        });
    }
    Ok(Some(members))
}

fn aggregate_resource_members(
    provider: String,
    breakaway_prevented: bool,
    members: Vec<ResidentResourceMember>,
) -> Result<ResidentResourceSnapshot, ManagedJobOwnerError> {
    let mut cpu_time_ns = 0_u128;
    let mut rss_bytes = 0_u128;
    let mut page_faults_total = 0_u128;
    let mut page_faults_soft = Some(0_u128);
    let mut page_faults_hard = Some(0_u128);
    for member in &members {
        cpu_time_ns = checked_resource_sum(cpu_time_ns, &member.cpu_time_ns)?;
        rss_bytes = checked_resource_sum(rss_bytes, &member.rss_bytes)?;
        page_faults_total = checked_resource_sum(page_faults_total, &member.page_faults_total)?;
        page_faults_soft =
            checked_optional_resource_sum(page_faults_soft, member.page_faults_soft.as_deref())?;
        page_faults_hard =
            checked_optional_resource_sum(page_faults_hard, member.page_faults_hard.as_deref())?;
    }
    Ok(ResidentResourceSnapshot {
        provider,
        breakaway_prevented,
        membership_complete: true,
        members,
        cpu_time_ns: cpu_time_ns.to_string(),
        rss_bytes: rss_bytes.to_string(),
        page_faults_total: page_faults_total.to_string(),
        page_faults_soft: page_faults_soft.map(|value| value.to_string()),
        page_faults_hard: page_faults_hard.map(|value| value.to_string()),
    })
}

fn checked_resource_sum(total: u128, value: &str) -> Result<u128, ManagedJobOwnerError> {
    value
        .parse::<u128>()
        .ok()
        .and_then(|value| total.checked_add(value))
        .ok_or_else(|| ManagedJobOwnerError::new("managed_job_resource_counter_invalid"))
}

fn checked_optional_resource_sum(
    total: Option<u128>,
    value: Option<&str>,
) -> Result<Option<u128>, ManagedJobOwnerError> {
    match (total, value) {
        (Some(total), Some(value)) => checked_resource_sum(total, value).map(Some),
        _ => Ok(None),
    }
}

fn write_exact_and_flush(writer: &mut impl Write, bytes: &[u8]) -> Result<usize, usize> {
    let mut known_written = 0;
    while known_written < bytes.len() {
        match writer.write(&bytes[known_written..]) {
            Ok(0) => return Err(known_written),
            Ok(count) => known_written += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(known_written),
        }
    }
    writer.flush().map_err(|_| known_written)?;
    Ok(known_written)
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
#[cfg(test)]
pub(crate) fn start_owner_from_reader(
    reader: impl Read,
) -> Result<ResidentJobOwner, ManagedJobOwnerError> {
    let launch = read_launch(reader)?;
    start_owner_from_launch(launch)
}

/// Claim and start one launch that was already decoded and whose native
/// control endpoint has already been bound by the resident runtime.
pub(crate) fn start_owner_from_launch(
    launch: ManagedJobLaunch,
) -> Result<ResidentJobOwner, ManagedJobOwnerError> {
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

    let stdin_stream = child.take_stdin();
    let Some(stdin_stream) = stdin_stream else {
        let cleanup = child.terminate_and_wait(CLEANUP_WAIT);
        if cleanup.is_ok() {
            let _ = store.mark_start_failed(
                &launch.handle,
                &owner,
                "stdin_stream_unavailable",
                now_utc_ms()?,
            );
        }
        return Err(ManagedJobOwnerError::new(
            "managed_job_stdin_stream_unavailable",
        ));
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
        stdin: Some(stdin_stream),
        stdout,
        stderr,
        stdout_drain: Some(stdout_drain),
        stderr_drain: Some(stderr_drain),
        finished: false,
        terminal_report: None,
        lease_deadline: Instant::now()
            .checked_add(Duration::from_millis(launch.lease_ttl_ms))
            .ok_or_else(|| ManagedJobOwnerError::new("managed_job_lease_invalid"))?,
    })
}

/// Current synchronous internal entry point. A later detached-owner command
/// can pass its inherited pipe here without placing launch data in argv/env.
#[cfg(test)]
pub(crate) fn run_owner(reader: impl Read) -> Result<ManagedJobRunReport, ManagedJobOwnerError> {
    start_owner_from_reader(reader)?.run_to_completion()
}

pub(crate) fn read_launch(mut reader: impl Read) -> Result<ManagedJobLaunch, ManagedJobOwnerError> {
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
        || validate_lease_ttl(launch.lease_ttl_ms).is_err()
        || launch
            .limits
            .is_some_and(|limits| limits.native().validate().is_err())
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
    command
        .args(&launch.arguments)
        .pipe_stdin()
        .capture_output();
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
    if let Some(limits) = launch.limits {
        command.limits(limits.native());
    }
    command
}

fn validate_lease_ttl(ttl_ms: u64) -> Result<(), ManagedJobOwnerError> {
    if (LEASE_TTL_MIN_MS..=LEASE_TTL_MAX_MS).contains(&ttl_ms) {
        Ok(())
    } else {
        Err(ManagedJobOwnerError::new("managed_job_lease_invalid"))
    }
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

pub(crate) fn now_utc_ms() -> Result<i64, ManagedJobOwnerError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ManagedJobOwnerError::new("managed_job_clock_invalid"))?
        .as_millis();
    i64::try_from(millis).map_err(|_| ManagedJobOwnerError::new("managed_job_clock_invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Cursor};

    struct PartialWriter {
        remaining: usize,
    }

    impl Write for PartialWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "fixture failure"));
            }
            let count = self.remaining.min(bytes.len());
            self.remaining -= count;
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn partial_stdin_failure_reports_only_the_known_lower_bound() {
        assert_eq!(
            write_exact_and_flush(&mut PartialWriter { remaining: 2 }, b"abcdef"),
            Err(2)
        );
    }

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
            limits: None,
            output_capacity_bytes: 16 * 1024,
            lease_ttl_ms: 60_000,
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

    #[test]
    fn resource_aggregation_is_lossless_and_preserves_unavailable_subcounters() {
        let snapshot = aggregate_resource_members(
            "test-containment".into(),
            false,
            vec![
                ResidentResourceMember {
                    pid: 2,
                    start_identity: "two".into(),
                    cpu_time_ns: "1000001".into(),
                    rss_bytes: "20".into(),
                    page_faults_total: "3".into(),
                    page_faults_soft: Some("2".into()),
                    page_faults_hard: Some("1".into()),
                },
                ResidentResourceMember {
                    pid: 3,
                    start_identity: "three".into(),
                    cpu_time_ns: "2000002".into(),
                    rss_bytes: "40".into(),
                    page_faults_total: "5".into(),
                    page_faults_soft: None,
                    page_faults_hard: Some("4".into()),
                },
            ],
        )
        .expect("aggregate resources");
        assert_eq!(snapshot.cpu_time_ns, "3000003");
        assert_eq!(snapshot.rss_bytes, "60");
        assert_eq!(snapshot.page_faults_total, "8");
        assert_eq!(snapshot.page_faults_soft, None);
        assert_eq!(snapshot.page_faults_hard.as_deref(), Some("5"));
        assert!(snapshot.membership_complete);
        assert!(!snapshot.breakaway_prevented);
    }
}
