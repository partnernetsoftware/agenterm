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
    mechanism, observe,
    rdp_transport::{self, RdpEndpoint},
    receipt::{self, ReceiptLog},
    reply::{CuError, CuReply},
    ssh_transport::{self, SshEndpoint},
    target::TargetRef,
    target_binding::{CurrentIdentityProvider, resolve_target_binding},
    vnc_transport::{self, VncEndpoint},
};

/// Where the macOS Accessibility permission is granted. Quoted in the typed
/// `denied` reply so an agent can relay the repair path without guessing.
pub const ACCESSIBILITY_REPAIR_PATH: &str = "System Settings > Privacy & Security > Accessibility: enable the process that runs agenterm-cu (or its parent terminal / launcher), then rerun";

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

struct PersistedAuthorization {
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

    fn execute_persisted(&self, command: &Command, persisted: &PersistedAuthorization) -> CuReply {
        if command.target() != TargetRef::Current {
            return CuReply::err(
                command,
                CuError::new(
                    "persisted_grant_remote_unsupported",
                    "persisted grants currently authorize only the current target",
                ),
            );
        }
        let decision_id = match generated_decision_id() {
            Some(id) => id,
            None => {
                return CuReply::err(
                    command,
                    CuError::new("authorization_unavailable", "decision id generation failed"),
                );
            }
        };
        let mut audit = match self.open_audit() {
            Ok(audit) => audit,
            Err(error) => return CuReply::err(command, error),
        };
        let Some(state_dir) = persisted.store_path.parent() else {
            return CuReply::err(
                command,
                CuError::new("grant_store_unavailable", "grant store is unavailable"),
            );
        };
        let provider = CurrentIdentityProvider::at(state_dir);
        let binding = match self.resolve_current_binding(&provider) {
            Ok(binding) => binding,
            Err(_) => {
                return CuReply::err(
                    command,
                    CuError::new(
                        "target_binding_unavailable",
                        "verified current target identity is unavailable",
                    ),
                );
            }
        };
        let required = command.required_grant();
        let mut store = match AuthStore::open_private_at(&persisted.store_path) {
            Ok(store) => store,
            Err(error) => return CuReply::err(command, map_store_authorization_error(&error)),
        };
        let now = match now_utc_ms() {
            Some(now) => now,
            None => {
                return CuReply::err(
                    command,
                    CuError::new("authorization_clock_invalid", "system clock is unavailable"),
                );
            }
        };
        let attempt = GrantAttempt::new(&persisted.grant_id, &binding, required);
        match store.reserve_attempt(&attempt, now) {
            Ok(GrantDecision::Denied(denial)) => {
                let outcome = match denial.kind {
                    GrantDenialKind::NotFound => "not_found",
                    GrantDenialKind::NotYetValid => "not_yet_valid",
                    GrantDenialKind::Expired => "expired",
                    GrantDenialKind::Revoked => "revoked",
                    GrantDenialKind::Exhausted => "exhausted",
                    GrantDenialKind::TargetMismatch => "target_mismatch",
                    GrantDenialKind::ScopeMissing => "scope_missing",
                };
                if let Err(error) = audit.record_persisted(
                    command.target(),
                    command,
                    required,
                    &decision_id,
                    binding.target_id(),
                    &persisted.grant_id,
                    "denied",
                    outcome,
                    None,
                ) {
                    return CuReply::err(command, error);
                }
                return CuReply::err(
                    command,
                    CuError::new("refused", format!("persisted grant is {outcome}")),
                );
            }
            Ok(GrantDecision::Authorized(_)) => {}
            Err(error) => return CuReply::err(command, map_store_authorization_error(&error)),
        }
        if let Err(error) = audit.record_persisted(
            command.target(),
            command,
            required,
            &decision_id,
            binding.target_id(),
            &persisted.grant_id,
            "authorized",
            "attempt",
            None,
        ) {
            return CuReply::err(command, error);
        }
        let revalidated = match self.resolve_current_binding(&provider) {
            Ok(binding) => binding,
            Err(_) => {
                return self.persisted_pre_dispatch_failure(
                    command,
                    &mut audit,
                    required,
                    &decision_id,
                    &binding,
                    &persisted.grant_id,
                    "target_binding_unavailable",
                );
            }
        };
        if revalidated != binding {
            return self.persisted_pre_dispatch_failure(
                command,
                &mut audit,
                required,
                &decision_id,
                &binding,
                &persisted.grant_id,
                "target_binding_changed",
            );
        }
        let reply = self.execute_current(command);
        let outcome = if reply.ok { "ok" } else { "failed" };
        let detail = reply.data.clone().or_else(|| {
            reply
                .error
                .as_ref()
                .and_then(|error| serde_json::to_value(error).ok())
        });
        if let Err(mut error) = audit.record_persisted(
            command.target(),
            command,
            required,
            &decision_id,
            binding.target_id(),
            &persisted.grant_id,
            "authorized",
            outcome,
            detail,
        ) {
            let effect = if reply.ok {
                reply.data.as_ref().and_then(|data| data.get("effect"))
            } else {
                reply
                    .error
                    .as_ref()
                    .and_then(|error| error.detail.as_ref())
                    .and_then(|detail| detail.get("effect"))
            }
            .cloned()
            .unwrap_or(serde_json::Value::String("unknown".into()));
            error.detail = Some(serde_json::json!({
                "stage": "audit_outcome",
                "effect": effect,
                "decision_id": decision_id,
                "original_reply": reply,
            }));
            return CuReply::err(command, error);
        }
        reply
    }

    fn resolve_current_binding(
        &self,
        provider: &CurrentIdentityProvider,
    ) -> Result<crate::target_binding::TargetBinding, crate::target_binding::TargetBindingError>
    {
        #[cfg(test)]
        if let Some(binding) = self.persisted_binding.as_ref() {
            return Ok(binding.clone());
        }
        resolve_target_binding(TargetRef::Current, Some(provider))
    }

    #[allow(clippy::too_many_arguments)]
    fn persisted_pre_dispatch_failure(
        &self,
        command: &Command,
        audit: &mut AuditLog,
        required: Grant,
        decision_id: &str,
        binding: &crate::target_binding::TargetBinding,
        grant_id: &str,
        code: &'static str,
    ) -> CuReply {
        let error = CuError::new(
            code,
            "verified current target identity changed before dispatch",
        );
        if let Err(audit_error) = audit.record_persisted(
            command.target(),
            command,
            required,
            decision_id,
            binding.target_id(),
            grant_id,
            "authorized",
            "failed",
            serde_json::to_value(&error).ok(),
        ) {
            return CuReply::err(command, audit_error);
        }
        CuReply::err(command, error)
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

    fn open_audit(&self) -> Result<AuditLog, CuError> {
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

    /// `<audit dir>/cu-receipts`: beside the audit log this executor writes
    /// (the injected test path, or the production resolution).
    fn receipt_dir(&self) -> Result<PathBuf, CuError> {
        #[cfg(test)]
        if let Some(path) = self.audit_path.as_ref() {
            return Ok(receipt::receipt_dir_beside(path));
        }
        let audit_path = crate::audit::resolved_audit_path()
            .map_err(|error| CuError::new("receipt_unavailable", error))?;
        Ok(receipt::receipt_dir_beside(&audit_path))
    }

    /// The crash-persistent receipt file for `target`, opened before the
    /// mechanism is touched: failure to open it is failure to act.
    fn open_receipts(&self, target: TargetRef) -> Result<ReceiptLog, CuError> {
        ReceiptLog::open_in(&self.receipt_dir()?, target)
    }

    fn execute_current(&self, command: &Command) -> CuReply {
        match self.run_current(command) {
            Ok(data) => CuReply::ok(command, data),
            Err(error) => CuReply::err(command, error),
        }
    }

    fn run_current(&self, command: &Command) -> Result<serde_json::Value, CuError> {
        match command {
            Command::Capabilities { .. } => Ok(capabilities_payload()),
            Command::Windows {
                pid,
                app,
                title,
                focused,
                minimized,
                offset,
                max,
                ..
            } => windows_payload(
                observe::WindowFilter {
                    pid: *pid,
                    app: app.clone(),
                    title: title.clone(),
                    focused: *focused,
                    minimized: *minimized,
                },
                *offset,
                *max,
            ),
            Command::WindowsWatch {
                pid,
                app,
                title,
                duration_ms,
                interval_ms,
                max_events,
                ..
            } => windows_watch_payload(
                observe::WindowFilter {
                    pid: *pid,
                    app: app.clone(),
                    title: title.clone(),
                    focused: None,
                    minimized: None,
                },
                *duration_ms,
                *interval_ms,
                *max_events,
            ),
            Command::Apps { all, .. } => apps_payload(*all),
            Command::Tree {
                window,
                depth,
                max_nodes,
                flat,
                ..
            } => tree_payload(*window, *depth, *max_nodes, *flat),
            Command::Query {
                window,
                depth,
                max_nodes,
                role,
                text,
                text_exact,
                identifier,
                actionable,
                within,
                offset,
                max,
                selector,
                ..
            } => query_payload(
                *window,
                *depth,
                *max_nodes,
                observe::NodeFilter::from_parts(
                    role,
                    text.as_deref(),
                    text_exact.as_deref(),
                    identifier.as_deref(),
                    *actionable,
                    *within,
                ),
                text.is_some() && text_exact.is_some(),
                *offset,
                *max,
                selector.as_deref(),
            ),
            Command::Invoke {
                window,
                node,
                index,
                name,
                identifier,
                role,
                action,
                value,
                focused,
                selector,
                ..
            } => invoke_payload(
                *window,
                observe::TargetSpec {
                    node: node.clone(),
                    index: *index,
                    name: name.clone(),
                    identifier: identifier.clone(),
                    role: role.clone(),
                    focused: *focused,
                },
                *action,
                value.as_deref(),
                selector.as_deref(),
                &mut self.open_receipts(command.target())?,
            ),
            Command::MenuInspect {
                window,
                depth,
                max_nodes,
                title,
                exact,
                enabled,
                offset,
                max,
                ..
            } => menu_inspect_payload(
                *window,
                *depth,
                *max_nodes,
                observe::MenuFilter {
                    title: title.clone(),
                    exact: *exact,
                    enabled: *enabled,
                },
                *offset,
                *max,
            ),
            Command::MenuInvoke { window, path, .. } => {
                menu_invoke_payload(*window, path, &mut self.open_receipts(command.target())?)
            }
            Command::Focused {
                window,
                role,
                max_value_bytes,
                ..
            } => focused_payload(*window, role.as_deref(), *max_value_bytes),
            Command::Observe {
                window,
                duration_ms,
                depth,
                max_nodes,
                max_events,
                notifications,
                interval_ms,
                mode,
                ..
            } => observe_payload(
                *window,
                *duration_ms,
                *depth,
                *max_nodes,
                *max_events,
                notifications,
                *interval_ms,
                mode.as_deref(),
            ),
            Command::Verify { window, expect, .. } => verify_payload(*window, expect),
            Command::PageJs {
                expression, port, ..
            } => page_js_payload(expression.as_deref(), *port),
            Command::App {
                window,
                action,
                snapshot,
                expect,
                pid,
                path,
                ..
            } => app_payload(
                *window,
                *action,
                *snapshot,
                expect.as_deref(),
                *pid,
                path.as_deref(),
                &mut self.open_receipts(command.target())?,
            ),
            Command::Spaces { .. } => spaces_payload(),
            Command::Displays { .. } => displays_payload(),
            Command::Unlock { window, .. } => unlock_payload(*window),
            Command::Align { group, .. } => Err(CuError::new(
                "unsupported",
                crate::mcu_surface::typed_reason_for_verb(group),
            )
            .with_detail(serde_json::json!({
                "verb": group,
                "group": crate::mcu_surface::group_id_for_verb(group),
                "os": crate::mcu_surface::host_os(),
            }))),
            Command::Screenshot { path, window, .. } => screenshot(path, *window),
            Command::PointerMove { x, y, .. } => pointer_move(*x, *y),
            Command::PointerPosition { .. } => pointer_position(),
            Command::Click { .. } => {
                click_command(command, &mut self.open_receipts(command.target())?)
            }
            Command::Focus {
                window,
                node,
                name,
                role,
                ..
            } => focus(
                *window,
                node.as_deref(),
                name.as_deref(),
                role.as_deref(),
                &mut self.open_receipts(command.target())?,
            ),
            Command::SendText {
                text,
                window,
                name,
                role,
                ..
            } => send_text(text, *window, name.as_deref(), role.as_deref()),
            Command::ClipboardRead { .. } => clipboard_read(),
            Command::Copy {
                window, name, role, ..
            } => copy(*window, name.as_deref(), role.as_deref()),
            Command::Paste {
                text,
                window,
                name,
                role,
                ..
            } => paste(text.as_deref(), *window, name.as_deref(), role.as_deref()),
            Command::SendKeys {
                keys,
                window,
                name,
                role,
                ..
            } => send_keys(keys, *window, name.as_deref(), role.as_deref()),
            Command::Scroll {
                window, name, role, ..
            } => scroll(*window, name.as_deref(), role.as_deref()),
            Command::GetExtents {
                window, name, role, ..
            } => get_extents(*window, name.as_deref(), role.as_deref()),
            Command::Select {
                start,
                end,
                window,
                name,
                role,
                ..
            } => select(*window, name.as_deref(), role.as_deref(), *start, *end),
            Command::GetSelection {
                window, name, role, ..
            } => get_selection(*window, name.as_deref(), role.as_deref()),
            Command::SetCaret {
                offset,
                window,
                name,
                role,
                ..
            } => set_caret(*window, name.as_deref(), role.as_deref(), *offset),
            Command::GetCaret {
                window, name, role, ..
            } => get_caret(*window, name.as_deref(), role.as_deref()),
            Command::GetText {
                window, name, role, ..
            } => get_text(*window, name.as_deref(), role.as_deref()),
            Command::Wait {
                timeout_ms,
                condition,
                ..
            } => wait(*timeout_ms, condition),
            Command::WindowPlace {
                action,
                window,
                frame,
                ..
            } => window_place(action, *window, *frame),
            Command::OrderWin {
                window,
                relation,
                relative,
                ..
            } => orderwin_payload(*window, *relation, *relative),
            Command::Close {
                window,
                pid,
                title,
                snapshot,
                expect,
                ..
            } => close_payload(
                *window,
                *pid,
                title.as_deref(),
                *snapshot,
                expect.as_deref(),
                &mut self.open_receipts(command.target())?,
            ),
            Command::Receipts { window, max, .. } => {
                receipts_payload(&self.receipt_dir()?, command.target(), *window, *max)
            }
        }
    }
}

fn map_store_authorization_error(error: &crate::auth_store::AuthStoreError) -> CuError {
    if error.published {
        return CuError::new(
            "authorization_in_doubt",
            "grant consumption may have been published without confirmed durability",
        )
        .with_detail(serde_json::json!({
            "effect": "not_applied",
            "authorization": "possibly_consumed",
        }));
    }
    let (code, message) = match error.kind {
        AuthStoreErrorKind::Parse
        | AuthStoreErrorKind::Validate
        | AuthStoreErrorKind::LegacyUnverified => {
            ("grant_store_corrupt", "grant store is corrupt or untrusted")
        }
        AuthStoreErrorKind::LockContended => ("grant_store_contended", "grant store is busy"),
        _ => ("grant_store_unavailable", "grant store is unavailable"),
    };
    CuError::new(code, message)
}

fn generated_decision_id() -> Option<String> {
    let bytes = agenterm_platform::entropy::secure_random_array::<16>().ok()?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(3 + bytes.len() * 2);
    output.push_str("d1_");
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Some(output)
}

fn now_utc_ms() -> Option<i64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(millis).ok()
}

fn pointer_move(x: i32, y: i32) -> Result<serde_json::Value, CuError> {
    pointer_move_with(x, y, |x, y| {
        mechanism::input_inject::pointer_move(x, y).map_err(map_mechanism_err)
    })
}

fn pointer_position() -> Result<serde_json::Value, CuError> {
    pointer_position_with(|| mechanism::input_inject::pointer_position().map_err(map_mechanism_err))
}

fn pointer_position_with(
    observe_once: impl FnOnce() -> Result<(i32, i32), CuError>,
) -> Result<serde_json::Value, CuError> {
    let (x, y) = observe_once()?;
    Ok(serde_json::json!({
        "effect": "observed",
        "addressing": "absolute-screen-coordinates",
        "coords": [x, y],
        "mechanism": "libagenterm",
    }))
}

fn pointer_move_with(
    x: i32,
    y: i32,
    move_once: impl FnOnce(i32, i32) -> Result<(), CuError>,
) -> Result<serde_json::Value, CuError> {
    move_once(x, y)?;
    Ok(serde_json::json!({
        "effect": "committed",
        "addressing": "absolute-screen-coordinates",
        "coords": [x, y],
        "mechanism": "libagenterm",
        "button_effect": "none",
    }))
}

/// Read the target session's native Unicode-text clipboard through the
/// existing bounded libagenterm two-stage ABI. The payload is returned only
/// in this observe command's reply; audit records never receive it because
/// they are restricted to authorized actuation metadata.
/// `clipboard-read`: the Unicode text, plus what else the clipboard is
/// carrying.
///
/// The type list matters even though this verb only reads text: an agent
/// that copies an image and then reads an empty string would otherwise
/// conclude the clipboard is empty. `types` names what is actually there
/// in the host's own spelling; `types_available` false means this host
/// cannot enumerate them, which is a different fact from an empty list.
fn clipboard_read() -> Result<serde_json::Value, CuError> {
    let text = mechanism::clipboard::get_text().map_err(map_mechanism_err)?;
    let bytes = text.len();
    let (types, types_available, types_reason) = match mechanism::clipboard::available_types() {
        Ok(names) => (names, true, None),
        Err(mechanism::MechanismError::Unsupported { reason }) => (Vec::new(), false, Some(reason)),
        Err(error) => (Vec::new(), false, Some(format!("{error:?}"))),
    };
    let mut payload = serde_json::json!({
        "text": text,
        "bytes": bytes,
        "format": "text/plain;charset=utf-8",
        "mechanism": "libagenterm",
        "types": types,
        "types_available": types_available,
    });
    if let Some(reason) = types_reason
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("types_reason".into(), serde_json::json!(reason));
    }
    Ok(payload)
}

fn capabilities_payload() -> serde_json::Value {
    let status = |capability: mechanism::Capability| {
        format!("{:?}", mechanism::capability_status(capability))
    };
    // The *verb* status is one stable word. `status()` above is the
    // capability's Debug form, which for `Available` happens to lowercase
    // into "available" and for anything else lowercases into the whole
    // struct -- `unsupported { reason: "host adapter unavailable" }` was
    // being published as a status value. Nothing on macOS could show that,
    // because every capability there is Available; running on Linux did.
    let verb_status = |capability: mechanism::Capability| -> &'static str {
        match mechanism::capability_status(capability) {
            mechanism::CapabilityStatus::Available => "available",
            mechanism::CapabilityStatus::Unsupported { .. } => "unsupported",
            mechanism::CapabilityStatus::Failed { .. } => "failed",
        }
    };
    // The reason, when there is one, belongs in its own field rather than
    // smuggled into the word a caller matches on.
    let verb_reason = |capability: mechanism::Capability| -> Option<String> {
        match mechanism::capability_status(capability) {
            mechanism::CapabilityStatus::Available => None,
            mechanism::CapabilityStatus::Unsupported { reason } => Some(reason),
            mechanism::CapabilityStatus::Failed { code, message } => {
                Some(format!("{code}: {message}"))
            }
        }
    };
    let capability_verb = |capability: mechanism::Capability, extra: serde_json::Value| {
        let mut declaration = serde_json::json!({ "status": verb_status(capability) });
        if let (Some(object), Some(reason)) = (declaration.as_object_mut(), verb_reason(capability))
        {
            object.insert("reason".into(), serde_json::json!(reason));
        }
        if let (Some(object), Some(extra)) = (declaration.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                object.insert(key.clone(), value.clone());
            }
        }
        declaration
    };
    // ABI 1.12: the a11y capability answers three ways. `Denied` is an OS
    // permission the caller can repair (macOS Accessibility); it is neither
    // "unsupported" (no adapter) nor an empty tree.
    let (tree_status, tree_verb) =
        match mechanism::capability_status(mechanism::Capability::AccessibilityTree) {
            mechanism::CapabilityStatus::Available => {
                ("Available", serde_json::json!({ "status": "available" }))
            }
            mechanism::CapabilityStatus::Failed { code, message }
                if code == "a11y_permission_denied" =>
            {
                (
                    "Denied",
                    serde_json::json!({
                        "status": "denied",
                        "reason": code,
                        "message": message,
                        "permission": "accessibility",
                        "repair": ACCESSIBILITY_REPAIR_PATH,
                    }),
                )
            }
            mechanism::CapabilityStatus::Failed { code, message } => (
                "Failed",
                serde_json::json!({ "status": "failed", "reason": code, "message": message }),
            ),
            mechanism::CapabilityStatus::Unsupported { reason } => (
                "Unsupported",
                serde_json::json!({ "status": "unsupported", "reason": reason }),
            ),
        };
    // The background menu bar is mapped on all three backends now, by two
    // different routes: macOS asks the application for its `AXMenuBar`,
    // while Linux and Windows find the menu-bar node in the window's own
    // bounded tree. The search route is a weaker claim -- a toolkit that
    // populates a closed menu lazily publishes nothing to find -- so those
    // two say which route they took rather than copying the tree status
    // unqualified.
    let menu_verb = if cfg!(target_os = "macos") {
        tree_verb.clone()
    } else {
        let mut declaration = tree_verb.clone();
        if let Some(object) = declaration.as_object_mut() {
            object.insert("mode".into(), serde_json::json!("tree-search"));
        }
        declaration
    };
    // The App-local focused control is read three ways: macOS asks the
    // application for its `AXFocusedUIElement`, while Linux and Windows
    // search the window's own bounded tree for the node the backend marks
    // focused (`STATE_FOCUSED` / `HasKeyboardFocus`). A search is a weaker
    // claim than a toolkit naming its own focus, so those two say which
    // route they took instead of copying the tree status unqualified.
    let focused_verb = if cfg!(target_os = "macos") {
        tree_verb.clone()
    } else {
        let mut declaration = tree_verb.clone();
        if let Some(object) = declaration.as_object_mut() {
            object.insert("mode".into(), serde_json::json!("state-search"));
        }
        declaration
    };
    // The destructive verb rides the platform's own close control on all
    // three hosts now: macOS AX `AXCloseButton`, Windows `WM_CLOSE`, and
    // Linux the EWMH `_NET_CLOSE_WINDOW` request. All three are requests,
    // not kills -- which is exactly why the gate reads the handle back
    // instead of trusting the call.
    let close_verb = capability_verb(mechanism::Capability::WindowOp, serde_json::json!({}));
    // Reading the pointer is an observation on every host, and it stays
    // available even where injection is not: the read never posts an event,
    // so it must not be gated behind the injection capability.
    let pointer_position_verb = serde_json::json!({
        "status": "available",
        "mode": "read-only",
        "group": "pointer",
    });
    // Injection is the opposite: it moves the *user's* real cursor or types
    // into whatever is frontmost, so the declaration says `desktop` scope
    // out loud. macOS has no window-local pointer route at all -- events
    // posted to a pid arrive without a window and no view ever sees them --
    // which is why `pointer-move --to <handle>` is refused rather than
    // approximated.
    // Both observation modes are declared, because they are not
    // interchangeable: polling carries `before` / `after` on every event,
    // notifications carry the order and arrival time of changes polling
    // never sees. `default` names the one a caller gets without asking.
    let observe_verb = {
        let mut declaration = tree_verb.clone();
        if let Some(object) = declaration.as_object_mut() {
            object.insert("default_mode".into(), serde_json::json!("poll-diff"));
            object.insert(
                "modes".into(),
                serde_json::json!({
                    "poll-diff": "available",
                    "notifications": if cfg!(target_os = "macos") { "available" } else { "unsupported" },
                }),
            );
        }
        declaration
    };
    // The Screenshot capability covers the PNG *writer*, which every host
    // has. Capturing a window's pixels is a separate mechanism, and macOS
    // does not have one this build can call: `CGWindowListCreateImage` was
    // obsoleted in macOS 15.0 and removed from the SDK, and its
    // replacement needs the Screen Recording grant. Declaring the verb
    // `available` from the writer's status would promise a capture that
    // always fails.
    // Linux captures with X11 GetImage (cut 3.58) and Windows with GDI;
    // only macOS has no route left.
    let screenshot_verb = if cfg!(target_os = "macos") {
        serde_json::json!({
            "status": "unsupported",
            "group": "capture",
            "reason": "native window capture needs ScreenCaptureKit; CGWindowListCreateImage was obsoleted in macOS 15.0",
        })
    } else {
        capability_verb(
            mechanism::Capability::Screenshot,
            serde_json::json!({ "group": "capture" }),
        )
    };
    let pointer_inject_verb = capability_verb(
        mechanism::Capability::InputInject,
        serde_json::json!({ "scope": "desktop", "group": "pointer" }),
    );
    // One place to look for "what am I not allowed to do, and how is that
    // fixed". `setup` / `doctor` / `permissions` stay typed -- the wizard
    // is MCU's -- but the *reporting* has to be complete, and until now the
    // repair path was buried inside the `tree` verb while input injection
    // depends on the very same grant. A caller should not have to know
    // that to find it.
    let permissions = if cfg!(target_os = "macos") {
        let accessibility =
            match mechanism::capability_status(mechanism::Capability::AccessibilityTree) {
                mechanism::CapabilityStatus::Available => serde_json::json!({
                    "status": "granted",
                }),
                mechanism::CapabilityStatus::Failed { code, message }
                    if code == "a11y_permission_denied" =>
                {
                    serde_json::json!({
                        "status": "denied",
                        "reason": code,
                        "message": message,
                        "repair": ACCESSIBILITY_REPAIR_PATH,
                    })
                }
                other => serde_json::json!({
                    "status": "unknown",
                    "detail": format!("{other:?}"),
                }),
            };
        serde_json::json!({
            "accessibility": {
                "grant": accessibility,
                // Every verb that stops working when this grant is missing,
                // including the input verbs: on macOS the same Accessibility
                // entry gates posting events.
                "gates": [
                    "tree", "query", "invoke", "verify", "wait", "focused",
                    "observe", "menu-inspect", "menu-invoke", "click", "focus",
                    "send-text", "send-keys", "scroll", "get-extents", "select",
                    "get-selection", "set-caret", "get-caret", "get-text",
                    "close", "unlock", "pointer-move", "pointer-position",
                ],
            },
            "screen_recording": {
                "grant": {
                    "status": "not_required",
                    "reason": "no capture path exists on this host: CGWindowListCreateImage was obsoleted in macOS 15.0 and ScreenCaptureKit is not wired",
                },
                "gates": ["screenshot"],
            },
        })
    } else {
        serde_json::json!({
            "model": "none",
            "reason": "this host has no per-application permission gate; a mechanism is available or it is not",
        })
    };
    // Host-specific tree mapping only. Do not list unproven peers (live
    // RDP/UIA-over-RDP) as if this host ships them.
    let tree_mapping = current_tree_mapping();
    let mut payload = serde_json::json!({
        "target": "current",
        "transport": {
            "status": "in_process",
            "available": true,
        },
        "mechanism": "libagenterm",
        "mechanism_target": "current",
        "capabilities": {
            "windows": status(mechanism::Capability::WindowEnumerate),
            "tree": tree_status,
            "screenshot": status(mechanism::Capability::Screenshot),
            "input": status(mechanism::Capability::InputInject),
            "window_place": status(mechanism::Capability::WindowOp),
            "window_placement_inspect": status(mechanism::Capability::WindowPlacementInspect),
            "desktop_host": status(mechanism::Capability::DesktopHost),
        },
        "verbs": {
            "capabilities": { "status": "available" },
            "windows": capability_verb(mechanism::Capability::WindowEnumerate, serde_json::json!({})),
            "windows-watch": capability_verb(
                mechanism::Capability::WindowEnumerate,
                serde_json::json!({ "mode": "poll-diff", "group": "discover" }),
            ),
            "apps": {
                "status": verb_status(mechanism::Capability::WindowEnumerate),
                // `running_only` describes the *default*, not a limit:
                // `--all` adds the installed-but-not-running half where the
                // host can enumerate it.
                "running_only": true,
                "installed": if cfg!(target_os = "macos") { "available" } else { "unsupported" },
                "group": "discover",
            },
            "tree": tree_verb,
            "query": tree_verb,
            "invoke": tree_verb,
            "verify": tree_verb,
            "menu-inspect": menu_verb,
            "menu-invoke": menu_verb,
            "focused": focused_verb,
            "observe": observe_verb,
            "close": close_verb,
            "orderwin": capability_verb(
                mechanism::Capability::WindowOp,
                serde_json::json!({ "group": "geometry", "mode": "raise" }),
            ),
            "screenshot": screenshot_verb,
            "receipts": { "status": "available" },
            // `hide` / `show` need an application-level hidden state, which
            // only macOS has; `quit` needs the application's own Quit menu
            // item, so it rides the menu verb's own status.
            "app": {
                "status": if cfg!(target_os = "macos") { "available" } else { "unsupported" },
                "group": "app",
                "actions": {
                    "hide": if cfg!(target_os = "macos") { "available" } else { "unsupported" },
                    "show": if cfg!(target_os = "macos") { "available" } else { "unsupported" },
                    "quit": if cfg!(target_os = "macos") { "available" } else { "mapped" },
                    "launch": if cfg!(target_os = "macos") { "available" } else { "unsupported" },
                },
                // `launch` cannot report a pid: the launcher service owns
                // the process it starts. Watch for the window instead.
                "launch_returns_pid": false,
                "quit_mechanism": "the application's own Quit menu item, pressed in the background; never a signal",
                "destructive": ["quit"],
            },
            "page-js": {
                "status": "available",
                "backend": observe::page_js_backend(),
                "mode": "cdp",
                "reason": observe::page_js_unsupported_reason(),
            },
            "spaces": crate::mcu_surface::verb_declaration("spaces"),
            "displays": crate::mcu_surface::verb_declaration("displays"),
            "pointer-position": pointer_position_verb,
            "pointer-move": pointer_inject_verb,
            "send-keys": capability_verb(
                mechanism::Capability::InputInject,
                serde_json::json!({ "scope": "desktop", "group": "input" }),
            ),
        },
        "permissions": permissions,
        "mcu_groups": crate::mcu_surface::GROUPS.iter().map(|g| g.id).collect::<Vec<_>>(),
        "alignment_tsv": crate::mcu_surface::alignment_matrix_text(),
        "mapping": {
            "windows": "libagenterm agt_window_enumerate",
            "tree": tree_mapping,
            "window_place": "Spectacle catalog via libagenterm agt_native_window_*",
        },
        "gaps": {
            "windows": "none — shared agenterm.dll (milestone 46)",
            "screenshot": "none — shared agenterm.dll (milestone 46)",
            "input_degraded": "none — shared agenterm.dll (milestone 46)",
            "rdp_live": "rdp tier is placeholder; never declared available on current",
            "macos_ax_live": "macOS AX observe (windows / tree / query), semantic actuation (invoke / verify / click / focus), background menus (menu inspect / invoke), the App-local focused control (focused / invoke --focused), the poll-diff observation stream (observe), the destructive close (gate: exact target + snapshot + postcondition) with crash-persistent receipts (receipts), the read-only pointer position and the window-place frame transaction are proven by scripts/qjs/cu-macos-smoke.qjs; invoke offers no quit / delete action; AX notifications are not subscribed (observe is poll-diff)",
        }
    });
    if let Some(verbs) = payload.get("verbs").cloned() {
        payload["verbs"] = crate::mcu_surface::merge_verbs(verbs);
    }
    if let Some(invoke) = payload["verbs"]["invoke"].as_object_mut() {
        invoke.insert(
            "actions".into(),
            serde_json::json!({
                "press": "mapped",
                "set-value": "mapped",
                "select-option": "mapped",
                "set-checked": "mapped",
                "set-expanded": "mapped",
                "increment": "mapped",
                "decrement": "mapped",
                "scroll-to": "mapped",
                "set-selection": "mapped",
                "set-selected": "mapped",
                "cancel": "mapped",
                "show-default-ui": "mapped",
            }),
        );
    }
    payload
}

