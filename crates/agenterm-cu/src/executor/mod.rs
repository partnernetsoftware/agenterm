//! Transports for the abstract command set (PRD_02_30).
//!
//! - `current`: in-process execution through the shared libagenterm dynamic
//!   library (`mechanism` + `dynlib`) only.
//! - `ssh`: OpenSSH `ssh` exec of a remote `agenterm-cu --target current`
//!   worker (`ssh_transport`). Same verbs; transport only.
//! - `vnc`: RFB handshake to a VNC endpoint, then a local
//!   `agenterm-cu --target current` worker against the shared session
//!   (`vnc_transport`). Same verbs; transport only.
//! - `rdp`: fail-closed placeholder (`rdp_transport`). Parseable target and
//!   `--rdp host[:port]` endpoint; `capabilities` declares the placeholder
//!   truthfully with no socket I/O (cut 3.47); every other authorized
//!   command returns `rdp_unavailable` with no socket I/O (cut 3.46).

use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::mechanism::window_enumerate::WindowInfo;

use crate::{
    audit::{self, AuditLog},
    auth::{Authorization, Grant},
    auth_store::{AuthStore, AuthStoreErrorKind, GrantAttempt, GrantDecision, GrantDenialKind},
    command::{
        Command, InvokeAction, InvokeValueKind, OrderRelation, PointerButton, WaitCondition,
    },
    idempotency_store::{
        FinalOutcome, FinalOutcomeKind, FinalReplay, IdempotencyStore, MAX_RETENTION_TTL_MS,
        ReserveDecision, fingerprint_canonical_request_with_secret,
    },
    mechanism, network_probe, observe,
    rdp_transport::{self, RdpEndpoint},
    receipt::{self, ReceiptLog},
    reply::{CuError, CuReply},
    runtime_coordinator::RuntimeCoordinator,
    ssh_transport::{self, SshEndpoint},
    target::TargetRef,
    target_binding::{CurrentIdentityProvider, resolve_target_binding},
    vnc_transport::{self, VncEndpoint},
};

mod a11y_actuate;
mod a11y_observe;
mod app_lifecycle;
mod browser;
mod browser_sessions;
mod capabilities;
mod clipboard;
mod desktop_state;
mod device_capture;
mod dispatch;
mod errors;
mod files;
mod host_open;
mod invoke;
mod managed_jobs;
mod menus;
mod network_interfaces;
mod node_match;
mod persisted;
mod placement;
mod pointer;
mod process;
mod profiles;
mod pty_jobs;
mod receipts;
mod runtime;
mod shell_exec;
mod snapshots;
mod terminal;
#[cfg(test)]
mod test_support;
mod text_input;
mod wait;
mod window_state;
mod windows;

pub use errors::{ACCESSIBILITY_REPAIR_PATH, SCREEN_RECORDING_REPAIR_PATH};

use a11y_actuate::*;
use a11y_observe::*;
use app_lifecycle::*;
use browser::*;
use browser_sessions::*;
use capabilities::*;
use clipboard::*;
use desktop_state::*;
use device_capture::*;
use errors::*;
use files::*;
use host_open::*;
use invoke::*;
use managed_jobs::*;
use menus::*;
use network_interfaces::*;
use node_match::*;
use persisted::now_utc_ms;
use placement::*;
use pointer::*;
use process::*;
use profiles::*;
use pty_jobs::*;
use receipts::*;
use runtime::*;
use shell_exec::*;
use snapshots::*;
use terminal::*;
#[cfg(test)]
use test_support::*;
use text_input::*;
use wait::*;
use window_state::*;
use windows::*;

pub struct Executor {
    auth: Authorization,
    ssh: Option<SshEndpoint>,
    vnc: Option<VncEndpoint>,
    rdp: Option<RdpEndpoint>,
    persisted: Option<PersistedAuthorization>,
    request_identity: Option<RequestIdentity>,
    request_effect_scope: Option<String>,
    #[cfg(test)]
    audit_path: Option<PathBuf>,
    #[cfg(test)]
    audit_failure: Option<crate::audit::InjectedAuditFailure>,
    #[cfg(test)]
    persisted_binding: Option<crate::target_binding::TargetBinding>,
    #[cfg(test)]
    request_store_path: Option<PathBuf>,
    #[cfg(test)]
    runtime_path: Option<PathBuf>,
}

/// Caller-owned at-most-once identity for one mutating command. The lease is a
/// bearer secret and is never serialized into command, audit or request state.
#[derive(Clone)]
pub struct RequestIdentity {
    pub request_id: String,
    pub session_id: String,
    pub session_lease: String,
}

