//! Bounded request/reply protocol for one resident managed-job owner.
//!
//! Transport binding is deliberately outside this module. The caller must pass
//! a stream already accepted by `agenterm-platform`'s current-user native IPC
//! transport (Unix socket or Windows named pipe). TCP is not a valid carrier.
//! Requests never contain a bearer/session lease, and replies never echo job
//! launch arguments, environment values, or stdin bytes.

use std::{
    io::{self, Read, Write},
    time::{Duration, Instant},
};

use agenterm_platform::ipc::{
    IpcEndpoint, IpcTransportError, IpcTransportErrorCode, NativeListener,
};
use agenterm_platform::{entropy::secure_random_array, ipc::NativeStream};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::command::JobResourcePolicy;
use crate::deadline_frame_io::{self, FrameIoError, FrameLimits};
use crate::managed_job_owner::{
    ManagedJobOwnerError, OutputCursorError, RESOURCE_MEMBERS_MAX, ResidentJobOwner,
    ResidentJobState, ResidentJobStatus, ResidentPriorityResult, ResidentResourcePolicyReply,
    ResidentResourceSnapshot, StdinWriteError, now_utc_ms, read_launch, start_owner_from_launch,
};
use crate::managed_job_store::{ManagedJobHandle, ManagedJobStore};

const SCHEMA_VERSION: u32 = 3;
pub(crate) const FRAME_MAX_BYTES: usize = 128 * 1024;
const REQUEST_ID_MAX_BYTES: usize = 128;
const STDIN_BYTES_MAX: usize = 64 * 1024;
const OUTPUT_BYTES_MAX: usize = 64 * 1024;
const WAIT_MAX_MS: u64 = 300_000;
const ACCEPT_TICK: Duration = Duration::from_millis(100);
const STREAM_TIMEOUT: Duration = Duration::from_secs(305);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// One owner-side wait slice inside a cooperative `job-wait` call.
///
/// The wait is sliced so a call-scoped cancellation probe is sampled at a round
/// boundary instead of after one long blocking exchange. This constant and
/// [`WAIT_ROUND_MARGIN`] are the frozen pair behind [`WAIT_ROUND_BUDGET`].
pub(crate) const WAIT_QUANTUM: Duration = Duration::from_millis(25);

/// One cooperative round's transport margin above the caller's owner-side wait.
///
/// This margin is the extra time one complete round may spend connecting,
/// writing, flushing and reading before it is declared inconclusive. It is
/// deliberately *not* a per-fragment allowance: the round's single absolute
/// deadline is recomputed from the remaining budget, never reset.
pub(crate) const WAIT_ROUND_MARGIN: Duration = Duration::from_millis(25);

/// The frozen absolute budget for one **production** round: connect, every write
/// fragment, flush, every read fragment and the owner-side wait together.
///
/// `executor::managed_jobs::drive_wait` is the only place that computes a round
/// deadline, and it computes exactly `min(remaining, WAIT_QUANTUM) +
/// WAIT_ROUND_MARGIN`, so no round can exceed this value. A test in that module
/// records the observed per-round bound and asserts it; a harness that needs a
/// larger outer deadline must say so explicitly instead of widening this one.
/// The sum stays far below the worker's hard-cancel grace (`cancel_grace_ms:
/// 150` in `src/script_catalog.rs`).
pub(crate) const WAIT_ROUND_BUDGET: Duration = Duration::from_millis(50);

/// Protocol code for a round that exhausted its own absolute budget.
///
/// It is deliberately distinct from `managed_job_protocol_io`: a round that ran
/// out of its own short budget is *inconclusive*, so the caller must sample
/// cancellation and the caller deadline instead of reporting a transport error.
pub(crate) const ROUND_TIMEOUT_CODE: &str = "managed_job_round_timeout";

pub(crate) fn run_resident(reader: impl Read) -> Result<(), ManagedJobOwnerError> {
    let launch = read_launch(reader)?;
    let store = ManagedJobStore::open_at(&launch.state_path)
        .map_err(|_| ManagedJobOwnerError::new("managed_job_store_unavailable"))?;
    let endpoint = endpoint_for(&launch.handle)?;
    // Binding precedes the durable Starting transition and contained spawn. A
    // live job can therefore never be published without a control listener.
    let mut listener = match NativeListener::bind(&endpoint) {
        Ok(listener) => listener,
        Err(_) => {
            let _ = store.mark_unclaimed_start_failed(
                &launch.handle,
                "managed_job_endpoint_unavailable",
                now_utc_ms()?,
            );
            return Err(ManagedJobOwnerError::new(
                "managed_job_endpoint_unavailable",
            ));
        }
    };
    let mut owner = start_owner_from_launch(launch)?;
    loop {
        let status = owner.status()?;
        if !matches!(status.state, ResidentJobState::Running) && status.lease_remaining_ms == 0 {
            return Ok(());
        }
        match listener.accept(ACCEPT_TICK) {
            Ok(mut stream) => {
                if stream.set_io_timeout(STREAM_TIMEOUT).is_err() {
                    continue;
                }
                if serve_authenticated_request(&mut owner, &mut stream).is_ok() {
                    let _ = stream.finish_server_response();
                }
            }
            Err(error) if error.code == IpcTransportErrorCode::AcceptTimeout => {}
            Err(_) => {
                return Err(ManagedJobOwnerError::new(
                    "managed_job_endpoint_unavailable",
                ));
            }
        }
    }
}

