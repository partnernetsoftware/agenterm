//! Bounded authenticated protocol for one resident device owner.

use std::{
    io::{self, Read, Write},
    time::Duration,
};

use agenterm_platform::{
    device_io::{DeviceReadState, DeviceWriteDelivery},
    entropy::secure_random_array,
    ipc::{IpcEndpoint, IpcTransportErrorCode, NativeListener, NativeStream},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    device_lease_owner::{
        DeviceOwnerError, ResidentDeviceOwner, now_utc_ms, read_launch, start_owner_from_launch,
    },
    device_lease_store::{DeviceLeaseHandle, DeviceLeaseState, DeviceLeaseStore},
    managed_job_ipc::{base64_decode, base64_encode},
};

const SCHEMA_VERSION: u32 = 1;
const FRAME_MAX_BYTES: usize = 128 * 1024;
const REQUEST_ID_MAX_BYTES: usize = 128;
const DATA_MAX_BYTES: usize = 64 * 1024;
const IO_TIMEOUT_MAX_MS: u64 = 300_000;
const ACCEPT_TICK: Duration = Duration::from_millis(100);
const STREAM_TIMEOUT: Duration = Duration::from_secs(305);

pub(crate) fn run_resident(reader: impl Read) -> Result<(), DeviceOwnerError> {
    let launch = read_launch(reader)?;
    let store = DeviceLeaseStore::open_at(&launch.state_path)
        .map_err(|_| DeviceOwnerError::new("device_lease_store_unavailable"))?;
    let endpoint = endpoint_for(&launch.handle)?;
    let mut listener = match NativeListener::bind(&endpoint) {
        Ok(listener) => listener,
        Err(_) => {
            let _ = store.mark_unclaimed_open_failed(
                &launch.handle,
                "device_endpoint_unavailable",
                now_utc_ms()?,
            );
            return Err(DeviceOwnerError::new("device_endpoint_unavailable"));
        }
    };
    let mut owner = start_owner_from_launch(launch)?;
    loop {
        if owner.expire_if_due()? {
            return Ok(());
        }
        match listener.accept(ACCEPT_TICK) {
            Ok(mut stream) => {
                if stream.set_io_timeout(STREAM_TIMEOUT).is_err() {
                    continue;
                }
                let terminal = serve_authenticated_request(&mut owner, &mut stream)?;
                let _ = stream.finish_server_response();
                if terminal {
                    return Ok(());
                }
            }
            Err(error) if error.code == IpcTransportErrorCode::AcceptTimeout => {}
            Err(_) => return Err(DeviceOwnerError::new("device_endpoint_unavailable")),
        }
    }
}