fn current_tree_mapping() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "libagenterm agt_a11y_* → Linux AT-SPI2"
    }
    #[cfg(windows)]
    {
        "libagenterm agt_a11y_* → Windows UIA"
    }
    #[cfg(target_os = "macos")]
    {
        "libagenterm agt_a11y_* → macOS AX (observe + invoke live: cu-macos-smoke)"
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        "libagenterm agt_a11y_*"
    }
}

fn invalid_input(message: String) -> CuError {
    CuError::new("invalid_input", message)
}

fn tree_budget(
    depth: Option<u32>,
    max_nodes: Option<usize>,
) -> Result<mechanism::TreeBudget, CuError> {
    observe::validate_budget(depth, max_nodes).map_err(invalid_input)?;
    Ok(mechanism::TreeBudget {
        max_depth: depth,
        max_nodes,
    })
}

fn budget_json(depth: Option<u32>, max_nodes: Option<usize>) -> serde_json::Value {
    // `null` means the platform adapter's own default for that dimension.
    serde_json::json!({ "depth": depth, "max_nodes": max_nodes })
}

/// Bounded tree. `flat` returns the same nodes in walk order, each with its
/// flatten `index` and `depth`; the identities are the tree's own ids.
fn tree_payload(
    window: Option<isize>,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    flat: bool,
) -> Result<serde_json::Value, CuError> {
    let budget = tree_budget(depth, max_nodes)?;
    let tree = mechanism::tree_for_window_bounded(window, budget).map_err(map_mechanism_err)?;
    let nodes = if flat {
        serde_json::to_value(observe::flatten(&tree))
    } else {
        serde_json::to_value(&tree.nodes)
    }
    .map_err(|error| CuError::new("serialize", error.to_string()))?;
    let ax = observe::classify_ax_tree(&tree);
    let app = window_app_name(tree.window_handle);
    Ok(serde_json::json!({
        "degraded": false,
        "backend": tree.backend,
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "window": tree.window_handle,
        "root_id": tree.root_id,
        "flat": flat,
        "budget": budget_json(depth, max_nodes),
        "truncated": tree.truncated,
        "visited": tree.visited,
        "returned": tree.returned,
        "ax": ax.as_str(),
        "next_actions": observe::empty_chrome_next_actions(ax, &app),
        "nodes": nodes,
    }))
}

fn window_app_name(handle: Option<isize>) -> String {
    let Some(handle) = handle else {
        return String::new();
    };
    mechanism::window_enumerate::enumerate_top_level()
        .ok()
        .and_then(|rows| rows.into_iter().find(|row| row.handle == handle))
        .map(|row| row.app_name)
        .unwrap_or_default()
}

/// `unlock`: read the window's tree, ask the owning application to build
/// its full accessibility tree, read the tree again, and report what
/// actually changed.
///
/// A browser engine leaves its web tree unbuilt until an assistive client
/// asks for it, so a walk of an idle Chromium or WebKit window returns
/// chrome and no page -- "empty chrome" is not an empty page. macOS spells
/// the request `AXManualAccessibility`.
///
/// **The poke's own status is not the outcome.** AppKit reports the
/// attribute as unsupported even when the poke lands (measured on a
/// WKWebView: three nodes before, fourteen after, the same AXError both
/// times), so this reads the tree again and reports `grew` from the node
/// counts. A host with no such mechanism reports `poked: false` with the
/// backend's own reason and still returns the classification, because
/// knowing the tree is empty chrome is useful either way.
fn unlock_payload(window: isize) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "unlock requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    let budget = tree_budget(Some(12), None)?;
    let before =
        mechanism::tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?;
    let (poked, poke_reason) = match mechanism::poke_manual_accessibility(window) {
        Ok(()) => (true, None),
        Err(error) => {
            let reason = match &error {
                mechanism::MechanismError::Unsupported { reason } => reason.clone(),
                other => format!("{other:?}"),
            };
            (false, Some(reason))
        }
    };
    let after = if poked {
        mechanism::tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?
    } else {
        before.clone()
    };
    let ax = observe::classify_ax_tree(&after);
    let app = window_app_name(Some(window));
    let mut payload = serde_json::json!({
        "ax": ax.as_str(),
        "poked": poked,
        "grew": after.returned > before.returned,
        "returned_before": before.returned,
        "next_actions": observe::empty_chrome_next_actions(ax, &app),
        "window": window,
        "visited": after.visited,
        "returned": after.returned,
        "truncated": after.truncated,
    });
    if let Some(reason) = poke_reason
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("reason".into(), serde_json::json!(reason));
    }
    Ok(payload)
}

/// Bounded, filtered flat node list over the same walk `tree` makes.
#[allow(clippy::too_many_arguments)]
fn query_payload(
    window: isize,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    filter: observe::NodeFilter,
    text_and_text_exact: bool,
    offset: Option<usize>,
    max: Option<usize>,
    selector: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "query requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    if text_and_text_exact {
        return Err(invalid_input(
            "query accepts --text or --text-exact, not both".into(),
        ));
    }
    let budget = tree_budget(depth, max_nodes)?;
    let page = observe::Page::new(offset, max).map_err(invalid_input)?;
    let tree =
        mechanism::tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?;
    let flat = observe::flatten(&tree);
    let scoped: Vec<&observe::FlatNode<'_>> = if let Some(selector) = selector {
        observe::query_selector_scope(&tree, &flat, selector).map_err(invalid_input)?
    } else {
        flat.iter().collect()
    };
    let owned: Vec<observe::FlatNode<'_>> = scoped.into_iter().cloned().collect();
    let (hits, counts) = observe::query(&owned, &filter, page, tree.truncated);
    let nodes = serde_json::to_value(&hits)
        .map_err(|error| CuError::new("serialize", error.to_string()))?;
    Ok(serde_json::json!({
        "degraded": false,
        "backend": tree.backend,
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "window": window,
        "root_id": tree.root_id,
        "budget": budget_json(depth, max_nodes),
        "filter": {
            "role": filter.roles,
            "text": filter.text,
            "text_exact": filter.text_exact,
            "identifier": filter.identifier,
            "actionable": filter.actionable,
            "within": filter.within,
            "selector": selector,
        },
        "visited": counts.visited,
        "matched": counts.matched,
        "returned": counts.returned,
        "offset": counts.offset,
        "truncated": counts.truncated,
        "scan_truncated": counts.scan_truncated,
        "page_truncated": counts.page_truncated,
        "ax": observe::classify_ax_tree(&tree).as_str(),
        "next_actions": observe::empty_chrome_next_actions(
            observe::classify_ax_tree(&tree),
            &window_app_name(Some(window)),
        ),
        "nodes": nodes,
    }))
}

/// Window inventory. The bare verb keeps its array reply; any filter or page
/// field switches to the inventory object with counts.
fn windows_payload(
    filter: observe::WindowFilter,
    offset: Option<usize>,
    max: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    let page = observe::Page::new(offset, max).map_err(invalid_input)?;
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    // Stacking is an additional read, and a host without one is not an
    // error: the rows simply carry no z_index / occluded_percent, and the
    // envelope says why.
    let (stacking, stacking_reason) = match mechanism::window_enumerate::stacking() {
        Ok(rows) => (rows, None),
        Err(mechanism::MechanismError::Unsupported { reason }) => (Vec::new(), Some(reason)),
        Err(error) => (Vec::new(), Some(format!("{error:?}"))),
    };
    let row_json = |window: &WindowInfo| observe::window_row_json_with_stacking(window, &stacking);
    let rows = serde_json::Value::Array(windows.iter().map(row_json).collect());
    if filter.is_empty() && offset.is_none() && max.is_none() {
        return Ok(rows);
    }
    let (hits, counts) = observe::inventory(&windows, &filter, page);
    let rows = serde_json::Value::Array(hits.iter().copied().map(row_json).collect());
    Ok(serde_json::json!({
        "mechanism": "libagenterm",
        "stacking": match &stacking_reason {
            None => serde_json::json!({ "status": "available", "order": "front-to-back" }),
            Some(reason) => serde_json::json!({ "status": "unsupported", "reason": reason }),
        },
        "filter": {
            "pid": filter.pid,
            "app": filter.app,
            "title": filter.title,
            "focused": filter.focused,
            "minimized": filter.minimized,
        },
        "visited": counts.visited,
        "matched": counts.matched,
        "returned": counts.returned,
        "offset": counts.offset,
        "truncated": counts.truncated,
        "windows": rows,
    }))
}

fn filtered_windows(filter: &observe::WindowFilter) -> Result<Vec<WindowInfo>, CuError> {
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    Ok(windows
        .into_iter()
        .filter(|window| filter.matches(window))
        .collect())
}

/// `apps`: the applications with a window, and with `--all` the ones that
/// are merely installed.
///
/// The two halves answer different questions from different mechanisms: a
/// running application is one the window inventory can see, an installed
/// one is a bundle on disk that may never have been started.
/// `installed_available: false` says this host cannot enumerate installed
/// applications, which is not the same as having none.
fn apps_payload(all: bool) -> Result<serde_json::Value, CuError> {
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "mechanism": "libagenterm",
        "running_only": !all,
        "installed": false,
        "apps": observe::running_apps_json(&windows),
    });
    if !all {
        return Ok(payload);
    }
    let (installed, truncated, reason) = match mechanism::list_installed_apps() {
        Ok((apps, truncated)) => (apps, truncated, None),
        Err(mechanism::MechanismError::Unsupported { reason }) => (Vec::new(), false, Some(reason)),
        Err(error) => return Err(map_mechanism_err(error)),
    };
    // Which installed ones are up right now, matched by the name the window
    // inventory reports, so a caller asking "installed but not running?"
    // gets the answer in one read instead of joining two lists itself.
    let running_names: Vec<&str> = windows
        .iter()
        .map(|window| window.app_name.as_str())
        .collect();
    let rows: Vec<serde_json::Value> = installed
        .iter()
        .map(|app| {
            serde_json::json!({
                "name": app.name,
                "path": app.path,
                "running": running_names.contains(&app.name.as_str()),
            })
        })
        .collect();
    if let Some(object) = payload.as_object_mut() {
        object.insert("installed".into(), serde_json::json!(reason.is_none()));
        object.insert(
            "installed_available".into(),
            serde_json::json!(reason.is_none()),
        );
        object.insert("installed_apps".into(), serde_json::json!(rows));
        object.insert("installed_truncated".into(), serde_json::json!(truncated));
        if let Some(reason) = reason {
            object.insert("installed_reason".into(), serde_json::json!(reason));
        }
    }
    Ok(payload)
}

fn windows_watch_payload(
    filter: observe::WindowFilter,
    duration_ms: u64,
    interval_ms: Option<u64>,
    max_events: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    observe::validate_windows_watch(duration_ms, max_events, interval_ms).map_err(invalid_input)?;
    let max_events = max_events.unwrap_or(observe::DEFAULT_OBSERVE_EVENTS);
    let interval =
        Duration::from_millis(observe::windows_watch_interval_ms(duration_ms, interval_ms));
    let started = Instant::now();
    let mut previous = filtered_windows(&filter)?;
    let mut events = Vec::new();
    let mut seq = 0u64;
    let mut polls = 1usize;
    let mut truncated = false;
    let extra_once = duration_ms == 0;
    let deadline = started + Duration::from_millis(duration_ms);
    loop {
        if extra_once {
            if !interval.is_zero() {
                thread::sleep(interval);
            }
        } else {
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(interval.min(deadline.saturating_duration_since(Instant::now())));
        }
        polls += 1;
        let current = filtered_windows(&filter)?;
        let batch = observe::diff_window_inventory(&previous, &current);
        let t_ms = started.elapsed().as_millis() as u64;
        for event in batch {
            seq += 1;
            events.push(observe::window_watch_event_json(seq, t_ms, &event));
            if events.len() >= max_events {
                truncated = true;
                break;
            }
        }
        previous = current;
        if truncated || extra_once {
            break;
        }
    }
    Ok(serde_json::json!({
        "mechanism": "libagenterm",
        "mode": "poll-diff",
        "polls": polls,
        "emitted": events.len(),
        "truncated": truncated,
        "duration_ms": duration_ms,
        "interval_ms": interval.as_millis() as u64,
        "events": events,
        "windows": previous.iter().map(observe::window_row_json).collect::<Vec<_>>(),
    }))
}

/// MCU `orderwin`: `above` raises `window`, `below` raises `relative`.
fn orderwin_payload(
    window: isize,
    relation: OrderRelation,
    relative: isize,
) -> Result<serde_json::Value, CuError> {
    if window == 0 || relative == 0 {
        return Err(invalid_input(
            "orderwin requires --window H --relative H (non-zero handles from windows)".into(),
        ));
    }
    if window == relative {
        return Err(invalid_input(
            "orderwin --window and --relative must be distinct handles".into(),
        ));
    }
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let target = windows.iter().find(|item| item.handle == window);
    let other = windows.iter().find(|item| item.handle == relative);
    if target.is_none() {
        return Err(CuError::new(
            "a11y_window_gone",
            format!("orderwin --window {window} is not in the current inventory"),
        ));
    }
    if other.is_none() {
        return Err(CuError::new(
            "a11y_window_gone",
            format!("orderwin --relative {relative} is not in the current inventory"),
        ));
    }
    let raised = match relation {
        OrderRelation::Above => window,
        OrderRelation::Below => relative,
    };
    mechanism::window_op::show(raised, mechanism::window_op::SHOW).map_err(map_mechanism_err)?;
    Ok(serde_json::json!({
        "mechanism": "libagenterm",
        "via": "native-window-show",
        "relation": relation.as_str(),
        "window": window,
        "relative": relative,
        "raised": raised,
    }))
}

fn displays_payload() -> Result<serde_json::Value, CuError> {
    let screens = mechanism::window_enumerate::list_screens().map_err(map_mechanism_err)?;
    Ok(serde_json::json!({
        "mechanism": "libagenterm",
        "via": "agt_screen_list",
        "displays": screens.iter().enumerate().map(|(index, screen)| serde_json::json!({
            "index": index,
            "primary": screen.primary,
            "frame": screen.frame,
            "workArea": screen.visible,
        })).collect::<Vec<_>>(),
        "returned": screens.len(),
    }))
}

fn spaces_payload() -> Result<serde_json::Value, CuError> {
    #[cfg(target_os = "macos")]
    {
        crate::macos_spaces::inventory().map_err(|error| {
            CuError::new("unsupported", error.reason).with_detail(serde_json::json!({
                "group": "geometry",
                "os": "macos",
                "provider": "skylight-private-read",
            }))
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(
            CuError::new("unsupported", "spaces inventory is macOS SkyLight only").with_detail(
                serde_json::json!({
                    "group": "geometry",
                    "os": crate::mcu_surface::host_os(),
                    "provider": "none",
                }),
            ),
        )
    }
}

fn page_js_payload(
    expression: Option<&str>,
    port: Option<u16>,
) -> Result<serde_json::Value, CuError> {
    let expression = expression.unwrap_or("");
    let port = port.unwrap_or(crate::page_js::DEFAULT_PORT);
    crate::page_js::evaluate(port, expression)
        .map_err(|error| CuError::new(error.code, error.message).with_detail(error.detail))
}

fn screenshot(path: &str, window: Option<isize>) -> Result<serde_json::Value, CuError> {
    if path.is_empty() {
        return Err(CuError::new("invalid_input", "screenshot path is required"));
    }
    let raw = window.unwrap_or(0);
    if raw == 0 {
        return Err(CuError::new(
            "invalid_input",
            "screenshot window handle must be non-zero",
        ));
    }
    let result = mechanism::screenshot::capture_native_window_png(raw, std::path::Path::new(path))
        .map_err(map_mechanism_err)?;
    Ok(serde_json::json!({
        "path": path,
        "window": window,
        "output_width": result.output_width,
        "output_height": result.output_height,
        "output_pixels": result.output_pixels,
    }))
}

fn click_command(
    command: &Command,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    let Command::Click {
        window,
        node,
        name,
        role,
        coords,
        degraded,
        clicks,
        button,
        ..
    } = command
    else {
        return Err(CuError::new(
            "invalid_input",
            "internal: expected click command",
        ));
    };
    let window = *window;
    let node = node.as_deref();
    let name = name.as_deref();
    let role = role.as_deref();
    let coords = *coords;
    let degraded = *degraded;
    let clicks = *clicks;
    let button = *button;
    if name.filter(|value| !value.is_empty()).is_some() && coords.is_some() {
        return Err(CuError::new(
            "invalid_input",
            "click --name is accessibility-tree addressing; do not pass --coords",
        ));
    }
    if let Some(resolved) = resolve_actuation_node(window, node, name, role, "click")? {
        // Receipt (reserved before the press) and read-back: the window
        // tree before and after, the same `tree-diff` proof `invoke press`
        // uses. Without a window scope there is nothing to diff, which the
        // reply says instead of claiming a verified click.
        let before = window
            .map(|handle| mechanism::tree_for_window(Some(handle)).map_err(map_mechanism_err))
            .transpose()?;
        let before_node = before
            .as_ref()
            .and_then(|tree| observe::node_by_id(tree, &resolved.node_id))
            .map(observe::node_state_json);
        let mut payload = click_tree_payload(&resolved, window, clicks, button);
        let ticket = receipts.reserve(
            "click",
            window.unwrap_or(0),
            serde_json::json!({
                "action": "click",
                "node": { "id": resolved.node_id, "name": resolved.matched.as_ref().map(|node| node.name.clone()), "role": resolved.matched.as_ref().map(|node| node.role.clone()) },
                "clicks": clicks.max(1),
                "before": before_node,
            }),
        )?;
        let mut mechanism_error = None;
        for _ in 0..clicks.max(1) {
            if let Err(error) = mechanism::perform_node_action(
                window,
                &resolved.node_id,
                mechanism::NodeAction::Click,
            ) {
                mechanism_error = Some(map_mechanism_err(error));
                break;
            }
        }
        let after = window
            .map(|handle| mechanism::tree_for_window(Some(handle)).map_err(map_mechanism_err))
            .transpose()?;
        let (verified, method, reason) = match (&before, &after) {
            (Some(was), Some(is)) if observe::tree_changed(was, is) => (true, "tree-diff", None),
            (Some(_), Some(_)) => (false, "tree-diff", Some("no_observable_change")),
            _ => (false, "none", Some("no_window_scope")),
        };
        let verified = verified && mechanism_error.is_none();
        let after_node = after
            .as_ref()
            .and_then(|tree| observe::node_by_id(tree, &resolved.node_id))
            .map(observe::node_state_json);
        payload["performed"] = serde_json::json!(true);
        payload["verified"] = serde_json::json!(verified);
        payload["verification"] = serde_json::json!({
            "method": method,
            "reason": if mechanism_error.is_some() { Some("mechanism_failed") } else { reason },
        });
        payload["before"] = before_node.unwrap_or(serde_json::Value::Null);
        payload["after"] = after_node.clone().unwrap_or(serde_json::Value::Null);
        payload["receipt"] = ticket.json();
        receipts.complete(
            &ticket,
            "click",
            window.unwrap_or(0),
            verified,
            serde_json::json!({
                "after": after_node,
                "verification": payload["verification"].clone(),
                "error": mechanism_error.as_ref().map(error_payload),
            }),
        )?;
        if let Some(error) = mechanism_error {
            return Err(error.with_detail(serde_json::json!({ "receipt": payload })));
        }
        return Ok(payload);
    }
    let Some([x, y]) = coords else {
        return Err(CuError::new(
            "invalid_input",
            "click requires --window + --node, --window + --name, or --coords with --degraded",
        ));
    };
    if !degraded {
        return Err(CuError::new(
            "invalid_input",
            "coordinate click requires --degraded so callers can see pixel addressing explicitly",
        ));
    }
    let inject_button = match button {
        PointerButton::Left => mechanism::input_inject::PointerButton::Left,
        PointerButton::Right => mechanism::input_inject::PointerButton::Right,
        PointerButton::Middle => mechanism::input_inject::PointerButton::Middle,
    };
    mechanism::input_inject::pointer_click(x, y, inject_button, clicks)
        .map_err(map_mechanism_err)?;
    Ok(serde_json::json!({
        "addressing": "degraded-coordinates",
        "coords": [x, y],
        "window": window,
        "button": button,
        "clicks": clicks,
    }))
}

fn focus(
    window: Option<isize>,
    node: Option<&str>,
    name: Option<&str>,
    role: Option<&str>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    let resolved = resolve_actuation_node(window, node, name, role, "focus")?.ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "focus requires --node <path-id> or --window + --name",
        )
    })?;
    // Receipt reserved before the focus move; read back as the node's own
    // `focused` state in the window tree (no window scope: unverifiable).
    let before_node = window
        .map(|handle| mechanism::tree_for_window(Some(handle)).map_err(map_mechanism_err))
        .transpose()?
        .as_ref()
        .and_then(|tree| observe::node_by_id(tree, &resolved.node_id))
        .map(observe::node_state_json);
    let ticket = receipts.reserve(
        "focus",
        window.unwrap_or(0),
        serde_json::json!({
            "action": "focus",
            "node": { "id": resolved.node_id, "name": resolved.matched.as_ref().map(|node| node.name.clone()), "role": resolved.matched.as_ref().map(|node| node.role.clone()) },
            "before": before_node,
        }),
    )?;
    let mechanism_error =
        mechanism::perform_node_action(window, &resolved.node_id, mechanism::NodeAction::Focus)
            .err()
            .map(map_mechanism_err);
    let after_node = window
        .map(|handle| mechanism::tree_for_window(Some(handle)).map_err(map_mechanism_err))
        .transpose()?
        .as_ref()
        .and_then(|tree| observe::node_by_id(tree, &resolved.node_id))
        .cloned();
    let (verified, method, reason) = match &after_node {
        Some(node) => match observe::focused_state(node) {
            observe::Tri::True => (true, "focused-readback", None),
            observe::Tri::False | observe::Tri::Mixed => {
                (false, "focused-readback", Some("state_mismatch"))
            }
            observe::Tri::Unknown => (false, "focused-readback", Some("state_unobservable")),
        },
        None if window.is_some() => (false, "node-readback", Some("node_gone")),
        None => (false, "none", Some("no_window_scope")),
    };
    let verified = verified && mechanism_error.is_none();
    let after_state = after_node.as_ref().map(observe::node_state_json);
    let mut payload = focus_tree_payload(&resolved, window);
    payload["performed"] = serde_json::json!(true);
    payload["verified"] = serde_json::json!(verified);
    payload["verification"] = serde_json::json!({
        "method": method,
        "reason": if mechanism_error.is_some() { Some("mechanism_failed") } else { reason },
    });
    payload["before"] = before_node.unwrap_or(serde_json::Value::Null);
    payload["after"] = after_state.clone().unwrap_or(serde_json::Value::Null);
    payload["receipt"] = ticket.json();
    receipts.complete(
        &ticket,
        "focus",
        window.unwrap_or(0),
        verified,
        serde_json::json!({
            "after": after_state,
            "verification": payload["verification"].clone(),
            "error": mechanism_error.as_ref().map(error_payload),
        }),
    )?;
    if let Some(error) = mechanism_error {
        return Err(error.with_detail(serde_json::json!({ "receipt": payload })));
    }
    Ok(payload)
}

