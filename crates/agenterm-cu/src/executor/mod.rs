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
    audit::AuditLog,
    auth::{Authorization, Grant},
    auth_store::{AuthStore, AuthStoreErrorKind, GrantAttempt, GrantDecision, GrantDenialKind},
    command::{
        Command, InvokeAction, InvokeValueKind, OrderRelation, PointerButton, WaitCondition,
    },
    mechanism, network_probe, observe,
    rdp_transport::{self, RdpEndpoint},
    receipt::{self, ReceiptLog},
    reply::{CuError, CuReply},
    ssh_transport::{self, SshEndpoint},
    target::TargetRef,
    target_binding::{CurrentIdentityProvider, resolve_target_binding},
    vnc_transport::{self, VncEndpoint},
};

mod a11y_actuate;
mod a11y_observe;
mod app_lifecycle;
mod browser;
mod capabilities;
mod clipboard;
mod dispatch;
mod errors;
mod invoke;
mod menus;
mod node_match;
mod persisted;
mod placement;
mod pointer;
mod process;
mod profiles;
mod receipts;
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
use capabilities::*;
use clipboard::*;
use errors::*;
use invoke::*;
use menus::*;
use node_match::*;
use placement::*;
use pointer::*;
use process::*;
use profiles::*;
use receipts::*;
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
    #[cfg(test)]
    audit_path: Option<PathBuf>,
    #[cfg(test)]
    audit_failure: Option<crate::audit::InjectedAuditFailure>,
    #[cfg(test)]
    persisted_binding: Option<crate::target_binding::TargetBinding>,
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
            #[cfg(test)]
            audit_path: None,
            #[cfg(test)]
            audit_failure: None,
            #[cfg(test)]
            persisted_binding: None,
        }
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
        match ssh_transport::run_remote(endpoint, command, &self.auth) {
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
        match vnc_transport::run_session(endpoint, command, &self.auth) {
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
        audit.record_actuation(
            command.target(),
            command,
            Grant::Actuate,
            outcome,
            reply.data.clone().or_else(|| {
                reply.error.as_ref().map(|error| {
                    serde_json::to_value(error).unwrap_or_else(
                        |_| serde_json::json!({ "code": error.code, "message": error.message }),
                    )
                })
            }),
        )
    }

    pub(super) fn execute_current(&self, command: &Command) -> CuReply {
        match self.run_current(command) {
            Ok(data) => CuReply::ok(command, data),
            Err(error) => CuReply::err(command, error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