pub(crate) fn endpoint_for(handle: &DeviceLeaseHandle) -> Result<IpcEndpoint, DeviceOwnerError> {
    let canonical =
        serde_json::to_vec(handle).map_err(|_| DeviceOwnerError::new("device_endpoint_invalid"))?;
    let suffix: String = Sha256::digest(&canonical)[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    #[cfg(windows)]
    {
        Ok(IpcEndpoint::NamedPipe(format!(
            r"\\.\pipe\agenterm-cu-device-{suffix}"
        )))
    }
    #[cfg(unix)]
    {
        Ok(IpcEndpoint::UnixSocket(
            agenterm_platform::ipc::native_runtime_directory()
                .join(format!("cu-device-{suffix}.sock"))
                .to_string_lossy()
                .into_owned(),
        ))
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceRequest {
    pub schema_version: u32,
    pub request_id: String,
    pub authority: DeviceAuthority,
    pub operation: DeviceOperation,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub(crate) enum DeviceAuthority {
    Lease { secret: String },
    Session { session_id: String, lease: String },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub(crate) enum DeviceOperation {
    Read {
        max_bytes: usize,
        timeout_ms: u64,
    },
    Write {
        data_base64: String,
        timeout_ms: u64,
    },
    Renew {
        ttl_ms: u64,
    },
    Release,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceReply {
    pub schema_version: u32,
    pub request_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<DeviceResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DeviceProtocolError>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub(crate) enum DeviceResult {
    Read {
        data_base64: String,
        bytes: usize,
        state: String,
        total_bytes_read: String,
    },
    Write {
        requested_bytes: usize,
        written_bytes: usize,
        delivery: String,
        total_bytes_written: String,
    },
    Renewed {
        expires_at_utc_ms: i64,
    },
    Released {
        state: String,
        bytes_read: String,
        bytes_written: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceProtocolError {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub known_written_lower_bound: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_uncertain: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_safe: Option<bool>,
}

pub(crate) fn client_request(
    handle: &DeviceLeaseHandle,
    lease_secret: &str,
    operation: DeviceOperation,
) -> Result<DeviceResult, DeviceProtocolError> {
    validate_secret(lease_secret).map_err(protocol_error)?;
    client_request_with_authority(
        handle,
        DeviceAuthority::Lease {
            secret: lease_secret.to_owned(),
        },
        operation,
    )
}

pub(crate) fn client_session_release(
    handle: &DeviceLeaseHandle,
    session_id: &str,
    session_lease: &str,
) -> Result<DeviceResult, DeviceProtocolError> {
    validate_session_authority(session_id, session_lease).map_err(protocol_error)?;
    client_request_with_authority(
        handle,
        DeviceAuthority::Session {
            session_id: session_id.to_owned(),
            lease: session_lease.to_owned(),
        },
        DeviceOperation::Release,
    )
}

fn client_request_with_authority(
    handle: &DeviceLeaseHandle,
    authority: DeviceAuthority,
    operation: DeviceOperation,
) -> Result<DeviceResult, DeviceProtocolError> {
    validate_operation(&operation).map_err(protocol_error)?;
    let endpoint = endpoint_for(handle).map_err(|error| protocol_error(error.code))?;
    let request_id = random_request_id()?;
    let request = DeviceRequest {
        schema_version: SCHEMA_VERSION,
        request_id: request_id.clone(),
        authority,
        operation,
    };
    round_trip(&endpoint, request_id, &request)
}

fn round_trip(
    endpoint: &IpcEndpoint,
    request_id: String,
    request: &DeviceRequest,
) -> Result<DeviceResult, DeviceProtocolError> {
    let bytes =
        serde_json::to_vec(request).map_err(|_| protocol_error("device_request_invalid"))?;
    let mut stream = NativeStream::connect(endpoint, Duration::from_secs(2))
        .map_err(|_| protocol_error("device_owner_unavailable"))?;
    stream
        .set_io_timeout(STREAM_TIMEOUT)
        .map_err(|_| protocol_error("device_protocol_io"))?;
    write_frame(&mut stream, &bytes).map_err(|_| protocol_error("device_protocol_io"))?;
    let reply_bytes = read_frame(&mut stream)
        .map_err(|_| protocol_error("device_protocol_io"))?
        .ok_or_else(|| protocol_error("device_response_missing"))?;
    let reply: DeviceReply = serde_json::from_slice(&reply_bytes)
        .map_err(|_| protocol_error("device_response_invalid"))?;
    if reply.schema_version != SCHEMA_VERSION
        || reply.request_id != request_id
        || reply.ok != reply.result.is_some()
        || reply.ok == reply.error.is_some()
    {
        return Err(protocol_error("device_response_invalid"));
    }
    match (reply.result, reply.error) {
        (Some(result), None) => Ok(result),
        (None, Some(error)) => Err(error),
        _ => Err(protocol_error("device_response_invalid")),
    }
}

fn serve_authenticated_request(
    owner: &mut ResidentDeviceOwner,
    stream: &mut NativeStream,
) -> Result<bool, DeviceOwnerError> {
    let request_bytes = read_frame(stream)
        .map_err(|_| DeviceOwnerError::new("device_protocol_io"))?
        .ok_or_else(|| DeviceOwnerError::new("device_request_missing"))?;
    let request = decode_request(&request_bytes)?;
    let request_id = request.request_id.clone();
    let authorized = match &request.authority {
        DeviceAuthority::Lease { secret } => owner.authenticate(secret),
        DeviceAuthority::Session { session_id, lease }
            if matches!(request.operation, DeviceOperation::Release) =>
        {
            owner.authenticate_session(session_id, lease)
        }
        DeviceAuthority::Session { .. } => {
            Err(DeviceOwnerError::new("device_session_operation_invalid"))
        }
    };
    let reply = match authorized {
        Ok(()) => execute(owner, request),
        Err(error) => failure(request_id, error),
    };
    let terminal = matches!(reply.result, Some(DeviceResult::Released { .. }));
    let bytes =
        serde_json::to_vec(&reply).map_err(|_| DeviceOwnerError::new("device_response_invalid"))?;
    write_frame(stream, &bytes).map_err(|_| DeviceOwnerError::new("device_protocol_io"))?;
    Ok(terminal)
}

fn decode_request(bytes: &[u8]) -> Result<DeviceRequest, DeviceOwnerError> {
    let request: DeviceRequest = serde_json::from_slice(bytes)
        .map_err(|_| DeviceOwnerError::new("device_request_invalid"))?;
    if request.schema_version != SCHEMA_VERSION
        || request.request_id.is_empty()
        || request.request_id.len() > REQUEST_ID_MAX_BYTES
        || request.request_id.chars().any(char::is_control)
        || validate_authority(&request.authority).is_err()
        || validate_operation(&request.operation).is_err()
    {
        return Err(DeviceOwnerError::new("device_request_invalid"));
    }
    Ok(request)
}

fn validate_authority(authority: &DeviceAuthority) -> Result<(), &'static str> {
    match authority {
        DeviceAuthority::Lease { secret } => validate_secret(secret),
        DeviceAuthority::Session { session_id, lease } => {
            validate_session_authority(session_id, lease)
        }
    }
}

fn validate_session_authority(session_id: &str, lease: &str) -> Result<(), &'static str> {
    if session_id.is_empty()
        || session_id.len() > 128
        || session_id.chars().any(char::is_control)
        || lease.is_empty()
        || lease.len() > 512
        || lease.chars().any(char::is_control)
    {
        Err("device_session_authority_invalid")
    } else {
        Ok(())
    }
}

fn execute(owner: &mut ResidentDeviceOwner, request: DeviceRequest) -> DeviceReply {
    let request_id = request.request_id;
    let result = match request.operation {
        DeviceOperation::Read {
            max_bytes,
            timeout_ms,
        } => owner
            .read_once(max_bytes, Duration::from_millis(timeout_ms))
            .map(|outcome| {
                let state = match outcome.state {
                    DeviceReadState::Data => "data",
                    DeviceReadState::WouldBlock => "would_block",
                    DeviceReadState::EndOfFile => "eof",
                };
                let bytes = outcome.bytes.len();
                DeviceResult::Read {
                    data_base64: base64_encode(&outcome.bytes),
                    bytes,
                    state: state.to_owned(),
                    total_bytes_read: owner
                        .record()
                        .map(|record| record.bytes_read.to_string())
                        .unwrap_or_else(|_| "unknown".to_owned()),
                }
            }),
        DeviceOperation::Write {
            data_base64,
            timeout_ms,
        } => match base64_decode(&data_base64) {
            Ok(bytes) => owner
                .write_once(&bytes, Duration::from_millis(timeout_ms))
                .map(|outcome| DeviceResult::Write {
                    requested_bytes: outcome.requested_bytes,
                    written_bytes: outcome.written_bytes,
                    delivery: match outcome.delivery {
                        DeviceWriteDelivery::Complete => "complete",
                        DeviceWriteDelivery::Partial => "partial",
                    }
                    .to_owned(),
                    total_bytes_written: owner
                        .record()
                        .map(|record| record.bytes_written.to_string())
                        .unwrap_or_else(|_| "unknown".to_owned()),
                }),
            Err(()) => Err(DeviceOwnerError::new("device_encoding_invalid")),
        },
        DeviceOperation::Renew { ttl_ms } => {
            owner.renew(ttl_ms).map(|record| DeviceResult::Renewed {
                expires_at_utc_ms: record.expires_at_utc_ms,
            })
        }
        DeviceOperation::Release => owner.release().map(|record| DeviceResult::Released {
            state: state_name(&record.state).to_owned(),
            bytes_read: record.bytes_read.to_string(),
            bytes_written: record.bytes_written.to_string(),
        }),
    };
    match result {
        Ok(result) => success(request_id, result),
        Err(error) => failure(request_id, error),
    }
}

fn success(request_id: String, result: DeviceResult) -> DeviceReply {
    DeviceReply {
        schema_version: SCHEMA_VERSION,
        request_id,
        ok: true,
        result: Some(result),
        error: None,
    }
}

fn failure(request_id: String, error: DeviceOwnerError) -> DeviceReply {
    DeviceReply {
        schema_version: SCHEMA_VERSION,
        request_id,
        ok: false,
        result: None,
        error: Some(DeviceProtocolError {
            code: error.code,
            known_written_lower_bound: error.known_written_lower_bound,
            delivery_uncertain: error.delivery_uncertain,
            retry_safe: error.retry_safe,
        }),
    }
}

fn state_name(state: &DeviceLeaseState) -> &'static str {
    match state {
        DeviceLeaseState::ClaimIntent => "claim_intent",
        DeviceLeaseState::Opening => "opening",
        DeviceLeaseState::Active => "active",
        DeviceLeaseState::Released => "released",
        DeviceLeaseState::Expired => "expired",
        DeviceLeaseState::OpenFailed { .. } => "open_failed",
        DeviceLeaseState::OwnerLost => "owner_lost",
        DeviceLeaseState::CleanupUncertain { .. } => "cleanup_uncertain",
    }
}

fn validate_operation(operation: &DeviceOperation) -> Result<(), &'static str> {
    match operation {
        DeviceOperation::Read {
            max_bytes,
            timeout_ms,
        } => {
            if !(1..=DATA_MAX_BYTES).contains(max_bytes) {
                return Err("device_read_limit");
            }
            validate_timeout(*timeout_ms)
        }
        DeviceOperation::Write {
            data_base64,
            timeout_ms,
        } => {
            validate_timeout(*timeout_ms)?;
            if data_base64.len() > DATA_MAX_BYTES.div_ceil(3) * 4 {
                return Err("device_write_limit");
            }
            let decoded = base64_decode(data_base64).map_err(|_| "device_encoding_invalid")?;
            if decoded.is_empty() || decoded.len() > DATA_MAX_BYTES {
                return Err("device_write_limit");
            }
            Ok(())
        }
        DeviceOperation::Renew { ttl_ms } => {
            if !(1_000..=86_400_000).contains(ttl_ms) {
                return Err("device_ttl_invalid");
            }
            Ok(())
        }
        DeviceOperation::Release => Ok(()),
    }
}

fn validate_timeout(timeout_ms: u64) -> Result<(), &'static str> {
    if !(1..=IO_TIMEOUT_MAX_MS).contains(&timeout_ms) {
        Err("device_io_timeout_invalid")
    } else {
        Ok(())
    }
}

fn validate_secret(secret: &str) -> Result<(), &'static str> {
    if secret.len() == 64
        && secret
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err("device_lease_invalid")
    }
}

fn random_request_id() -> Result<String, DeviceProtocolError> {
    secure_random_array::<16>()
        .map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect())
        .map_err(|_| protocol_error("device_request_entropy_unavailable"))
}

fn protocol_error(code: impl Into<String>) -> DeviceProtocolError {
    DeviceProtocolError {
        code: code.into(),
        known_written_lower_bound: None,
        delivery_uncertain: None,
        retry_safe: None,
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
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid device frame"))?;
    if length == 0 || length > FRAME_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "device frame size invalid",
        ));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(Some(bytes))
}