/// `send-text` with `--name` writes through native AT-SPI
/// `EditableText` (`SetTextContents` / `InsertText`) or, when the named
/// showing node exposes `Text` + `editable` but not `EditableText`
/// (Chrome 151, WebKitGTK/Reasonix `<textarea>`), through AT-SPI `Text`
/// plus the toolkit set-value. Success is confirmed by `Text.GetText`.
/// The WebKit eval helper's `OK` and `last_text_write_via` are write-path
/// reports; `wait --text-equals` must poll GetText again. A named showing
/// node with no writeable text interface typed-fails
/// (`a11y_text_unavailable`) and never falls through to XTest /
/// `input_inject::type_text`.
///
/// `--window` without `--name` writes that same path on the showing
/// focused node — the same innermost `Text.GetText` candidate
/// `get-text --window` reads — so `focus --name X` then
/// `send-text --window H TEXT` then `get-text --window H` closes the
/// loop on agenterm-con `Command` (native `EditableText`), Chrome
/// `GetTextField`, and the Reasonix composer (`Message Reasonix…`
/// under `scripts/reasonix-desktop-a11y.sh`). WebKit 2.52 still has no
/// `EditableText`; the write is AT-SPI `Text` plus the eval-helper
/// set-value (`id=composer-input`). Never XTest when `--window` is set.
/// Without `--window` it stays the plain "type into whatever is
/// focused" inject.
fn send_text(
    text: &str,
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    if let Some(resolved) = resolve_actuation_node(window, None, name, role, "send-text")? {
        return send_text_to_node(text, window, resolved);
    }
    if role.filter(|value| !value.is_empty()).is_some() {
        return Err(CuError::new(
            "invalid_input",
            "send-text --role requires --name <pattern>",
        ));
    }
    if window.is_some() {
        let (resolved, _current) = get_text_focused(window)?;
        return send_text_to_node(text, window, resolved);
    }
    mechanism::input_inject::type_text(text).map_err(map_mechanism_err)?;
    Ok(serde_json::json!({ "typed": text }))
}

fn send_text_to_node(
    text: &str,
    window: Option<isize>,
    resolved: ResolvedNode,
) -> Result<serde_json::Value, CuError> {
    mechanism::set_node_text(window, &resolved.node_id, text).map_err(map_mechanism_err)?;
    let _ = mechanism::accessibility_tree::drain_bus();
    let via = mechanism::accessibility_tree::last_text_write_via().unwrap_or_default();
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "send-text",
        "typed": text,
        "via": via,
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `copy --name` reads AT-SPI `Text.GetText` (`agt_a11y_node_get_text`)
/// from the unique showing named node and publishes that UTF-8 onto the
/// native clipboard (`agt_clipboard_set_text`). On Linux X11 the owner
/// process stays in the `SetSelectionOwner` event loop so a later
/// `paste --name` (no `--text`) can `ConvertSelection`. A named showing
/// node with no Text interface typed-fails (`a11y_text_unavailable`) and
/// never falls through to XTest / `--coords` / screenshot.
///
/// `--window` without `--name` copies that same GetText path on the
/// showing focused node — the same innermost `Text.GetText` candidate
/// `get-text --window` reads — so `focus --name X` then
/// `copy --window H` then `paste --window H` / `get-text --window H`
/// closes the loop on agenterm-con `Command` (`via=gettext` on a second
/// con that never steals the resident control socket), Chrome
/// `GetTextField`, and the Reasonix composer (`Message Reasonix…` under
/// `scripts/reasonix-desktop-a11y.sh`, `via=gettext`). Never XTest when
/// `--window` is set. Without `--window` copy is invalid: there is no
/// plain "copy whatever is focused" inject verb. `matched.text` is the
/// resolve-time snapshot; the copied payload is independent GetText.
/// Live close-the-circuit: seed unique string → focused copy → clear →
/// focused paste → independent GetText equals seed. Paste after copy on
/// Reasonix still uses the WebKit eval-helper set-value path; con paste
/// restore is native `EditableText` (`via=editable-text`); only
/// independent GetText proves the restore.
fn copy(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let resolved = if let Some(resolved) = resolve_actuation_node(window, None, name, role, "copy")?
    {
        resolved
    } else if role.filter(|value| !value.is_empty()).is_some() {
        return Err(CuError::new(
            "invalid_input",
            "copy --role requires --name <pattern>",
        ));
    } else if window.is_some() {
        let (resolved, _current) = get_text_focused(window)?;
        resolved
    } else {
        return Err(CuError::new(
            "invalid_input",
            "copy requires --window <handle> [--name <pattern>]",
        ));
    };
    let text = mechanism::get_node_text(window, &resolved.node_id).map_err(map_mechanism_err)?;
    mechanism::clipboard::publish_text(&text).map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "copy",
        "text": text,
        "via": "gettext",
        "clipboard": true,
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `paste --name` writes clipboard text into the unique showing named
/// field through the same native AT-SPI `EditableText` / `Text` path as
/// named `send-text`. `--text` only seeds the clipboard; the field write
/// always reads `agt_clipboard_get_text` first. A named showing node with
/// no writeable text interface typed-fails (`a11y_text_unavailable`) and
/// never falls through to XTest / `--coords` / screenshot.
///
/// `--window` without `--name` writes that same clipboard path on the
/// showing focused node — the same innermost `Text.GetText` candidate
/// `get-text --window` reads — so `focus --name X` then
/// `paste --window H` (optional `--text` seed) then
/// `get-text --window H` closes the loop on agenterm-con `Command`
/// (native `EditableText`, `via=editable-text` on a second con that
/// never steals the resident control socket), Chrome `GetTextField`,
/// and the Reasonix composer (`Message Reasonix…` under
/// `scripts/reasonix-desktop-a11y.sh`, eval-helper set-value,
/// `via=text`). Never XTest when `--window` is set. Without `--window`
/// paste is invalid: there is no plain "paste into whatever is focused"
/// inject verb. A miss or an ambiguous name writes nothing.
/// `matched.text` is the resolve-time snapshot; independent
/// `get-text --window` / `wait --text-equals` must poll `Text.GetText`.
fn paste(
    seed: Option<&str>,
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let resolved =
        if let Some(resolved) = resolve_actuation_node(window, None, name, role, "paste")? {
            resolved
        } else if role.filter(|value| !value.is_empty()).is_some() {
            return Err(CuError::new(
                "invalid_input",
                "paste --role requires --name <pattern>",
            ));
        } else if window.is_some() {
            let (resolved, _current) = get_text_focused(window)?;
            resolved
        } else {
            return Err(CuError::new(
                "invalid_input",
                "paste requires --window <handle> [--name <pattern>]",
            ));
        };
    if let Some(seed) = seed {
        mechanism::clipboard::set_text(seed).map_err(map_mechanism_err)?;
    }
    let pasted = mechanism::clipboard::get_text().map_err(map_mechanism_err)?;
    mechanism::set_node_text(window, &resolved.node_id, &pasted).map_err(map_mechanism_err)?;
    let _ = mechanism::accessibility_tree::drain_bus();
    let via = mechanism::accessibility_tree::last_text_write_via().unwrap_or_default();
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "paste",
        "typed": pasted,
        "via": via,
        "clipboard": true,
        "seeded": seed.is_some(),
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `send-keys` with `--name` delivers the chord through native AT-SPI
/// Device/key events (`DeviceEventListener.NotifyEvent`). A named showing
/// node with no key interface typed-fails (`a11y_key_unavailable`) and
/// never falls through to XTest / `input_inject::send_keys`.
///
/// `--window` without `--name` targets the showing focused node — the
/// same innermost `Text.GetText` candidate `get-text --window` reads —
/// so `focus --name X` then `send-keys --window H KEYS` then
/// `get-text --window H` closes the loop on agenterm-con `Command`
/// (native `EditableText`, `via=editable-text` on a second con that
/// never steals the resident control socket), Chrome `GetTextField`,
/// and the Reasonix composer (`Message Reasonix…` under
/// `scripts/reasonix-desktop-a11y.sh`). Prefer
/// `DeviceEventListener.NotifyEvent`. When that interface is absent
/// (con Command; Chrome renderer entry; WebKitGTK textarea) and `KEYS`
/// is plain typeable text, write through the same AT-SPI
/// `EditableText` / `Text` path as focused `send-text` so the typed
/// string is still native AT-SPI and never XTest. Special chords
/// (`enter`, `ctrl+a`, …) without a key interface still typed-fail.
/// Without `--window` it stays the plain "send to whatever is focused"
/// inject.
fn send_keys(
    keys: &str,
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    if let Some(resolved) = resolve_actuation_node(window, None, name, role, "send-keys")? {
        return send_keys_to_node(keys, window, resolved);
    }
    if role.filter(|value| !value.is_empty()).is_some() {
        return Err(CuError::new(
            "invalid_input",
            "send-keys --role requires --name <pattern>",
        ));
    }
    if window.is_some() {
        let (resolved, _current) = get_text_focused(window)?;
        return send_keys_to_focused_node(keys, window, resolved);
    }
    mechanism::input_inject::send_keys(keys).map_err(map_mechanism_err)?;
    Ok(serde_json::json!({ "keys": keys }))
}

/// What the local backend actually did to deliver a chord. Linux and
/// Windows put the keys on the wire (AT-SPI `DeviceEventController`, UIA
/// focus + `SendInput`); macOS cannot hand a keystroke to an application it
/// refuses to activate, so it performs the AX action the chord *means*
/// (`AXConfirm` / `AXCancel`) and must not claim a key was delivered. This
/// names the mechanism of the local target only -- a remote worker sends
/// its own label back in its reply.
fn local_key_delivery_via() -> &'static str {
    if cfg!(target_os = "macos") {
        "ax-action"
    } else {
        "device-event"
    }
}

fn send_keys_to_node(
    keys: &str,
    window: Option<isize>,
    resolved: ResolvedNode,
) -> Result<serde_json::Value, CuError> {
    mechanism::send_node_keys(window, &resolved.node_id, keys).map_err(map_mechanism_err)?;
    let _ = mechanism::accessibility_tree::drain_bus();
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "send-keys",
        "keys": keys,
        "via": local_key_delivery_via(),
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// Focused-node key delivery: Device/key first; plain typeable text may
/// fall back to AT-SPI EditableText/Text write when the node has no
/// `DeviceEventListener` (con Command, Chrome GetTextField, Reasonix
/// composer). Never XTest.
fn send_keys_to_focused_node(
    keys: &str,
    window: Option<isize>,
    resolved: ResolvedNode,
) -> Result<serde_json::Value, CuError> {
    match mechanism::send_node_keys(window, &resolved.node_id, keys) {
        Ok(()) => {
            let _ = mechanism::accessibility_tree::drain_bus();
            let mut payload = serde_json::json!({
                "addressing": "accessibility-tree",
                "mechanism": "libagenterm",
                "node": resolved.node_id,
                "window": window,
                "action": "send-keys",
                "keys": keys,
                "via": local_key_delivery_via(),
            });
            attach_name_match(&mut payload, &resolved);
            Ok(payload)
        }
        Err(error) if focused_keys_may_use_text_write(keys, &error) => {
            mechanism::set_node_text(window, &resolved.node_id, keys).map_err(map_mechanism_err)?;
            let _ = mechanism::accessibility_tree::drain_bus();
            let via = mechanism::accessibility_tree::last_text_write_via().unwrap_or_default();
            let mut payload = serde_json::json!({
                "addressing": "accessibility-tree",
                "mechanism": "libagenterm",
                "node": resolved.node_id,
                "window": window,
                "action": "send-keys",
                "keys": keys,
                "via": via,
            });
            attach_name_match(&mut payload, &resolved);
            Ok(payload)
        }
        Err(error) => Err(map_mechanism_err(error)),
    }
}

/// Plain typeable text (no modifier chords / named special keys) may use
/// the AT-SPI Text write path when Device/key is missing or the chord
/// parser rejects a multi-character literal. `enter` / `ctrl+a` stay on
/// the Device/key typed-fail contract.
fn focused_keys_may_use_text_write(keys: &str, error: &mechanism::MechanismError) -> bool {
    if !is_plain_typeable_text(keys) {
        return false;
    }
    match error {
        mechanism::MechanismError::Failed { code, .. } => {
            code == "a11y_key_unavailable" || code == "invalid_input"
        }
        mechanism::MechanismError::Unsupported { .. } => false,
    }
}

/// Printable text payload for focused send-keys Text fallback: no `+`
/// modifier chords, not a single named special key token (`enter`,
/// `tab`, …). Multi-character literals and single printable letters
/// qualify so Chrome can close `send-keys --window` → `get-text --window`
/// without DeviceEventListener or XTest.
fn is_plain_typeable_text(keys: &str) -> bool {
    if keys.is_empty() || keys.contains('+') {
        return false;
    }
    if is_named_special_key(keys) {
        return false;
    }
    keys.chars().all(|ch| !ch.is_control())
}

fn is_named_special_key(keys: &str) -> bool {
    matches!(
        keys.to_ascii_lowercase().as_str(),
        "backspace"
            | "tab"
            | "enter"
            | "return"
            | "escape"
            | "esc"
            | "space"
            | "home"
            | "left"
            | "up"
            | "right"
            | "down"
            | "delete"
            | "del"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
    )
}

/// `scroll --name` is one-shot AT-SPI `Component.ScrollTo(TopEdge)`
/// (`agt_a11y_node_scroll`). Missing / false / `UnknownMethod` typed-fails
/// (`a11y_scroll_unavailable`). Never Action `scroll*`, XTest wheel,
/// `GenerateMouseEvent`, or `--coords`. `matched.extents` / snapshot
/// bounds do not count as proof.
fn scroll(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let name = name.filter(|value| !value.is_empty()).ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "scroll requires --window <handle> --name <pattern>",
        )
    })?;
    let resolved =
        resolve_actuation_node(window, None, Some(name), role, "scroll")?.ok_or_else(|| {
            CuError::new(
                "invalid_input",
                "scroll requires --window <handle> --name <pattern>",
            )
        })?;
    mechanism::scroll_node(window, &resolved.node_id).map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "scroll",
        "via": "scroll-to",
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `get-extents --name` reads independent AT-SPI `Component.GetExtents(Screen)`
/// (`agt_a11y_node_get_extents`). Snapshot `node.bounds` do not count.
/// Empty extents typed-fail (`a11y_extents_unavailable`).
fn get_extents(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let name = name.filter(|value| !value.is_empty()).ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "get-extents requires --window <handle> --name <pattern>",
        )
    })?;
    let resolved = resolve_actuation_node(window, None, Some(name), role, "get-extents")?
        .ok_or_else(|| {
            CuError::new(
                "invalid_input",
                "get-extents requires --window <handle> --name <pattern>",
            )
        })?;
    let extents =
        mechanism::get_node_extents(window, &resolved.node_id).map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "get-extents",
        "via": "get-extents",
        "extents": {
            "x": extents.x,
            "y": extents.y,
            "width": extents.width,
            "height": extents.height,
        },
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `select --name` is one-shot AT-SPI `Text.SetSelection`
/// (`agt_a11y_node_set_selection`). Missing Text / `UnknownMethod`
/// typed-fails (`a11y_selection_unavailable`). SetSelection false
/// typed-fails (`a11y_selection_no_effect`). Never XTest, mouse-drag,
/// `--coords`, or screenshot. The reply is not proof — `get-selection`
/// is the independent `GetNSelections` / `GetSelection` readback.
fn select(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
    start: i32,
    end: i32,
) -> Result<serde_json::Value, CuError> {
    if start < 0 || end < start {
        return Err(CuError::new(
            "invalid_input",
            format!("select requires 0 <= --start <= --end; got {start}..{end}"),
        ));
    }
    let name = name.filter(|value| !value.is_empty()).ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "select requires --window <handle> --name <pattern> --start N --end M",
        )
    })?;
    let resolved =
        resolve_actuation_node(window, None, Some(name), role, "select")?.ok_or_else(|| {
            CuError::new(
                "invalid_input",
                "select requires --window <handle> --name <pattern> --start N --end M",
            )
        })?;
    mechanism::set_node_selection(window, &resolved.node_id, start, end)
        .map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "select",
        "via": "set-selection",
        "start": start,
        "end": end,
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `get-selection --name` reads independent AT-SPI `Text.GetNSelections`
/// + `GetSelection(0)` (`agt_a11y_node_get_selection`). The `select`
///
/// The reply payload does not count. Missing Text typed-fails
/// (`a11y_selection_unavailable`). `n == 0` is empty success.
fn get_selection(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let name = name.filter(|value| !value.is_empty()).ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "get-selection requires --window <handle> --name <pattern>",
        )
    })?;
    let resolved = resolve_actuation_node(window, None, Some(name), role, "get-selection")?
        .ok_or_else(|| {
            CuError::new(
                "invalid_input",
                "get-selection requires --window <handle> --name <pattern>",
            )
        })?;
    let selection =
        mechanism::get_node_selection(window, &resolved.node_id).map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "get-selection",
        "via": "get-selection",
        "n": selection.n,
        "start": selection.start,
        "end": selection.end,
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `set-caret --name` is one-shot AT-SPI `Text.SetCaretOffset`
/// (`agt_a11y_node_set_caret_offset`). Missing Text / `UnknownMethod`
/// typed-fails (`a11y_caret_unavailable`). SetCaretOffset false
/// typed-fails (`a11y_caret_no_effect`). Never XTest, `--coords`, or
/// screenshot. The reply is not proof — `get-caret` is the independent
/// `CaretOffset` readback.
fn set_caret(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
    offset: i32,
) -> Result<serde_json::Value, CuError> {
    if offset < 0 {
        return Err(CuError::new(
            "invalid_input",
            format!("set-caret requires --offset >= 0; got {offset}"),
        ));
    }
    let name = name.filter(|value| !value.is_empty()).ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "set-caret requires --window <handle> --name <pattern> --offset N",
        )
    })?;
    let resolved = resolve_actuation_node(window, None, Some(name), role, "set-caret")?
        .ok_or_else(|| {
            CuError::new(
                "invalid_input",
                "set-caret requires --window <handle> --name <pattern> --offset N",
            )
        })?;
    mechanism::set_node_caret_offset(window, &resolved.node_id, offset)
        .map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "set-caret",
        "via": "set-caret-offset",
        "offset": offset,
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `get-caret --name` reads independent AT-SPI `Text.CaretOffset`
/// (`agt_a11y_node_get_caret_offset`). The `set-caret` reply payload
/// does not count. Missing Text typed-fails (`a11y_caret_unavailable`).
fn get_caret(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let name = name.filter(|value| !value.is_empty()).ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "get-caret requires --window <handle> --name <pattern>",
        )
    })?;
    let resolved = resolve_actuation_node(window, None, Some(name), role, "get-caret")?
        .ok_or_else(|| {
            CuError::new(
                "invalid_input",
                "get-caret requires --window <handle> --name <pattern>",
            )
        })?;
    let offset =
        mechanism::get_node_caret_offset(window, &resolved.node_id).map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "get-caret",
        "via": "get-caret-offset",
        "offset": offset,
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `get-text --name` reads independent AT-SPI `Text.GetText`
/// (`agt_a11y_node_get_text`) once for the unique showing named node.
/// Without `--name` it reads the focused node instead: toolkits may mark
/// a whole ancestor chain `focused` (Reasonix marks a container that has
/// no Text interface), so the candidates are every showing node carrying
/// the AT-SPI `focused` state, probed innermost-first, and the winner is
/// the innermost one that actually exposes `Text.GetText`. So
/// `focus --name X` then `get-text --window H` closes the loop on
/// whatever holds focus. This is the same text authority
/// `wait --text-equals` polls, exposed as a first-class one-shot readback
/// so an independent observation does not need a wait timeout. Not
/// `send-text` / `paste` / `copy` `matched.text`, `last_text_write_via`,
/// the WebKit eval helper's queued-job `OK`, or a tree snapshot `text`.
/// No focused candidate with Text typed-fails (`a11y_text_unavailable`).
/// Never XTest / `--coords` / screenshot.
fn get_text(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let name = name.filter(|value| !value.is_empty());
    let (resolved, text) = match name {
        Some(name) => {
            let resolved = resolve_actuation_node(window, None, Some(name), role, "get-text")?
                .ok_or_else(|| {
                    CuError::new(
                        "invalid_input",
                        "get-text requires --window <handle> [--name <pattern>]",
                    )
                })?;
            let text =
                mechanism::get_node_text(window, &resolved.node_id).map_err(map_mechanism_err)?;
            (resolved, text)
        }
        None => {
            if role.is_some() {
                return Err(CuError::new(
                    "invalid_input",
                    "get-text --role requires --name <pattern>",
                ));
            }
            get_text_focused(window)?
        }
    };
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "get-text",
        "via": "gettext",
        "text": text,
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

struct ResolvedNode {
    node_id: String,
    matched: Option<mechanism::A11yNode>,
    backend: Option<String>,
}

/// Focused-node text readback: no name pattern, no coordinates — the
/// toolkit's own focus report picks the node. Probes every showing
/// `focused` node innermost-first with independent `Text.GetText` and
/// returns the first that exposes it. A `focused` ancestor without the
/// Text interface (`a11y_text_unavailable`) falls through to the next
/// candidate; any other mechanism failure aborts. All candidates missing
/// Text re-raises the innermost candidate's `a11y_text_unavailable`.
fn get_text_focused(window: Option<isize>) -> Result<(ResolvedNode, String), CuError> {
    let Some(handle) = window else {
        return Err(CuError::new(
            "invalid_input",
            "get-text without --name requires --window <handle>",
        ));
    };
    let tree = mechanism::tree_for_window(Some(handle)).map_err(map_mechanism_err)?;
    let candidates = focused_candidates_innermost_first(&tree.nodes);
    if candidates.is_empty() {
        return Err(CuError::new(
            "a11y_node_not_found",
            "no showing focused accessibility node in window tree",
        ));
    }
    let mut text_unavailable: Option<CuError> = None;
    for node in candidates {
        match mechanism::get_node_text(window, &node.id) {
            Ok(text) => {
                let resolved = ResolvedNode {
                    node_id: node.id.clone(),
                    matched: Some(node.clone()),
                    backend: Some(tree.backend.clone()),
                };
                return Ok((resolved, text));
            }
            Err(mechanism::MechanismError::Failed { code, message })
                if code == "a11y_text_unavailable" =>
            {
                text_unavailable.get_or_insert(CuError::new(code, message));
            }
            Err(other) => return Err(map_mechanism_err(other)),
        }
    }
    Err(text_unavailable.expect("non-empty candidates yield Ok or a stored error"))
}

/// Every showing node carrying the AT-SPI `focused` state, deepest child
/// path first, so an innermost real widget wins over a `focused` ancestor
/// container. Depth is the child-index path length; the stable sort keeps
/// snapshot pre-order between equal depths.
fn focused_candidates_innermost_first(nodes: &[mechanism::A11yNode]) -> Vec<&mechanism::A11yNode> {
    let mut candidates: Vec<&mechanism::A11yNode> = nodes
        .iter()
        .filter(|node| node_is_showing(node))
        .filter(|node| node.states.iter().any(|state| state == "focused"))
        .collect();
    candidates.sort_by_key(|node| std::cmp::Reverse(node.id.matches('/').count()));
    candidates
}

/// Shared addressing gate for structured click/focus: `--node` or `--name`,
/// never both, and `--name` never opens a coordinate/screenshot path.
/// `--name` requires exactly one showing/visible match.
fn resolve_actuation_node(
    window: Option<isize>,
    node: Option<&str>,
    name: Option<&str>,
    role: Option<&str>,
    verb: &str,
) -> Result<Option<ResolvedNode>, CuError> {
    let node = node.filter(|value| !value.is_empty());
    let name = name.filter(|value| !value.is_empty());
    if node.is_some() && name.is_some() {
        return Err(CuError::new(
            "invalid_input",
            format!("{verb} accepts --node or --name, not both"),
        ));
    }
    if let Some(pattern) = name {
        let (tree, matched) = resolve_named_node(window, pattern, role)?;
        return Ok(Some(ResolvedNode {
            node_id: matched.id.clone(),
            matched: Some(matched),
            backend: Some(tree.backend),
        }));
    }
    Ok(node.map(|node_id| ResolvedNode {
        node_id: node_id.to_owned(),
        matched: None,
        backend: None,
    }))
}

fn resolve_named_node(
    window: Option<isize>,
    pattern: &str,
    role: Option<&str>,
) -> Result<(mechanism::A11yTree, mechanism::A11yNode), CuError> {
    let Some(window) = window else {
        return Err(CuError::new(
            "invalid_input",
            "name addressing requires --window <handle>",
        ));
    };
    let tree = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    let node = require_unique_showing_node(&tree.nodes, pattern, role)?.clone();
    Ok((tree, node))
}

fn click_tree_payload(
    resolved: &ResolvedNode,
    window: Option<isize>,
    clicks: u32,
    button: PointerButton,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "click",
        "clicks": clicks,
        "button": button,
    });
    attach_name_match(&mut payload, resolved);
    payload
}

fn focus_tree_payload(resolved: &ResolvedNode, window: Option<isize>) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "focus",
    });
    attach_name_match(&mut payload, resolved);
    payload
}

fn attach_name_match(payload: &mut serde_json::Value, resolved: &ResolvedNode) {
    let Some(matched) = &resolved.matched else {
        return;
    };
    if let Some(backend) = &resolved.backend {
        payload["backend"] = serde_json::json!(backend);
    }
    payload["matched"] = serde_json::to_value(matched).unwrap_or(serde_json::Value::Null);
}

fn name_scope(pattern: &str, role: Option<&str>) -> String {
    match role {
        Some(role) => format!("name contains '{pattern}' and role '{role}'"),
        None => format!("name contains '{pattern}'"),
    }
}

/// What `window-place` was asked to do: a catalog action, or (PRD_02_32
/// `frame`, slice 4) an explicit rect that replaces the geometry step and
/// rides the same preflight / apply / read-back / history transaction.
#[derive(Clone, Copy, Debug)]
enum PlaceRequest {
    Catalog(crate::place::PlaceAction),
    Frame(crate::place::Rect),
}

impl PlaceRequest {
    fn kebab(self) -> &'static str {
        match self {
            Self::Catalog(action) => action.kebab(),
            Self::Frame(_) => "frame",
        }
    }

    fn spectacle_id(self) -> &'static str {
        match self {
            Self::Catalog(action) => action.spectacle_id(),
            // Not a Spectacle constant: `frame` is agenterm's own closed id.
            Self::Frame(_) => "AgentermWindowActionFrame",
        }
    }

    fn history(self) -> Option<crate::place::PlaceAction> {
        match self {
            Self::Catalog(action) if action.is_history() => Some(action),
            _ => None,
        }
    }
}

const FRAME_MAX_EXTENT: i32 = 32_768;

fn window_place(
    action_raw: &str,
    window: Option<isize>,
    frame: Option<[i32; 4]>,
) -> Result<serde_json::Value, CuError> {
    let request = if action_raw.trim() == "frame" {
        let Some([x, y, width, height]) = frame else {
            return Err(CuError::new(
                "invalid_input",
                "window-place --action frame requires --x X --y Y --width W --height H",
            ));
        };
        if width <= 0 || height <= 0 {
            return Err(CuError::new(
                "invalid_input",
                format!("frame width and height must be positive, got {width}x{height}"),
            ));
        }
        if [x, y, width, height]
            .iter()
            .any(|value| value.abs() > FRAME_MAX_EXTENT)
        {
            return Err(CuError::new(
                "invalid_input",
                format!("frame coordinates must be within ±{FRAME_MAX_EXTENT}"),
            ));
        }
        PlaceRequest::Frame(crate::place::Rect::new(
            f64::from(x),
            f64::from(y),
            f64::from(width),
            f64::from(height),
        ))
    } else {
        if frame.is_some() {
            return Err(CuError::new(
                "invalid_input",
                format!("--x/--y/--width/--height belong to --action frame, not '{action_raw}'"),
            ));
        }
        PlaceRequest::Catalog(crate::place::PlaceAction::parse(action_raw).ok_or_else(|| {
            CuError::new(
                "invalid_input",
                format!("unknown window-place action '{action_raw}'"),
            )
        })?)
    };
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let screens = mechanism::window_enumerate::list_screens().map_err(map_mechanism_err)?;
    if screens.is_empty() {
        return Err(CuError::new("failed", "no screens available"));
    }
    let target_window = if let Some(handle) = window {
        windows
            .iter()
            .find(|item| item.handle == handle)
            .ok_or_else(|| CuError::new("failed", format!("window handle {handle} not found")))?
    } else {
        windows
            .iter()
            .find(|item| item.focused)
            .or_else(|| windows.first())
            .ok_or_else(|| CuError::new("failed", "no top-level window to place"))?
    };
    let history = crate::place::PlaceHistory::open()
        .map_err(|error| CuError::new("failed", format!("history: {error}")))?;
    window_place_transaction(
        request,
        target_window,
        &screens,
        history,
        &mut NativePlaceRuntime,
        &mut NativeHistoryCommitter,
    )
}

