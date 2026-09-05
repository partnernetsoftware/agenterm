//! Bounded request/reply protocol for one resident managed-job owner.
//!
//! Transport binding is deliberately outside this module. The caller must pass
//! a stream already accepted by `agenterm-platform`'s current-user native IPC
//! transport (Unix socket or Windows named pipe). TCP is not a valid carrier.
//! Requests never contain a bearer/session lease, and replies never echo job
//! launch arguments, environment values, or stdin bytes.

use std::{
    io::{self, Read, Write},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::managed_job_owner::{
    ManagedJobOwnerError, OutputCursorError, ResidentJobOwner, ResidentJobState, ResidentJobStatus,
    StdinWriteError,
};

const SCHEMA_VERSION: u32 = 1;
const FRAME_MAX_BYTES: usize = 128 * 1024;
const REQUEST_ID_MAX_BYTES: usize = 128;
const STDIN_BYTES_MAX: usize = 64 * 1024;
const OUTPUT_BYTES_MAX: usize = 64 * 1024;
const WAIT_MAX_MS: u64 = 300_000;

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
    Output {
        stream: OutputStream,
        cursor: u64,
        max_bytes: usize,
    },
    Write {
        data_base64: String,
    },
    CloseStdin,
    Wait {
        timeout_ms: u64,
    },
    Stop,
    Renew {
        ttl_ms: u64,
    },
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
        ManagedJobOperation::Output { max_bytes, .. } => {
            if !(1..=OUTPUT_BYTES_MAX).contains(max_bytes) {
                return Err("managed_job_output_limit");
            }
        }
        ManagedJobOperation::Write { data_base64 } => {
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
        ManagedJobOperation::Write { data_base64 } => match base64_decode(&data_base64) {
            Ok(bytes) if bytes.len() <= STDIN_BYTES_MAX => match owner.write_stdin(&bytes) {
                Ok(accepted_bytes) => Ok(ManagedJobResult::Write {
                    accepted_bytes,
                    delivery: WriteDelivery::Complete,
                }),
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
            },
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
    let length = usize::try_from(u32::from_be_bytes(length))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid frame length"))?;
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
    let length = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "reply too large"))?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(bytes)?;
    writer.flush()
}

fn encoded_len(bytes: usize) -> usize {
    bytes.div_ceil(3) * 4
}

fn base64_encode(bytes: &[u8]) -> String {
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

fn base64_decode(value: &str) -> Result<Vec<u8>, ()> {
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

    use crate::{
        managed_job_owner::{
            ManagedJobEnvironment, ManagedJobLaunch, ManagedJobTerminal, start_owner_from_reader,
        },
        managed_job_store::ManagedJobStore,
    };

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
            schema_version: 1,
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
    }

    #[test]
    fn binary_stdin_and_independent_output_cursors_cross_the_owner_boundary() {
        let (directory, mut owner) = owner_fixture("binary");
        let binary = [0, 255, b'\n', 1, 2, 3];
        let write_reply = dispatch(
            &mut owner,
            ManagedJobRequest {
                schema_version: SCHEMA_VERSION,
                request_id: "write-1".into(),
                operation: ManagedJobOperation::Write {
                    data_base64: base64_encode(&binary),
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