fn write_frame(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    if bytes.is_empty() || bytes.len() > FRAME_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "device frame size invalid",
        ));
    }
    let length = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "device reply too large"))?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(bytes)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_rejects_oversized_or_malformed_byte_operations() {
        assert_eq!(
            validate_operation(&DeviceOperation::Read {
                max_bytes: 0,
                timeout_ms: 1,
            }),
            Err("device_read_limit")
        );
        assert_eq!(
            validate_operation(&DeviceOperation::Write {
                data_base64: "%%%=".to_owned(),
                timeout_ms: 1,
            }),
            Err("device_encoding_invalid")
        );
        assert_eq!(
            validate_operation(&DeviceOperation::Renew { ttl_ms: 999 }),
            Err("device_ttl_invalid")
        );
    }

    #[test]
    fn endpoint_never_contains_public_device_or_secret_material() {
        let handle = DeviceLeaseHandle {
            lease_id: "00000000-0000-4000-8000-000000000001".to_owned(),
            generation: 1,
            owner_nonce: "a".repeat(32),
        };
        let endpoint = format!("{:?}", endpoint_for(&handle).unwrap());
        assert!(!endpoint.contains(&handle.lease_id));
        assert!(!endpoint.contains("agt-device"));
    }

    #[test]
    fn failure_preserves_retry_safety_independently_from_delivery_certainty() {
        let reply = failure(
            "request".to_owned(),
            DeviceOwnerError {
                code: "device_lease_store_unavailable".to_owned(),
                known_written_lower_bound: Some(7),
                delivery_uncertain: Some(false),
                retry_safe: Some(false),
            },
        );
        let error = reply.error.expect("failure carries protocol error");
        assert_eq!(error.known_written_lower_bound, Some(7));
        assert_eq!(error.delivery_uncertain, Some(false));
        assert_eq!(error.retry_safe, Some(false));
    }
}
