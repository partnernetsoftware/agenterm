//! Public typed privilege apply and the fixed Linux polkit launcher.

#[cfg(target_os = "linux")]
use std::{
    io::Read,
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
use agenterm_platform::{
    contained_process::{ContainedChild, ContainedHeadlessCommand},
    process_spawn::ProcessExit,
};
use serde_json::{Value, json};

#[cfg(any(target_os = "linux", test))]
use crate::privilege_apply::PrivilegeApplyReplyV1;
use crate::{
    Command, CuError, CuReply,
    privilege_apply::{
        PRIVILEGE_APPLY_PROTOCOL_VERSION, PRIVILEGE_PROVIDER_CONTRACT_VERSION,
        PrivilegeApplyRequestV1, PrivilegeAuthorizationV1, PrivilegeClientV1, PrivilegeOriginV1,
        PrivilegeTargetScope, parse_apply_request,
    },
};

use super::{Executor, RequestIdentity};

#[cfg(target_os = "linux")]
const PKEXEC: &str = "/usr/bin/pkexec";
#[cfg(target_os = "linux")]
const PROVIDER: &str = "/usr/libexec/agenterm/agenterm-cu";
#[cfg(target_os = "linux")]
const POLL_INTERVAL: Duration = Duration::from_millis(5);
#[cfg(target_os = "linux")]
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

impl Executor {
    pub(super) fn execute_privilege_with_request_identity(
        &self,
        command: &Command,
        identity: &RequestIdentity,
    ) -> CuReply {
        let mut audit = match self.begin_audit(command) {
            Ok(audit) => audit,
            Err(error) => return CuReply::err(command, error),
        };
        let reply = match privilege_apply_payload(command, identity) {
            Ok(data) => CuReply::ok(command, data),
            Err(error) => CuReply::err(command, error),
        };
        if let Err(mut error) = Self::audit_after(&mut audit, command, &reply) {
            error.detail = Some(json!({
                "stage": "audit_outcome",
                "effect": "unknown",
                "request_id": identity.request_id,
                "original_reply": reply,
            }));
            return CuReply::err(command, error);
        }
        reply
    }
}

fn privilege_apply_payload(
    command: &Command,
    identity: &RequestIdentity,
) -> Result<Value, CuError> {
    let Command::PrivilegeApply {
        plan,
        approval_digest,
        provider_timeout_ms,
        ..
    } = command
    else {
        return Err(CuError::new(
            "privilege_command_invalid",
            "privilege launcher received another command family",
        ));
    };
    command
        .validate()
        .map_err(|message| CuError::new("invalid_input", message))?;
    let request = PrivilegeApplyRequestV1 {
        protocol_version: PRIVILEGE_APPLY_PROTOCOL_VERSION,
        request_id: identity.request_id.clone(),
        plan: plan.clone(),
        authorization: PrivilegeAuthorizationV1::OneShotNativeConsent,
        origin: PrivilegeOriginV1 {
            session_id: identity.session_id.clone(),
            target_scope: PrivilegeTargetScope::Current,
        },
        client: PrivilegeClientV1 {
            contract_version: PRIVILEGE_PROVIDER_CONTRACT_VERSION,
        },
    };
    if approval_digest != plan.approval_digest() {
        return Err(CuError::new(
            "privilege_approval_mismatch",
            "privilege approval does not match the typed expiring plan",
        ));
    }
    let canonical = serde_json::to_vec(&request).map_err(|_| {
        CuError::new(
            "privilege_request_invalid",
            "privilege request could not be serialized canonically",
        )
    })?;
    // Reuse the provider decoder locally so the public process is guaranteed
    // to send exactly one protocol-valid bounded request.
    parse_apply_request(&canonical)?;
    launch_linux_provider(&request, canonical, *provider_timeout_ms)
}

#[cfg(target_os = "linux")]
fn launch_linux_provider(
    request: &PrivilegeApplyRequestV1,
    canonical: Vec<u8>,
    timeout_ms: u64,
) -> Result<Value, CuError> {
    let canonical = String::from_utf8(canonical).map_err(|_| {
        CuError::new(
            "privilege_request_invalid",
            "canonical privilege request was not UTF-8",
        )
    })?;
    let mut command = ContainedHeadlessCommand::new(PKEXEC);
    command
        .args([PROVIDER, crate::PRIVILEGE_PROVIDER_ARG])
        .stdin_text(canonical)
        .capture_output();
    let mut child = command.spawn().map_err(|_| transport_unknown("spawn"))?;
    let stdout = take_stream(&mut child, true)?;
    let stderr = take_stream(&mut child, false)?;
    let stdout_thread = drain_bounded(stdout);
    let stderr_thread = drain_bounded(stderr);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let exit = loop {
        match child.try_wait() {
            Ok(Some(exit)) => break exit,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                if cleanup(&mut child) {
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();
                }
                return Err(transport_unknown("timeout"));
            }
            Err(_) => {
                if cleanup(&mut child) {
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();
                }
                return Err(transport_unknown("wait"));
            }
        }
    };
    // The root may exit while a descendant retains a pipe. Reap the complete
    // containment before joining either concurrent drain.
    if child.terminate_and_wait(CLEANUP_TIMEOUT).is_err() {
        return Err(transport_unknown("cleanup"));
    }
    let stdout = stdout_thread
        .join()
        .map_err(|_| transport_unknown("stdout-drain"))?
        .map_err(|_| transport_unknown("stdout-read"))?;
    let stderr = stderr_thread
        .join()
        .map_err(|_| transport_unknown("stderr-drain"))?
        .map_err(|_| transport_unknown("stderr-read"))?;

    // A complete tagged reply is the business result regardless of the root
    // exit status. The sole status exception is pkexec's documented explicit
    // user-cancellation code, and only when no reply bytes were emitted.
    if !stdout.exceeded
        && let Ok(reply) = crate::privilege_apply::parse_apply_reply(&stdout.bytes)
    {
        validate_reply_binding(request, &reply)?;
        return project_reply(reply);
    }
    if matches!(exit, ProcessExit::Code(126)) && stdout.bytes.is_empty() && !stderr.exceeded {
        return Err(CuError::new(
            "privilege_consent_canceled",
            "native privilege consent was canceled",
        )
        .with_detail(json!({"effect": "not_performed"})));
    }
    Err(transport_unknown(if stdout.exceeded || stderr.exceeded {
        "output-limit"
    } else {
        "reply"
    }))
}

