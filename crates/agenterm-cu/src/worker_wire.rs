//! Versioned stdin envelope used by SSH/VNC session workers.
//!
//! Request leases are bearer secrets. They travel only in the worker's stdin,
//! never in argv, environment variables, command JSON, audit rows, or replies.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{Command, CuError, RequestIdentity, TargetRef};

const PROTOCOL: &str = "agenterm-cu-worker";
const SCHEMA: u8 = 1;
pub const MAX_WORKER_REQUEST_BYTES: usize = 1_048_576;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerEnvelope {
    protocol: String,
    schema: u8,
    command: Command,
    request_identity: RequestIdentityWire,
    effect_scope: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestIdentityWire {
    request_id: String,
    session_id: String,
    session_lease: String,
}

/// Encode a remote worker request. Commands without caller identity retain the
/// original raw-Command wire shape for backward compatibility.
pub fn encode(
    command: &Command,
    identity: Option<&RequestIdentity>,
    effect_scope: Option<&str>,
) -> Result<String, CuError> {
    let encoded = if let Some(identity) = identity {
        let effect_scope = effect_scope.ok_or_else(|| {
            CuError::new(
                "worker_effect_scope_missing",
                "remote request identity requires an exact effect scope",
            )
        })?;
        validate_effect_scope(effect_scope)?;
        serde_json::to_string(&WorkerEnvelope {
            protocol: PROTOCOL.to_owned(),
            schema: SCHEMA,
            command: command.clone(),
            request_identity: RequestIdentityWire {
                request_id: identity.request_id.clone(),
                session_id: identity.session_id.clone(),
                session_lease: identity.session_lease.clone(),
            },
            effect_scope: effect_scope.to_owned(),
        })
    } else {
        if effect_scope.is_some() {
            return Err(CuError::new(
                "worker_effect_scope_without_identity",
                "effect scope is valid only with request identity",
            ));
        }
        serde_json::to_string(command)
    }
    .map_err(|error| {
        CuError::new(
            "serialize",
            format!("worker request could not be serialized: {error}"),
        )
    })?;
    if encoded.len() > MAX_WORKER_REQUEST_BYTES {
        return Err(CuError::new(
            "worker_request_too_large",
            format!("worker request exceeds the {MAX_WORKER_REQUEST_BYTES}-byte limit"),
        ));
    }
    Ok(encoded)
}

/// Decode either the versioned envelope or the legacy raw Command. An
/// envelope-looking payload never falls back to Command parsing when its
/// protocol/schema is invalid.
pub fn decode(raw: &str) -> Result<(Command, Option<(RequestIdentity, String)>), CuError> {
    if raw.len() > MAX_WORKER_REQUEST_BYTES {
        return Err(CuError::new(
            "worker_request_too_large",
            format!("worker request exceeds the {MAX_WORKER_REQUEST_BYTES}-byte limit"),
        ));
    }
    let value: serde_json::Value = serde_json::from_str(raw).map_err(invalid_worker_json)?;
    if value.get("protocol").is_none() {
        let command = serde_json::from_value(value).map_err(invalid_worker_json)?;
        return Ok((command, None));
    }

    let envelope: WorkerEnvelope =
        serde_json::from_value(value).map_err(invalid_worker_envelope)?;
    if envelope.protocol != PROTOCOL || envelope.schema != SCHEMA {
        return Err(CuError::new(
            "worker_protocol_unsupported",
            "worker request protocol or schema is unsupported",
        ));
    }
    if envelope.command.target() != TargetRef::Current {
        return Err(CuError::new(
            "worker_target_invalid",
            "request-bearing worker envelopes must target current",
        ));
    }
    validate_effect_scope(&envelope.effect_scope)?;
    Ok((
        envelope.command,
        Some((
            RequestIdentity {
                request_id: envelope.request_identity.request_id,
                session_id: envelope.request_identity.session_id,
                session_lease: envelope.request_identity.session_lease,
            },
            envelope.effect_scope,
        )),
    ))
}

/// Opaque, non-secret binding retained in the request fingerprint after a
/// transport rewrites its command target to `current`.
pub fn effect_scope(kind: &str, components: &[&str]) -> String {
    let mut digest = Sha256::new();
    digest.update((kind.len() as u64).to_be_bytes());
    digest.update(kind.as_bytes());
    for component in components {
        digest.update((component.len() as u64).to_be_bytes());
        digest.update(component.as_bytes());
    }
    let digest = digest.finalize();
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{kind}:{hex}")
}

fn validate_effect_scope(scope: &str) -> Result<(), CuError> {
    let Some((kind, digest)) = scope.split_once(':') else {
        return Err(CuError::new(
            "worker_effect_scope_invalid",
            "worker effect scope is malformed",
        ));
    };
    if !matches!(kind, "ssh" | "vnc")
        || digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(CuError::new(
            "worker_effect_scope_invalid",
            "worker effect scope is malformed",
        ));
    }
    Ok(())
}