#[derive(Clone, Debug)]
struct PlaceIdentity {
    handle: isize,
    process_id: u32,
    app_name: String,
}

trait PlaceRuntime {
    fn read_rect(&mut self, handle: isize) -> Result<crate::place::Rect, CuError>;
    fn inspect_placement(
        &mut self,
        handle: isize,
        expected_pid: u32,
    ) -> Result<mechanism::window_placement::PlacementWindowInfo, CuError>;
    fn apply_rect(
        &mut self,
        handle: isize,
        target: crate::place::Rect,
        visible: crate::place::Rect,
    ) -> Result<(crate::place::Rect, bool, bool), CuError>;
    fn identity_matches(&mut self, identity: &PlaceIdentity) -> Result<bool, CuError>;
}

struct NativePlaceRuntime;

impl PlaceRuntime for NativePlaceRuntime {
    fn read_rect(&mut self, handle: isize) -> Result<crate::place::Rect, CuError> {
        crate::place::read_rect(handle).map_err(map_mechanism_err)
    }

    fn inspect_placement(
        &mut self,
        handle: isize,
        expected_pid: u32,
    ) -> Result<mechanism::window_placement::PlacementWindowInfo, CuError> {
        mechanism::window_placement::inspect(handle, expected_pid).map_err(map_mechanism_err)
    }

    fn apply_rect(
        &mut self,
        handle: isize,
        target: crate::place::Rect,
        visible: crate::place::Rect,
    ) -> Result<(crate::place::Rect, bool, bool), CuError> {
        crate::place::apply_rect(handle, target, visible).map_err(map_mechanism_err)
    }

    fn identity_matches(&mut self, identity: &PlaceIdentity) -> Result<bool, CuError> {
        let windows =
            mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
        Ok(windows.iter().any(|window| {
            window.handle == identity.handle
                && window.process_id == identity.process_id
                && window.app_name == identity.app_name
        }))
    }
}

#[derive(Debug)]
struct HistoryCommitFailure {
    message: String,
    published: bool,
}

trait HistoryCommitter {
    fn commit(&mut self, history: &crate::place::PlaceHistory) -> Result<(), HistoryCommitFailure>;
}

struct NativeHistoryCommitter;

impl HistoryCommitter for NativeHistoryCommitter {
    fn commit(&mut self, history: &crate::place::PlaceHistory) -> Result<(), HistoryCommitFailure> {
        history.save().map_err(|error| HistoryCommitFailure {
            message: error.to_string(),
            published: error.published(),
        })
    }
}

#[cfg(test)]
fn window_place_resolved<R, H>(
    action: crate::place::PlaceAction,
    target_window: &mechanism::window_enumerate::WindowInfo,
    screens: &[mechanism::window_enumerate::ScreenInfo],
    history: crate::place::PlaceHistory,
    runtime: &mut R,
    committer: &mut H,
) -> Result<serde_json::Value, CuError>
where
    R: PlaceRuntime,
    H: HistoryCommitter,
{
    window_place_transaction(
        PlaceRequest::Catalog(action),
        target_window,
        screens,
        history,
        runtime,
        committer,
    )
}

fn window_place_transaction<R, H>(
    request: PlaceRequest,
    target_window: &mechanism::window_enumerate::WindowInfo,
    screens: &[mechanism::window_enumerate::ScreenInfo],
    history: crate::place::PlaceHistory,
    runtime: &mut R,
    committer: &mut H,
) -> Result<serde_json::Value, CuError>
where
    R: PlaceRuntime,
    H: HistoryCommitter,
{
    let identity = PlaceIdentity {
        handle: target_window.handle,
        process_id: target_window.process_id,
        app_name: target_window.app_name.clone(),
    };
    let app_key = format!("{}:{}", identity.process_id, identity.app_name);
    let before = runtime.read_rect(identity.handle).map_err(|error| {
        CuError::new(
            "window_state_unavailable",
            format!(
                "could not read exact window bounds before placement: {error_message}",
                error_message = error.message
            ),
        )
        .with_detail(serde_json::json!({
            "stage": "read_before",
            "effect": "not_applied",
            "history": "unchanged",
            "window": identity.handle,
            "cause": error_payload(&error),
        }))
    })?;
    let geo_screens: Vec<_> = screens.iter().map(crate::place::screen_from_info).collect();

    let (requested_target, planned_history) = if let Some(action) = request.history() {
        let step = if matches!(action, crate::place::PlaceAction::Undo) {
            history.plan_undo(&app_key)
        } else {
            history.plan_redo(&app_key)
        };
        let Some((planned, rect)) = step else {
            return Err(CuError::new(
                "unsupported",
                format!("{} has no {} history", app_key, action.kebab()),
            ));
        };
        (rect, Some(planned))
    } else {
        let dest = match request {
            PlaceRequest::Frame(rect) => rect,
            PlaceRequest::Catalog(action) => crate::place::place(action, before, &geo_screens)
                .ok_or_else(|| CuError::new("failed", "could not compute destination rectangle"))?,
        };
        (dest, None)
    };

    let inspection = runtime
        .inspect_placement(identity.handle, identity.process_id)
        .map_err(|error| {
            CuError::new(error.code.clone(), error.message.clone()).with_detail(serde_json::json!({
                "stage": "placement_preflight",
                "effect": "not_applied",
                "history": "unchanged",
                "window": identity.handle,
                "app": app_key,
                "cause": error_payload(&error),
            }))
        })?;
    let constraint = placement_target(before, requested_target, inspection).map_err(|error| {
        CuError::new(error.code.clone(), error.message.clone()).with_detail(serde_json::json!({
            "stage": "placement_preflight",
            "effect": "not_applied",
            "history": "unchanged",
            "window": identity.handle,
            "app": app_key,
            "cause": error_payload(&error),
        }))
    })?;
    let after_target = constraint.target;

    let (screen_index, screen) = screen_for_rect(after_target, screens)
        .ok_or_else(|| CuError::new("failed", "could not resolve destination screen"))?;
    let visible = crate::place::rect_from_bounds(screen.visible);
    let rollback_visible = screen_for_rect(before, screens)
        .map(|(_, screen)| crate::place::rect_from_bounds(screen.visible))
        .unwrap_or(visible);
    match runtime.identity_matches(&identity) {
        Ok(true) => {}
        Ok(false) => {
            return Err(CuError::new(
                "window_identity_changed",
                "selected window identity changed before placement",
            )
            .with_detail(serde_json::json!({
                "stage": "identity_before_apply",
                "effect": "not_applied",
                "history": "unchanged",
                "window": identity.handle,
                "app": app_key,
            })));
        }
        Err(error) => {
            return Err(CuError::new(
                "window_identity_unavailable",
                format!(
                    "could not revalidate selected window identity: {}",
                    error.message
                ),
            )
            .with_detail(serde_json::json!({
                "stage": "identity_before_apply",
                "effect": "not_applied",
                "history": "unchanged",
                "window": identity.handle,
                "app": app_key,
                "cause": error_payload(&error),
            })));
        }
    }
    let current_before_apply = runtime.read_rect(identity.handle).map_err(|error| {
        CuError::new(
            "window_state_unavailable",
            format!(
                "could not revalidate window bounds before placement: {}",
                error.message
            ),
        )
        .with_detail(serde_json::json!({
            "stage": "state_before_apply",
            "effect": "not_applied",
            "history": "unchanged",
            "window": identity.handle,
            "app": app_key,
            "before": rect_payload(before),
            "cause": error_payload(&error),
        }))
    })?;
    if current_before_apply != before {
        return Err(CuError::new(
            "window_state_changed",
            "window bounds changed while placement was being prepared",
        )
        .with_detail(serde_json::json!({
            "stage": "state_before_apply",
            "effect": "not_applied",
            "history": "unchanged",
            "window": identity.handle,
            "app": app_key,
            "before": rect_payload(before),
            "observed": rect_payload(current_before_apply),
        })));
    }
    let (after, quantized, clamped) =
        match runtime.apply_rect(identity.handle, after_target, visible) {
            Ok(outcome) => outcome,
            Err(error) => {
                return Err(recover_after_place_failure(
                    error,
                    runtime,
                    PlaceRecovery {
                        stage: "actuation",
                        identity: &identity,
                        before,
                        intended: after_target,
                        expected_current: None,
                        rollback_visible,
                        history_state: "unchanged",
                    },
                ));
            }
        };
    let next_history = planned_history
        .unwrap_or_else(|| history.plan_record(&app_key, identity.handle, before, after));
    if let Err(error) = committer.commit(&next_history) {
        if error.published {
            return Err(CuError::new(
                "history_durability_uncertain",
                format!(
                    "history was published but its directory durability is uncertain: {}",
                    error.message
                ),
            )
            .with_detail(serde_json::json!({
                "stage": "history_commit",
                "effect": "committed",
                "history": "published_durability_uncertain",
                "window": identity.handle,
                "app": app_key,
                "action": request.kebab(),
                "before": rect_payload(before),
                "intended": rect_payload(after_target),
                "applied": rect_payload(after),
                "cause": { "code": "history_sync_failed", "message": error.message },
            })));
        }
        return Err(recover_after_place_failure(
            CuError::new("history_commit_failed", error.message),
            runtime,
            PlaceRecovery {
                stage: "history_commit",
                identity: &identity,
                before,
                intended: after_target,
                expected_current: Some(after),
                rollback_visible,
                history_state: "unchanged",
            },
        ));
    }

    let constraint_adjusted = constraint.adjusted
        || (constraint.mode == "application_enforced" && !after.almost_eq(after_target));
    Ok(serde_json::json!({
        "effect": "committed",
        "history": "committed",
        "action": request.kebab(),
        "spectacle_id": request.spectacle_id(),
        "window": identity.handle,
        "app": app_key,
        "screen": {
            "index": screen_index,
            "frame": screen.frame,
            "visible": screen.visible,
            "primary": screen.primary,
        },
        "before": { "x": before.x, "y": before.y, "width": before.width, "height": before.height },
        "after": { "x": after.x, "y": after.y, "width": after.width, "height": after.height },
        "quantized": quantized,
        "clamped": clamped,
        "constraint_mode": constraint.mode,
        "constraint_adjusted": constraint_adjusted,
    }))
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PlacementTarget {
    target: crate::place::Rect,
    mode: &'static str,
    adjusted: bool,
}

fn placement_target(
    before: crate::place::Rect,
    requested: crate::place::Rect,
    inspection: mechanism::window_placement::PlacementWindowInfo,
) -> Result<PlacementTarget, CuError> {
    use mechanism::window_placement::{PlacementRole, SizeConstraints, Support};

    if !matches!(
        inspection.role,
        PlacementRole::Standard | PlacementRole::Dialog
    ) {
        return Err(CuError::new(
            "window_role_refused",
            format!(
                "window role {:?} is not eligible for placement",
                inspection.role
            ),
        ));
    }
    let (bx, by, bw, bh) = before.to_i32();
    let (rx, ry, rw, rh) = requested.to_i32();
    let moves = (bx, by) != (rx, ry);
    let resizes = (bw, bh) != (rw, rh);
    if moves && inspection.movable != Support::Yes {
        return Err(CuError::new(
            "window_not_movable",
            format!(
                "window movable support is {:?}, not Yes",
                inspection.movable
            ),
        ));
    }
    if resizes && inspection.resizable != Support::Yes {
        return Err(CuError::new(
            "window_not_resizable",
            format!(
                "window resizable support is {:?}, not Yes",
                inspection.resizable
            ),
        ));
    }
    match inspection.constraints {
        SizeConstraints::Unknown if resizes => Err(CuError::new(
            "window_constraints_unknown",
            "window size constraints are unknown; refusing resize",
        )),
        SizeConstraints::Unknown => Ok(PlacementTarget {
            target: requested,
            mode: "unknown",
            adjusted: false,
        }),
        SizeConstraints::ApplicationEnforced => Ok(PlacementTarget {
            target: requested,
            mode: "application_enforced",
            adjusted: false,
        }),
        SizeConstraints::Explicit {
            min,
            max,
            increment,
        } => {
            if !resizes {
                return Ok(PlacementTarget {
                    target: requested,
                    mode: "explicit",
                    adjusted: false,
                });
            }
            let normalize_axis = |value: u32,
                                  min: Option<u32>,
                                  max: Option<u32>,
                                  increment: Option<u32>|
             -> Result<u32, CuError> {
                let lower = min.unwrap_or(1);
                let upper = max.unwrap_or(u32::MAX);
                let mut normalized = value.clamp(lower, upper);
                if let Some(step) = increment {
                    let base = u64::from(min.unwrap_or(0));
                    let step = u64::from(step);
                    let lower_delta = u64::from(lower).saturating_sub(base);
                    let upper_delta = u64::from(upper).saturating_sub(base);
                    let first = lower_delta.div_ceil(step);
                    let last = upper_delta / step;
                    if first > last {
                        return Err(CuError::new(
                            "window_constraints_invalid",
                            "size increment has no value inside the min/max range",
                        ));
                    }
                    let desired_delta = u64::from(normalized).saturating_sub(base);
                    let nearest = (desired_delta + step / 2) / step;
                    let steps = nearest.clamp(first, last);
                    normalized = u32::try_from(base + steps * step).map_err(|_| {
                        CuError::new(
                            "window_constraints_invalid",
                            "normalized size exceeds the ABI dimension range",
                        )
                    })?;
                }
                Ok(normalized)
            };
            let width = normalize_axis(
                rw,
                min.map(|s| s.width),
                max.map(|s| s.width),
                increment.map(|s| s.width),
            )?;
            let height = normalize_axis(
                rh,
                min.map(|s| s.height),
                max.map(|s| s.height),
                increment.map(|s| s.height),
            )?;
            let target = crate::place::Rect::new(rx as f64, ry as f64, width as f64, height as f64);
            Ok(PlacementTarget {
                adjusted: !target.almost_eq(requested),
                target,
                mode: "explicit",
            })
        }
    }
}

#[derive(Clone, Copy)]
struct PlaceRecovery<'a> {
    stage: &'a str,
    identity: &'a PlaceIdentity,
    before: crate::place::Rect,
    intended: crate::place::Rect,
    expected_current: Option<crate::place::Rect>,
    rollback_visible: crate::place::Rect,
    history_state: &'a str,
}

fn recover_after_place_failure<R: PlaceRuntime>(
    cause: CuError,
    runtime: &mut R,
    recovery: PlaceRecovery<'_>,
) -> CuError {
    let PlaceRecovery {
        stage,
        identity,
        before,
        intended,
        expected_current,
        rollback_visible,
        history_state,
    } = recovery;
    let observed = match runtime.read_rect(identity.handle) {
        Ok(observed) if observed == before => {
            return CuError::new(
                if stage == "history_commit" {
                    "history_commit_failed"
                } else {
                    "window_place_failed"
                },
                format!("{}; window bounds remained unchanged", cause.message),
            )
            .with_detail(serde_json::json!({
                "stage": stage,
                "effect": "not_applied",
                "history": history_state,
                "rollback": "not_needed",
                "window": identity.handle,
                "app": format!("{}:{}", identity.process_id, identity.app_name),
                "before": rect_payload(before),
                "intended": rect_payload(intended),
                "observed": rect_payload(observed),
                "cause": error_payload(&cause),
            }));
        }
        Ok(observed) => {
            if expected_current.is_none() {
                return in_doubt_error(
                    stage,
                    history_state,
                    identity,
                    before,
                    intended,
                    Some(observed),
                    "skipped_unverified_apply_state",
                    &cause,
                    None,
                );
            }
            if expected_current.is_some_and(|expected| observed != expected) {
                return in_doubt_error(
                    stage,
                    history_state,
                    identity,
                    before,
                    intended,
                    Some(observed),
                    "skipped_external_change",
                    &cause,
                    None,
                );
            }
            observed
        }
        Err(read_error) => {
            return in_doubt_error(
                stage,
                history_state,
                identity,
                before,
                intended,
                None,
                "readback_failed",
                &cause,
                Some(&read_error),
            );
        }
    };
    match runtime.identity_matches(identity) {
        Ok(true) => {}
        Ok(false) => {
            return in_doubt_error(
                stage,
                history_state,
                identity,
                before,
                intended,
                Some(observed),
                "skipped_identity_changed",
                &cause,
                None,
            );
        }
        Err(identity_error) => {
            return in_doubt_error(
                stage,
                history_state,
                identity,
                before,
                intended,
                Some(observed),
                "identity_check_failed",
                &cause,
                Some(&identity_error),
            );
        }
    }
    match runtime.read_rect(identity.handle) {
        Ok(current) if current == observed => {}
        Ok(current) => {
            return in_doubt_error(
                stage,
                history_state,
                identity,
                before,
                intended,
                Some(current),
                "skipped_external_change",
                &cause,
                None,
            );
        }
        Err(read_error) => {
            return in_doubt_error(
                stage,
                history_state,
                identity,
                before,
                intended,
                None,
                "rollback_read_failed",
                &cause,
                Some(&read_error),
            );
        }
    }
    if let Err(rollback_error) = runtime.apply_rect(identity.handle, before, rollback_visible) {
        let current = runtime.read_rect(identity.handle).ok();
        return in_doubt_error(
            stage,
            history_state,
            identity,
            before,
            intended,
            current,
            "rollback_failed",
            &cause,
            Some(&rollback_error),
        );
    }
    match runtime.read_rect(identity.handle) {
        Ok(restored) if restored == before => CuError::new(
            if stage == "history_commit" {
                "history_commit_failed"
            } else {
                "window_place_failed"
            },
            format!("{}; window placement was rolled back", cause.message),
        )
        .with_detail(serde_json::json!({
            "stage": stage,
            "effect": "rolled_back",
            "history": history_state,
            "rollback": "verified",
            "window": identity.handle,
            "app": format!("{}:{}", identity.process_id, identity.app_name),
            "before": rect_payload(before),
            "intended": rect_payload(intended),
            "applied": rect_payload(observed),
            "observed": rect_payload(restored),
            "cause": error_payload(&cause),
        })),
        Ok(restored) => in_doubt_error(
            stage,
            history_state,
            identity,
            before,
            intended,
            Some(restored),
            "rollback_unverified",
            &cause,
            None,
        ),
        Err(read_error) => in_doubt_error(
            stage,
            history_state,
            identity,
            before,
            intended,
            None,
            "rollback_readback_failed",
            &cause,
            Some(&read_error),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn in_doubt_error(
    stage: &str,
    history_state: &str,
    identity: &PlaceIdentity,
    before: crate::place::Rect,
    intended: crate::place::Rect,
    observed: Option<crate::place::Rect>,
    rollback: &str,
    cause: &CuError,
    rollback_error: Option<&CuError>,
) -> CuError {
    let mut detail = serde_json::json!({
        "stage": stage,
        "effect": "possibly_applied",
        "history": history_state,
        "rollback": rollback,
        "window": identity.handle,
        "app": format!("{}:{}", identity.process_id, identity.app_name),
        "before": rect_payload(before),
        "intended": rect_payload(intended),
        "cause": error_payload(cause),
    });
    if let Some(observed) = observed {
        detail["observed"] = rect_payload(observed);
    }
    if let Some(error) = rollback_error {
        detail["rollback_error"] = error_payload(error);
    }
    CuError::new(
        "window_place_in_doubt",
        format!(
            "window placement may have changed the window and could not be verified or restored: {}",
            cause.message
        ),
    )
    .with_detail(detail)
}

fn rect_payload(rect: crate::place::Rect) -> serde_json::Value {
    serde_json::json!({
        "x": rect.x,
        "y": rect.y,
        "width": rect.width,
        "height": rect.height,
    })
}

fn error_payload(error: &CuError) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "code": error.code,
        "message": error.message,
    });
    if let Some(detail) = &error.detail {
        payload["detail"] = detail.clone();
    }
    payload
}

fn screen_for_rect(
    rect: crate::place::Rect,
    screens: &[mechanism::window_enumerate::ScreenInfo],
) -> Option<(usize, &mechanism::window_enumerate::ScreenInfo)> {
    let mut best: Option<(f64, usize, &mechanism::window_enumerate::ScreenInfo)> = None;
    for (index, screen) in screens.iter().enumerate() {
        let frame = crate::place::rect_from_bounds(screen.frame);
        if frame.contains(rect) {
            return Some((index, screen));
        }
        if let Some(hit) = rect.intersection(frame) {
            let proportion = hit.area() / rect.area().max(1.0);
            if best
                .as_ref()
                .map(|(current, _, _)| proportion > *current)
                .unwrap_or(true)
            {
                best = Some((proportion, index, screen));
            }
        }
    }
    best.map(|(_, index, screen)| (index, screen))
        .or_else(|| screens.first().map(|screen| (0, screen)))
}

fn wait(timeout_ms: u64, condition: &WaitCondition) -> Result<serde_json::Value, CuError> {
    match condition {
        WaitCondition::Expect { window, expect } => {
            return wait_expect(timeout_ms, *window, expect);
        }
        WaitCondition::NodeNameContains {
            pattern,
            role,
            window,
        } => return wait_node(timeout_ms, pattern, role.as_deref(), *window),
        WaitCondition::NodeTextEquals {
            expected,
            name,
            role,
            window,
        } => {
            return wait_node_text(
                timeout_ms,
                expected,
                name,
                role.as_deref(),
                *window,
                NodeTextMatch::Equals,
            );
        }
        WaitCondition::NodeTextContains {
            substring,
            name,
            role,
            window,
        } => {
            return wait_node_text(
                timeout_ms,
                substring,
                name,
                role.as_deref(),
                *window,
                NodeTextMatch::Contains,
            );
        }
        _ => {}
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(120_000));
    let poll = Duration::from_millis(50);
    let mut last_observation = serde_json::json!({ "windows": [] });

    while Instant::now() < deadline {
        let windows =
            mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
        last_observation = serde_json::json!({ "window_count": windows.len(), "windows": windows });
        if condition_met(condition, &windows) {
            return Ok(serde_json::json!({
                "met": true,
                "observation": last_observation,
            }));
        }
        thread::sleep(poll);
    }

    Ok(serde_json::json!({
        "met": false,
        "timeout_ms": timeout_ms,
        "observation": last_observation,
    }))
}

fn condition_met(condition: &WaitCondition, windows: &[WindowInfo]) -> bool {
    match condition {
        WaitCondition::WindowCountGte { count } => windows.len() >= *count,
        WaitCondition::WindowTitleContains { pattern } => {
            let pat = pattern.to_ascii_lowercase();
            windows
                .iter()
                .any(|window| window.title.to_ascii_lowercase().contains(&pat))
        }
        WaitCondition::FocusedHandle { handle } => windows
            .iter()
            .any(|window| window.focused && window.handle == *handle),
        // Polled against the accessibility tree, not the window list.
        WaitCondition::Expect { .. }
        | WaitCondition::NodeNameContains { .. }
        | WaitCondition::NodeTextEquals { .. }
        | WaitCondition::NodeTextContains { .. } => false,
    }
}

/// Polls `tree` until exactly one showing node whose name contains `pattern`
/// (and whose role matches `role`, when given) appears. Two or more showing
/// hits fail typed (`a11y_node_ambiguous`) instead of taking the first.
/// Timeout is a typed failure so loop-until callers break on `ok:false`
/// instead of retrying blind.
fn wait_node(
    timeout_ms: u64,
    pattern: &str,
    role: Option<&str>,
    window: Option<isize>,
) -> Result<serde_json::Value, CuError> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(120_000));
    let poll = Duration::from_millis(50);
    let mut polls = 0usize;
    let mut last_node_count = 0usize;
    let mut last_error: Option<CuError> = None;

    loop {
        polls += 1;
        match mechanism::tree_for_window(window) {
            Ok(tree) => {
                last_node_count = tree.nodes.len();
                last_error.take();
                let matches = showing_name_matches(&tree.nodes, pattern, role);
                match matches.len() {
                    0 => {}
                    1 => {
                        return Ok(serde_json::json!({
                            "met": true,
                            "addressing": "accessibility-tree",
                            "mechanism": "libagenterm",
                            "backend": tree.backend,
                            "window": window,
                            "polls": polls,
                            "node": matches[0],
                            "observation": { "node_count": last_node_count },
                        }));
                    }
                    count => return Err(name_match_error(pattern, role, count)),
                }
            }
            // The tree can be missing outright; that is not something more
            // polling will fix.
            Err(mechanism::MechanismError::Unsupported { .. }) => {
                return Err(map_mechanism_err(mechanism::MechanismError::Unsupported {
                    reason: "accessibility-tree mechanism unavailable".to_owned(),
                }));
            }
            // A scoped window may not have an AT-SPI root yet — keep polling and
            // report the last failure if we run out of time.
            Err(error) => last_error = Some(map_mechanism_err(error)),
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(poll);
    }

    let detail = match last_error {
        Some(error) => format!("last tree read failed: {} ({})", error.message, error.code),
        None => format!("last tree read had {last_node_count} nodes"),
    };
    Err(CuError::new(
        "timeout",
        format!(
            "no showing accessibility node with {} after {timeout_ms}ms ({polls} polls, {detail})",
            name_scope(pattern, role)
        ),
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NodeTextMatch {
    Equals,
    Contains,
}

impl NodeTextMatch {
    fn flag(self) -> &'static str {
        match self {
            Self::Equals => "--text-equals",
            Self::Contains => "--text-contains",
        }
    }

    fn matches(self, text: &str, expected: &str) -> bool {
        match self {
            Self::Equals => text == expected,
            Self::Contains => text.contains(expected),
        }
    }

    fn timeout_verb(self) -> &'static str {
        match self {
            Self::Equals => "did not reach text",
            Self::Contains => "did not contain",
        }
    }
}

/// Polls AT-SPI `Text.GetText` (`agt_a11y_node_get_text`) on the unique
/// showing node addressed by `name` until that independent text equals
/// `expected` (`--text-equals`) or contains it (`--text-contains`). The
/// tree snapshot `node.text`, a prior `send-text` / `paste` / `copy`
/// `matched.text`, `last_text_write_via`, and the WebKit eval helper's
/// queued-job `OK` (Reasonix composer) are not this predicate. Timeout
/// is typed so loop-until callers break on `ok:false`.
fn wait_node_text(
    timeout_ms: u64,
    expected: &str,
    name: &str,
    role: Option<&str>,
    window: Option<isize>,
    match_kind: NodeTextMatch,
) -> Result<serde_json::Value, CuError> {
    if window.is_none() {
        return Err(CuError::new(
            "invalid_input",
            format!("wait {} requires --window <handle>", match_kind.flag()),
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(120_000));
    let poll = Duration::from_millis(50);
    let mut polls = 0usize;
    let mut last_node_count = 0usize;
    let mut last_text: Option<String> = None;
    let mut last_error: Option<CuError> = None;

    loop {
        polls += 1;
        match mechanism::tree_for_window(window) {
            Ok(tree) => {
                last_node_count = tree.nodes.len();
                last_error.take();
                let matches = showing_name_matches(&tree.nodes, name, role);
                match matches.len() {
                    0 => {}
                    1 => match mechanism::get_node_text(window, &matches[0].id) {
                        Ok(text) => {
                            last_text = Some(text.clone());
                            if match_kind.matches(&text, expected) {
                                return Ok(text_equals_success(
                                    &tree.backend,
                                    window,
                                    polls,
                                    matches[0],
                                    &text,
                                    last_node_count,
                                ));
                            }
                        }
                        Err(error @ mechanism::MechanismError::Unsupported { .. }) => {
                            return Err(map_mechanism_err(error));
                        }
                        Err(error) => last_error = Some(map_mechanism_err(error)),
                    },
                    count => return Err(name_match_error(name, role, count)),
                }
            }
            Err(error @ mechanism::MechanismError::Unsupported { .. }) => {
                return Err(map_mechanism_err(error));
            }
            Err(error) => last_error = Some(map_mechanism_err(error)),
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(poll);
    }

    Err(CuError::new(
        "timeout",
        format!(
            "accessibility node with {} {} {expected:?} after {timeout_ms}ms ({polls} polls, {})",
            name_scope(name, role),
            match_kind.timeout_verb(),
            text_equals_timeout_detail(last_text.as_deref(), last_error.as_ref(), last_node_count,)
        ),
    ))
}

/// Success payload for `--text-equals` / `--text-contains`. `gettext` is
/// the only text authority: snapshot `node.text` is overwritten so a
/// sidecar tree walk or `send-text` / `paste` `matched.text` cannot be
/// mistaken for the hit. Published `text` is the full independent GetText.
fn text_equals_success(
    backend: &str,
    window: Option<isize>,
    polls: usize,
    node: &mechanism::A11yNode,
    gettext: &str,
    node_count: usize,
) -> serde_json::Value {
    let mut node = node.clone();
    node.text = Some(gettext.to_owned());
    serde_json::json!({
        "met": true,
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": backend,
        "window": window,
        "polls": polls,
        "node": node,
        "text": gettext,
        "via": "gettext",
        "observation": {
            "node_count": node_count,
            "text": gettext,
        },
    })
}

fn text_equals_timeout_detail(
    last_text: Option<&str>,
    last_error: Option<&CuError>,
    last_node_count: usize,
) -> String {
    match (last_text, last_error) {
        (Some(text), _) => format!("last GetText={text:?}"),
        (None, Some(error)) => {
            format!("last GetText failed: {} ({})", error.message, error.code)
        }
        (None, None) => format!("last tree read had {last_node_count} nodes"),
    }
}

fn showing_name_matches<'a>(
    nodes: &'a [mechanism::A11yNode],
    pattern: &str,
    role: Option<&str>,
) -> Vec<&'a mechanism::A11yNode> {
    let name_pat = pattern.to_ascii_lowercase();
    let role_pat = role.map(str::to_ascii_lowercase);
    nodes
        .iter()
        .filter(|node| node_matches(node, &name_pat, role_pat.as_deref()))
        .collect()
}

fn require_unique_showing_node<'a>(
    nodes: &'a [mechanism::A11yNode],
    pattern: &str,
    role: Option<&str>,
) -> Result<&'a mechanism::A11yNode, CuError> {
    let matches = showing_name_matches(nodes, pattern, role);
    match matches.len() {
        1 => Ok(matches[0]),
        count => Err(name_match_error(pattern, role, count)),
    }
}