pub(super) struct PersistedAuthorization {
    grant_id: String,
    store_path: PathBuf,
}

impl Executor {
    pub fn new(auth: Authorization) -> Self {
        Self {
            auth,
            ssh: None,
            vnc: None,
            rdp: None,
            persisted: None,
            request_identity: None,
            request_effect_scope: None,
            #[cfg(test)]
            audit_path: None,
            #[cfg(test)]
            audit_failure: None,
            #[cfg(test)]
            persisted_binding: None,
            #[cfg(test)]
            request_store_path: None,
            #[cfg(test)]
            runtime_path: None,
        }
    }

    pub fn with_request_identity(mut self, identity: RequestIdentity) -> Self {
        self.request_identity = Some(identity);
        self
    }

    #[doc(hidden)]
    pub fn with_request_effect_scope(mut self, effect_scope: String) -> Self {
        self.request_effect_scope = Some(effect_scope);
        self
    }

    #[cfg(test)]
    fn with_audit_path(mut self, path: PathBuf) -> Self {
        self.audit_path = Some(path);
        self
    }

    #[cfg(test)]
    fn with_audit_failure(mut self, failure: crate::audit::InjectedAuditFailure) -> Self {
        self.audit_failure = Some(failure);
        self
    }

    #[cfg(test)]
    fn with_persisted_binding(mut self, binding: crate::target_binding::TargetBinding) -> Self {
        self.persisted_binding = Some(binding);
        self
    }

    #[cfg(test)]
    fn with_request_state_paths(mut self, request_store: PathBuf, runtime: PathBuf) -> Self {
        self.request_store_path = Some(request_store);
        self.runtime_path = Some(runtime);
        self
    }

    pub fn with_ssh(mut self, endpoint: SshEndpoint) -> Self {
        self.ssh = Some(endpoint);
        self
    }

    pub fn with_vnc(mut self, endpoint: VncEndpoint) -> Self {
        self.vnc = Some(endpoint);
        self
    }

    pub fn with_rdp(mut self, endpoint: RdpEndpoint) -> Self {
        self.rdp = Some(endpoint);
        self
    }

    pub fn with_persisted_grant(
        mut self,
        grant_id: impl Into<String>,
        store_path: impl Into<PathBuf>,
    ) -> Self {
        self.persisted = Some(PersistedAuthorization {
            grant_id: grant_id.into(),
            store_path: store_path.into(),
        });
        self
    }