#[cfg(not(target_os = "linux"))]
fn launch_linux_provider(
    _request: &PrivilegeApplyRequestV1,
    _canonical: Vec<u8>,
    _timeout_ms: u64,
) -> Result<Value, CuError> {
    Err(CuError::new(
        "privilege_provider_unsupported",
        "the public native-consent privilege provider is currently available only on Linux",
    )
    .with_detail(json!({"effect": "not_performed"})))
}

#[cfg(target_os = "linux")]
fn take_stream(
    child: &mut ContainedChild,
    stdout: bool,
) -> Result<impl Read + Send + 'static, CuError> {
    let stream = if stdout {
        child.take_stdout()
    } else {
        child.take_stderr()
    };
    stream.ok_or_else(|| {
        let _ = cleanup(child);
        transport_unknown(if stdout {
            "stdout-capture"
        } else {
            "stderr-capture"
        })
    })
}

#[cfg(target_os = "linux")]
struct BoundedOutput {
    bytes: Vec<u8>,
    exceeded: bool,
}

#[cfg(target_os = "linux")]
fn drain_bounded(
    mut stream: impl Read + Send + 'static,
) -> thread::JoinHandle<std::io::Result<BoundedOutput>> {
    thread::spawn(move || {
        let mut bytes = Vec::with_capacity(crate::privilege_apply::MAX_REPLY_BYTES.min(4096));
        let mut exceeded = false;
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            let remaining = crate::privilege_apply::MAX_REPLY_BYTES.saturating_sub(bytes.len());
            let accepted = remaining.min(read);
            bytes.extend_from_slice(&buffer[..accepted]);
            exceeded |= accepted != read;
        }
        Ok(BoundedOutput { bytes, exceeded })
    })
}

#[cfg(target_os = "linux")]
fn cleanup(child: &mut ContainedChild) -> bool {
    child.terminate_and_wait(CLEANUP_TIMEOUT).is_ok()
}