fn name_match_error(pattern: &str, role: Option<&str>, count: usize) -> CuError {
    if count == 0 {
        return CuError::new(
            "a11y_node_not_found",
            format!(
                "no showing accessibility node with {}",
                name_scope(pattern, role)
            ),
        );
    }
    CuError::new(
        "a11y_node_ambiguous",
        format!(
            "{count} showing accessibility nodes with {}",
            name_scope(pattern, role)
        ),
    )
    .with_count(count)
}

fn node_matches(node: &mechanism::A11yNode, name_pat: &str, role_pat: Option<&str>) -> bool {
    if !node_is_showing(node) {
        return false;
    }
    if !node.name.to_ascii_lowercase().contains(name_pat) {
        return false;
    }
    match role_pat {
        Some(role) => node.role.to_ascii_lowercase().contains(role),
        None => true,
    }
}

fn node_is_showing(node: &mechanism::A11yNode) -> bool {
    node.states
        .iter()
        .any(|state| state.eq_ignore_ascii_case("showing") || state.eq_ignore_ascii_case("visible"))
}

// ---------------------------------------------------------------------------
// invoke / verify / wait --expect (PRD 29 default loop, PRD 31 invariants).
// ---------------------------------------------------------------------------

fn target_error(error: observe::TargetError) -> CuError {
    match error {
        observe::TargetError::Invalid(message) => CuError::new("invalid_input", message),
        observe::TargetError::Missing(message) => CuError::new("a11y_node_not_found", message),
        observe::TargetError::Ambiguous { count, scope } => CuError::new(
            "ambiguous",
            format!("{count} showing accessibility nodes with {scope}; refusing to guess"),
        )
        .with_count(count)
        .with_detail(serde_json::json!({ "matches": count })),
    }
}

/// The platform action for an `invoke` verb plus its validated value.
fn invoke_action(
    action: InvokeAction,
    value: Option<&str>,
) -> Result<mechanism::NodeAction, CuError> {
    match action.value_kind() {
        InvokeValueKind::None => {
            if value.is_some() {
                return Err(invalid_input(format!(
                    "invoke {} takes no value",
                    action.as_str()
                )));
            }
        }
        InvokeValueKind::Text => {
            if value.is_none() {
                return Err(invalid_input(format!(
                    "invoke {} requires a value",
                    action.as_str()
                )));
            }
        }
        InvokeValueKind::Flag => {
            if !matches!(value, Some("true") | Some("false")) {
                return Err(invalid_input(format!(
                    "invoke {} requires true or false",
                    action.as_str()
                )));
            }
        }
    }
    let flag = value == Some("true");
    let text = value.unwrap_or_default().to_owned();
    Ok(match action {
        InvokeAction::Press => mechanism::NodeAction::Press,
        InvokeAction::SetValue => mechanism::NodeAction::SetValue(text),
        InvokeAction::SelectOption => mechanism::NodeAction::SelectOption(text),
        InvokeAction::SetChecked => mechanism::NodeAction::SetChecked(flag),
        InvokeAction::SetExpanded => mechanism::NodeAction::SetExpanded(flag),
        InvokeAction::Increment => mechanism::NodeAction::Increment,
        InvokeAction::Decrement => mechanism::NodeAction::Decrement,
        InvokeAction::ScrollTo => {
            return Err(CuError::new(
                "invalid_input",
                "internal: invoke scroll-to uses agt_a11y_node_scroll, not NodeAction",
            ));
        }
        InvokeAction::SetSelection => {
            return Err(CuError::new(
                "invalid_input",
                "internal: invoke set-selection uses agt_a11y_node_set_selection, not NodeAction",
            ));
        }
        InvokeAction::SetSelected => mechanism::NodeAction::SetSelected(flag),
        InvokeAction::Cancel => mechanism::NodeAction::Cancel,
        InvokeAction::ShowDefaultUi => mechanism::NodeAction::ShowDefaultUi,
    })
}

/// The normalized action name a node must list before cu even asks the
/// backend (`set-value` / `select-option` are attribute writes the backend
/// alone can judge).
fn required_node_action(action: InvokeAction) -> Option<&'static str> {
    match action {
        InvokeAction::Press | InvokeAction::SetChecked | InvokeAction::SetExpanded => Some("click"),
        InvokeAction::Increment => Some("increment"),
        InvokeAction::Decrement => Some("decrement"),
        InvokeAction::SetValue
        | InvokeAction::SelectOption
        | InvokeAction::SetSelected
        | InvokeAction::SetSelection
        | InvokeAction::ScrollTo
        | InvokeAction::Cancel
        | InvokeAction::ShowDefaultUi => None,
    }
}

/// One semantic action with a read-back receipt. Never activates or raises
/// the window: the only mechanism is the a11y node action.
fn invoke_payload(
    window: isize,
    mut spec: observe::TargetSpec,
    action: InvokeAction,
    value: Option<&str>,
    selector: Option<&str>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "invoke requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    if let Some(selector) = selector {
        if spec.node.is_some()
            || spec.index.is_some()
            || spec.name.is_some()
            || spec.identifier.is_some()
            || spec.focused
        {
            return Err(invalid_input(
                "invoke --selector cannot mix with --node/--index/--name/--identifier/--focused"
                    .into(),
            ));
        }
        let tree = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
        let hit = observe::walk_selector(&tree, selector).map_err(invalid_input)?;
        let Some(hit) = hit else {
            return Err(CuError::new(
                "a11y_node_not_found",
                format!("invoke --selector {selector:?} matched no node"),
            ));
        };
        spec.node = Some(hit.id.clone());
    }
    if action == InvokeAction::ScrollTo && value.is_some() {
        return Err(invalid_input("invoke scroll-to takes no value".into()));
    }
    if action == InvokeAction::SetSelection {
        let raw = value.ok_or_else(|| {
            invalid_input("invoke set-selection requires <start>:<length>".into())
        })?;
        observe::parse_text_selection(raw).map_err(invalid_input)?;
    }
    let node_action = if matches!(action, InvokeAction::ScrollTo | InvokeAction::SetSelection) {
        None
    } else {
        Some(invoke_action(action, value)?)
    };
    // `--focused`: the platform names the application's own focused control
    // first; the tree read that follows must still show the same identity
    // (id, role, identifier) at that path, so PID + window + focused
    // identity are bound in one observation before anything is pressed.
    let focused_identity = if spec.focused {
        if spec.node.is_some()
            || spec.index.is_some()
            || spec.name.is_some()
            || spec.identifier.is_some()
        {
            return Err(invalid_input(
                "--focused addresses the focused control; combine it only with --role".into(),
            ));
        }
        Some(focused_control(window, spec.role.as_deref())?.1)
    } else {
        None
    };
    let before = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    let target = match &focused_identity {
        Some(focused) => {
            let Some(now) = observe::node_by_id(&before, &focused.id) else {
                return Err(CuError::new(
                    "a11y_node_recycled",
                    format!(
                        "the focused control at {} is not in the window tree any more",
                        focused.id
                    ),
                ));
            };
            if now.role != focused.role || now.identifier != focused.identifier {
                return Err(CuError::new(
                    "a11y_node_recycled",
                    format!(
                        "the focused control at {} changed identity between reads ({} {:?} -> {} {:?})",
                        focused.id, focused.role, focused.identifier, now.role, now.identifier
                    ),
                ));
            }
            now.clone()
        }
        None => {
            let flat = observe::flatten(&before);
            let hit = observe::resolve_target(&flat, &spec).map_err(target_error)?;
            hit.node.clone()
        }
    };
    // Refuse to press what the node does not offer -- but only where an
    // empty action list is a *claim*. The contract says an empty list means
    // the backend reported none and never that it was not asked; the AT-SPI
    // adapter breaks that on purpose, skipping Action during the walk
    // because WebKitGTK hangs `GetActions`. So on that backend every node
    // reports no actions, and this guard refused `invoke press` on a live
    // GTK button -- measured against a real widget tree, where the whole
    // verb was unreachable through name addressing.
    //
    // Where the walk does not read action names, the mechanism judges
    // instead: it asks the node itself and fails typed
    // (`a11y_action_unavailable`) if the action is missing. That is one
    // round trip later than this check, and honest, which this check was
    // not.
    let backend_publishes_actions = before.backend != "at-spi2";
    if let Some(required) = required_node_action(action)
        && backend_publishes_actions
        && !target
            .actions
            .iter()
            .any(|offered| offered.eq_ignore_ascii_case(required))
    {
        return Err(CuError::new(
            "unsupported",
            format!(
                "node {} ({} {:?}) does not offer {} (actions: {})",
                target.id,
                target.role,
                target.name,
                action.as_str(),
                if target.actions.is_empty() {
                    "none".to_owned()
                } else {
                    target.actions.join(", ")
                }
            ),
        )
        .with_detail(serde_json::json!({
            "reason": "node_action_missing",
            "required": required,
            "offered": target.actions,
        })));
    }
    // Desired-state verbs: an unobservable state is refused before any
    // action; an already-matching state is a verified no-op.
    let desired = match &node_action {
        Some(mechanism::NodeAction::SetChecked(flag)) => {
            Some(("checked", *flag, observe::checked_state(&target)))
        }
        Some(mechanism::NodeAction::SetExpanded(flag)) => {
            Some(("expanded", *flag, observe::expanded_state(&target)))
        }
        Some(mechanism::NodeAction::SetSelected(flag)) => {
            Some(("selected", *flag, observe::selected_state(&target)))
        }
        _ => None,
    };
    let mut performed = true;
    if let Some((field, flag, state)) = desired {
        match state {
            observe::Tri::Unknown => {
                return Err(CuError::new(
                    "unsupported",
                    format!(
                        "node {} ({} {:?}) exposes no {field} state; refusing to press blind",
                        target.id, target.role, target.name
                    ),
                )
                .with_detail(
                    serde_json::json!({ "reason": "state_unobservable", "state": field }),
                ));
            }
            observe::Tri::True | observe::Tri::False if state.as_bool() == Some(flag) => {
                performed = false;
            }
            _ => {}
        }
    }
    // The crash-persistent receipt is reserved here — after every refusal
    // that needs no mechanism, before the mechanism is touched — so a line
    // with no `completed` / `failed` partner means "uncertain", never "did
    // not happen".
    let node_json = serde_json::json!({
        "id": target.id,
        "role": target.role,
        "name": target.name,
        "identifier": target.identifier,
        "index": before.nodes.iter().position(|node| node.id == target.id),
    });
    let ticket = receipts.reserve(
        "invoke",
        window,
        serde_json::json!({
            "spec": spec.json(),
            "node": node_json,
            "action": action.as_str(),
            "value": value,
            "performed": performed,
            "before": observe::node_state_json(&target),
        }),
    )?;
    let mut mechanism_error = None;
    if performed {
        let result = if action == InvokeAction::ScrollTo {
            mechanism::scroll_node(Some(window), &target.id)
        } else if action == InvokeAction::SetSelection {
            let raw = value.unwrap_or("");
            let (start, end) = observe::parse_text_selection(raw).map_err(invalid_input)?;
            mechanism::set_node_selection(Some(window), &target.id, start, end)
        } else {
            mechanism::perform_node_action(
                Some(window),
                &target.id,
                node_action.clone().expect("mapped invoke action"),
            )
        };
        if let Err(error) = result {
            mechanism_error = Some(map_mechanism_err(error));
        }
    }
    let after = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    let after_node = observe::node_by_id(&after, &target.id).cloned();
    if action == InvokeAction::SetSelection {
        let verified = mechanism_error.is_none();
        let verification = serde_json::json!({
            "method": "set-selection",
            "reason": if mechanism_error.is_some() { Some("mechanism_failed") } else { None::<&str> },
        });
        let after_state = after_node.as_ref().map(observe::node_state_json);
        let receipt = serde_json::json!({
            "addressing": "accessibility-tree",
            "mechanism": "libagenterm",
            "backend": before.backend,
            "window": window,
            "target": spec.json(),
            "node": node_json,
            "action": "set-selection",
            "via": "set-selection",
            "value": value,
            "performed": performed,
            "verified": verified,
            "verification": verification,
            "before": observe::node_state_json(&target),
            "after": after_state,
            "tree_changed": observe::tree_changed(&before, &after),
            "receipt": ticket.json(),
        });
        receipts.complete(
            &ticket,
            "invoke",
            window,
            verified,
            serde_json::json!({
                "after": after_state,
                "verification": verification,
                "error": mechanism_error.as_ref().map(error_payload),
            }),
        )?;
        if let Some(error) = mechanism_error {
            return Err(error.with_detail(serde_json::json!({ "receipt": receipt })));
        }
        return Ok(receipt);
    }
    if action == InvokeAction::ScrollTo {
        let verified = mechanism_error.is_none();
        let verification = serde_json::json!({
            "method": "scroll-to",
            "reason": if mechanism_error.is_some() { Some("mechanism_failed") } else { None::<&str> },
        });
        let after_state = after_node.as_ref().map(observe::node_state_json);
        let receipt = serde_json::json!({
            "addressing": "accessibility-tree",
            "mechanism": "libagenterm",
            "backend": before.backend,
            "window": window,
            "target": spec.json(),
            "node": node_json,
            "action": "scroll-to",
            "via": "scroll-to",
            "performed": performed,
            "verified": verified,
            "verification": verification,
            "before": observe::node_state_json(&target),
            "after": after_state,
            "tree_changed": observe::tree_changed(&before, &after),
            "receipt": ticket.json(),
        });
        receipts.complete(
            &ticket,
            "invoke",
            window,
            verified,
            serde_json::json!({
                "after": after_state,
                "verification": verification,
                "tree_changed": observe::tree_changed(&before, &after),
                "error": mechanism_error.as_ref().map(error_payload),
            }),
        )?;
        if let Some(error) = mechanism_error {
            return Err(error.with_detail(serde_json::json!({ "receipt": receipt })));
        }
        return Ok(receipt);
    }
    let node_action = node_action.expect("mapped invoke action");
    let (verified, method, reason) = match (&node_action, &after_node) {
        (mechanism::NodeAction::SetValue(wanted), Some(now))
        | (mechanism::NodeAction::SelectOption(wanted), Some(now)) => {
            let hit = now.text.as_deref() == Some(wanted.as_str());
            (
                hit,
                "value-readback",
                if hit { None } else { Some("value_mismatch") },
            )
        }
        (mechanism::NodeAction::SetChecked(wanted), Some(now)) => {
            let hit = observe::checked_state(now).as_bool() == Some(*wanted);
            (
                hit,
                "checked-readback",
                if hit { None } else { Some("state_mismatch") },
            )
        }
        (mechanism::NodeAction::SetSelected(wanted), Some(now)) => {
            let hit = observe::selected_state(now).as_bool() == Some(*wanted);
            (
                hit,
                "selected-readback",
                if hit { None } else { Some("state_mismatch") },
            )
        }
        (mechanism::NodeAction::SetExpanded(wanted), Some(now)) => {
            let hit = observe::expanded_state(now).as_bool() == Some(*wanted);
            (
                hit,
                "expanded-readback",
                if hit { None } else { Some("state_mismatch") },
            )
        }
        (mechanism::NodeAction::Increment, Some(now))
        | (mechanism::NodeAction::Decrement, Some(now)) => {
            match (observe::numeric_text(&target), observe::numeric_text(now)) {
                (Some(was), Some(is)) if was != is => (true, "value-readback", None),
                (Some(_), Some(_)) => (false, "value-readback", Some("value_unchanged")),
                _ => (false, "value-readback", Some("value_unreadable")),
            }
        }
        (mechanism::NodeAction::Press, Some(_)) => {
            if observe::tree_changed(&before, &after) {
                (true, "tree-diff", None)
            } else {
                (false, "tree-diff", Some("no_observable_change"))
            }
        }
        (mechanism::NodeAction::Press, None) => (true, "tree-diff", Some("node_gone")),
        (_, None) => (false, "node-readback", Some("node_gone")),
        _ => (false, "none", Some("unverifiable_action")),
    };
    let verified = verified && mechanism_error.is_none();
    let verification = serde_json::json!({
        "method": method,
        "reason": if mechanism_error.is_some() { Some("mechanism_failed") } else { reason },
    });
    let after_state = after_node.as_ref().map(observe::node_state_json);
    let receipt = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": before.backend,
        "window": window,
        "target": spec.json(),
        "node": node_json,
        "action": action.as_str(),
        "value": value,
        "performed": performed,
        "verified": verified,
        "verification": verification,
        "before": observe::node_state_json(&target),
        "after": after_state,
        "tree_changed": observe::tree_changed(&before, &after),
        "receipt": ticket.json(),
    });
    receipts.complete(
        &ticket,
        "invoke",
        window,
        verified,
        serde_json::json!({
            "after": after_state,
            "verification": verification,
            "tree_changed": observe::tree_changed(&before, &after),
            "error": mechanism_error.as_ref().map(error_payload),
        }),
    )?;
    if let Some(error) = mechanism_error {
        return Err(error.with_detail(serde_json::json!({ "receipt": receipt })));
    }
    Ok(receipt)
}

// ---------------------------------------------------------------------------
// Background menus, the App-local focused control, and the observation
// stream (slice 3 of plan/design-mcu-absorption.md).
// ---------------------------------------------------------------------------

fn menu_budget(
    depth: Option<u32>,
    max_nodes: Option<usize>,
) -> Result<mechanism::TreeBudget, CuError> {
    observe::validate_menu_budget(depth, max_nodes).map_err(invalid_input)?;
    Ok(mechanism::TreeBudget {
        max_depth: Some(observe::menu_node_depth(
            depth.unwrap_or(observe::DEFAULT_MENU_DEPTH),
        )),
        max_nodes: Some(max_nodes.unwrap_or(observe::DEFAULT_MENU_NODE_BUDGET)),
    })
}

/// Background menu inventory: the application's menu bar walked under a
/// menu-level / node budget, flattened to items with exact title paths.
fn menu_inspect_payload(
    window: isize,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    filter: observe::MenuFilter,
    offset: Option<usize>,
    max: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "menu inspect requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    let budget = menu_budget(depth, max_nodes)?;
    let page = observe::Page::new(offset, max).map_err(invalid_input)?;
    let tree =
        mechanism::menu_tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?;
    let items = observe::menu_items(&tree);
    let (hits, counts) = observe::menu_query(&items, &filter, page, tree.truncated);
    let rows = serde_json::to_value(&hits)
        .map_err(|error| CuError::new("serialize", error.to_string()))?;
    Ok(serde_json::json!({
        "addressing": "menu-path",
        "mechanism": "libagenterm",
        "backend": tree.backend,
        "window": window,
        "budget": {
            "depth": depth.unwrap_or(observe::DEFAULT_MENU_DEPTH),
            "max_nodes": max_nodes.unwrap_or(observe::DEFAULT_MENU_NODE_BUDGET),
        },
        "filter": {
            "title": filter.title,
            "exact": filter.exact,
            "enabled": filter.enabled,
        },
        "nodes_visited": tree.visited,
        "visited": counts.visited,
        "matched": counts.matched,
        "returned": counts.returned,
        "offset": counts.offset,
        "truncated": counts.truncated,
        "scan_truncated": counts.scan_truncated,
        "page_truncated": counts.page_truncated,
        "items": rows,
    }))
}

/// Press one menu item by exact title path in the background, verified by
/// the item's mark read-back and a whole-window tree diff.
fn menu_invoke_payload(
    window: isize,
    path: &[String],
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "menu invoke requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    if path.len() < 2 || path.iter().any(String::is_empty) {
        return Err(invalid_input(
            "menu invoke needs --path with a menu title and at least one non-empty item title"
                .into(),
        ));
    }
    let before = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    // The platform resolves the whole path (and refuses) before pressing,
    // so a refusal there leaves a `failed` receipt with nothing performed.
    let ticket = receipts.reserve(
        "menu-invoke",
        window,
        serde_json::json!({
            "path": path,
            "action": "press",
            "before": { "nodes": before.returned },
        }),
    )?;
    let receipt = match mechanism::invoke_menu_path(Some(window), path) {
        Ok(receipt) => receipt,
        Err(error) => {
            let error = map_mechanism_err(error);
            receipts.complete(
                &ticket,
                "menu-invoke",
                window,
                false,
                serde_json::json!({
                    "performed": false,
                    "verification": { "method": "none", "reason": "mechanism_failed" },
                    "error": error_payload(&error),
                }),
            )?;
            return Err(error.with_detail(serde_json::json!({ "receipt": ticket.json() })));
        }
    };
    let after = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    let tree_changed = observe::tree_changed(&before, &after);
    let mark_changed = receipt.mark_before != receipt.mark_after;
    let (method, reason) = if mark_changed {
        ("mark-readback", None)
    } else if tree_changed {
        ("tree-diff", None)
    } else {
        ("tree-diff", Some("no_observable_change"))
    };
    let verification = serde_json::json!({ "method": method, "reason": reason });
    receipts.complete(
        &ticket,
        "menu-invoke",
        window,
        reason.is_none(),
        serde_json::json!({
            "performed": true,
            "after": { "nodes": after.returned },
            "verification": verification,
            "mark_before": receipt.mark_before,
            "mark_after": receipt.mark_after,
            "tree_changed": tree_changed,
        }),
    )?;
    Ok(serde_json::json!({
        "addressing": "menu-path",
        "mechanism": "libagenterm",
        "backend": before.backend,
        "window": window,
        "path": path,
        "action": "press",
        "performed": true,
        "verified": reason.is_none(),
        "verification": verification,
        "mark_before": receipt.mark_before,
        "mark_after": receipt.mark_after,
        "tree_changed": tree_changed,
        "nodes_before": before.returned,
        "nodes_after": after.returned,
        "receipt": ticket.json(),
    }))
}

// ---------------------------------------------------------------------------
// The destructive verb and the receipt read-back (slice 4 of
// plan/design-mcu-absorption.md).
// ---------------------------------------------------------------------------

/// Bounds of the prior snapshot a destructive action writes to its receipt.
const CLOSE_SNAPSHOT_DEPTH: u32 = 6;
const CLOSE_SNAPSHOT_NODES: usize = 500;
/// How long the postcondition read-back polls the window inventory.
const CLOSE_READBACK: Duration = Duration::from_millis(2_500);
const CLOSE_READBACK_POLL: Duration = Duration::from_millis(50);