fn invalid_worker_json(error: serde_json::Error) -> CuError {
    CuError::new(
        "invalid_worker_json",
        format!("invalid command JSON: {error}"),
    )
}

fn invalid_worker_envelope(error: serde_json::Error) -> CuError {
    CuError::new(
        "invalid_worker_envelope",
        format!("invalid worker request envelope: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TargetRef;

    fn command() -> Command {
        Command::ClipboardClear {
            target: TargetRef::Current,
            apply: true,
        }
    }

    #[test]
    fn request_identity_round_trips_only_inside_the_envelope() {
        let identity = RequestIdentity {
            request_id: "request-17".to_owned(),
            session_id: "session-5".to_owned(),
            session_lease: "fixture-bearer-secret".to_owned(),
        };
        let scope = effect_scope("ssh", &["fixture-host", "22"]);
        let encoded = encode(&command(), Some(&identity), Some(&scope)).expect("encode envelope");
        let (decoded_command, decoded_identity) = decode(&encoded).expect("decode envelope");
        assert!(matches!(
            decoded_command,
            Command::ClipboardClear {
                target: TargetRef::Current,
                apply: true
            }
        ));
        let (decoded_identity, decoded_scope) = decoded_identity.expect("request identity");
        assert_eq!(decoded_identity.request_id, identity.request_id);
        assert_eq!(decoded_identity.session_id, identity.session_id);
        assert_eq!(decoded_identity.session_lease, identity.session_lease);
        assert_eq!(decoded_scope, scope);
    }

    #[test]
    fn raw_command_remains_backward_compatible() {
        let encoded = encode(&command(), None, None).expect("encode command");
        let (decoded, identity) = decode(&encoded).expect("decode command");
        assert!(matches!(
            decoded,
            Command::ClipboardClear {
                target: TargetRef::Current,
                apply: true
            }
        ));
        assert!(identity.is_none());
        assert!(!encoded.contains(PROTOCOL));
    }

    #[test]
    fn envelope_like_input_fails_closed_on_unknown_schema() {
        let scope = effect_scope("ssh", &["fixture-host", "22"]);
        let encoded = encode(
            &command(),
            Some(&RequestIdentity {
                request_id: "request-17".to_owned(),
                session_id: "session-5".to_owned(),
                session_lease: "fixture-bearer-secret".to_owned(),
            }),
            Some(&scope),
        )
        .expect("encode envelope")
        .replace("\"schema\":1", "\"schema\":2");
        let error = match decode(&encoded) {
            Ok(_) => panic!("unknown schema must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code, "worker_protocol_unsupported");

        let unknown_field = encoded
            .replace("\"schema\":2", "\"schema\":1")
            .replace("\"command\":", "\"unexpected\":true,\"command\":");
        let error = match decode(&unknown_field) {
            Ok(_) => panic!("unknown envelope field must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code, "invalid_worker_envelope");
    }

    #[test]
    fn request_envelope_rejects_non_current_target_and_oversize() {
        let identity = RequestIdentity {
            request_id: "request-17".to_owned(),
            session_id: "session-5".to_owned(),
            session_lease: "fixture-bearer-secret".to_owned(),
        };
        let scope = effect_scope("ssh", &["fixture-host", "22"]);
        let remote = Command::ClipboardClear {
            target: TargetRef::Ssh,
            apply: true,
        };
        let encoded = encode(&remote, Some(&identity), Some(&scope)).expect("encode envelope");
        let error = match decode(&encoded) {
            Ok(_) => panic!("remote inner target must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code, "worker_target_invalid");

        let oversized = "x".repeat(MAX_WORKER_REQUEST_BYTES + 1);
        let error = match decode(&oversized) {
            Ok(_) => panic!("oversize must fail before JSON"),
            Err(error) => error,
        };
        assert_eq!(error.code, "worker_request_too_large");
    }
}