#[cfg(any(target_os = "linux", test))]
fn validate_reply_binding(
    request: &PrivilegeApplyRequestV1,
    reply: &PrivilegeApplyReplyV1,
) -> Result<(), CuError> {
    let (request_id, contract_digest, approval_digest) = reply_identity(reply);
    if request_id != request.request_id
        || contract_digest != request.plan.contract_digest()
        || approval_digest != request.plan.approval_digest()
    {
        return Err(transport_unknown("reply-binding"));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
fn reply_identity(reply: &PrivilegeApplyReplyV1) -> (&str, &str, &str) {
    match reply {
        PrivilegeApplyReplyV1::Refused {
            request_id,
            contract_digest,
            approval_digest,
            ..
        }
        | PrivilegeApplyReplyV1::ConsentCanceled {
            request_id,
            contract_digest,
            approval_digest,
            ..
        }
        | PrivilegeApplyReplyV1::Completed {
            request_id,
            contract_digest,
            approval_digest,
            ..
        }
        | PrivilegeApplyReplyV1::FailedBeforeEffect {
            request_id,
            contract_digest,
            approval_digest,
            ..
        }
        | PrivilegeApplyReplyV1::FailedAfterEffect {
            request_id,
            contract_digest,
            approval_digest,
            ..
        }
        | PrivilegeApplyReplyV1::OutcomeUnknown {
            request_id,
            contract_digest,
            approval_digest,
            ..
        } => (request_id, contract_digest, approval_digest),
    }
}

#[cfg(any(target_os = "linux", test))]
fn project_reply(reply: PrivilegeApplyReplyV1) -> Result<Value, CuError> {
    match &reply {
        PrivilegeApplyReplyV1::Completed { .. } => serde_json::to_value(reply).map_err(|_| {
            CuError::new(
                "privilege_reply_invalid",
                "completed provider reply could not be serialized",
            )
        }),
        PrivilegeApplyReplyV1::Refused { error_code, .. } => Err(provider_error(
            error_code,
            "the fixed privilege provider refused the request",
            "not_performed",
            reply.clone(),
        )),
        PrivilegeApplyReplyV1::ConsentCanceled { .. } => Err(provider_error(
            "privilege_consent_canceled",
            "native privilege consent was canceled",
            "not_performed",
            reply.clone(),
        )),
        PrivilegeApplyReplyV1::FailedBeforeEffect { error_code, .. } => Err(provider_error(
            error_code,
            "the privilege provider failed before attempting the effect",
            "not_performed",
            reply.clone(),
        )),
        PrivilegeApplyReplyV1::FailedAfterEffect { error_code, .. } => Err(provider_error(
            error_code,
            "the privilege provider reported failure after attempting the effect",
            "unknown",
            reply.clone(),
        )),
        PrivilegeApplyReplyV1::OutcomeUnknown { .. } => Err(provider_error(
            "privilege_outcome_unknown",
            "the privilege provider cannot prove the effect outcome",
            "unknown",
            reply.clone(),
        )),
    }
}

#[cfg(any(target_os = "linux", test))]
fn provider_error(
    code: &str,
    message: &'static str,
    effect: &'static str,
    reply: PrivilegeApplyReplyV1,
) -> CuError {
    CuError::new(code.to_owned(), message).with_detail(json!({
        "effect": effect,
        "provider_reply": reply,
    }))
}

#[cfg(any(target_os = "linux", test))]
fn transport_unknown(stage: &'static str) -> CuError {
    CuError::new(
        "privilege_outcome_unknown",
        "the privilege provider transport ended without a complete bound reply",
    )
    .with_detail(json!({
        "effect": "unknown",
        "stage": stage,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::privilege_apply::PrivilegePlanV1;

    fn request() -> PrivilegeApplyRequestV1 {
        let plan =
            crate::privilege_plan::process_priority_plan(std::process::id(), 0, 60, 1_000_000)
                .unwrap();
        PrivilegeApplyRequestV1 {
            protocol_version: PRIVILEGE_APPLY_PROTOCOL_VERSION,
            request_id: "request-1".into(),
            plan: PrivilegePlanV1::ProcessPriority(plan),
            authorization: PrivilegeAuthorizationV1::OneShotNativeConsent,
            origin: PrivilegeOriginV1 {
                session_id: "session-1".into(),
                target_scope: PrivilegeTargetScope::Current,
            },
            client: PrivilegeClientV1 {
                contract_version: PRIVILEGE_PROVIDER_CONTRACT_VERSION,
            },
        }
    }

    #[test]
    fn reply_binding_rejects_another_request_identity() {
        let request = request();
        let reply = PrivilegeApplyReplyV1::ConsentCanceled {
            protocol_version: PRIVILEGE_APPLY_PROTOCOL_VERSION,
            request_id: "request-2".into(),
            contract_digest: request.plan.contract_digest().into(),
            approval_digest: request.plan.approval_digest().into(),
        };
        assert_eq!(
            validate_reply_binding(&request, &reply).unwrap_err().code,
            "privilege_outcome_unknown"
        );
    }

    #[test]
    fn tagged_outcome_unknown_stays_unknown() {
        let request = request();
        let reply = PrivilegeApplyReplyV1::OutcomeUnknown {
            protocol_version: PRIVILEGE_APPLY_PROTOCOL_VERSION,
            request_id: request.request_id.clone(),
            contract_digest: request.plan.contract_digest().into(),
            approval_digest: request.plan.approval_digest().into(),
            receipt_id: "123e4567-e89b-42d3-a456-426614174000".into(),
            provider_identity_digest: "a".repeat(64),
            origin_principal_digest: "b".repeat(64),
        };
        validate_reply_binding(&request, &reply).unwrap();
        assert_eq!(
            project_reply(reply).unwrap_err().code,
            "privilege_outcome_unknown"
        );
    }

    #[test]
    fn failed_after_effect_never_claims_a_known_effect_result() {
        let request = request();
        let reply = PrivilegeApplyReplyV1::FailedAfterEffect {
            protocol_version: PRIVILEGE_APPLY_PROTOCOL_VERSION,
            request_id: request.request_id.clone(),
            contract_digest: request.plan.contract_digest().into(),
            approval_digest: request.plan.approval_digest().into(),
            error_code: "privilege_signal_readback_failed".into(),
            receipt_id: "123e4567-e89b-42d3-a456-426614174000".into(),
            receipt_sha256: "c".repeat(64),
            provider_identity_digest: "a".repeat(64),
            origin_principal_digest: "b".repeat(64),
        };
        let error = project_reply(reply).unwrap_err();
        assert_eq!(error.code, "privilege_signal_readback_failed");
        assert_eq!(
            error
                .detail
                .as_ref()
                .and_then(|detail| detail["effect"].as_str()),
            Some("unknown")
        );
    }
}
