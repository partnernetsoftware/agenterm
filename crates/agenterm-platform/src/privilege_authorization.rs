//! Typed Linux native-consent boundary for the system privilege broker.
//!
//! A caller cannot supply a pid, uid, process start time, action id, details,
//! or a pre-authorized flag. The only public input carrying an authorization
//! subject is [`SystemBrokerStream`], whose peer identity is derived and
//! retained by the kernel-authenticated broker transport.

use std::{fmt, time::Duration};

use crate::system_broker::{SystemBrokerError, SystemBrokerStream};

#[path = "adapters/linux/privilege_authorization.rs"]
mod native;

/// The sole production action accepted by this facade.
pub const PRIVILEGE_ACTION_ID: &str = "com.partnernetsoftware.agenterm.cu.privilege";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PrivilegeAuthorizationDecision {
    Authorized,
    Denied,
    Dismissed,
    Challenge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PrivilegeAuthorizationErrorKind {
    InvalidPeer,
    StalePeer,
    SystemBusUnavailable,
    TransportFailed,
    InvalidNativeResult,
    TimedOut,
    CancellationFailed,
    AuthorizationCanceled,
    CancellationIdConflict,
    NotAuthorized,
    Unsupported,
}

#[derive(Debug)]
pub struct PrivilegeAuthorizationError {
    kind: PrivilegeAuthorizationErrorKind,
    message: String,
}

impl PrivilegeAuthorizationError {
    pub(crate) fn new(kind: PrivilegeAuthorizationErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> PrivilegeAuthorizationErrorKind {
        self.kind
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PrivilegeAuthorizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PrivilegeAuthorizationError {}

pub type PrivilegeAuthorizationResult<T> = Result<T, PrivilegeAuthorizationError>;

/// Ask polkit to authorize the exact live process retained by `peer`.
///
/// The peer is checked immediately before and after the native call. A caller
/// therefore cannot substitute a self-authored `unix-process` subject, and a
/// process that exits while consent is pending cannot yield authorization.
pub fn authorize_user_initiated(
    peer: &SystemBrokerStream,
    timeout: Duration,
) -> PrivilegeAuthorizationResult<PrivilegeAuthorizationDecision> {
    if timeout.is_zero() {
        return Err(PrivilegeAuthorizationError::new(
            PrivilegeAuthorizationErrorKind::TimedOut,
            "polkit authorization deadline must be greater than zero",
        ));
    }
    require_live_peer(peer, "before polkit authorization")?;
    let decision = native::authorize(peer.peer(), timeout);
    let still_live = require_live_peer(peer, "after polkit authorization");
    still_live.and(decision)
}

fn require_live_peer(peer: &SystemBrokerStream, phase: &str) -> PrivilegeAuthorizationResult<()> {
    match peer.peer_is_alive() {
        Ok(true) => Ok(()),
        Ok(false) => Err(PrivilegeAuthorizationError::new(
            PrivilegeAuthorizationErrorKind::StalePeer,
            format!("system broker peer exited {phase}"),
        )),
        Err(error) => Err(map_peer_error(error, phase)),
    }
}

fn map_peer_error(error: SystemBrokerError, phase: &str) -> PrivilegeAuthorizationError {
    PrivilegeAuthorizationError::new(
        PrivilegeAuthorizationErrorKind::InvalidPeer,
        format!("cannot authenticate system broker peer {phase}: {error}"),
    )
}