    pub fn execute(&self, command: &Command) -> CuReply {
        if let Some(identity) = self.request_identity.as_ref() {
            return self.execute_with_request_identity(command, identity);
        }
        if let Some(persisted) = self.persisted.as_ref() {
            return self.execute_persisted(command, persisted);
        }
        let required = command.required_grant();
        if !self.auth.allows(required) {
            return CuReply::err(
                command,
                CuError::new(
                    "refused",
                    format!(
                        "command requires {:?} grant; pass --grant or set AGENTERM_CU_GRANT",
                        required
                    ),
                ),
            );
        }

        let mut audit = if required == Grant::Actuate {
            match self.begin_audit(command) {
                Ok(audit) => Some(audit),
                Err(error) => return CuReply::err(command, error),
            }
        } else {
            None
        };

        let reply = match command.target() {
            TargetRef::Current => self.execute_current(command),
            TargetRef::Ssh => self.execute_ssh(command),
            TargetRef::Vnc => self.execute_vnc(command),
            TargetRef::Rdp => self.execute_rdp(command),
        };

        if let Some(audit) = audit.as_mut()
            && let Err(mut error) = Self::audit_after(audit, command, &reply)
        {
            let mechanism_reply = serde_json::to_string(&reply)
                .unwrap_or_else(|_| "<unserializable mechanism reply>".to_owned());
            let effect = if reply.ok {
                reply
                    .data
                    .as_ref()
                    .and_then(|data| data.get("effect"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("committed")
            } else {
                reply
                    .error
                    .as_ref()
                    .and_then(|error| error.detail.as_ref())
                    .and_then(|detail| detail.get("effect"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown")
            };
            error.detail = Some(serde_json::json!({
                "stage": "audit_outcome",
                "effect": effect,
                "original_reply": reply,
            }));
            error.message = format!(
                "{}; original mechanism reply: {mechanism_reply}",
                error.message
            );
            return CuReply::err(command, error);
        }

        reply
    }

    fn execute_with_request_identity(
        &self,
        command: &Command,
        identity: &RequestIdentity,
    ) -> CuReply {
        if command.required_grant() != Grant::Actuate {
            return CuReply::err(
                command,
                CuError::new(
                    "request_identity_not_actuation",
                    "caller request identity is valid only for mutating commands",
                ),
            );
        }
        if self.persisted.is_some() {
            return CuReply::err(
                command,
                CuError::new(
                    "request_identity_persisted_grant_unavailable",
                    "caller request identity is not yet composed with persisted grant consumption",
                ),
            );
        }
        if !self.auth.allows(Grant::Actuate) {
            return CuReply::err(
                command,
                CuError::new(
                    "refused",
                    "command requires Actuate grant; pass --grant or set AGENTERM_CU_GRANT",
                ),
            );
        }

        match command.target() {
            TargetRef::Ssh => return self.execute_ssh_with_request_identity(command, identity),
            TargetRef::Vnc => return self.execute_vnc_with_request_identity(command, identity),
            TargetRef::Rdp => return self.execute_rdp(command),
            TargetRef::Current => {}
        }

        let now_ms = match now_utc_ms() {
            Some(now) => now,
            None => {
                return CuReply::err(
                    command,
                    CuError::new("request_clock_invalid", "system clock is unavailable"),
                );
            }
        };
        let now_s = now_ms / 1_000;
        let runtime = match self.open_runtime_coordinator() {
            Ok(runtime) => runtime,
            Err(error) => return CuReply::err(command, error),
        };
        if let Err(error) =
            runtime.session_verify(&identity.session_id, &identity.session_lease, now_s)
        {
            return CuReply::err(command, error);
        }

        let canonical = match serde_json::to_vec(&serde_json::json!({
            "session_id": identity.session_id,
            "effect_scope": self.request_effect_scope.as_deref().unwrap_or("current"),
            "command": command,
        })) {
            Ok(bytes) => bytes,
            Err(_) => {
                return CuReply::err(
                    command,
                    CuError::new(
                        "request_fingerprint_unavailable",
                        "command could not be projected into a request fingerprint",
                    ),
                );
            }
        };
        let fingerprint = match fingerprint_canonical_request_with_secret(
            &canonical,
            identity.session_lease.as_bytes(),
        ) {
            Ok(fingerprint) => fingerprint,
            Err(error) => return CuReply::err(command, error),
        };
        let store = match self.open_idempotency_store() {
            Ok(store) => store,
            Err(error) => return CuReply::err(command, error),
        };
        let reservation = match store.reserve(
            &identity.request_id,
            &fingerprint,
            MAX_RETENTION_TTL_MS,
            now_ms,
        ) {
            Ok(ReserveDecision::Fresh(reservation)) => reservation,
            Ok(ReserveDecision::ReplayFinalized(status)) => {
                let succeeded = status
                    .outcome
                    .as_ref()
                    .is_some_and(|outcome| outcome.kind == FinalOutcomeKind::Succeeded);
                if succeeded {
                    if matches!(command, Command::JobSpawn { .. }) {
                        return match status
                            .outcome
                            .as_ref()
                            .and_then(|outcome| outcome.replay.as_ref())
                        {
                            Some(replay @ FinalReplay::JobSpawn { .. }) => {
                                CuReply::ok(command, replay_payload(replay))
                            }
                            None => CuReply::err(
                                command,
                                CuError::new(
                                    "managed_job_replay_projection_missing",
                                    "completed job-spawn has no sealed public replay result",
                                ),
                            ),
                        };
                    }
                    return CuReply::ok(
                        command,
                        serde_json::json!({
                            "effect": "not_repeated",
                            "idempotent": true,
                            "request": status,
                        }),
                    );
                }
                let detail = if matches!(command, Command::JobSpawn { .. }) {
                    serde_json::json!({
                        "effect": "not_repeated",
                        "idempotent": true,
                    })
                } else {
                    serde_json::json!({
                        "effect": "not_repeated",
                        "idempotent": true,
                        "request": status,
                    })
                };
                return CuReply::err(
                    command,
                    CuError::new(
                        "request_previously_failed",
                        "this request identity already has a terminal failed receipt",
                    )
                    .with_detail(detail),
                );
            }
            Ok(ReserveDecision::Uncertain(status)) => {
                let detail = if matches!(command, Command::JobSpawn { .. }) {
                    serde_json::json!({
                        "effect": "unknown",
                        "idempotent": true,
                    })
                } else {
                    serde_json::json!({
                        "effect": "unknown",
                        "idempotent": true,
                        "request": status,
                    })
                };
                return CuReply::err(
                    command,
                    CuError::new(
                        "request_outcome_unknown",
                        "this request identity may already have been delivered; automatic replay is refused",
                    )
                    .with_detail(detail),
                );
            }
            Err(error) => return CuReply::err(command, error),
        };

        let mut audit = match self.begin_audit(command) {
            Ok(audit) => audit,
            Err(error) => {
                let _ = store.finalize(
                    &identity.request_id,
                    &fingerprint,
                    &reservation.completion_token,
                    FinalOutcome::new(FinalOutcomeKind::Failed, "audit_unavailable", None)
                        .expect("static outcome is valid"),
                    now_ms,
                );
                return CuReply::err(command, error);
            }
        };
        let reply = match self.run_current(
            command,
            Some(&JobRequestContext {
                session_id: &identity.session_id,
                session_lease: &identity.session_lease,
                runtime: &runtime,
            }),
        ) {
            Ok(data) => CuReply::ok(command, data),
            Err(error) => CuReply::err(command, error),
        };
        if let Err(mut error) = Self::audit_after(&mut audit, command, &reply) {
            let _ = store.mark_outcome_unknown(
                &identity.request_id,
                &fingerprint,
                &reservation.completion_token,
                now_utc_ms().unwrap_or(now_ms),
            );
            error.detail = Some(serde_json::json!({
                "stage": "audit_outcome",
                "effect": "unknown",
                "request_id": identity.request_id,
                "original_reply": reply,
            }));
            return CuReply::err(command, error);
        }

        if reply
            .error
            .as_ref()
            .is_some_and(|error| error.code == "managed_job_outcome_unknown")
        {
            let _ = store.mark_outcome_unknown(
                &identity.request_id,
                &fingerprint,
                &reservation.completion_token,
                now_utc_ms().unwrap_or(now_ms),
            );
            return reply;
        }

        let outcome = if reply.ok {
            let outcome = FinalOutcome::new(FinalOutcomeKind::Succeeded, "ok", None);
            if matches!(command, Command::JobSpawn { .. }) {
                outcome.and_then(|outcome| {
                    reply
                        .data
                        .as_ref()
                        .ok_or_else(|| {
                            CuError::new(
                                "managed_job_replay_projection_invalid",
                                "successful job-spawn omitted its public identity",
                            )
                        })
                        .and_then(replay_from_spawn_reply)
                        .and_then(|replay| outcome.with_replay(replay))
                })
            } else {
                outcome
            }
        } else {
            FinalOutcome::new(
                FinalOutcomeKind::Failed,
                reply
                    .error
                    .as_ref()
                    .map_or("failed", |error| error.code.as_str()),
                None,
            )
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = store.mark_outcome_unknown(
                    &identity.request_id,
                    &fingerprint,
                    &reservation.completion_token,
                    now_utc_ms().unwrap_or(now_ms),
                );
                return CuReply::err(command, error);
            }
        };
        if let Err(error) = store.finalize(
            &identity.request_id,
            &fingerprint,
            &reservation.completion_token,
            outcome,
            now_utc_ms().unwrap_or(now_ms),
        ) {
            let _ = store.mark_outcome_unknown(
                &identity.request_id,
                &fingerprint,
                &reservation.completion_token,
                now_utc_ms().unwrap_or(now_ms),
            );
            return CuReply::err(
                command,
                CuError::new(
                    "request_outcome_persist_failed",
                    "effect finished but its at-most-once receipt could not be durably finalized",
                )
                .with_detail(serde_json::json!({
                    "effect": "unknown",
                    "request_id": identity.request_id,
                    "store_error": error.code,
                })),
            );
        }
        reply
    }

    fn open_runtime_coordinator(&self) -> Result<RuntimeCoordinator, CuError> {
        #[cfg(test)]
        if let Some(path) = self.runtime_path.as_ref() {
            return RuntimeCoordinator::open_at(path);
        }
        RuntimeCoordinator::open()
    }

    fn open_idempotency_store(&self) -> Result<IdempotencyStore, CuError> {
        #[cfg(test)]
        if let Some(path) = self.request_store_path.as_ref() {
            return IdempotencyStore::open_at(path);
        }
        IdempotencyStore::open()
    }

    fn execute_ssh(&self, command: &Command) -> CuReply {
        let Some(endpoint) = self.ssh.as_ref() else {
            return CuReply::err(
                command,
                CuError::new(
                    "invalid_input",
                    "ssh target requires --ssh <user@host> (or AGENTERM_CU_SSH)",
                ),
            );
        };
        match ssh_transport::run_remote(endpoint, command, &self.auth, None) {
            Ok(reply) => reply,
            Err(error) => CuReply::err(command, error),
        }
    }

    fn execute_ssh_with_request_identity(
        &self,
        command: &Command,
        identity: &RequestIdentity,
    ) -> CuReply {
        let Some(endpoint) = self.ssh.as_ref() else {
            return CuReply::err(
                command,
                CuError::new(
                    "invalid_input",
                    "ssh target requires --ssh <user@host> (or AGENTERM_CU_SSH)",
                ),
            );
        };
        match ssh_transport::run_remote(endpoint, command, &self.auth, Some(identity)) {
            Ok(reply) => reply,
            Err(error) => CuReply::err(command, error),
        }
    }

    fn execute_vnc(&self, command: &Command) -> CuReply {
        let Some(endpoint) = self.vnc.as_ref() else {
            return CuReply::err(
                command,
                CuError::new(
                    "invalid_input",
                    "vnc target requires --vnc <host[:port]> (or AGENTERM_CU_VNC)",
                ),
            );
        };
        match vnc_transport::run_session(endpoint, command, &self.auth, None) {
            Ok(reply) => reply,
            Err(error) => CuReply::err(command, error),
        }
    }

    fn execute_vnc_with_request_identity(
        &self,
        command: &Command,
        identity: &RequestIdentity,
    ) -> CuReply {
        let Some(endpoint) = self.vnc.as_ref() else {
            return CuReply::err(
                command,
                CuError::new(
                    "invalid_input",
                    "vnc target requires --vnc <host[:port]> (or AGENTERM_CU_VNC)",
                ),
            );
        };
        match vnc_transport::run_session(endpoint, command, &self.auth, Some(identity)) {
            Ok(reply) => reply,
            Err(error) => CuReply::err(command, error),
        }
    }

    fn execute_rdp(&self, command: &Command) -> CuReply {
        // Cut 3.47: capabilities is the one observe path that succeeds for
        // the RDP placeholder. It returns a static declaration (transport
        // placeholder/unavailable; tree unsupported) and never dials.
        // Authorization already ran above, so a missing observe grant stays
        // `refused`. Missing endpoint still declares the tier truthfully.
        if matches!(command, Command::Capabilities { .. }) {
            return CuReply::ok(
                command,
                rdp_transport::capabilities_declaration(self.rdp.as_ref()),
            );
        }
        // Missing endpoint and configured-but-unimplemented transport both
        // fail closed as `rdp_unavailable` (cut 3.46).
        let Some(endpoint) = self.rdp.as_ref() else {
            return CuReply::err(
                command,
                CuError::new(
                    "rdp_unavailable",
                    "RDP target requires --rdp HOST[:PORT]; the RDP transport is not implemented",
                ),
            );
        };
        match rdp_transport::run_session(endpoint, command, &self.auth) {
            Ok(reply) => reply,
            Err(error) => CuReply::err(command, error),
        }
    }

    fn begin_audit(&self, command: &Command) -> Result<AuditLog, CuError> {
        let mut audit = self.open_audit()?;
        audit.record_actuation(command.target(), command, Grant::Actuate, "attempt", None)?;
        Ok(audit)
    }

    pub(super) fn open_audit(&self) -> Result<AuditLog, CuError> {
        #[cfg(test)]
        let mut audit = if let Some(path) = self.audit_path.as_ref() {
            AuditLog::open_at(path)?
        } else {
            AuditLog::open()?
        };
        #[cfg(not(test))]
        let audit = AuditLog::open()?;
        #[cfg(test)]
        if let Some(failure) = self.audit_failure {
            audit.inject_failure(failure);
        }
        Ok(audit)
    }

    fn audit_after(
        audit: &mut AuditLog,
        command: &Command,
        reply: &CuReply,
    ) -> Result<(), CuError> {
        let outcome = if reply.ok { "ok" } else { "failed" };
        let detail = if matches!(command, Command::SessionStart { .. }) {
            reply
                .data
                .as_ref()
                .map(|data| {
                    serde_json::json!({
                        "session_id": data.get("session_id"),
                        "label": data.get("label"),
                        "expires_at_utc_s": data.get("expires_at_utc_s"),
                        "lease_redacted": true,
                    })
                })
                .or_else(|| {
                    reply.error.as_ref().map(|error| {
                        serde_json::json!({
                            "code": error.code,
                            "lease_redacted": true,
                        })
                    })
                })
        } else if matches!(command, Command::ShellExec { .. }) {
            // Shell output routinely contains credentials and other private
            // material. Preserve bounded execution evidence without copying
            // either stream into the persistent actuation journal.
            reply
                .data
                .as_ref()
                .map(|data| {
                    serde_json::json!({
                        "schema_version": data.get("schema_version"),
                        "shell": data.get("shell"),
                        "pid": data.get("pid"),
                        "elapsed_ms": data.get("elapsed_ms"),
                        "exit": data.get("exit"),
                        "success": data.get("success"),
                        "stdout_bytes": data.get("stdout_bytes"),
                        "stderr_bytes": data.get("stderr_bytes"),
                        "output_complete": data.get("output_complete"),
                        "cleanup": data.get("cleanup"),
                        "output_redacted": true,
                    })
                })
                .or_else(|| {
                    reply.error.as_ref().map(|error| {
                    serde_json::json!({
                        "code": error.code,
                        "cleanup": error.detail.as_ref().and_then(|detail| detail.get("cleanup")),
                        "output_redacted": true,
                    })
                })
                })
        } else {
            reply.data.clone().or_else(|| {
                reply.error.as_ref().map(|error| {
                    serde_json::to_value(error).unwrap_or_else(
                        |_| serde_json::json!({ "code": error.code, "message": error.message }),
                    )
                })
            })
        };
        audit.record_actuation(command.target(), command, Grant::Actuate, outcome, detail)
    }

    pub(super) fn execute_current(&self, command: &Command) -> CuReply {
        match self.run_current(command, None) {
            Ok(data) => CuReply::ok(command, data),
            Err(error) => CuReply::err(command, error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_request_identity_replays_terminal_receipt_without_repeating_effect() {
        let audit_path = audit_scratch("request-identity");
        let root = audit_path.parent().expect("scratch root");
        let runtime_path = root.join("runtime.json");
        let request_path = root.join("requests.json");
        let now_ms = now_utc_ms().expect("test clock");
        let session = RuntimeCoordinator::open_at(&runtime_path)
            .unwrap()
            .session_start(Some("request fixture"), 60, now_ms / 1_000)
            .unwrap();
        let identity = RequestIdentity {
            request_id: "fixture.request-1".into(),
            session_id: session.session_id,
            session_lease: session.lease.clone(),
        };
        let executor = actuate_executor()
            .with_audit_path(audit_path.clone())
            .with_request_state_paths(request_path.clone(), runtime_path)
            .with_request_identity(identity);
        let command = Command::ClipboardClear {
            target: TargetRef::Current,
            apply: false,
        };

        let first = executor.execute(&command);
        assert!(first.ok, "{first:?}");
        assert_eq!(first.data.as_ref().unwrap()["status"], "planned");

        let replay = executor.execute(&command);
        assert!(replay.ok, "{replay:?}");
        assert_eq!(replay.data.as_ref().unwrap()["effect"], "not_repeated");
        assert_eq!(replay.data.as_ref().unwrap()["idempotent"], true);

        let conflict = executor.execute(&Command::ClipboardClear {
            target: TargetRef::Current,
            apply: true,
        });
        assert!(!conflict.ok);
        assert_eq!(conflict.error.as_ref().unwrap().code, "request_id_conflict");
        let state = std::fs::read_to_string(request_path).unwrap();
        assert!(!state.contains(&session.lease));
        remove_audit_scratch(&audit_path);
    }

    #[test]
    fn request_identity_conflicts_when_transport_effect_scope_changes() {
        let audit_path = audit_scratch("request-effect-scope");
        let root = audit_path.parent().expect("scratch root");
        let runtime_path = root.join("runtime.json");
        let request_path = root.join("requests.json");
        let now_ms = now_utc_ms().expect("test clock");
        let session = RuntimeCoordinator::open_at(&runtime_path)
            .unwrap()
            .session_start(Some("scope fixture"), 60, now_ms / 1_000)
            .unwrap();
        let identity = RequestIdentity {
            request_id: "fixture.scope-request".into(),
            session_id: session.session_id,
            session_lease: session.lease,
        };
        let command = Command::ClipboardClear {
            target: TargetRef::Current,
            apply: false,
        };
        let first = actuate_executor()
            .with_audit_path(audit_path.clone())
            .with_request_state_paths(request_path.clone(), runtime_path.clone())
            .with_request_identity(identity.clone())
            .with_request_effect_scope(crate::worker_wire::effect_scope("vnc", &["fixture:5900"]))
            .execute(&command);
        assert!(first.ok, "{first:?}");

        let conflict = actuate_executor()
            .with_audit_path(audit_path.clone())
            .with_request_state_paths(request_path, runtime_path)
            .with_request_identity(identity)
            .with_request_effect_scope(crate::worker_wire::effect_scope("vnc", &["fixture:5901"]))
            .execute(&command);
        assert!(!conflict.ok);
        assert_eq!(conflict.error.as_ref().unwrap().code, "request_id_conflict");
        remove_audit_scratch(&audit_path);
    }

    #[test]
    fn audit_open_failure_prevents_actuation_dispatch() {
        let path = audit_scratch("open-failure");
        std::fs::create_dir_all(&path).expect("create directory at audit file path");
        let executor = actuate_executor().with_audit_path(path.clone());
        let command = Command::WindowPlace {
            target: TargetRef::Rdp,
            action: "left-half".into(),
            window: None,
            frame: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "audit_unavailable");
        assert!(
            !reply
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("rdp_unavailable"),
            "RDP dispatch must not run after the audit transaction cannot open"
        );
        remove_audit_scratch(&path);
    }

    #[test]
    fn outcome_append_failure_returns_typed_error_with_mechanism_context() {
        let path = audit_scratch("outcome-failure");
        let executor = actuate_executor()
            .with_audit_path(path.clone())
            .with_audit_failure(crate::audit::InjectedAuditFailure::AppendAfter(1));
        let command = Command::WindowPlace {
            target: TargetRef::Rdp,
            action: "left-half".into(),
            window: None,
            frame: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        let error = reply.error.as_ref().expect("typed audit failure");
        assert_eq!(error.code, "audit_unavailable");
        assert!(error.message.contains("original mechanism reply"));
        assert!(
            error.message.contains("rdp_unavailable"),
            "the replaced mechanism failure must remain observable: {}",
            error.message
        );
        let detail = error.detail.as_ref().expect("structured audit context");
        assert_eq!(detail["stage"], "audit_outcome");
        assert_eq!(detail["effect"], "unknown");
        assert_eq!(detail["original_reply"]["error"]["code"], "rdp_unavailable");
        let records: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .expect("read isolated audit")
            .lines()
            .map(|line| serde_json::from_str(line).expect("record JSON"))
            .collect();
        assert_eq!(records.len(), 1, "failed outcome append must add no record");
        assert_eq!(records[0]["outcome"], "attempt");
        remove_audit_scratch(&path);
    }

    #[test]
    fn failed_audit_outcome_preserves_structured_effect_detail() {
        let path = audit_scratch("structured-effect");
        let mut audit = AuditLog::open_at(&path).expect("open isolated audit");
        let command = Command::WindowPlace {
            target: TargetRef::Current,
            action: "left-half".into(),
            window: Some(7),
            frame: None,
        };
        let reply = CuReply::err(
            &command,
            CuError::new("history_commit_failed", "injected")
                .with_detail(serde_json::json!({ "effect": "rolled_back" })),
        );
        Executor::audit_after(&mut audit, &command, &reply).expect("audit outcome");
        let record: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(&path).expect("read audit").trim())
                .expect("audit JSON");
        assert_eq!(record["outcome"], "failed");
        assert_eq!(record["detail"]["code"], "history_commit_failed");
        assert_eq!(record["detail"]["detail"]["effect"], "rolled_back");
        remove_audit_scratch(&path);
    }

    #[test]
    fn shell_exec_audit_keeps_metrics_but_never_persists_output() {
        let path = audit_scratch("shell-output-redaction");
        let mut audit = AuditLog::open_at(&path).expect("open isolated audit");
        let command = Command::ShellExec {
            target: TargetRef::Current,
            command: "printf private-command-material".into(),
            timeout_ms: 1_000,
            max_output_bytes: 1_024,
        };
        let reply = CuReply::ok(
            &command,
            serde_json::json!({
                "schema_version": 1,
                "shell": "sh",
                "pid": 7,
                "elapsed_ms": 2,
                "exit": {"kind": "code", "code": 0},
                "success": true,
                "stdout": "private-stdout-material",
                "stderr": "private-stderr-material",
                "stdout_bytes": 23,
                "stderr_bytes": 23,
                "output_complete": true,
                "cleanup": "root-exited",
            }),
        );
        Executor::audit_after(&mut audit, &command, &reply).expect("audit outcome");
        let text = std::fs::read_to_string(&path).expect("read audit");
        assert!(!text.contains("private-command-material"));
        assert!(!text.contains("private-stdout-material"));
        assert!(!text.contains("private-stderr-material"));
        let record: serde_json::Value = serde_json::from_str(text.trim()).expect("audit JSON");
        assert_eq!(record["detail"]["output_redacted"], true);
        assert_eq!(record["detail"]["stdout_bytes"], 23);
        assert_eq!(record["detail"]["stderr_bytes"], 23);
        remove_audit_scratch(&path);
    }

    #[test]
    fn rdp_tree_without_endpoint_is_rdp_unavailable() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Tree {
            target: TargetRef::Rdp,
            window: Some(0x1000),
            depth: None,
            max_nodes: None,
            flat: false,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.target, "rdp");
        assert_eq!(reply.command, "tree");
        assert_eq!(reply.error.as_ref().unwrap().code, "rdp_unavailable");
        assert!(reply.data.is_none());
    }

    #[test]
    fn rdp_capabilities_declares_placeholder_without_endpoint() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Capabilities {
            target: TargetRef::Rdp,
        };
        let reply = executor.execute(&command);
        assert!(reply.ok);
        assert_eq!(reply.target, "rdp");
        assert_eq!(reply.command, "capabilities");
        let data = reply.data.as_ref().expect("data");
        assert_eq!(data["target"], "rdp");
        assert_eq!(data["transport"]["status"], "placeholder");
        assert_eq!(data["transport"]["available"], false);
        assert_eq!(data["transport"]["reason"], "rdp_unavailable");
        assert_eq!(data["verbs"]["capabilities"]["status"], "available");
        assert_eq!(data["verbs"]["tree"]["status"], "unsupported");
        assert_eq!(data["verbs"]["tree"]["reason"], "rdp_unavailable");
    }

    #[test]
    fn rdp_capabilities_with_endpoint_does_not_connect() {
        use std::net::TcpListener;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind sentinel");
        listener
            .set_nonblocking(true)
            .expect("nonblocking sentinel");
        let addr = listener.local_addr().expect("local addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_bg = Arc::clone(&hits);
        let sentinel = thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_millis(200);
            while std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok(_) => {
                        hits_bg.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        let endpoint = RdpEndpoint {
            host: addr.ip().to_string(),
            port: addr.port(),
        };
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth).with_rdp(endpoint.clone());
        let command = Command::Capabilities {
            target: TargetRef::Rdp,
        };
        let reply = executor.execute(&command);
        assert!(reply.ok);
        assert_eq!(reply.target, "rdp");
        assert_eq!(reply.command, "capabilities");
        let data = reply.data.as_ref().expect("data");
        assert_eq!(data["target"], "rdp");
        assert_eq!(data["transport"]["reason"], "rdp_unavailable");
        assert_eq!(
            data["transport"]["endpoint"],
            format!("{}:{}", addr.ip(), addr.port())
        );
        assert_eq!(data["verbs"]["tree"]["status"], "unsupported");

        sentinel.join().expect("sentinel join");
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        // tree remains fail-closed after a successful capabilities declaration
        let tree = executor.execute(&Command::Tree {
            target: TargetRef::Rdp,
            window: Some(1),
            depth: None,
            max_nodes: None,
            flat: false,
        });
        assert!(!tree.ok);
        assert_eq!(tree.error.as_ref().unwrap().code, "rdp_unavailable");
        let _ = endpoint;
    }

    #[test]
    fn rdp_capabilities_missing_observe_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth).with_rdp(RdpEndpoint {
            host: "WINDOWS_HOST".into(),
            port: 3389,
        });
        let command = Command::Capabilities {
            target: TargetRef::Rdp,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.target, "rdp");
        assert_eq!(reply.command, "capabilities");
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn rdp_tree_with_endpoint_is_rdp_unavailable_and_does_not_connect() {
        use std::net::TcpListener;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind sentinel");
        listener
            .set_nonblocking(true)
            .expect("nonblocking sentinel");
        let addr = listener.local_addr().expect("local addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_bg = Arc::clone(&hits);
        let sentinel = thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_millis(200);
            while std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok(_) => {
                        hits_bg.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        let endpoint = RdpEndpoint {
            host: addr.ip().to_string(),
            port: addr.port(),
        };
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth).with_rdp(endpoint);
        let command = Command::Tree {
            target: TargetRef::Rdp,
            window: Some(0x1000),
            depth: None,
            max_nodes: None,
            flat: false,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.target, "rdp");
        assert_eq!(reply.command, "tree");
        assert_eq!(reply.error.as_ref().unwrap().code, "rdp_unavailable");

        sentinel.join().expect("sentinel join");
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn rdp_missing_observe_grant_is_refused_not_rdp_unavailable() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth).with_rdp(RdpEndpoint {
            host: "127.0.0.1".into(),
            port: 3389,
        });
        let command = Command::Tree {
            target: TargetRef::Rdp,
            window: Some(1),
            depth: None,
            max_nodes: None,
            flat: false,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.target, "rdp");
        assert_eq!(reply.command, "tree");
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }
}