pub(crate) fn endpoint_for(handle: &ManagedJobHandle) -> Result<IpcEndpoint, ManagedJobOwnerError> {
    let canonical = serde_json::to_vec(handle)
        .map_err(|_| ManagedJobOwnerError::new("managed_job_endpoint_invalid"))?;
    // A 128-bit opaque suffix is collision-resistant for a local 1024-record
    // registry while leaving enough room under Darwin's short sun_path limit.
    let suffix: String = Sha256::digest(&canonical)[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    #[cfg(windows)]
    {
        Ok(IpcEndpoint::NamedPipe(format!(
            r"\\.\pipe\agenterm-cu-job-{suffix}"
        )))
    }
    #[cfg(unix)]
    {
        let path = agenterm_platform::ipc::native_runtime_directory()
            .join(format!("cu-job-{suffix}.sock"));
        Ok(IpcEndpoint::UnixSocket(path.to_string_lossy().into_owned()))
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManagedJobRequest {
    pub schema_version: u32,
    pub request_id: String,
    pub operation: ManagedJobOperation,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub(crate) enum ManagedJobOperation {
    Status,
    Resources {
        max_members: usize,
    },
    Priority {
        nice: i32,
    },
    ResourcePolicyStatus,
    ResourcePolicySet {
        policy: JobResourcePolicy,
    },
    ResourcePolicyClear,
    Output {
        stream: OutputStream,
        cursor: u64,
        max_bytes: usize,
    },
    Write {
        data_base64: String,
        close_stdin: bool,
    },
    CloseStdin,
    Wait {
        timeout_ms: u64,
    },
    Stop,
    StopAndRelease,
    Renew {
        ttl_ms: u64,
    },
}

/// Bounds one complete request/reply exchange with the resident owner.
#[derive(Clone, Copy)]
enum IoBudget {
    /// The bounded legacy budget: a short connect plus the long stream timeout.
    Blocking,
    /// One cooperative round: connect, every write and read fragment, and flush
    /// all share this single absolute deadline.
    Before(Instant),
}

pub(crate) fn client_request(
    handle: &ManagedJobHandle,
    operation: ManagedJobOperation,
) -> Result<ManagedJobResult, ManagedJobProtocolError> {
    request_once(handle, operation, IoBudget::Blocking)
}

/// Sends one request whose connect, write, flush and read must all complete
/// before `deadline`.
///
/// The caller keeps its own observation cadence: when this returns
/// [`ROUND_TIMEOUT_CODE`] the round was inconclusive rather than failed, so a
/// cancellation probe sampled at the round boundary still owns the outcome.
/// The stream is dropped on return, so no request, stream or helper thread of
/// this round outlives the call.
pub(crate) fn client_request_before(
    handle: &ManagedJobHandle,
    operation: ManagedJobOperation,
    deadline: Instant,
) -> Result<ManagedJobResult, ManagedJobProtocolError> {
    request_once(handle, operation, IoBudget::Before(deadline))
}

fn request_once(
    handle: &ManagedJobHandle,
    operation: ManagedJobOperation,
    budget: IoBudget,
) -> Result<ManagedJobResult, ManagedJobProtocolError> {
    validate_operation(&operation).map_err(protocol_error)?;
    let endpoint = endpoint_for(handle).map_err(|error| protocol_error(error.code))?;
    let request_id = random_request_id()?;
    let request = ManagedJobRequest {
        schema_version: SCHEMA_VERSION,
        request_id: request_id.clone(),
        operation,
    };
    let encoded =
        serde_json::to_vec(&request).map_err(|_| protocol_error("managed_job_request_invalid"))?;
    let reply_bytes = exchange(&endpoint, &encoded, budget)?;
    decode_reply(&reply_bytes, &request_id)
}

fn exchange(
    endpoint: &IpcEndpoint,
    encoded: &[u8],
    budget: IoBudget,
) -> Result<Vec<u8>, ManagedJobProtocolError> {
    match budget {
        IoBudget::Blocking => {
            let mut stream = NativeStream::connect(endpoint, CONNECT_TIMEOUT)
                .map_err(|_| protocol_error("managed_job_owner_unavailable"))?;
            stream
                .set_io_timeout(STREAM_TIMEOUT)
                .map_err(|_| protocol_error("managed_job_protocol_io"))?;
            write_frame(&mut stream, encoded)
                .map_err(|_| protocol_error("managed_job_protocol_io"))?;
            read_frame(&mut stream)
                .map_err(|_| protocol_error("managed_job_protocol_io"))?
                .ok_or_else(|| protocol_error("managed_job_response_missing"))
        }
        IoBudget::Before(deadline) => {
            // The connect call gets the smaller of its own bounded attempt and
            // whatever is left, so a cancellation round is never held by a
            // connect that outlives the round budget.
            let remaining_before_connect =
                deadline_frame_io::remaining(deadline).map_err(round_io)?;
            let connect = remaining_before_connect.min(CONNECT_TIMEOUT);
            let mut stream = NativeStream::connect(endpoint, connect).map_err(|error| {
                protocol_error(classify_round_connect_failure(
                    remaining_before_connect.is_zero(),
                    &error,
                ))
            })?;
            deadline_frame_io::prepare(&stream)
                .map_err(|_| protocol_error("managed_job_protocol_io"))?;
            let limits = FrameLimits {
                max_bytes: FRAME_MAX_BYTES,
                little_endian: false,
            };
            deadline_frame_io::write_frame_before(&mut stream, encoded, deadline, limits)
                .map_err(round_io)?;
            deadline_frame_io::flush_before(&mut stream, deadline).map_err(round_io)?;
            deadline_frame_io::read_frame_before(&mut stream, deadline, limits).map_err(round_io)
        }
    }
}

/// Classifies a failed `Before`-budget connect.
///
/// A connect that ran out of the round's own budget is **inconclusive**: the
/// owner may be healthy and merely slow to accept, so it carries the
/// round-timeout code and the caller keeps sampling cancellation and its own
/// deadline. Only a real transport refusal — no listener, a refused connection,
/// an unusable endpoint — means the owner is unavailable. Unix also surfaces an
/// in-progress nonblocking connect as `WouldBlock` (`EAGAIN`/`EINPROGRESS`),
/// which is another "not yet refused" result rather than a missing owner.
pub(crate) fn classify_round_connect_failure(
    budget_exhausted: bool,
    error: &IpcTransportError,
) -> &'static str {
    if budget_exhausted
        || error.code == IpcTransportErrorCode::ConnectTimeout
        || matches!(
            error.io_kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        )
    {
        ROUND_TIMEOUT_CODE
    } else {
        "managed_job_owner_unavailable"
    }
}

fn round_io(error: FrameIoError) -> ManagedJobProtocolError {
    match error {
        FrameIoError::Deadline => protocol_error(ROUND_TIMEOUT_CODE),
        FrameIoError::Eof | FrameIoError::FrameSize | FrameIoError::Io(_) => {
            protocol_error("managed_job_protocol_io")
        }
    }
}

fn decode_reply(
    reply_bytes: &[u8],
    request_id: &str,
) -> Result<ManagedJobResult, ManagedJobProtocolError> {
    let reply: ManagedJobReply = serde_json::from_slice(reply_bytes)
        .map_err(|_| protocol_error("managed_job_response_invalid"))?;
    if reply.schema_version != SCHEMA_VERSION
        || reply.request_id != request_id
        || reply.ok != reply.result.is_some()
        || reply.ok == reply.error.is_some()
    {
        return Err(protocol_error("managed_job_response_invalid"));
    }
    match (reply.result, reply.error) {
        (Some(result), None) => Ok(result),
        (None, Some(error)) => Err(error),
        _ => Err(protocol_error("managed_job_response_invalid")),
    }
}

fn random_request_id() -> Result<String, ManagedJobProtocolError> {
    let bytes = secure_random_array::<16>()
        .map_err(|_| protocol_error("managed_job_request_entropy_unavailable"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn protocol_error(code: impl Into<String>) -> ManagedJobProtocolError {
    ManagedJobProtocolError {
        code: code.into(),
        known_written_lower_bound: None,
        delivery_uncertain: None,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManagedJobReply {
    pub schema_version: u32,
    pub request_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ManagedJobResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ManagedJobProtocolError>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub(crate) enum ManagedJobResult {
    Status {
        status: JobStatus,
    },
    Resources {
        snapshot: ResidentResourceSnapshot,
    },
    Priority {
        result: ResidentPriorityResult,
    },
    ResourcePolicy {
        result: ResidentResourcePolicyReply,
    },
    Output {
        stream: OutputStream,
        cursor: u64,
        next_cursor: u64,
        current_cursor: u64,
        data_base64: String,
        finalized: bool,
        read_error: Option<String>,
    },
    Write {
        accepted_bytes: usize,
        delivery: WriteDelivery,
    },
    CloseStdin {
        was_open: bool,
    },
    Wait {
        completed: bool,
        status: JobStatus,
    },
    Stop {
        status: JobStatus,
    },
    Renew {
        renewed_ttl_ms: u64,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WriteDelivery {
    Complete,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JobStatus {
    pub state: JobState,
    pub adopted: bool,
    pub stdin_open: bool,
    pub lease_remaining_ms: u64,
    pub stdout_earliest_cursor: u64,
    pub stdout_current_cursor: u64,
    pub stderr_earliest_cursor: u64,
    pub stderr_current_cursor: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub(crate) enum JobState {
    Running,
    Exited { exit_code: i32 },
    Signaled { signal: u16 },
    Detached,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManagedJobProtocolError {
    pub code: String,
    /// Present only for stdin delivery uncertainty. The failed operation must
    /// not be retried: more bytes may have reached the child than this bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub known_written_lower_bound: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_uncertain: Option<bool>,
}

/// Serve one bounded frame over an already authenticated local-native stream.
///
/// Each frame is a four-byte big-endian length followed by one closed-schema
/// JSON document. The native stream must already have a bounded I/O timeout.
/// One request per connection prevents an idle client from suppressing the
/// owner's accept-loop lease/exit checks.
pub(crate) fn serve_authenticated_request(
    owner: &mut ResidentJobOwner,
    stream: &mut (impl Read + Write),
) -> io::Result<()> {
    let frame = read_frame(stream)?.ok_or_else(|| {
        io::Error::new(io::ErrorKind::UnexpectedEof, "managed-job request missing")
    })?;
    let reply = match decode_request(&frame) {
        Ok(request) => dispatch(owner, request),
        Err(code) => failure(String::new(), code, None),
    };
    let encoded = serde_json::to_vec(&reply)
        .map_err(|_| io::Error::other("managed-job reply serialization failed"))?;
    write_frame(stream, &encoded)
}

fn decode_request(bytes: &[u8]) -> Result<ManagedJobRequest, &'static str> {
    if bytes.is_empty() || bytes.len() > FRAME_MAX_BYTES {
        return Err("managed_job_request_size_invalid");
    }
    let request: ManagedJobRequest =
        serde_json::from_slice(bytes).map_err(|_| "managed_job_request_invalid")?;
    if request.schema_version != SCHEMA_VERSION
        || request.request_id.is_empty()
        || request.request_id.len() > REQUEST_ID_MAX_BYTES
        || request.request_id.chars().any(char::is_control)
    {
        return Err("managed_job_request_invalid");
    }
    validate_operation(&request.operation)?;
    Ok(request)
}

fn validate_operation(operation: &ManagedJobOperation) -> Result<(), &'static str> {
    match operation {
        ManagedJobOperation::Resources { max_members }
            if !(1..=RESOURCE_MEMBERS_MAX).contains(max_members) =>
        {
            return Err("managed_job_resource_member_limit");
        }
        ManagedJobOperation::Priority { nice } if !(-20..=19).contains(nice) => {
            return Err("managed_job_priority_invalid");
        }
        ManagedJobOperation::ResourcePolicySet { policy } => policy
            .validate()
            .map_err(|_| "managed_job_policy_invalid")?,
        ManagedJobOperation::Output { max_bytes, .. } => {
            if !(1..=OUTPUT_BYTES_MAX).contains(max_bytes) {
                return Err("managed_job_output_limit");
            }
        }
        ManagedJobOperation::Write { data_base64, .. } => {
            if data_base64.len() > encoded_len(STDIN_BYTES_MAX) {
                return Err("managed_job_stdin_limit");
            }
        }
        ManagedJobOperation::Wait { timeout_ms } if *timeout_ms > WAIT_MAX_MS => {
            return Err("managed_job_wait_limit");
        }
        _ => {}
    }
    Ok(())
}

fn dispatch(owner: &mut ResidentJobOwner, request: ManagedJobRequest) -> ManagedJobReply {
    let request_id = request.request_id;
    let result = match request.operation {
        ManagedJobOperation::Status => owner.status().map(|status| ManagedJobResult::Status {
            status: status.into(),
        }),
        ManagedJobOperation::Resources { max_members } => owner
            .resource_snapshot(max_members)
            .map(|snapshot| ManagedJobResult::Resources { snapshot }),
        ManagedJobOperation::Priority { nice } => owner
            .set_priority(nice)
            .map(|result| ManagedJobResult::Priority { result }),
        ManagedJobOperation::ResourcePolicyStatus => owner
            .resource_policy_status()
            .map(|result| ManagedJobResult::ResourcePolicy { result }),
        ManagedJobOperation::ResourcePolicySet { policy } => owner
            .set_resource_policy(policy)
            .map(|result| ManagedJobResult::ResourcePolicy { result }),
        ManagedJobOperation::ResourcePolicyClear => owner
            .clear_resource_policy()
            .map(|result| ManagedJobResult::ResourcePolicy { result }),
        ManagedJobOperation::Output {
            stream,
            cursor,
            max_bytes,
        } => {
            if let Err(error) = owner.status() {
                Err(error)
            } else {
                owner
                    .output_page(matches!(stream, OutputStream::Stderr), cursor, max_bytes)
                    .map(|page| ManagedJobResult::Output {
                        stream,
                        cursor: page.cursor,
                        next_cursor: page.next_cursor,
                        current_cursor: page.current_cursor,
                        data_base64: base64_encode(&page.bytes),
                        finalized: page.finalized,
                        read_error: page.read_error.map(str::to_owned),
                    })
                    .map_err(cursor_error)
            }
        }
        ManagedJobOperation::Write {
            data_base64,
            close_stdin,
        } => match base64_decode(&data_base64) {
            Ok(bytes) if bytes.len() <= STDIN_BYTES_MAX => match owner.write_stdin(&bytes) {
                Ok(accepted_bytes) => {
                    let result = ManagedJobResult::Write {
                        accepted_bytes,
                        delivery: WriteDelivery::Complete,
                    };
                    if close_stdin {
                        owner.close_stdin().map(|_| result)
                    } else {
                        Ok(result)
                    }
                }
                Err(StdinWriteError::DeliveryUncertain { known_written }) => {
                    return failure(
                        request_id,
                        "managed_job_stdin_write_uncertain",
                        Some(known_written),
                    );
                }
                Err(StdinWriteError::Limit) => {
                    Err(ManagedJobOwnerError::new("managed_job_stdin_limit"))
                }
                Err(StdinWriteError::Closed) => {
                    Err(ManagedJobOwnerError::new("managed_job_stdin_closed"))
                }
                Err(StdinWriteError::Owner(error)) => Err(error),
            },
            Ok(_) => Err(ManagedJobOwnerError::new("managed_job_stdin_limit")),
            Err(()) => Err(ManagedJobOwnerError::new(
                "managed_job_stdin_encoding_invalid",
            )),
        },
        ManagedJobOperation::CloseStdin => owner
            .close_stdin()
            .map(|was_open| ManagedJobResult::CloseStdin { was_open }),
        ManagedJobOperation::Wait { timeout_ms } => owner
            .wait(Duration::from_millis(timeout_ms))
            .and_then(|report| {
                owner.status().map(|status| ManagedJobResult::Wait {
                    completed: report.is_some(),
                    status: status.into(),
                })
            }),
        ManagedJobOperation::Stop => owner.stop().and_then(|_| {
            owner.status().map(|status| ManagedJobResult::Stop {
                status: status.into(),
            })
        }),
        ManagedJobOperation::StopAndRelease => owner.stop_and_release().and_then(|_| {
            owner.status().map(|status| ManagedJobResult::Stop {
                status: status.into(),
            })
        }),
        ManagedJobOperation::Renew { ttl_ms } => owner
            .renew(ttl_ms)
            .map(|renewed_ttl_ms| ManagedJobResult::Renew { renewed_ttl_ms }),
    };
    match result {
        Ok(result) => success(request_id, result),
        Err(error) => failure(request_id, error.code, None),
    }
}

impl From<ResidentJobStatus> for JobStatus {
    fn from(status: ResidentJobStatus) -> Self {
        Self {
            state: match status.state {
                ResidentJobState::Running => JobState::Running,
                ResidentJobState::Exited(exit_code) => JobState::Exited { exit_code },
                ResidentJobState::Signaled(signal) => JobState::Signaled { signal },
                ResidentJobState::Detached => JobState::Detached,
            },
            adopted: status.adopted,
            stdin_open: status.stdin_open,
            lease_remaining_ms: status.lease_remaining_ms,
            stdout_earliest_cursor: status.stdout_earliest_cursor,
            stdout_current_cursor: status.stdout_current_cursor,
            stderr_earliest_cursor: status.stderr_earliest_cursor,
            stderr_current_cursor: status.stderr_current_cursor,
        }
    }
}

fn cursor_error(error: OutputCursorError) -> ManagedJobOwnerError {
    match error {
        OutputCursorError::RetentionGap { .. } => {
            ManagedJobOwnerError::new("managed_job_output_retention_gap")
        }
        OutputCursorError::FutureCursor { .. } => {
            ManagedJobOwnerError::new("managed_job_output_future_cursor")
        }
        OutputCursorError::PageLimit => ManagedJobOwnerError::new("managed_job_output_limit"),
    }
}

fn success(request_id: String, result: ManagedJobResult) -> ManagedJobReply {
    ManagedJobReply {
        schema_version: SCHEMA_VERSION,
        request_id,
        ok: true,
        result: Some(result),
        error: None,
    }
}

fn failure(
    request_id: String,
    code: impl Into<String>,
    known_written: Option<usize>,
) -> ManagedJobReply {
    ManagedJobReply {
        schema_version: SCHEMA_VERSION,
        request_id,
        ok: false,
        result: None,
        error: Some(ManagedJobProtocolError {
            code: code.into(),
            known_written_lower_bound: known_written,
            delivery_uncertain: known_written.map(|_| true),
        }),
    }
}

fn read_frame(reader: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut length = [0_u8; 4];
    loop {
        match reader.read(&mut length[..1]) {
            Ok(0) => return Ok(None),
            Ok(1) => break,
            Ok(_) => unreachable!(),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    reader.read_exact(&mut length[1..])?;
    let length = deadline_frame_io::frame_length(length, false);
    if length == 0 || length > FRAME_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed-job frame size invalid",
        ));
    }
    let mut bytes = vec![0_u8; length];
    reader.read_exact(&mut bytes)?;
    Ok(Some(bytes))
}

fn write_frame(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    if bytes.is_empty() || bytes.len() > FRAME_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed-job reply frame size invalid",
        ));
    }
    let length = deadline_frame_io::frame_length_u32(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "reply too large"))?;
    writer.write_all(&deadline_frame_io::frame_header(length, false))?;
    writer.write_all(bytes)?;
    writer.flush()
}

fn encoded_len(bytes: usize) -> usize {
    bytes.div_ceil(3) * 4
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(encoded_len(bytes.len()));
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[(a >> 2) as usize] as char);
        output.push(TABLE[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(c & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}

pub(crate) fn base64_decode(value: &str) -> Result<Vec<u8>, ()> {
    if !value.len().is_multiple_of(4) || !value.is_ascii() {
        return Err(());
    }
    let mut output = Vec::with_capacity(value.len() / 4 * 3);
    for (chunk_index, chunk) in value.as_bytes().chunks_exact(4).enumerate() {
        let final_chunk = chunk_index + 1 == value.len() / 4;
        let a = base64_value(chunk[0])?;
        let b = base64_value(chunk[1])?;
        let c = if chunk[2] == b'=' {
            if !final_chunk || chunk[3] != b'=' {
                return Err(());
            }
            0
        } else {
            base64_value(chunk[2])?
        };
        let d = if chunk[3] == b'=' {
            if !final_chunk {
                return Err(());
            }
            0
        } else {
            base64_value(chunk[3])?
        };
        output.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            output.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            output.push((c << 6) | d);
        }
    }
    Ok(output)
}

fn base64_value(byte: u8) -> Result<u8, ()> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::Cursor,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    /// The cooperative round writer must emit the same wire bytes as the legacy
    /// blocking path: one big-endian length prefix followed by the body. A body
    /// written without its prefix makes the owner read the body's first four
    /// bytes as a length, fail framing, and drop the connection.
    #[cfg(unix)]
    #[test]
    fn deadline_frame_writer_prefixes_the_body_on_the_wire() {
        use std::io::Read as _;

        use agenterm_platform::ipc::NativeStreamExt as _;

        let (left, right) = std::os::unix::net::UnixStream::pair().expect("frame socket pair");
        let endpoint = IpcEndpoint::UnixSocket("/tmp/agenterm-cu-frame-test".to_owned());
        let mut writer =
            NativeStream::from_owned_fd(left.into(), &endpoint, Duration::from_secs(1))
                .expect("writer stream");
        let mut wire = right;
        let deadline = Instant::now() + Duration::from_secs(5);
        let body = br#"{"schema_version":3}"#;
        let limits = FrameLimits {
            max_bytes: FRAME_MAX_BYTES,
            little_endian: false,
        };
        deadline_frame_io::prepare(&writer).expect("deadline-bounded mode");
        deadline_frame_io::write_frame_before(&mut writer, body, deadline, limits)
            .expect("deadline frame write");
        deadline_frame_io::flush_before(&mut writer, deadline).expect("deadline frame flush");

        let mut header = [0_u8; 4];
        wire.read_exact(&mut header).expect("length prefix");
        assert_eq!(
            deadline_frame_io::frame_length(header, false),
            body.len(),
            "the cooperative round must prefix its body with its exact length"
        );
        let mut received = vec![0_u8; body.len()];
        wire.read_exact(&mut received).expect("frame body");
        assert_eq!(received, body);
    }

    use crate::{
        managed_job_owner::{
            LAUNCH_SCHEMA_VERSION, ManagedJobEnvironment, ManagedJobLaunch, ManagedJobTerminal,
            start_owner_from_reader,
        },
        managed_job_store::ManagedJobStore,
    };

    fn test_handle(nonce: char) -> ManagedJobHandle {
        ManagedJobHandle {
            job_id: "00000000-0000-4000-8000-000000000001".into(),
            generation: 1,
            nonce: nonce.to_string().repeat(32),
        }
    }

    fn test_directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agenterm-managed-job-ipc-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test clock")
                .as_nanos()
        ));
        fs::create_dir(&path).expect("create test directory");
        path.canonicalize().expect("canonicalize test directory")
    }

    fn owner_fixture(label: &str) -> (PathBuf, ResidentJobOwner) {
        let directory = test_directory(label);
        let state_path = directory.join("jobs.json");
        let store = ManagedJobStore::open_at(&state_path).expect("open store");
        let now = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_millis(),
        )
        .expect("clock range");
        let record = store.reserve_start(None, now).expect("reserve");
        let launch = ManagedJobLaunch {
            schema_version: LAUNCH_SCHEMA_VERSION,
            state_path,
            handle: record.handle(),
            program: std::env::current_exe().expect("test executable"),
            arguments: vec![
                "--exact".into(),
                "managed_job_ipc::tests::binary_stdio_probe".into(),
                "--ignored".into(),
                "--nocapture".into(),
            ],
            current_directory: None,
            environment: Vec::<ManagedJobEnvironment>::new(),
            limits: None,
            adoption: None,
            on_expiry: crate::managed_job_store::ManagedJobOnExpiry::Stop,
            output_capacity_bytes: 32 * 1024,
            lease_ttl_ms: 60_000,
        };
        let owner = start_owner_from_reader(Cursor::new(
            serde_json::to_vec(&launch).expect("serialize launch"),
        ))
        .expect("start owner");
        (directory, owner)
    }

    #[test]
    #[ignore = "spawned by the IPC lifecycle test"]
    fn binary_stdio_probe() {
        let mut input = Vec::new();
        io::stdin().read_to_end(&mut input).expect("read stdin");
        io::stdout().write_all(b"OUT:").expect("stdout prefix");
        io::stdout().write_all(&input).expect("stdout input");
        io::stderr().write_all(b"ERR-END").expect("stderr marker");
    }

    #[test]
    fn request_schema_and_frames_are_bounded_and_closed() {
        assert_eq!(
            decode_request(
                br#"{"schema_version":1,"request_id":"r","operation":{"kind":"status"},"extra":1}"#
            )
            .expect_err("unknown field"),
            "managed_job_request_invalid"
        );
        let oversized = vec![b'x'; FRAME_MAX_BYTES + 1];
        assert_eq!(
            decode_request(&oversized).expect_err("oversized"),
            "managed_job_request_size_invalid"
        );
        assert!(base64_decode("AA=A").is_err());
        assert_eq!(
            base64_decode(&base64_encode(&[0, 255, 1])).unwrap(),
            [0, 255, 1]
        );
        assert_eq!(
            validate_operation(&ManagedJobOperation::Resources { max_members: 0 })
                .expect_err("zero members"),
            "managed_job_resource_member_limit"
        );
        assert!(
            validate_operation(&ManagedJobOperation::Resources {
                max_members: RESOURCE_MEMBERS_MAX
            })
            .is_ok()
        );
        assert_eq!(
            validate_operation(&ManagedJobOperation::Priority { nice: 20 })
                .expect_err("nice ceiling"),
            "managed_job_priority_invalid"
        );
    }

    #[test]
    fn endpoint_is_native_opaque_bounded_and_generation_specific() {
        let first = endpoint_for(&test_handle('a')).expect("first endpoint");
        let second = endpoint_for(&test_handle('b')).expect("second endpoint");
        assert_ne!(first, second);
        assert_ne!(first.transport_name(), "tcp");
        let rendered = first.to_string();
        assert!(!rendered.contains("00000000"));
        assert!(!rendered.contains(&"a".repeat(32)));
        #[cfg(unix)]
        assert!(
            first
                .unix_socket_path()
                .and_then(|path| path.file_name().map(|name| name.len()))
                .is_some_and(|length| length < 48)
        );
        #[cfg(windows)]
        assert!(rendered.starts_with(r"pipe:\\.\pipe\agenterm-cu-job-"));
    }

    /// Item 2 of the CU-JW1 ruling: the cost of a **real** resident owner, not a
    /// socket emulation.
    ///
    /// The owner is the production `ResidentJobOwner` from `owner_fixture` (a
    /// real child process plus a real durable store), served over its real
    /// native endpoint the way `run_resident` serves it. Only semantics and the
    /// cancellation bound are asserted; cost values stay diagnostics so a loaded
    /// host cannot turn them into a brittle gate.
    #[cfg(unix)]
    #[test]
    fn resident_owner_wait_cost_and_status_latency_are_measured() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let (directory, mut owner) = owner_fixture("jw1-cost");
        let store = ManagedJobStore::open_at(directory.join("jobs.json")).expect("open store");
        let handle = store
            .list()
            .expect("list records")
            .into_iter()
            .next()
            .expect("the fixture reserves exactly one job")
            .handle();
        let endpoint = endpoint_for(&handle).expect("derived endpoint");
        let mut listener = NativeListener::bind(&endpoint).expect("bind owner endpoint");
        let serving = Arc::new(AtomicBool::new(true));
        let stop = Arc::clone(&serving);
        let server = std::thread::spawn(move || {
            let mut rounds = 0_u64;
            while stop.load(Ordering::Relaxed) {
                let Ok(mut stream) = listener.accept(ACCEPT_TICK) else {
                    continue;
                };
                if serve_authenticated_request(&mut owner, &mut stream).is_ok() {
                    let _ = stream.finish_server_response();
                    rounds += 1;
                }
            }
            (rounds, owner)
        });

        let window = Duration::from_secs(6);
        let started = Instant::now();
        let mut rounds = 0_u64;
        let mut budget_exhaustions = 0_u64;
        while started.elapsed() < window {
            // The harness uses the frozen production budget, never a wider one,
            // so a failure path cannot hide behind a generous outer deadline.
            match client_request_before(
                &handle,
                ManagedJobOperation::Wait { timeout_ms: 25 },
                Instant::now() + WAIT_ROUND_BUDGET,
            ) {
                Ok(ManagedJobResult::Wait { .. }) => rounds += 1,
                Ok(_) => panic!("a wait round may only answer with a wait reply"),
                Err(error) => {
                    assert_eq!(
                        error.code, ROUND_TIMEOUT_CODE,
                        "a live owner may only exhaust a short round's own budget"
                    );
                    budget_exhaustions += 1;
                }
            }
        }
        let window_wall = started.elapsed();

        let flag = Arc::new(AtomicBool::new(false));
        let token_set = Arc::new(std::sync::Mutex::new(None::<Instant>));
        let canceller_flag = Arc::clone(&flag);
        let canceller_at = Arc::clone(&token_set);
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            *canceller_at.lock().expect("token instant") = Some(Instant::now());
            canceller_flag.store(true, Ordering::Relaxed);
        });
        let cancel_return = loop {
            let reply = client_request_before(
                &handle,
                ManagedJobOperation::Wait { timeout_ms: 25 },
                Instant::now() + WAIT_ROUND_BUDGET,
            );
            if flag.load(Ordering::Relaxed) {
                break Instant::now();
            }
            match reply {
                Ok(ManagedJobResult::Wait { .. }) => {}
                Ok(_) => panic!("a wait round may only answer with a wait reply"),
                Err(error) => panic!("a live owner must answer the round: {}", error.code),
            }
        };
        canceller.join().expect("canceller");
        let token_at = token_set
            .lock()
            .expect("token instant")
            .expect("token instant");
        let cancel_to_return = cancel_return.saturating_duration_since(token_at);

        let status_started = Instant::now();
        let status = client_request_before(
            &handle,
            ManagedJobOperation::Status,
            Instant::now() + WAIT_ROUND_BUDGET,
        );
        let status_latency = status_started.elapsed();
        serving.store(false, Ordering::Relaxed);
        let (served_rounds, _owner) = server.join().expect("owner server");

        eprintln!(
            "MEASURED real_owner window={window_wall:?} rounds={rounds} \
             budget_exhaustions={budget_exhaustions} served_rounds={served_rounds} \
             cancel_to_return={cancel_to_return:?} status_latency={status_latency:?}"
        );
        let _ = std::fs::remove_file(endpoint.unix_socket_path().expect("socket path"));
        assert!(rounds > 0, "the real owner must answer cooperative rounds");
        assert!(
            cancel_to_return < Duration::from_millis(150),
            "cancel -> callback return must stay inside the worker grace, measured {cancel_to_return:?}"
        );
        assert!(
            matches!(status, Ok(ManagedJobResult::Status { .. })),
            "a status call right after a cancellation must still succeed"
        );
        assert!(
            status_latency < Duration::from_millis(150),
            "the owner must not leave a lock delay behind a cancelled wait, measured {status_latency:?}"
        );
    }

    /// The frozen round budget is the sum of exactly the two values the wait
    /// loop uses; a harness must not quietly widen it to pass the grace gate.
    /// The classification is by cause, not by "a connect failed": only a spent
    /// round budget or a timeout-ish transport result is a round timeout.
    #[test]
    fn round_connect_failures_are_classified_by_cause() {
        let refused = IpcTransportError::new(
            IpcTransportErrorCode::InvalidEndpoint,
            "unix:/tmp/absent-owner.sock",
            io::Error::new(io::ErrorKind::ConnectionRefused, "refused"),
        );
        assert_eq!(
            classify_round_connect_failure(false, &refused),
            "managed_job_owner_unavailable"
        );
        let missing = IpcTransportError::new(
            IpcTransportErrorCode::InvalidEndpoint,
            "unix:/tmp/absent-owner.sock",
            io::Error::new(io::ErrorKind::NotFound, "missing"),
        );
        assert_eq!(
            classify_round_connect_failure(false, &missing),
            "managed_job_owner_unavailable"
        );
        // A spent budget is inconclusive whatever the platform reported.
        assert_eq!(
            classify_round_connect_failure(true, &refused),
            ROUND_TIMEOUT_CODE
        );
        // Timeouts and in-progress nonblocking connects are not absences.
        for kind in [io::ErrorKind::TimedOut, io::ErrorKind::WouldBlock] {
            let error = IpcTransportError::new(
                IpcTransportErrorCode::ConnectTimeout,
                "unix:/tmp/absent-owner.sock",
                io::Error::new(kind, "not yet refused"),
            );
            assert_eq!(
                classify_round_connect_failure(false, &error),
                ROUND_TIMEOUT_CODE
            );
        }
    }

    /// A real absent endpoint is an owner failure, and a round that starts with
    /// no budget left is a round timeout — neither case waits on the scheduler.
    #[test]
    fn a_real_absent_endpoint_and_a_spent_budget_are_distinct() {
        let absent = client_request_before(
            &test_handle('z'),
            ManagedJobOperation::Status,
            Instant::now() + Duration::from_secs(1),
        )
        .expect_err("no owner is listening");
        assert_eq!(absent.code, "managed_job_owner_unavailable");

        let spent = client_request_before(
            &test_handle('y'),
            ManagedJobOperation::Status,
            Instant::now(),
        )
        .expect_err("an exhausted round budget is not an owner failure");
        assert_eq!(spent.code, ROUND_TIMEOUT_CODE);
    }

    #[test]
    fn frozen_round_budget_is_the_sum_of_the_wait_quantum_and_its_margin() {
        assert_eq!(WAIT_ROUND_BUDGET, WAIT_QUANTUM + WAIT_ROUND_MARGIN);
        assert!(WAIT_ROUND_BUDGET < Duration::from_millis(150));
    }

    #[test]
    fn resources_refuse_when_the_resident_owner_is_absent() {
        let error = client_request(
            &test_handle('f'),
            ManagedJobOperation::Resources { max_members: 8 },
        )
        .expect_err("an absent owner must not be reconstructed from process IDs");
        assert_eq!(error.code, "managed_job_owner_unavailable");
    }

    #[test]
    fn binary_stdin_and_independent_output_cursors_cross_the_owner_boundary() {
        let (directory, mut owner) = owner_fixture("binary");
        let resources = dispatch(
            &mut owner,
            ManagedJobRequest {
                schema_version: SCHEMA_VERSION,
                request_id: "resources-1".into(),
                operation: ManagedJobOperation::Resources { max_members: 8 },
            },
        );
        let Some(ManagedJobResult::Resources { snapshot }) = resources.result else {
            panic!("resource snapshot result")
        };
        assert!(resources.ok);
        assert!(snapshot.membership_complete);
        assert_eq!(snapshot.members.len(), 1);
        assert_ne!(snapshot.members[0].pid, 0);
        let binary = [0, 255, b'\n', 1, 2, 3];
        let write_reply = dispatch(
            &mut owner,
            ManagedJobRequest {
                schema_version: SCHEMA_VERSION,
                request_id: "write-1".into(),
                operation: ManagedJobOperation::Write {
                    data_base64: base64_encode(&binary),
                    close_stdin: false,
                },
            },
        );
        assert!(write_reply.ok);
        assert!(
            dispatch(
                &mut owner,
                ManagedJobRequest {
                    schema_version: SCHEMA_VERSION,
                    request_id: "close-1".into(),
                    operation: ManagedJobOperation::CloseStdin,
                },
            )
            .ok
        );
        let report = owner
            .wait(Duration::from_secs(10))
            .expect("bounded wait")
            .expect("completed");
        assert_eq!(report.terminal, ManagedJobTerminal::Exited(0));
        let expected = [b"OUT:".as_slice(), binary.as_slice()].concat();
        assert!(
            report
                .stdout
                .retained
                .windows(binary.len() + 4)
                .any(|window| window == expected)
        );
        assert!(
            report
                .stderr
                .retained
                .windows(7)
                .any(|window| window == b"ERR-END")
        );

        let stdout = owner.output_page(false, 0, OUTPUT_BYTES_MAX);
        let stderr = owner.output_page(true, 0, OUTPUT_BYTES_MAX);
        assert!(stdout.is_ok(), "stdout has an independent cursor");
        assert!(stderr.is_ok(), "stderr has an independent cursor");
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn zero_wait_is_bounded_and_stop_closes_the_contained_tree() {
        let (directory, mut owner) = owner_fixture("stop");
        assert!(owner.wait(Duration::ZERO).expect("poll").is_none());
        let report = owner.stop().expect("stop contained child");
        assert!(matches!(
            report.terminal,
            ManagedJobTerminal::Exited(_) | ManagedJobTerminal::Signaled(_)
        ));
        assert!(!owner.status().expect("terminal status").stdin_open);
        fs::remove_dir_all(directory).expect("remove test directory");
    }
}