/// The three-part destructive gate (PRD_02_31), checked before any
/// inventory or tree read: every missing part is named in one refusal.
fn destructive_gate(window: isize, snapshot: bool, expect: Option<&str>) -> Result<(), CuError> {
    let mut missing = Vec::new();
    if window == 0 {
        missing.push("target");
    }
    if !snapshot {
        missing.push("snapshot");
    }
    match expect {
        Some("gone") => {}
        Some(other) => {
            return Err(invalid_input(format!(
                "close --expect accepts only 'gone' (the window is read back as absent), got {other:?}"
            )));
        }
        None => missing.push("postcondition"),
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(CuError::new(
        "refused",
        "close is destructive: it needs an exact target (--window HANDLE), a prior snapshot \
         (--snapshot) and a checkable postcondition (--expect gone); nothing was performed",
    )
    .with_detail(serde_json::json!({
        "reason": "destructive_gate",
        "missing": missing,
        "required": {
            "target": "--window HANDLE [--pid N] [--title T]",
            "snapshot": "--snapshot",
            "postcondition": "--expect gone",
        },
        "effect": "not_performed",
    })))
}

/// A compact node record for the snapshot a receipt carries.
fn snapshot_node_json(node: &mechanism::A11yNode) -> serde_json::Value {
    let text = node.text.as_deref().map(|text| {
        let mut end = text.len().min(200);
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_owned()
    });
    serde_json::json!({
        "id": node.id,
        "role": node.role,
        "name": node.name,
        "identifier": node.identifier,
        "text": text,
        "states": node.states,
    })
}

fn window_identity_json(row: &WindowInfo) -> serde_json::Value {
    serde_json::json!({
        "handle": row.handle,
        "ref": observe::window_stable_ref(row),
        "pid": row.process_id,
        "app": row.app_name,
        "title": row.title,
        "bounds": row.bounds,
        "focused": row.focused,
    })
}

/// Close one top-level window through the platform's close control, in the
/// background. Order: gate → exact target bound in one inventory read →
/// prior snapshot → receipt reserved → close → postcondition read back
/// (absent from the inventory) → receipt completed → reply.
/// `app launch --path P`: ask the host to start an application.
///
/// The reply says the request was **accepted**, not that the application
/// is up, and says it in a field rather than in prose. Every host route
/// hands the new process to a launcher service that owns it, so no pid
/// comes back and none is invented: the caller watches for the window,
/// which is also the only evidence the application really started rather
/// than merely being asked to.
fn launch_payload(path: Option<&str>) -> Result<serde_json::Value, CuError> {
    let Some(path) = path else {
        return Err(invalid_input(
            "app launch requires --path <application> (as `apps --all` lists it)".into(),
        ));
    };
    mechanism::launch_app(path).map_err(map_mechanism_err)?;
    Ok(serde_json::json!({
        "addressing": "application-path",
        "mechanism": "libagenterm",
        "action": "launch",
        "path": path,
        "requested": true,
        // Deliberately not `performed` / `verified`: the launcher owns the
        // process, so this call cannot know either one. Watch for the window.
        "pid": serde_json::Value::Null,
        "pid_source": "none: the launcher service owns the process; watch for its window",
    }))
}

/// `app hide|show|quit` on the application owning `window`.
///
/// `hide` / `show` are the application stepping aside and back: nothing is
/// closed, and asking for the state it is already in performs nothing.
/// `quit` ends an application, so it carries the same three-part gate as
/// `close` -- an exact target, a prior snapshot, and a checkable
/// postcondition -- and its mechanism is the application's **own Quit menu
/// item**, pressed in the background. A signal would be a kill, not a
/// quit: the application would lose its chance to run its shutdown path
/// and ask about unsaved work.
fn app_payload(
    window: isize,
    action: crate::command::AppAction,
    snapshot: bool,
    expect: Option<&str>,
    pid: Option<u32>,
    path: Option<&str>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    use crate::command::AppAction;
    if matches!(action, AppAction::Launch) {
        return launch_payload(path);
    }
    if window == 0 && pid.is_none() {
        return Err(invalid_input(
            "app requires --window <handle> (a non-zero handle from `windows`) or --pid <n>".into(),
        ));
    }
    if action.is_destructive() && window == 0 {
        return Err(invalid_input(
            "app quit requires --window <handle>: the gate needs an exact target and a prior snapshot of it".into(),
        ));
    }
    if !action.is_destructive() {
        if snapshot || expect.is_some() {
            return Err(invalid_input(format!(
                "app {} takes no --snapshot / --expect; those belong to the destructive quit",
                action.as_str()
            )));
        }
        let hidden = matches!(action, AppAction::Hide);
        // Hiding takes the application's windows out of the inventory, so
        // `show` cannot be addressed by a handle that no longer resolves:
        // it needs the pid, which outlives the hide. `hide` accepts either
        // and looks the pid up while the window is still there.
        let process_id = match (pid, hidden) {
            (Some(pid), _) => pid,
            (None, true) => {
                let windows = mechanism::window_enumerate::enumerate_top_level()
                    .map_err(map_mechanism_err)?;
                let Some(row) = windows.iter().find(|row| row.handle == window) else {
                    return Err(CuError::new(
                        "window_not_found",
                        format!("no top-level window with handle {window}"),
                    ));
                };
                row.process_id
            }
            (None, false) => {
                return Err(invalid_input(
                    "app show needs --pid: hiding removed the application's windows, so a window handle no longer names it".into(),
                ));
            }
        };
        mechanism::set_application_hidden(process_id, hidden).map_err(map_mechanism_err)?;
        // Read the inventory back: a hidden application's windows stop
        // being enumerable, which is the observable half of the verb.
        //
        // Polled, not sampled once. The adapter already waited for
        // `AXHidden` to read back, but the window server drops the windows
        // from its own list a beat later -- a single read right after the
        // write catches the old inventory and reports a working hide as
        // unverified.
        let started = Instant::now();
        let mut listed = usize::MAX;
        while started.elapsed() < CLOSE_READBACK {
            let windows =
                mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
            listed = windows
                .iter()
                .filter(|row| row.process_id == process_id)
                .count();
            if (listed == 0) == hidden {
                break;
            }
            thread::sleep(CLOSE_READBACK_POLL);
        }
        return Ok(serde_json::json!({
            "addressing": "process-id",
            "mechanism": "libagenterm",
            "process_id": process_id,
            "action": action.as_str(),
            "performed": true,
            "windows_listed": listed,
            "verified": (listed == 0) == hidden,
            "verification": {
                "method": "window-inventory-by-pid",
                "elapsed_ms": started.elapsed().as_millis(),
            },
        }));
    }
    destructive_gate(window, snapshot, expect)?;
    let not_performed = |error: CuError| {
        let mut detail = error.detail.clone().unwrap_or(serde_json::json!({}));
        detail["effect"] = serde_json::json!("not_performed");
        error.with_detail(detail)
    };
    let windows = mechanism::window_enumerate::enumerate_top_level()
        .map_err(map_mechanism_err)
        .map_err(not_performed)?;
    let Some(row) = windows.iter().find(|item| item.handle == window) else {
        return Err(not_performed(CuError::new(
            "window_not_found",
            format!("no top-level window with handle {window}"),
        )));
    };
    if let Some(pid) = pid
        && row.process_id != pid
    {
        return Err(not_performed(
            CuError::new(
                "window_identity_mismatch",
                format!(
                    "window {window} belongs to pid {} not {pid}; refusing to quit another process",
                    row.process_id
                ),
            )
            .with_detail(
                serde_json::json!({ "expected": { "pid": pid }, "observed": window_identity_json(row) }),
            ),
        ));
    }
    let identity = window_identity_json(row);
    let application = row.app_name.clone();
    let target_pid = row.process_id;
    let tree = mechanism::tree_for_window_bounded(
        Some(window),
        mechanism::TreeBudget {
            max_depth: Some(CLOSE_SNAPSHOT_DEPTH),
            max_nodes: Some(CLOSE_SNAPSHOT_NODES),
        },
    )
    .map_err(map_mechanism_err)
    .map_err(not_performed)?;
    let snapshot_json = serde_json::json!({
        "backend": tree.backend,
        "budget": { "depth": CLOSE_SNAPSHOT_DEPTH, "max_nodes": CLOSE_SNAPSHOT_NODES },
        "visited": tree.visited,
        "returned": tree.returned,
        "truncated": tree.truncated,
        "nodes": tree.nodes.iter().map(snapshot_node_json).collect::<Vec<_>>(),
    });
    // The application's own Quit item, by the two spellings a menu bar
    // uses. Resolved through the same background menu path `menu invoke`
    // uses, so a missing / duplicated / disabled item refuses there with
    // nothing pressed.
    let candidates = [
        vec![application.clone(), format!("Quit {application}")],
        vec![application.clone(), "Quit".to_owned()],
    ];
    let ticket = receipts.reserve(
        "app-quit",
        window,
        serde_json::json!({
            "action": "quit",
            "window_identity": identity,
            "postcondition": "gone",
            "before": { "present": true, "nodes": tree.returned },
            "snapshot": snapshot_json,
        }),
    )?;
    let started = Instant::now();
    let mut mechanism_error = None;
    let mut pressed_path: Option<Vec<String>> = None;
    for path in &candidates {
        match mechanism::invoke_menu_path(Some(window), path) {
            Ok(_) => {
                pressed_path = Some(path.clone());
                mechanism_error = None;
                break;
            }
            Err(error) => mechanism_error = Some(map_mechanism_err(error)),
        }
    }
    // Postcondition: no window of that process is left in the inventory.
    let mut polls = 0usize;
    let mut present = true;
    let mut readback_error = None;
    while started.elapsed() < CLOSE_READBACK {
        polls += 1;
        match mechanism::window_enumerate::enumerate_top_level() {
            Ok(now) => present = now.iter().any(|item| item.process_id == target_pid),
            Err(error) => {
                readback_error = Some(map_mechanism_err(error));
                break;
            }
        }
        if !present || mechanism_error.is_some() {
            break;
        }
        thread::sleep(CLOSE_READBACK_POLL);
    }
    let verified = !present && mechanism_error.is_none() && readback_error.is_none();
    let reason = if mechanism_error.is_some() {
        Some("mechanism_failed")
    } else if readback_error.is_some() {
        Some("readback_failed")
    } else if present {
        Some("application_still_present")
    } else {
        None
    };
    let verification = serde_json::json!({
        "method": "window-inventory-by-pid",
        "reason": reason,
        "polls": polls,
        "elapsed_ms": started.elapsed().as_millis(),
        "menu_path": pressed_path,
    });
    receipts.complete(
        &ticket,
        "app-quit",
        window,
        verified,
        serde_json::json!({
            "performed": mechanism_error.is_none(),
            "after": { "present": present },
            "verification": verification,
            "error": mechanism_error.as_ref().or(readback_error.as_ref()).map(error_payload),
        }),
    )?;
    if let Some(error) = mechanism_error.or(readback_error) {
        return Err(error);
    }
    Ok(serde_json::json!({
        "addressing": "window-handle",
        "mechanism": "libagenterm",
        "backend": tree.backend,
        "window": window,
        "target": identity,
        "action": "quit",
        "postcondition": "gone",
        "performed": true,
        "verified": verified,
        "verification": verification,
        "snapshot": snapshot_json,
    }))
}

fn close_payload(
    window: isize,
    pid: Option<u32>,
    title: Option<&str>,
    snapshot: bool,
    expect: Option<&str>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    destructive_gate(window, snapshot, expect)?;
    let not_performed = |error: CuError| {
        let mut detail = error.detail.clone().unwrap_or(serde_json::json!({}));
        detail["effect"] = serde_json::json!("not_performed");
        error.with_detail(detail)
    };
    let windows = mechanism::window_enumerate::enumerate_top_level()
        .map_err(map_mechanism_err)
        .map_err(not_performed)?;
    let Some(row) = windows.iter().find(|item| item.handle == window) else {
        return Err(not_performed(CuError::new(
            "window_not_found",
            format!("no top-level window with handle {window}"),
        )));
    };
    if let Some(pid) = pid
        && row.process_id != pid
    {
        return Err(not_performed(
            CuError::new(
                "window_identity_mismatch",
                format!(
                    "window {window} belongs to pid {} not {pid}; refusing to close another process's window",
                    row.process_id
                ),
            )
            .with_detail(serde_json::json!({ "expected": { "pid": pid }, "observed": window_identity_json(row) })),
        ));
    }
    if let Some(title) = title
        && row.title != title
    {
        return Err(not_performed(
            CuError::new(
                "window_identity_mismatch",
                format!(
                    "window {window} is titled {:?} not {title:?}; refusing to close it",
                    row.title
                ),
            )
            .with_detail(serde_json::json!({ "expected": { "title": title }, "observed": window_identity_json(row) })),
        ));
    }
    let identity = window_identity_json(row);
    let tree = mechanism::tree_for_window_bounded(
        Some(window),
        mechanism::TreeBudget {
            max_depth: Some(CLOSE_SNAPSHOT_DEPTH),
            max_nodes: Some(CLOSE_SNAPSHOT_NODES),
        },
    )
    .map_err(map_mechanism_err)
    .map_err(not_performed)?;
    let snapshot_json = serde_json::json!({
        "backend": tree.backend,
        "budget": { "depth": CLOSE_SNAPSHOT_DEPTH, "max_nodes": CLOSE_SNAPSHOT_NODES },
        "visited": tree.visited,
        "returned": tree.returned,
        "truncated": tree.truncated,
        "nodes": tree.nodes.iter().map(snapshot_node_json).collect::<Vec<_>>(),
    });
    let ticket = receipts.reserve(
        "close",
        window,
        serde_json::json!({
            "action": "close",
            "window_identity": identity,
            "postcondition": "gone",
            "before": { "present": true, "nodes": tree.returned },
            "snapshot": snapshot_json,
        }),
    )?;
    let started = Instant::now();
    let mechanism_error = mechanism::window_op::close(window)
        .err()
        .map(map_mechanism_err);
    // Postcondition: the handle (bound to its pid) leaves the inventory.
    let mut polls = 0usize;
    let mut present = true;
    let mut readback_error = None;
    while started.elapsed() < CLOSE_READBACK {
        polls += 1;
        match mechanism::window_enumerate::enumerate_top_level() {
            Ok(now) => {
                present = now
                    .iter()
                    .any(|item| item.handle == window && item.process_id == row.process_id);
            }
            Err(error) => {
                readback_error = Some(map_mechanism_err(error));
                break;
            }
        }
        if !present || mechanism_error.is_some() {
            break;
        }
        thread::sleep(CLOSE_READBACK_POLL);
    }
    let verified = !present && mechanism_error.is_none() && readback_error.is_none();
    let reason = if mechanism_error.is_some() {
        Some("mechanism_failed")
    } else if readback_error.is_some() {
        Some("readback_failed")
    } else if present {
        Some("window_still_present")
    } else {
        None
    };
    let verification = serde_json::json!({
        "method": "window-inventory",
        "reason": reason,
        "polls": polls,
        "elapsed_ms": started.elapsed().as_millis(),
    });
    let after = serde_json::json!({ "present": present });
    receipts.complete(
        &ticket,
        "close",
        window,
        verified,
        serde_json::json!({
            "performed": mechanism_error.is_none(),
            "after": after,
            "verification": verification,
            "error": mechanism_error.as_ref().or(readback_error.as_ref()).map(error_payload),
        }),
    )?;
    let payload = serde_json::json!({
        "addressing": "window-handle",
        "mechanism": "libagenterm",
        "backend": tree.backend,
        "window": window,
        "target": identity,
        "action": "close",
        "postcondition": "gone",
        "performed": mechanism_error.is_none(),
        "verified": verified,
        "verification": verification,
        "before": { "present": true, "nodes": tree.returned },
        "after": after,
        "snapshot": {
            "visited": tree.visited,
            "returned": tree.returned,
            "truncated": tree.truncated,
            "in_receipt": true,
        },
        "receipt": ticket.json(),
    });
    if let Some(error) = mechanism_error.or(readback_error) {
        return Err(error.with_detail(serde_json::json!({ "receipt": payload })));
    }
    if present {
        return Err(CuError::new(
            "unverified",
            format!(
                "close was delivered to window {window} but it is still in the inventory after {} polls",
                polls
            ),
        )
        .with_detail(serde_json::json!({ "reason": "window_still_present", "receipt": payload })));
    }
    Ok(payload)
}

/// `receipts --window H --max N`: the target's receipt file read back in
/// order. Observation only — the file is not created here.
fn receipts_payload(
    dir: &std::path::Path,
    target: TargetRef,
    window: Option<isize>,
    max: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    let max = receipt::validate_list_max(max).map_err(invalid_input)?;
    let path = dir.join(format!("{}.jsonl", target.as_str()));
    let (lines, total) = receipt::list_file(&path, window, max)?;
    Ok(serde_json::json!({
        "addressing": "receipt-file",
        "path": path,
        "target": target.as_str(),
        "window": window,
        "max": max,
        "total": total,
        "returned": lines.len(),
        "truncated": total > lines.len(),
        "receipts": lines,
    }))
}

/// The application's own focused control inside `window`, role-bound when
/// the caller names one (a mismatch is typed `unverified`, never a guess).
fn focused_control(
    window: isize,
    role: Option<&str>,
) -> Result<(String, mechanism::A11yNode), CuError> {
    let tree = mechanism::focused_node(Some(window)).map_err(map_mechanism_err)?;
    let backend = tree.backend;
    let Some(node) = tree.nodes.into_iter().next() else {
        return Err(CuError::new(
            "a11y_focus_unavailable",
            "the platform returned no focused control",
        ));
    };
    if let Some(wanted) = role
        && observe::normalize_role(&node.role) != observe::normalize_role(wanted)
    {
        return Err(CuError::new(
            "unverified",
            format!(
                "the focused control is {} {:?} (identifier {}), not role {wanted:?}",
                node.role,
                node.name,
                node.identifier.as_deref().unwrap_or("none")
            ),
        )
        .with_detail(serde_json::json!({ "observed": observe::node_state_json(&node) })));
    }
    Ok((backend, node))
}

/// `focused --window H [--role R] [--max-value-bytes N]`.
fn focused_payload(
    window: isize,
    role: Option<&str>,
    max_value_bytes: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "focused requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    observe::validate_max_value_bytes(max_value_bytes).map_err(invalid_input)?;
    let max_value_bytes = max_value_bytes.unwrap_or(observe::DEFAULT_MAX_VALUE_BYTES);
    let (backend, node) = focused_control(window, role)?;
    let full = node.text.clone().unwrap_or_default();
    let (preview, cut) = observe::preview_value(&full, max_value_bytes);
    let adapter_truncated = node.states.iter().any(|state| state == "text-truncated");
    let mut state = observe::node_state_json(&node);
    state["bounds"] = serde_json::to_value(&node.bounds).unwrap_or(serde_json::Value::Null);
    state["actions"] = serde_json::json!(node.actions);
    state["text"] = serde_json::Value::Null;
    Ok(serde_json::json!({
        "addressing": "focused-control",
        "mechanism": "libagenterm",
        "backend": backend,
        "window": window,
        "role_bound": role,
        "node": state,
        "value": preview,
        "value_bytes": full.len(),
        "value_truncated": cut || adapter_truncated,
        "max_value_bytes": max_value_bytes,
    }))
}

/// The reply for a run that used the backend's own notifications.
///
/// It reports `mode: "notifications"` and no `polls` count, because there
/// were none: a caller comparing two runs must be able to tell which
/// mechanism produced the events. `filtered` still applies -- a caller can
/// ask for a subset of the vocabulary either way.
fn native_observe_payload(
    window: isize,
    duration_ms: u64,
    max_events: usize,
    wanted: &[String],
    events: Vec<mechanism::A11yEvent>,
) -> serde_json::Value {
    let total = events.len();
    let mut emitted = Vec::new();
    let mut filtered = 0usize;
    for event in events {
        if !wanted.contains(&event.notification) {
            filtered += 1;
            continue;
        }
        let seq = emitted.len() as u64;
        emitted.push(serde_json::json!({
            "seq": seq,
            "t_ms": event.t_ms,
            "notification": event.notification,
            "node": {
                "id": event.node_id,
                "role": event.role,
                "name": event.name,
            },
        }));
    }
    serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": "ax",
        "mode": "notifications",
        "window": window,
        "duration_ms": duration_ms,
        "notifications": wanted,
        "max_events": max_events,
        "received": total,
        "emitted": emitted.len(),
        "filtered": filtered,
        "truncated": total >= max_events,
        "stopped": if total >= max_events { "max-events" } else { "deadline" },
        "events": emitted,
    })
}

/// `observe`: poll the bounded tree and emit the semantic differences
/// between consecutive walks as a monotonic, filtered, bounded stream. AX
/// notifications are not subscribed (the platform crate wires no
/// AXObserver); the reply says `mode: "poll-diff"`.
#[allow(clippy::too_many_arguments)]
fn observe_payload(
    window: isize,
    duration_ms: u64,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    max_events: Option<usize>,
    notifications: &[String],
    interval_ms: Option<u64>,
    mode: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "observe requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    observe::validate_observe(duration_ms, max_events, interval_ms).map_err(invalid_input)?;
    let budget = tree_budget(depth, max_nodes)?;
    let max_events = max_events.unwrap_or(observe::DEFAULT_OBSERVE_EVENTS);
    let interval =
        Duration::from_millis(interval_ms.unwrap_or(observe::DEFAULT_OBSERVE_INTERVAL_MS));
    let wanted: Vec<String> = if notifications.is_empty() {
        observe::OBSERVE_NOTIFICATIONS
            .iter()
            .map(|name| (*name).to_owned())
            .collect()
    } else {
        let mut merged = Vec::new();
        for raw in notifications {
            for name in observe::parse_notifications(raw).map_err(invalid_input)? {
                if !merged.contains(&name) {
                    merged.push(name);
                }
            }
        }
        merged
    };
    // The two modes see different things and neither subsumes the other, so
    // the caller picks and the reply says which ran. Polling compares two
    // tree walks: every event carries `before` and `after`, but a change
    // that reverts between walks is invisible and an idle interface still
    // costs a walk per interval. The backend's own notifications carry the
    // order and arrival time of every change -- including ones that revert
    // -- and cost nothing while nothing happens, but a notification says
    // "this changed", not what it changed from. Defaulting to notifications
    // would silently drop `before`/`after` from every reply, so poll-diff
    // stays the default and `--mode notifications` is the explicit ask.
    if mode == Some("notifications") {
        return match mechanism::observe_window(window, duration_ms, max_events) {
            Ok(events) => Ok(native_observe_payload(
                window,
                duration_ms,
                max_events,
                &wanted,
                events,
            )),
            Err(error) => Err(map_mechanism_err(error)),
        };
    }
    let started = Instant::now();
    let deadline = started + Duration::from_millis(duration_ms);
    let mut previous =
        mechanism::tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?;
    let backend = previous.backend.clone();
    let mut events: Vec<serde_json::Value> = Vec::new();
    let mut seq = 0u64;
    let mut filtered = 0usize;
    let mut polls = 1usize;
    let mut poll_errors = 0usize;
    let mut last_poll_error: Option<serde_json::Value> = None;
    let mut stopped = "deadline";
    let mut truncated = false;
    loop {
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(interval.min(deadline.saturating_duration_since(Instant::now())));
        polls += 1;
        let current = match mechanism::tree_for_window_bounded(Some(window), budget) {
            Ok(tree) => tree,
            Err(mechanism::MechanismError::Unsupported { reason }) => {
                return Err(map_mechanism_err(mechanism::MechanismError::Unsupported {
                    reason,
                }));
            }
            Err(error) => {
                let error = map_mechanism_err(error);
                if error.code == "denied" {
                    return Err(error);
                }
                poll_errors += 1;
                last_poll_error = Some(error_payload(&error));
                continue;
            }
        };
        let t_ms = started.elapsed().as_millis() as u64;
        for event in observe::diff_events(&previous, &current) {
            if !wanted.iter().any(|name| name == event.notification) {
                filtered += 1;
                continue;
            }
            if events.len() >= max_events {
                truncated = true;
                stopped = "max-events";
                break;
            }
            let mut value = serde_json::to_value(&event)
                .map_err(|error| CuError::new("serialize", error.to_string()))?;
            value["seq"] = serde_json::json!(seq);
            value["t_ms"] = serde_json::json!(t_ms);
            seq += 1;
            events.push(value);
        }
        previous = current;
        if truncated {
            break;
        }
    }
    Ok(serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": backend,
        "mode": "poll-diff",
        "window": window,
        "duration_ms": duration_ms,
        "elapsed_ms": started.elapsed().as_millis() as u64,
        "interval_ms": interval.as_millis() as u64,
        "budget": budget_json(depth, max_nodes),
        "notifications": wanted,
        "max_events": max_events,
        "polls": polls,
        "poll_errors": poll_errors,
        "last_poll_error": last_poll_error,
        "emitted": events.len(),
        "filtered": filtered,
        "truncated": truncated,
        "stopped": stopped,
        "events": events,
    }))
}

/// One expectation checked against one flattened tree.
struct Verdict {
    item: serde_json::Value,
    met: bool,
    unknown: bool,
}

fn check_one(
    flat: &[observe::FlatNode<'_>],
    expectation: &crate::command::Expectation,
) -> Result<Verdict, CuError> {
    if !expectation.has_state() && !expectation.has_page_identity() {
        return Err(invalid_input(
            "every --expect item needs a state (value, checked, expanded, focused) or a title substring (name / titleIncludes)".into(),
        ));
    }
    let spec = observe::TargetSpec::from_expectation(expectation);
    let node = match observe::resolve_target(flat, &spec) {
        Ok(hit) => hit.node,
        Err(observe::TargetError::Missing(message)) => {
            return Ok(Verdict {
                item: serde_json::json!({
                    "target": spec.json(),
                    "node": null,
                    "met": false,
                    "reason": message,
                }),
                met: false,
                unknown: false,
            });
        }
        Err(error) => return Err(target_error(error)),
    };
    if !expectation.has_state() {
        return Ok(Verdict {
            item: serde_json::json!({
                "target": spec.json(),
                "node": observe::node_state_json(node),
                "checks": [],
                "met": true,
                "unknown": false,
                "page_identity": true,
            }),
            met: true,
            unknown: false,
        });
    }
    let checks = observe::check_expectation(node, expectation);
    let unknown = checks.iter().any(|check| check.met.is_none());
    let met = !unknown && checks.iter().all(|check| check.met == Some(true));
    Ok(Verdict {
        item: serde_json::json!({
            "target": spec.json(),
            "node": observe::node_state_json(node),
            "checks": checks,
            "met": met,
            "unknown": unknown,
        }),
        met,
        unknown,
    })
}

fn verify_payload(
    window: isize,
    expect: &[crate::command::Expectation],
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "verify requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    if expect.is_empty() {
        return Err(invalid_input(
            "verify requires a non-empty --expect array".into(),
        ));
    }
    let tree = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    let flat = observe::flatten(&tree);
    let mut results = Vec::with_capacity(expect.len());
    let mut unknown = false;
    let mut unmet = false;
    for expectation in expect {
        let verdict = check_one(&flat, expectation)?;
        unknown |= verdict.unknown;
        unmet |= !verdict.met;
        results.push(verdict.item);
    }
    let observation = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": tree.backend,
        "window": window,
        "visited": tree.visited,
        "truncated": tree.truncated,
        "results": results,
    });
    if unknown {
        return Err(CuError::new(
            "unsupported",
            "an expected state is not observable on its node; refusing to call it met",
        )
        .with_detail(
            serde_json::json!({ "reason": "state_unobservable", "observation": observation }),
        ));
    }
    if unmet {
        return Err(CuError::new(
            "unverified",
            "at least one expectation is not met by the current tree",
        )
        .with_detail(serde_json::json!({ "observation": observation })));
    }
    let mut payload = observation;
    payload["verified"] = serde_json::Value::Bool(true);
    Ok(payload)
}

/// Poll the same matcher until every expectation is met. A missing node
/// keeps polling; ambiguity and an unobservable state fail closed at once.
fn wait_expect(
    timeout_ms: u64,
    window: isize,
    expect: &[crate::command::Expectation],
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "wait --expect requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    if expect.is_empty() {
        return Err(invalid_input(
            "wait requires a non-empty --expect array".into(),
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(120_000));
    let poll = Duration::from_millis(50);
    let mut polls = 0usize;
    let mut last;
    loop {
        polls += 1;
        match mechanism::tree_for_window(Some(window)) {
            Ok(tree) => {
                let flat = observe::flatten(&tree);
                let mut results = Vec::with_capacity(expect.len());
                let mut all_met = true;
                for expectation in expect {
                    let verdict = check_one(&flat, expectation)?;
                    if verdict.unknown {
                        return Err(CuError::new(
                            "unsupported",
                            "an expected state is not observable on its node; more polling cannot make it so",
                        )
                        .with_detail(serde_json::json!({ "reason": "state_unobservable", "item": verdict.item })));
                    }
                    all_met &= verdict.met;
                    results.push(verdict.item);
                }
                last = serde_json::json!({
                    "backend": tree.backend,
                    "visited": tree.visited,
                    "truncated": tree.truncated,
                    "results": results,
                });
                if all_met {
                    return Ok(serde_json::json!({
                        "met": true,
                        "verified": true,
                        "addressing": "accessibility-tree",
                        "mechanism": "libagenterm",
                        "window": window,
                        "polls": polls,
                        "observation": last,
                    }));
                }
            }
            Err(mechanism::MechanismError::Unsupported { reason }) => {
                return Err(map_mechanism_err(mechanism::MechanismError::Unsupported {
                    reason,
                }));
            }
            Err(error) => {
                let error = map_mechanism_err(error);
                if error.code == "denied" {
                    return Err(error);
                }
                last = serde_json::json!({ "tree_error": error_payload(&error) });
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(poll);
    }
    Err(CuError::new(
        "timeout",
        format!("expectations not met after {timeout_ms}ms ({polls} polls)"),
    )
    .with_detail(serde_json::json!({ "observation": last })))
}

fn map_mechanism_err(error: mechanism::MechanismError) -> CuError {
    match error {
        mechanism::MechanismError::Unsupported { reason } => CuError::new("unsupported", reason),
        // An OS permission refusal is the PRD 31 `denied` vocabulary, with
        // the mechanism code and repair path kept in `detail` so a caller
        // never has to parse prose to know what to fix.
        mechanism::MechanismError::Failed { code, message } if code == "a11y_permission_denied" => {
            CuError::new("denied", message).with_detail(serde_json::json!({
                "reason": code,
                "permission": "accessibility",
                "repair": ACCESSIBILITY_REPAIR_PATH,
            }))
        }
        mechanism::MechanismError::Failed { code, message } => CuError::new(code, message),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeSet, VecDeque},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;
    use crate::command::Command;
    use crate::target::TargetRef;

    static NEXT_AUDIT_SCRATCH: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn persisted_one_shot_is_audited_revalidated_and_exhausted() {
        let audit_path = audit_scratch("persisted-one-shot");
        let root = audit_path.parent().unwrap();
        let store_path = root.join("cu-grants.json");
        let binding = crate::target_binding::TargetBinding {
            tier: TargetRef::Current,
            target_id: format!("agt-cu-tgt-v1-{}", "1".repeat(64)),
            session_binding: format!("agt-cu-ses-v1-{}", "2".repeat(64)),
        };
        let grant_id = format!("cu1_{}", "3".repeat(64));
        let now = now_utc_ms().unwrap();
        let mut store = AuthStore::open_private_at(&store_path).unwrap();
        store
            .create(crate::auth_store::GrantSpec::new(
                &grant_id,
                &binding,
                BTreeSet::from([Grant::Observe]),
                now,
                now,
                now + 60_000,
                1,
            ))
            .unwrap();
        drop(store);

        let command = Command::Capabilities {
            target: TargetRef::Current,
        };
        let executor = Executor::new(Authorization::new(BTreeSet::new()))
            .with_persisted_grant(&grant_id, &store_path)
            .with_persisted_binding(binding)
            .with_audit_path(audit_path.clone());
        assert!(executor.execute(&command).ok);
        let refused = executor.execute(&command);
        assert!(!refused.ok);
        assert_eq!(refused.error.as_ref().unwrap().code, "refused");
        assert!(
            refused
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("exhausted")
        );

        let raw = std::fs::read_to_string(&audit_path).unwrap();
        assert!(!raw.contains("agt-cu-ses"));
        let records = raw
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["decision"], "authorized");
        assert_eq!(records[0]["outcome"], "attempt");
        assert_eq!(records[1]["outcome"], "ok");
        assert_eq!(records[2]["decision"], "denied");
        assert_eq!(records[2]["outcome"], "exhausted");
        assert_eq!(records[0]["decision_id"], records[1]["decision_id"]);
        assert_ne!(records[1]["decision_id"], records[2]["decision_id"]);
        assert_eq!(
            AuthStore::open_private_at(&store_path).unwrap().list()[0].consumed_uses,
            1
        );
        remove_audit_scratch(&audit_path);
    }

    #[test]
    fn persisted_audit_open_failure_does_not_reserve_the_grant() {
        let audit_path = audit_scratch("persisted-audit-open");
        let root = audit_path.parent().unwrap();
        let store_path = root.join("cu-grants.json");
        let binding = crate::target_binding::TargetBinding {
            tier: TargetRef::Current,
            target_id: format!("agt-cu-tgt-v1-{}", "4".repeat(64)),
            session_binding: format!("agt-cu-ses-v1-{}", "5".repeat(64)),
        };
        let grant_id = format!("cu1_{}", "6".repeat(64));
        let now = now_utc_ms().unwrap();
        let mut store = AuthStore::open_private_at(&store_path).unwrap();
        store
            .create(crate::auth_store::GrantSpec::new(
                &grant_id,
                &binding,
                BTreeSet::from([Grant::Observe]),
                now,
                now,
                now + 60_000,
                1,
            ))
            .unwrap();
        drop(store);
        std::fs::create_dir_all(&audit_path).unwrap();

        let command = Command::Capabilities {
            target: TargetRef::Current,
        };
        let reply = Executor::new(Authorization::new(BTreeSet::new()))
            .with_persisted_grant(&grant_id, &store_path)
            .with_persisted_binding(binding)
            .with_audit_path(audit_path.clone())
            .execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "audit_unavailable");
        assert_eq!(
            AuthStore::open_private_at(&store_path).unwrap().list()[0].consumed_uses,
            0
        );
        remove_audit_scratch(&audit_path);
    }

    #[test]
    fn pointer_move_calls_only_move_once_and_returns_bounded_typed_reply() {
        let mut calls = Vec::new();
        let reply = pointer_move_with(-320, 1440, |x, y| {
            calls.push((x, y));
            Ok(())
        })
        .expect("pointer move");
        assert_eq!(calls, [(-320, 1440)]);
        assert_eq!(reply["effect"], "committed");
        assert_eq!(reply["coords"], serde_json::json!([-320, 1440]));
        assert_eq!(reply["button_effect"], "none");
        assert_eq!(reply.as_object().expect("object").len(), 5);
    }

    #[test]
    fn pointer_position_observes_once_without_injection() {
        let mut calls = 0;
        let reply = pointer_position_with(|| {
            calls += 1;
            Ok((-17, 2048))
        })
        .expect("pointer position");
        assert_eq!(calls, 1);
        assert_eq!(reply["effect"], "observed");
        assert_eq!(reply["coords"], serde_json::json!([-17, 2048]));
        assert_eq!(reply.as_object().expect("object").len(), 4);
    }

    #[test]
    fn pointer_move_requires_actuate_and_refusal_moves_nothing() {
        let command = Command::PointerMove {
            target: TargetRef::Current,
            x: 10,
            y: 20,
        };
        let reply = Executor::new(Authorization::new(Default::default())).execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.expect("typed refusal").code, "refused");
    }

    fn audit_scratch(label: &str) -> PathBuf {
        let sequence = NEXT_AUDIT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let scratch = std::env::temp_dir().join(format!(
            "agenterm-cu-executor-audit-{label}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&scratch).expect("create audit scratch root");
        // Resolve the macOS `/var` temp-root symlink before exercising the
        // production store's fail-closed ancestry check.
        std::fs::canonicalize(scratch)
            .expect("canonicalize audit scratch root")
            .join("audit.jsonl")
    }

    fn remove_audit_scratch(path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    enum FakeApply {
        Ok {
            actual: crate::place::Rect,
            quantized: bool,
            clamped: bool,
        },
        Err {
            error: CuError,
            observed: crate::place::Rect,
        },
    }

    struct FakePlaceRuntime {
        rect: crate::place::Rect,
        first_read_error: Option<CuError>,
        inspections: VecDeque<Result<mechanism::window_placement::PlacementWindowInfo, CuError>>,
        inspect_args: Vec<(isize, u32)>,
        identities: VecDeque<Result<bool, CuError>>,
        applies: VecDeque<FakeApply>,
        apply_handles: Vec<isize>,
    }

    impl FakePlaceRuntime {
        fn new(rect: crate::place::Rect, applies: impl IntoIterator<Item = FakeApply>) -> Self {
            Self {
                rect,
                first_read_error: None,
                inspections: VecDeque::from([Ok(placement_fixture())]),
                inspect_args: Vec::new(),
                identities: VecDeque::from([Ok(true), Ok(true)]),
                applies: applies.into_iter().collect(),
                apply_handles: Vec::new(),
            }
        }
    }

    impl PlaceRuntime for FakePlaceRuntime {
        fn read_rect(&mut self, _handle: isize) -> Result<crate::place::Rect, CuError> {
            if let Some(error) = self.first_read_error.take() {
                return Err(error);
            }
            Ok(self.rect)
        }

        fn inspect_placement(
            &mut self,
            handle: isize,
            expected_pid: u32,
        ) -> Result<mechanism::window_placement::PlacementWindowInfo, CuError> {
            self.inspect_args.push((handle, expected_pid));
            self.inspections
                .pop_front()
                .unwrap_or_else(|| Ok(placement_fixture()))
        }

        fn apply_rect(
            &mut self,
            handle: isize,
            _target: crate::place::Rect,
            _visible: crate::place::Rect,
        ) -> Result<(crate::place::Rect, bool, bool), CuError> {
            self.apply_handles.push(handle);
            match self.applies.pop_front().expect("scripted apply outcome") {
                FakeApply::Ok {
                    actual,
                    quantized,
                    clamped,
                } => {
                    self.rect = actual;
                    Ok((actual, quantized, clamped))
                }
                FakeApply::Err { error, observed } => {
                    self.rect = observed;
                    Err(error)
                }
            }
        }

        fn identity_matches(&mut self, _identity: &PlaceIdentity) -> Result<bool, CuError> {
            self.identities.pop_front().unwrap_or(Ok(true))
        }
    }

    struct SavingHistory;

    impl HistoryCommitter for SavingHistory {
        fn commit(
            &mut self,
            history: &crate::place::PlaceHistory,
        ) -> Result<(), HistoryCommitFailure> {
            history.save().map_err(|error| HistoryCommitFailure {
                message: error.to_string(),
                published: error.published(),
            })
        }
    }

    struct FailingHistory {
        published: bool,
    }

    impl HistoryCommitter for FailingHistory {
        fn commit(
            &mut self,
            _history: &crate::place::PlaceHistory,
        ) -> Result<(), HistoryCommitFailure> {
            Err(HistoryCommitFailure {
                message: "injected history commit failure".into(),
                published: self.published,
            })
        }
    }

    fn saga_scratch(label: &str) -> PathBuf {
        let sequence = NEXT_AUDIT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "agenterm-cu-saga-{label}-{}-{sequence}",
                std::process::id()
            ))
            .join("history.json")
    }

    fn saga_window(bounds: crate::place::Rect) -> mechanism::window_enumerate::WindowInfo {
        let (x, y, width, height) = bounds.to_i32();
        mechanism::window_enumerate::WindowInfo {
            handle: 7,
            title: "fixture".into(),
            process_id: 42,
            app_name: "fixture-app".into(),
            bounds: mechanism::window_enumerate::WindowBounds {
                x,
                y,
                width,
                height,
            },
            focused: true,
            minimized: false,
        }
    }

    fn saga_screen() -> mechanism::window_enumerate::ScreenInfo {
        let bounds = mechanism::window_enumerate::WindowBounds {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        mechanism::window_enumerate::ScreenInfo {
            frame: bounds,
            visible: bounds,
            primary: true,
        }
    }

    fn saga_rect(x: f64, y: f64, width: f64, height: f64) -> crate::place::Rect {
        crate::place::Rect::new(x, y, width, height)
    }

    fn placement_fixture() -> mechanism::window_placement::PlacementWindowInfo {
        use mechanism::window_placement::{
            PlacementRole, PlacementWindowInfo, SizeConstraints, Support,
        };
        PlacementWindowInfo {
            handle: 7,
            process_id: 42,
            role: PlacementRole::Standard,
            movable: Support::Yes,
            resizable: Support::Yes,
            constraints: SizeConstraints::Explicit {
                min: None,
                max: None,
                increment: None,
            },
        }
    }

    fn remove_saga_scratch(path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn placement_roles_and_unknown_support_fail_closed() {
        use mechanism::window_placement::{PlacementRole, Support};
        let before = saga_rect(100.0, 100.0, 800.0, 600.0);
        let moved = saga_rect(200.0, 100.0, 800.0, 600.0);
        for role in [
            PlacementRole::Sheet,
            PlacementRole::SystemDialog,
            PlacementRole::Other,
            PlacementRole::Unknown,
        ] {
            let mut info = placement_fixture();
            info.role = role;
            assert_eq!(
                placement_target(before, moved, info).unwrap_err().code,
                "window_role_refused"
            );
        }
        for role in [PlacementRole::Standard, PlacementRole::Dialog] {
            let mut info = placement_fixture();
            info.role = role;
            info.movable = Support::Unknown;
            assert_eq!(
                placement_target(before, moved, info).unwrap_err().code,
                "window_not_movable"
            );
            info.movable = Support::Yes;
            info.resizable = Support::Unknown;
            assert_eq!(
                placement_target(before, saga_rect(100.0, 100.0, 900.0, 600.0), info)
                    .unwrap_err()
                    .code,
                "window_not_resizable"
            );
        }
    }

    #[test]
    fn explicit_constraints_clamp_and_quantize_requested_size() {
        use mechanism::window_placement::{SizeConstraints, WindowSize};
        let before = saga_rect(10.0, 20.0, 400.0, 300.0);
        let mut info = placement_fixture();
        info.constraints = SizeConstraints::Explicit {
            min: Some(WindowSize {
                width: 300,
                height: 200,
            }),
            max: Some(WindowSize {
                width: 800,
                height: 700,
            }),
            increment: Some(WindowSize {
                width: 50,
                height: 20,
            }),
        };
        let result = placement_target(before, saga_rect(10.0, 20.0, 503.0, 407.0), info)
            .expect("explicit normalization");
        assert_eq!(result.mode, "explicit");
        assert!(result.adjusted);
        assert_eq!(result.target, saga_rect(10.0, 20.0, 500.0, 400.0));

        info.constraints = SizeConstraints::Unknown;
        assert_eq!(
            placement_target(before, saga_rect(10.0, 20.0, 500.0, 300.0), info)
                .unwrap_err()
                .code,
            "window_constraints_unknown"
        );
    }

    #[test]
    fn application_enforced_constraints_use_final_readback_and_expected_pid() {
        use mechanism::window_placement::SizeConstraints;
        let path = saga_scratch("application-enforced");
        let before = saga_rect(100.0, 100.0, 800.0, 600.0);
        let actual = saga_rect(0.0, 0.0, 900.0, 1000.0);
        let history = crate::place::PlaceHistory::open_at(path.clone()).expect("history");
        let mut runtime = FakePlaceRuntime::new(
            before,
            [FakeApply::Ok {
                actual,
                quantized: false,
                clamped: false,
            }],
        );
        let mut info = placement_fixture();
        info.constraints = SizeConstraints::ApplicationEnforced;
        runtime.inspections = VecDeque::from([Ok(info)]);
        let reply = window_place_resolved(
            crate::place::PlaceAction::LeftHalf,
            &saga_window(before),
            &[saga_screen()],
            history,
            &mut runtime,
            &mut SavingHistory,
        )
        .expect("application-enforced placement");
        assert_eq!(runtime.inspect_args, [(7, 42)]);
        assert_eq!(reply["constraint_mode"], "application_enforced");
        assert_eq!(reply["constraint_adjusted"], true);
        assert_eq!(reply["after"], rect_payload(actual));
        remove_saga_scratch(&path);
    }

    #[test]
    fn window_place_strict_read_failure_has_no_side_effect() {
        let path = saga_scratch("strict-read");
        let before = saga_rect(100.0, 100.0, 800.0, 600.0);
        let history = crate::place::PlaceHistory::open_at(path.clone()).expect("history");
        let mut runtime = FakePlaceRuntime::new(before, []);
        runtime.first_read_error = Some(CuError::new("read_failed", "injected strict read"));
        let error = window_place_resolved(
            crate::place::PlaceAction::LeftHalf,
            &saga_window(before),
            &[saga_screen()],
            history,
            &mut runtime,
            &mut SavingHistory,
        )
        .expect_err("strict read must fail");
        assert_eq!(error.code, "window_state_unavailable");
        assert_eq!(error.detail.as_ref().unwrap()["effect"], "not_applied");
        assert!(runtime.apply_handles.is_empty());
        assert!(!path.exists());
        remove_saga_scratch(&path);
    }

    #[test]
    fn history_commit_failure_rolls_window_back_and_retains_bytes() {
        let path = saga_scratch("commit-rollback");
        let before = saga_rect(100.0, 100.0, 800.0, 600.0);
        let after = saga_rect(0.0, 0.0, 960.0, 1040.0);
        let history = crate::place::PlaceHistory::open_at(path.clone())
            .expect("history")
            .plan_record("seed", 1, before, before);
        history.save().expect("seed history");
        let old_bytes = std::fs::read(&path).expect("old history bytes");
        let mut runtime = FakePlaceRuntime::new(
            before,
            [
                FakeApply::Ok {
                    actual: after,
                    quantized: false,
                    clamped: false,
                },
                FakeApply::Ok {
                    actual: before,
                    quantized: false,
                    clamped: false,
                },
            ],
        );
        let error = window_place_resolved(
            crate::place::PlaceAction::LeftHalf,
            &saga_window(before),
            &[saga_screen()],
            history,
            &mut runtime,
            &mut FailingHistory { published: false },
        )
        .expect_err("history commit must fail");
        assert_eq!(error.code, "history_commit_failed");
        assert_eq!(error.detail.as_ref().unwrap()["effect"], "rolled_back");
        assert!(runtime.rect.almost_eq(before));
        assert_eq!(runtime.apply_handles, [7, 7]);
        assert_eq!(std::fs::read(&path).expect("retained history"), old_bytes);
        remove_saga_scratch(&path);
    }

    #[test]
    fn partial_apply_failure_does_not_overwrite_unverified_window_state() {
        let path = saga_scratch("apply-rollback");
        let before = saga_rect(100.0, 100.0, 800.0, 600.0);
        let partial = saga_rect(0.0, 0.0, 970.0, 1040.0);
        let history = crate::place::PlaceHistory::open_at(path.clone()).expect("history");
        let mut runtime = FakePlaceRuntime::new(
            before,
            [FakeApply::Err {
                error: CuError::new("readback_failed", "injected apply readback failure"),
                observed: partial,
            }],
        );
        let error = window_place_resolved(
            crate::place::PlaceAction::LeftHalf,
            &saga_window(before),
            &[saga_screen()],
            history,
            &mut runtime,
            &mut SavingHistory,
        )
        .expect_err("partial apply must fail");
        assert_eq!(error.code, "window_place_in_doubt");
        assert_eq!(error.detail.as_ref().unwrap()["effect"], "possibly_applied");
        assert_eq!(
            error.detail.as_ref().unwrap()["rollback"],
            "skipped_unverified_apply_state"
        );
        assert!(runtime.rect.almost_eq(partial));
        assert_eq!(runtime.apply_handles, [7]);
        assert!(
            !path.exists(),
            "history must not commit after apply failure"
        );
        remove_saga_scratch(&path);
    }

    #[test]
    fn history_commit_and_rollback_failure_is_structured_in_doubt() {
        let path = saga_scratch("commit-in-doubt");
        let before = saga_rect(100.0, 100.0, 800.0, 600.0);
        let after = saga_rect(0.0, 0.0, 960.0, 1040.0);
        let partial = saga_rect(4.0, 4.0, 950.0, 1030.0);
        let history = crate::place::PlaceHistory::open_at(path.clone()).expect("history");
        let mut runtime = FakePlaceRuntime::new(
            before,
            [
                FakeApply::Ok {
                    actual: after,
                    quantized: false,
                    clamped: false,
                },
                FakeApply::Err {
                    error: CuError::new("rollback_failed", "injected rollback failure"),
                    observed: partial,
                },
            ],
        );
        let error = window_place_resolved(
            crate::place::PlaceAction::LeftHalf,
            &saga_window(before),
            &[saga_screen()],
            history,
            &mut runtime,
            &mut FailingHistory { published: false },
        )
        .expect_err("rollback failure must be in doubt");
        let detail = error.detail.as_ref().expect("structured detail");
        assert_eq!(error.code, "window_place_in_doubt");
        assert_eq!(detail["effect"], "possibly_applied");
        assert_eq!(detail["rollback"], "rollback_failed");
        assert_eq!(detail["observed"], rect_payload(partial));
        assert!(!path.exists(), "failed commit must not create history");
        remove_saga_scratch(&path);
    }

    #[test]
    fn undo_uses_current_validated_handle_not_stored_historical_handle() {
        let path = saga_scratch("stale-handle");
        let original = saga_rect(100.0, 100.0, 800.0, 600.0);
        let current = saga_rect(0.0, 0.0, 960.0, 1040.0);
        let history = crate::place::PlaceHistory::open_at(path.clone())
            .expect("history")
            .plan_record("42:fixture-app", 999, original, current);
        history.save().expect("seed history");
        let mut runtime = FakePlaceRuntime::new(
            current,
            [FakeApply::Ok {
                actual: original,
                quantized: false,
                clamped: false,
            }],
        );
        let reply = window_place_resolved(
            crate::place::PlaceAction::Undo,
            &saga_window(current),
            &[saga_screen()],
            history,
            &mut runtime,
            &mut SavingHistory,
        )
        .expect("undo");
        assert_eq!(reply["window"], 7);
        assert_eq!(runtime.apply_handles, [7]);
        let reopened = crate::place::PlaceHistory::open_at(path.clone()).expect("reopen");
        let (_, redo) = reopened.plan_redo("42:fixture-app").expect("redo remains");
        assert!(redo.almost_eq(current));
        remove_saga_scratch(&path);
    }

    #[test]
    fn quantized_final_readback_is_the_history_record() {
        let path = saga_scratch("quantized-final");
        let before = saga_rect(100.0, 100.0, 801.0, 601.0);
        let actual = saga_rect(0.0, 0.0, 958.0, 1038.0);
        let history = crate::place::PlaceHistory::open_at(path.clone()).expect("history");
        let mut runtime = FakePlaceRuntime::new(
            before,
            [FakeApply::Ok {
                actual,
                quantized: true,
                clamped: false,
            }],
        );
        let reply = window_place_resolved(
            crate::place::PlaceAction::LeftHalf,
            &saga_window(before),
            &[saga_screen()],
            history,
            &mut runtime,
            &mut SavingHistory,
        )
        .expect("place");
        assert_eq!(reply["quantized"], true);
        assert_eq!(reply["after"], rect_payload(actual));
        let reopened = crate::place::PlaceHistory::open_at(path.clone()).expect("reopen");
        let (undone, undo_target) = reopened.plan_undo("42:fixture-app").expect("undo");
        assert!(undo_target.almost_eq(before));
        let (_, redo_target) = undone.plan_redo("42:fixture-app").expect("redo");
        assert!(redo_target.almost_eq(actual));
        remove_saga_scratch(&path);
    }

    #[test]
    fn coordinate_click_requires_degraded_marker() {
        let auth = Authorization::new([Grant::Observe, Grant::Actuate].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Click {
            target: TargetRef::Current,
            window: None,
            node: None,
            name: None,
            role: None,
            coords: Some([1, 2]),
            degraded: false,
            clicks: 1,
            button: PointerButton::Left,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn node_click_uses_accessibility_tree_when_node_is_set() {
        let auth = Authorization::new([Grant::Actuate].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Click {
            target: TargetRef::Current,
            window: None,
            node: Some("/0/999999".into()),
            name: None,
            role: None,
            coords: None,
            degraded: false,
            clicks: 1,
            button: PointerButton::Left,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_invalid_node_id"
                    | "a11y_node_not_found"
                    | "a11y_backend_failed"
                    | "dylib_load"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    fn node(name: &str, role: &str, states: &[&str]) -> mechanism::A11yNode {
        node_at("/0/1", name, role, states)
    }

    fn node_at(id: &str, name: &str, role: &str, states: &[&str]) -> mechanism::A11yNode {
        mechanism::A11yNode {
            id: id.into(),
            parent_id: Some("/0".into()),
            role: role.into(),
            name: name.into(),
            states: states.iter().map(|state| (*state).to_owned()).collect(),
            bounds: mechanism::A11yBounds {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            actions: Vec::new(),
            text: None,
            identifier: None,
        }
    }

    #[test]
    fn node_match_is_case_insensitive_and_requires_showing() {
        let shown = node("Reload this page", "push button", &["showing", "enabled"]);
        assert!(node_matches(&shown, "reload", None));
        assert!(node_matches(&shown, "reload", Some("push button")));
        assert!(!node_matches(&shown, "reload", Some("entry")));
        assert!(!node_matches(&shown, "bookmark", None));

        let hidden = node("Reload this page", "push button", &["enabled"]);
        assert!(!node_matches(&hidden, "reload", None));
    }

    #[test]
    fn node_wait_timeout_is_a_typed_failure() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeNameContains {
                pattern: "agenterm-no-such-node".into(),
                role: None,
                window: Some(-1),
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok, "timeout must not report success");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(code, "timeout" | "unsupported"),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn node_text_equals_timeout_is_a_typed_failure() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeTextEquals {
                expected: "agenterm-no-such-text".into(),
                name: "agenterm-no-such-node".into(),
                role: None,
                window: Some(-1),
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok, "timeout must not report success");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(code, "timeout" | "unsupported"),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn text_equals_success_publishes_gettext_not_snapshot_text() {
        let mut snapshot = node("Message Reasonix…", "text", &["showing", "editable"]);
        snapshot.text = Some("stale-snapshot".into());
        snapshot.id = "/0/0/0/0/0/0/0/0/8/1/0".into();
        let payload =
            text_equals_success("at-spi2", Some(4194318), 2, &snapshot, "RXWAIT-TYPED", 130);
        assert_eq!(payload["via"], "gettext");
        assert_eq!(payload["text"], "RXWAIT-TYPED");
        assert_eq!(payload["observation"]["text"], "RXWAIT-TYPED");
        assert_eq!(payload["node"]["text"], "RXWAIT-TYPED");
        assert_ne!(payload["via"], "text");
        assert_ne!(payload["node"]["text"], "stale-snapshot");
    }

    #[test]
    fn focused_candidates_order_innermost_widget_before_focused_ancestor() {
        // Reasonix shape: a focused container without Text sits above the
        // focused composer textarea; the composer must be probed first.
        let panel = node_at("/0/0/0/0/0/0/0", "", "filler", &["showing", "focused"]);
        let composer = node_at(
            "/0/0/0/0/0/0/0/0/5/1/0",
            "Message Reasonix…",
            "text",
            &["showing", "editable", "focused"],
        );
        let hidden = node_at("/0/0/0/0/0/0/0/0/9", "", "text", &["focused"]);
        let unfocused = node_at("/0/1", "Send", "push button", &["showing"]);
        let nodes = vec![panel.clone(), composer.clone(), hidden, unfocused];
        let candidates = focused_candidates_innermost_first(&nodes);
        let ids: Vec<&str> = candidates.iter().map(|node| node.id.as_str()).collect();
        assert_eq!(ids, vec![composer.id.as_str(), panel.id.as_str()]);
    }

    #[test]
    fn text_equals_timeout_reports_last_gettext() {
        assert_eq!(
            text_equals_timeout_detail(Some("RXWAIT-TYPED"), None, 130),
            "last GetText=\"RXWAIT-TYPED\""
        );
        let failed = CuError::new("a11y_text_unavailable", "no Text.GetText");
        assert_eq!(
            text_equals_timeout_detail(None, Some(&failed), 130),
            "last GetText failed: no Text.GetText (a11y_text_unavailable)"
        );
    }

    #[test]
    fn node_text_equals_requires_window() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeTextEquals {
                expected: "x".into(),
                name: "FixtureField".into(),
                role: None,
                window: None,
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn node_text_contains_timeout_is_a_typed_failure() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeTextContains {
                substring: "agenterm-no-such-sub".into(),
                name: "agenterm-no-such-node".into(),
                role: None,
                window: Some(-1),
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok, "timeout must not report success");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(code, "timeout" | "unsupported"),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn node_text_contains_requires_window() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeTextContains {
                substring: "GATE".into(),
                name: "FixtureField".into(),
                role: None,
                window: None,
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
        assert!(
            reply
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("--text-contains"),
            "missing-window message should name the flag"
        );
    }

    #[test]
    fn text_contains_matches_substring_of_independent_gettext() {
        assert!(NodeTextMatch::Contains.matches("34aGATEXXXX", "GATE"));
        assert!(!NodeTextMatch::Contains.matches("34aGATEXXXX", "NOPE"));
        assert!(!NodeTextMatch::Equals.matches("34aGATEXXXX", "GATE"));
        assert!(NodeTextMatch::Equals.matches("34aGATEXXXX", "34aGATEXXXX"));
    }

    #[test]
    fn text_contains_success_publishes_full_gettext_not_substring() {
        let mut snapshot = node("FixtureField", "entry", &["showing", "editable"]);
        snapshot.text = Some("stale-snapshot".into());
        let payload =
            text_equals_success("at-spi2", Some(4194318), 2, &snapshot, "34aGATEXXXX", 12);
        assert_eq!(payload["via"], "gettext");
        assert_eq!(payload["text"], "34aGATEXXXX");
        assert!(payload["text"].as_str().unwrap().contains("GATE"));
        assert_ne!(payload["text"], "GATE");
        assert_ne!(payload["node"]["text"], "stale-snapshot");
    }

    fn actuate_executor() -> Executor {
        Executor::new(Authorization::new(
            [Grant::Observe, Grant::Actuate].into_iter().collect(),
        ))
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

    fn observe_executor() -> Executor {
        Executor::new(Authorization::new([Grant::Observe].into_iter().collect()))
    }

    #[test]
    fn name_click_requires_window() {
        let command = Command::Click {
            target: TargetRef::Current,
            window: None,
            node: None,
            name: Some("Reload".into()),
            role: None,
            coords: None,
            degraded: false,
            clicks: 1,
            button: PointerButton::Left,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_and_node_are_exclusive() {
        let command = Command::Click {
            target: TargetRef::Current,
            window: Some(1),
            node: Some("/0/1".into()),
            name: Some("Reload".into()),
            role: None,
            coords: None,
            degraded: false,
            clicks: 1,
            button: PointerButton::Left,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_and_coords_are_exclusive() {
        let command = Command::Click {
            target: TargetRef::Current,
            window: Some(1),
            node: None,
            name: Some("Reload".into()),
            role: None,
            coords: Some([1, 2]),
            degraded: true,
            clicks: 1,
            button: PointerButton::Left,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_click_missing_node_is_typed() {
        let command = Command::Click {
            target: TargetRef::Current,
            window: Some(-1),
            node: None,
            name: Some("agenterm-no-such-node".into()),
            role: None,
            coords: None,
            degraded: false,
            clicks: 1,
            button: PointerButton::Left,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok, "missing name must not report success");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn name_focus_missing_node_is_typed() {
        let command = Command::Focus {
            target: TargetRef::Current,
            window: Some(-1),
            node: None,
            name: Some("agenterm-no-such-node".into()),
            role: Some("button".into()),
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn find_showing_node_reuses_wait_matcher() {
        let nodes = vec![
            node("hidden Reload", "button", &["enabled"]),
            node("Reload this page", "push button", &["showing", "enabled"]),
        ];
        let matched =
            require_unique_showing_node(&nodes, "reload", Some("button")).expect("shown match");
        assert_eq!(matched.name, "Reload this page");
        let missing = require_unique_showing_node(&nodes, "reload", Some("entry")).unwrap_err();
        assert_eq!(missing.code, "a11y_node_not_found");
        assert_eq!(missing.count, None);
    }

    #[test]
    fn two_showing_nodes_named_alike_are_ambiguous() {
        let nodes = vec![
            node_at("/0/1", "Tab search", "push button", &["showing", "enabled"]),
            node_at("/0/2", "Tab search", "push button", &["visible", "enabled"]),
        ];
        let err = require_unique_showing_node(&nodes, "Tab search", None).unwrap_err();
        assert_eq!(err.code, "a11y_node_ambiguous");
        assert_eq!(err.count, Some(2));
        assert!(
            err.message.contains("2"),
            "ambiguous error must carry the match count: {}",
            err.message
        );

        // A hidden duplicate must not count; only showing/visible nodes do.
        let one_showing = vec![
            node_at("/0/1", "Tab search", "push button", &["showing"]),
            node_at("/0/2", "Tab search", "push button", &["enabled"]),
        ];
        let matched = require_unique_showing_node(&one_showing, "Tab search", None)
            .expect("hidden twin is not a match");
        assert_eq!(matched.id, "/0/1");
    }

    #[test]
    fn actuation_without_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth);
        let command = Command::SendText {
            target: TargetRef::Current,
            text: "hello".into(),
            window: None,
            name: None,
            role: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn window_place_without_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth);
        let command = Command::WindowPlace {
            target: TargetRef::Current,
            action: "left-half".into(),
            window: None,
            frame: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn window_place_unknown_action_is_invalid() {
        let auth = Authorization::new([Grant::Actuate].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::WindowPlace {
            target: TargetRef::Current,
            action: "tile-magic".into(),
            window: None,
            frame: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn window_place_resolves_the_destination_screen_for_clamp_and_reply() {
        use mechanism::window_enumerate::{ScreenInfo, WindowBounds};

        let screens = [
            ScreenInfo {
                frame: WindowBounds {
                    x: 0,
                    y: 0,
                    width: 1000,
                    height: 800,
                },
                visible: WindowBounds {
                    x: 0,
                    y: 40,
                    width: 1000,
                    height: 760,
                },
                primary: true,
            },
            ScreenInfo {
                frame: WindowBounds {
                    x: 1000,
                    y: 0,
                    width: 1200,
                    height: 900,
                },
                visible: WindowBounds {
                    x: 1000,
                    y: 0,
                    width: 1200,
                    height: 860,
                },
                primary: false,
            },
        ];

        let (index, screen) = screen_for_rect(
            crate::place::Rect::new(1300.0, 200.0, 500.0, 400.0),
            &screens,
        )
        .expect("destination screen");
        assert_eq!(index, 1);
        assert_eq!(screen.visible, screens[1].visible);

        let (index, _) = screen_for_rect(
            crate::place::Rect::new(850.0, 100.0, 500.0, 300.0),
            &screens,
        )
        .expect("largest intersection screen");
        assert_eq!(index, 1);
    }

    #[test]
    fn name_send_text_missing_node_is_typed_and_types_nothing() {
        let command = Command::SendText {
            target: TargetRef::Current,
            text: "hello".into(),
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok, "missing name must not type into the wrong place");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn name_send_text_requires_window() {
        let command = Command::SendText {
            target: TargetRef::Current,
            text: "hello".into(),
            window: None,
            name: Some("Address and search bar".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn send_text_role_without_name_is_typed() {
        let command = Command::SendText {
            target: TargetRef::Current,
            text: "hello".into(),
            window: Some(1),
            name: None,
            role: Some("entry".into()),
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
        assert!(
            reply
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("--role requires --name"),
            "role-without-name message should name the addressing contract"
        );
    }

    #[test]
    fn send_text_window_without_name_does_not_xtest() {
        // A synthetic window must take the focused AT-SPI path, not
        // input_inject::type_text. Success here would mean XTest spray.
        let command = Command::SendText {
            target: TargetRef::Current,
            text: "hello".into(),
            window: Some(-1),
            name: None,
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(
            !reply.ok,
            "send-text --window without --name must not fall through to XTest"
        );
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_text_unavailable"
                    | "a11y_backend_failed"
                    | "unsupported"
                    | "failed"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn focused_copy_without_live_focus_fails_typed() {
        // --window without --name is focused copy, not a missing-name usage
        // error. Without a real tree/focus it typed-fails on the a11y path.
        let command = Command::Copy {
            target: TargetRef::Current,
            window: Some(1),
            name: None,
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_text_unavailable"
                    | "a11y_backend_failed"
                    | "dylib_load"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn copy_role_requires_name() {
        let command = Command::Copy {
            target: TargetRef::Current,
            window: Some(1),
            name: None,
            role: Some("entry".into()),
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_copy_requires_window() {
        let command = Command::Copy {
            target: TargetRef::Current,
            window: None,
            name: Some("FixtureSource".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_copy_missing_node_is_typed_and_copies_nothing() {
        let command = Command::Copy {
            target: TargetRef::Current,
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok, "missing name must not seed the clipboard");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn copy_without_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth);
        let command = Command::Copy {
            target: TargetRef::Current,
            window: Some(1),
            name: Some("FixtureSource".into()),
            role: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn focused_paste_without_live_focus_fails_typed() {
        // --window without --name is focused paste, not a missing-name usage
        // error. Without a real tree/focus it typed-fails on the a11y path.
        let command = Command::Paste {
            target: TargetRef::Current,
            text: Some("hello".into()),
            window: Some(1),
            name: None,
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_text_unavailable"
                    | "a11y_backend_failed"
                    | "dylib_load"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn paste_role_requires_name() {
        let command = Command::Paste {
            target: TargetRef::Current,
            text: Some("hello".into()),
            window: Some(1),
            name: None,
            role: Some("entry".into()),
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_paste_requires_window() {
        let command = Command::Paste {
            target: TargetRef::Current,
            text: Some("hello".into()),
            window: None,
            name: Some("FixtureField".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_paste_missing_node_is_typed_and_writes_nothing() {
        let command = Command::Paste {
            target: TargetRef::Current,
            text: Some("hello".into()),
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(
            !reply.ok,
            "missing name must not paste into the wrong place"
        );
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn paste_without_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth);
        let command = Command::Paste {
            target: TargetRef::Current,
            text: Some("hello".into()),
            window: Some(1),
            name: Some("FixtureField".into()),
            role: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn name_send_keys_requires_window() {
        let command = Command::SendKeys {
            target: TargetRef::Current,
            keys: "enter".into(),
            window: None,
            name: Some("Address and search bar".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn send_keys_role_without_name_is_typed() {
        let command = Command::SendKeys {
            target: TargetRef::Current,
            keys: "k".into(),
            window: Some(1),
            name: None,
            role: Some("entry".into()),
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
        assert!(
            reply
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("--role requires --name"),
            "role-without-name message should name the addressing contract"
        );
    }

    #[test]
    fn send_keys_window_without_name_does_not_xtest() {
        // A synthetic window must take the focused AT-SPI path, not
        // input_inject::send_keys. Success here would mean XTest spray.
        let command = Command::SendKeys {
            target: TargetRef::Current,
            keys: "314GATE".into(),
            window: Some(-1),
            name: None,
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(
            !reply.ok,
            "send-keys --window without --name must not fall through to XTest"
        );
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_key_unavailable"
                    | "a11y_text_unavailable"
                    | "a11y_backend_failed"
                    | "dylib_load"
                    | "unsupported"
                    | "failed"
                    | "invalid_input"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn plain_typeable_text_accepts_gate_literal() {
        assert!(is_plain_typeable_text("314GATE123456"));
        assert!(is_plain_typeable_text("k"));
        assert!(!is_plain_typeable_text("enter"));
        assert!(!is_plain_typeable_text("ctrl+a"));
        assert!(!is_plain_typeable_text(""));
    }

    #[test]
    fn name_send_keys_missing_node_is_typed_and_sends_nothing() {
        let command = Command::SendKeys {
            target: TargetRef::Current,
            keys: "enter".into(),
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok, "missing name must not send keys somewhere else");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn name_send_keys_two_showing_matches_are_ambiguous() {
        // `send-keys --name` resolves through this exact matcher, so two
        // showing hits must abort before any chord reaches the display.
        let nodes = vec![
            node_at("/0/1", "Address and search bar", "entry", &["showing"]),
            node_at("/0/2", "Address and search bar", "entry", &["visible"]),
        ];
        let err = require_unique_showing_node(&nodes, "Address and search bar", None).unwrap_err();
        assert_eq!(err.code, "a11y_node_ambiguous");
        assert_eq!(err.count, Some(2));
    }

    #[test]
    fn name_scroll_requires_name() {
        let command = Command::Scroll {
            target: TargetRef::Current,
            window: Some(1),
            name: None,
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_scroll_requires_window() {
        let command = Command::Scroll {
            target: TargetRef::Current,
            window: None,
            name: Some("OffscreenField".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_scroll_missing_node_is_typed_and_scrolls_nothing() {
        let command = Command::Scroll {
            target: TargetRef::Current,
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok, "missing name must not scroll");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn scroll_without_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth);
        let command = Command::Scroll {
            target: TargetRef::Current,
            window: Some(1),
            name: Some("OffscreenField".into()),
            role: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn name_get_extents_requires_name() {
        let command = Command::GetExtents {
            target: TargetRef::Current,
            window: Some(1),
            name: None,
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_get_extents_requires_window() {
        let command = Command::GetExtents {
            target: TargetRef::Current,
            window: None,
            name: Some("OffscreenField".into()),
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_get_extents_missing_node_is_typed() {
        let command = Command::GetExtents {
            target: TargetRef::Current,
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok, "missing name must not invent extents");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn scroll_and_get_extents_verbs_are_named() {
        let scroll = Command::Scroll {
            target: TargetRef::Current,
            window: Some(1),
            name: Some("OffscreenField".into()),
            role: None,
        };
        let extents = Command::GetExtents {
            target: TargetRef::Current,
            window: Some(1),
            name: Some("ScrollViewport".into()),
            role: None,
        };
        assert_eq!(scroll.verb(), "scroll");
        assert_eq!(extents.verb(), "get-extents");
        assert_eq!(scroll.required_grant(), Grant::Actuate);
        assert_eq!(extents.required_grant(), Grant::Observe);
    }

    #[test]
    fn name_select_requires_name() {
        let command = Command::Select {
            target: TargetRef::Current,
            start: 0,
            end: 4,
            window: Some(1),
            name: None,
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_select_requires_window() {
        let command = Command::Select {
            target: TargetRef::Current,
            start: 0,
            end: 4,
            window: None,
            name: Some("SelectField".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_select_rejects_inverted_range() {
        let command = Command::Select {
            target: TargetRef::Current,
            start: 4,
            end: 0,
            window: Some(1),
            name: Some("SelectField".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_select_missing_node_is_typed_and_selects_nothing() {
        let command = Command::Select {
            target: TargetRef::Current,
            start: 0,
            end: 4,
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok, "missing name must not select");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn select_without_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth);
        let command = Command::Select {
            target: TargetRef::Current,
            start: 0,
            end: 4,
            window: Some(1),
            name: Some("SelectField".into()),
            role: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn name_get_selection_requires_name() {
        let command = Command::GetSelection {
            target: TargetRef::Current,
            window: Some(1),
            name: None,
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_get_selection_requires_window() {
        let command = Command::GetSelection {
            target: TargetRef::Current,
            window: None,
            name: Some("SelectField".into()),
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_get_selection_missing_node_is_typed() {
        let command = Command::GetSelection {
            target: TargetRef::Current,
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok, "missing name must not invent a selection");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn select_and_get_selection_verbs_are_named() {
        let select = Command::Select {
            target: TargetRef::Current,
            start: 0,
            end: 4,
            window: Some(1),
            name: Some("SelectField".into()),
            role: None,
        };
        let get_selection = Command::GetSelection {
            target: TargetRef::Current,
            window: Some(1),
            name: Some("SelectField".into()),
            role: None,
        };
        assert_eq!(select.verb(), "select");
        assert_eq!(get_selection.verb(), "get-selection");
        assert_eq!(select.required_grant(), Grant::Actuate);
        assert_eq!(get_selection.required_grant(), Grant::Observe);
    }

    #[test]
    fn set_caret_without_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth);
        let command = Command::SetCaret {
            target: TargetRef::Current,
            offset: 2,
            window: Some(1),
            name: Some("Command".into()),
            role: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn name_get_caret_requires_name() {
        let command = Command::GetCaret {
            target: TargetRef::Current,
            window: Some(1),
            name: None,
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn set_caret_and_get_caret_verbs_are_named() {
        let set_caret = Command::SetCaret {
            target: TargetRef::Current,
            offset: 2,
            window: Some(1),
            name: Some("Command".into()),
            role: None,
        };
        let get_caret = Command::GetCaret {
            target: TargetRef::Current,
            window: Some(1),
            name: Some("Command".into()),
            role: None,
        };
        assert_eq!(set_caret.verb(), "set-caret");
        assert_eq!(get_caret.verb(), "get-caret");
        assert_eq!(set_caret.required_grant(), Grant::Actuate);
        assert_eq!(get_caret.required_grant(), Grant::Observe);
    }

    #[test]
    fn get_text_without_name_requires_window() {
        let command = Command::GetText {
            target: TargetRef::Current,
            window: None,
            name: None,
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
        assert!(
            reply
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("requires --window <handle>"),
            "missing-window message should name the addressing contract"
        );
    }

    #[test]
    fn get_text_role_without_name_is_typed() {
        let command = Command::GetText {
            target: TargetRef::Current,
            window: Some(1),
            name: None,
            role: Some("text".into()),
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
        assert!(
            reply
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("--role requires --name"),
            "role-without-name message should name the addressing contract"
        );
    }

    #[test]
    fn get_text_verb_is_named_and_observe() {
        let get_text = Command::GetText {
            target: TargetRef::Current,
            window: Some(1),
            name: Some("Command".into()),
            role: None,
        };
        assert_eq!(get_text.verb(), "get-text");
        assert_eq!(get_text.required_grant(), Grant::Observe);
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
    fn permission_denial_is_typed_denied_with_repair_path() {
        let error = map_mechanism_err(mechanism::MechanismError::Failed {
            code: "a11y_permission_denied".into(),
            message: "AXIsProcessTrusted() is false".into(),
        });
        assert_eq!(error.code, "denied");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["reason"], "a11y_permission_denied");
        assert_eq!(detail["permission"], "accessibility");
        assert_eq!(detail["repair"], ACCESSIBILITY_REPAIR_PATH);
        // Every other mechanism code passes through unchanged.
        let other = map_mechanism_err(mechanism::MechanismError::Failed {
            code: "a11y_tree_empty".into(),
            message: "no nodes".into(),
        });
        assert_eq!(other.code, "a11y_tree_empty");
        assert!(other.detail.is_none());
    }

    #[test]
    fn tree_and_query_budgets_fail_typed_before_any_mechanism_call() {
        let executor = observe_executor();
        let too_deep = executor.execute(&Command::Tree {
            target: TargetRef::Current,
            window: Some(1),
            depth: Some(65),
            max_nodes: None,
            flat: false,
        });
        assert!(!too_deep.ok);
        assert_eq!(too_deep.error.as_ref().unwrap().code, "invalid_input");
        let zero_nodes = executor.execute(&Command::Tree {
            target: TargetRef::Current,
            window: Some(1),
            depth: None,
            max_nodes: Some(0),
            flat: false,
        });
        assert_eq!(zero_nodes.error.as_ref().unwrap().code, "invalid_input");
        let query =
            |window: isize, text: Option<&str>, text_exact: Option<&str>, max: Option<usize>| {
                executor.execute(&Command::Query {
                    target: TargetRef::Current,
                    window,
                    depth: None,
                    max_nodes: None,
                    role: Vec::new(),
                    text: text.map(str::to_owned),
                    text_exact: text_exact.map(str::to_owned),
                    identifier: None,
                    actionable: false,
                    within: None,
                    offset: None,
                    max,
                    selector: None,
                })
            };
        let no_window = query(0, None, None, None);
        assert_eq!(no_window.command, "query");
        assert_eq!(no_window.error.as_ref().unwrap().code, "invalid_input");
        let both_texts = query(1, Some("a"), Some("b"), None);
        assert_eq!(both_texts.error.as_ref().unwrap().code, "invalid_input");
        let bad_page = query(1, None, None, Some(0));
        assert_eq!(bad_page.error.as_ref().unwrap().code, "invalid_input");
        let bad_windows_page = executor.execute(&Command::Windows {
            target: TargetRef::Current,
            pid: None,
            app: None,
            title: None,
            focused: None,
            minimized: None,
            offset: None,
            max: Some(0),
        });
        assert_eq!(
            bad_windows_page.error.as_ref().unwrap().code,
            "invalid_input"
        );
    }

    #[test]
    fn current_capabilities_names_current_target() {
        let reply = observe_executor().execute(&Command::Capabilities {
            target: TargetRef::Current,
        });
        assert!(reply.ok);
        assert_eq!(reply.target, "current");
        assert_eq!(reply.command, "capabilities");
        let data = reply.data.as_ref().expect("data");
        assert_eq!(data["target"], "current");
        assert_eq!(data["transport"]["status"], "in_process");
        assert_eq!(data["transport"]["available"], true);
        assert_eq!(data["verbs"]["capabilities"]["status"], "available");
        assert_eq!(data["verbs"]["pty"]["status"], "unsupported");
        assert_eq!(
            data["verbs"]["page-js"]["backend"],
            "debugger-runtime-evaluate"
        );
        assert!(
            data["mcu_groups"].as_array().map(|g| g.len()).unwrap_or(0)
                >= crate::mcu_surface::GROUPS.len()
        );
        let tsv = data["alignment_tsv"].as_str().unwrap_or("");
        assert!(tsv.contains("shell-pty-job\tlinux\tunsupported\t"));
        assert!(!tsv.contains("still-gap"));
        assert_eq!(data["verbs"]["windows-watch"]["mode"], "poll-diff");
        assert_eq!(data["verbs"]["windows-watch"]["group"], "discover");
        assert_eq!(data["verbs"]["apps"]["running_only"], true);
        assert_eq!(data["verbs"]["apps"]["group"], "discover");
        assert_eq!(data["verbs"]["orderwin"]["mode"], "raise");
        assert_eq!(data["verbs"]["orderwin"]["group"], "geometry");
        assert_ne!(data["verbs"]["orderwin"]["status"], "");
        assert_eq!(data["verbs"]["page-js"]["status"], "available");
        assert_eq!(data["verbs"]["page-js"]["mode"], "cdp");
        assert_eq!(
            data["verbs"]["invoke"]["actions"]["set-selection"],
            "mapped"
        );
        // `mapped`, not `available`: the ABI carries these three now, but
        // whether a given node offers AXCancel / AXSelected / AXShowDefaultUI
        // is the backend's answer at call time, not a promise made here.
        assert_eq!(data["verbs"]["invoke"]["actions"]["cancel"], "mapped");
        assert_eq!(data["verbs"]["invoke"]["actions"]["set-selected"], "mapped");
        assert_eq!(
            data["verbs"]["invoke"]["actions"]["show-default-ui"],
            "mapped"
        );
        assert_eq!(data["verbs"]["displays"]["group"], "geometry");
        assert_eq!(data["verbs"]["displays"]["status"], "available");
        assert_eq!(data["verbs"]["spaces"]["group"], "geometry");
        if cfg!(target_os = "macos") {
            assert_eq!(data["verbs"]["spaces"]["status"], "available");
        } else {
            assert_eq!(data["verbs"]["spaces"]["status"], "unsupported");
        }
        assert_ne!(data["verbs"]["windows-watch"]["status"], "");
        assert_ne!(data["verbs"]["apps"]["status"], "");
        // Must not declare live RDP or unproven Mac AX as available.
        assert!(data["gaps"]["rdp_live"].as_str().is_some());
        assert!(data["gaps"]["macos_ax_live"].as_str().is_some());
        let mapping = data["mapping"]["tree"].as_str().unwrap_or("");
        assert!(
            !mapping.contains("RDP") && !mapping.to_lowercase().contains("rdp live"),
            "current mapping must not claim live RDP: {mapping}"
        );
    }

    #[test]
    fn displays_lists_native_screens() {
        let reply = observe_executor().execute(&Command::Displays {
            target: TargetRef::Current,
        });
        assert_eq!(reply.command, "displays");
        if reply.ok {
            let data = reply.data.as_ref().expect("displays");
            assert_eq!(data["via"], "agt_screen_list");
            assert!(data["displays"].is_array());
        } else {
            assert_ne!(reply.error.as_ref().unwrap().code, "usage");
        }
    }

    #[test]
    fn spaces_and_page_js_are_mapped_verbs() {
        let spaces = observe_executor().execute(&Command::Spaces {
            target: TargetRef::Current,
        });
        assert_eq!(spaces.command, "spaces");
        if cfg!(target_os = "macos") {
            if spaces.ok {
                let data = spaces.data.as_ref().expect("spaces");
                assert!(data["displays"].is_array());
                assert_eq!(data["moveProvider"]["available"], false);
            } else {
                assert_eq!(spaces.error.as_ref().unwrap().code, "unsupported");
            }
        } else {
            assert!(!spaces.ok);
            assert_eq!(spaces.error.as_ref().unwrap().code, "unsupported");
        }
        let page = observe_executor().execute(&Command::PageJs {
            target: TargetRef::Current,
            window: None,
            expression: Some("1+1".into()),
            port: Some(1),
        });
        assert!(!page.ok);
        assert_eq!(page.command, "page-js");
        let err = page.error.as_ref().expect("typed");
        assert_eq!(err.code, "unsupported");
        assert_eq!(
            err.detail.as_ref().unwrap()["backend"],
            "debugger-runtime-evaluate"
        );
        assert!(err.message.contains("remote-debugging-port"));
    }

    #[test]
    fn invoke_scroll_to_is_not_unmapped_spelling() {
        let reply = actuate_executor().execute(&Command::Invoke {
            target: TargetRef::Current,
            window: -1,
            node: None,
            index: None,
            name: Some("agenterm-no-such-node".into()),
            role: None,
            identifier: None,
            focused: false,
            action: InvokeAction::ScrollTo,
            value: None,
            selector: None,
        });
        assert!(!reply.ok);
        assert_eq!(reply.command, "invoke");
        let err = reply.error.as_ref().expect("typed");
        assert_ne!(err.code, "usage");
        if let Some(reason) = err.detail.as_ref().and_then(|d| d["reason"].as_str()) {
            assert_ne!(reason, "node_action_unmapped");
        }
        assert!(!err.message.contains("not mapped on the libagenterm"));
    }

    #[test]
    fn align_pty_and_windows_watch_use_group_reason_not_unknown() {
        let exec = observe_executor();
        let pty = exec.execute(&Command::Align {
            target: TargetRef::Current,
            group: "pty".into(),
        });
        assert!(!pty.ok);
        assert_eq!(pty.command, "pty");
        let err = pty.error.as_ref().expect("typed");
        assert_eq!(err.code, "unsupported");
        assert!(
            !err.message.contains("unknown MCU group"),
            "{}",
            err.message
        );
        assert_eq!(err.detail.as_ref().unwrap()["group"], "shell-pty-job");
        assert_eq!(err.detail.as_ref().unwrap()["verb"], "pty");
        let watch = exec.execute(&Command::WindowsWatch {
            target: TargetRef::Current,
            pid: None,
            app: None,
            title: None,
            duration_ms: 0,
            interval_ms: Some(0),
            max_events: Some(10),
        });
        if watch.ok {
            let data = watch.data.as_ref().expect("watch data");
            assert_eq!(data["mode"], "poll-diff");
            assert!(data["events"].is_array());
            assert!(data["windows"].is_array());
        } else {
            let werr = watch.error.as_ref().expect("typed");
            assert_ne!(werr.code, "usage");
            assert!(!werr.message.contains("unknown MCU group"));
        }
        let apps = exec.execute(&Command::Apps {
            target: TargetRef::Current,
            running: true,
            all: false,
        });
        if apps.ok {
            let data = apps.data.as_ref().expect("apps data");
            assert_eq!(data["running_only"], true);
            assert_eq!(data["installed"], false);
            assert!(data["apps"].is_array());
        } else {
            assert_ne!(apps.error.as_ref().unwrap().code, "usage");
        }
        let order_same = actuate_executor().execute(&Command::OrderWin {
            target: TargetRef::Current,
            window: 1,
            relation: OrderRelation::Above,
            relative: 1,
        });
        assert!(!order_same.ok);
        assert_eq!(order_same.command, "orderwin");
        assert_eq!(order_same.error.as_ref().unwrap().code, "invalid_input");
        let order_zero = actuate_executor().execute(&Command::OrderWin {
            target: TargetRef::Current,
            window: 0,
            relation: OrderRelation::Below,
            relative: 2,
        });
        assert_eq!(order_zero.error.as_ref().unwrap().code, "invalid_input");
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

    #[test]
    fn check_one_title_includes_heading_matches_webarea_identity() {
        let web = mechanism::A11yNode {
            id: "/0/1".into(),
            parent_id: None,
            role: "AXWebArea".into(),
            name: "Nepal floods latest: Head teacher".into(),
            states: vec!["showing".into()],
            bounds: mechanism::A11yBounds {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
            actions: Vec::new(),
            text: None,
            identifier: None,
        };
        let tree = mechanism::A11yTree {
            backend: "ax".into(),
            window_handle: Some(1),
            root_id: "/0".into(),
            nodes: vec![web],
            truncated: false,
            visited: 1,
            returned: 1,
        };
        let flat = observe::flatten(&tree);
        let expectation: crate::command::Expectation =
            serde_json::from_str(r#"{"role":"AXHeading","titleIncludes":"Nepal"}"#)
                .expect("titleIncludes");
        let verdict = super::check_one(&flat, &expectation).expect("identity-only expect");
        assert!(verdict.met);
        assert_eq!(verdict.item["page_identity"], true);
        assert!(verdict.item["checks"].as_array().unwrap().is_empty());
    }
}
