//! Typed native-consent boundary for the system privilege broker.
//!
//! A caller cannot supply a pid, uid, process start time, action id, details,
//! or a pre-authorized flag. The only public input carrying an authorization
//! subject is [`SystemBrokerStream`], whose peer identity is derived and
//! retained by the kernel-authenticated broker transport.

use std::{fmt, time::Duration};

#[cfg(target_os = "linux")]
use crate::system_broker::{SystemBrokerError, SystemBrokerStream};

#[cfg_attr(
    target_os = "linux",
    path = "adapters/linux/privilege_authorization.rs"
)]
#[cfg_attr(
    target_os = "macos",
    path = "adapters/macos/privilege_authorization.rs"
)]
mod native;

/// The sole production action accepted by this facade.
#[cfg(not(target_os = "macos"))]
pub const PRIVILEGE_ACTION_ID: &str = "com.partnernetsoftware.agenterm.cu.privilege";
/// The operation-scoped macOS Authorization Services right. It deliberately
/// differs from the launchd helper label and never contains a wildcard.
#[cfg(target_os = "macos")]
pub const PRIVILEGE_ACTION_ID: &str = "com.partnernetsoftware.agenterm.cu.privilege.process-signal";

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
    InvalidProof,
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

/// Move-only bearer proof produced by macOS Authorization Services.
///
/// It is intentionally neither `Clone`, `Debug` nor serializable. The fixed
/// 32-byte wire form can only be written/read through bounded methods. A client
/// proof keeps its AuthorizationRef alive until this value is dropped; the
/// broker consumes a separately received value and destroys its right.
#[cfg(target_os = "macos")]
pub struct MacosAuthorizationProof {
    bytes: [u8; native::EXTERNAL_FORM_LENGTH],
    cleanup: Option<std::sync::mpsc::Sender<()>>,
    transmitted: bool,
}

#[cfg(target_os = "macos")]
impl MacosAuthorizationProof {
    pub const WIRE_LENGTH: usize = native::EXTERNAL_FORM_LENGTH;

    /// Read exactly one fixed-size external form from an already bounded frame.
    pub fn read_from(reader: &mut impl std::io::Read) -> PrivilegeAuthorizationResult<Self> {
        let mut bytes = [0_u8; native::EXTERNAL_FORM_LENGTH];
        reader.read_exact(&mut bytes).map_err(|error| {
            PrivilegeAuthorizationError::new(
                PrivilegeAuthorizationErrorKind::InvalidProof,
                format!("cannot read the fixed macOS authorization proof: {error}"),
            )
        })?;
        Ok(Self {
            bytes,
            cleanup: None,
            transmitted: false,
        })
    }

    /// Write exactly one external form without exposing it through formatting.
    ///
    /// The attempt consumes this proof's transmission allowance before I/O,
    /// so a partial write can never be retried as a second bearer proof.
    pub fn write_to(
        &mut self,
        writer: &mut impl std::io::Write,
    ) -> PrivilegeAuthorizationResult<()> {
        if self.transmitted {
            return Err(PrivilegeAuthorizationError::new(
                PrivilegeAuthorizationErrorKind::InvalidProof,
                "macOS authorization proof has already been transmitted",
            ));
        }
        self.transmitted = true;
        writer.write_all(&self.bytes).map_err(|error| {
            PrivilegeAuthorizationError::new(
                PrivilegeAuthorizationErrorKind::TransportFailed,
                format!("cannot write the fixed macOS authorization proof: {error}"),
            )
        })
    }
}

#[cfg(target_os = "macos")]
impl Drop for MacosAuthorizationProof {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            let _ = cleanup.send(());
        }
        wipe_secret(&mut self.bytes);
    }
}

#[cfg(target_os = "macos")]
fn wipe_secret(bytes: &mut [u8]) {
    for byte in bytes {
        // SAFETY: every pointer comes from this live exclusive slice; volatile
        // stores prevent the secret-erasure writes from being optimized away.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Ask macOS Authorization Services for the one fixed privilege right.
///
/// Consent runs on an owned worker. A deadline returns typed `TimedOut`; a late
/// result cannot escape because the worker destroys the AuthorizationRef when
/// delivery fails or when the returned proof is dropped.
#[cfg(target_os = "macos")]
pub fn acquire_macos_authorization_proof(
    timeout: Duration,
) -> PrivilegeAuthorizationResult<MacosAuthorizationProof> {
    let (bytes, cleanup) = native::acquire(timeout)?;
    Ok(MacosAuthorizationProof {
        bytes,
        cleanup: Some(cleanup),
        transmitted: false,
    })
}

/// Consume and validate one external form inside the root system broker.
///
/// Validation never permits interaction or extends rights. The fixed right is
/// destroyed before return on every internalized-proof path.
#[cfg(target_os = "macos")]
pub fn verify_macos_authorization_proof(
    mut proof: MacosAuthorizationProof,
) -> PrivilegeAuthorizationResult<PrivilegeAuthorizationDecision> {
    native::verify(&mut proof.bytes)
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::{
        MacosAuthorizationProof, PrivilegeAuthorizationErrorKind, acquire_macos_authorization_proof,
    };

    #[test]
    fn proof_wire_form_is_fixed_and_single_transmission() {
        let bytes = [7_u8; MacosAuthorizationProof::WIRE_LENGTH];
        let mut proof = MacosAuthorizationProof::read_from(&mut bytes.as_slice())
            .expect("fixed-size proof should parse");
        let mut wire = Vec::new();
        proof
            .write_to(&mut wire)
            .expect("first proof transmission should succeed");
        assert_eq!(wire, bytes);
        let error = proof
            .write_to(&mut Vec::new())
            .expect_err("proof transmission must be one-shot");
        assert_eq!(error.kind(), PrivilegeAuthorizationErrorKind::InvalidProof);
    }

    #[test]
    fn proof_wire_form_rejects_short_input() {
        let mut short = [0_u8; MacosAuthorizationProof::WIRE_LENGTH - 1].as_slice();
        let error = match MacosAuthorizationProof::read_from(&mut short) {
            Ok(_) => panic!("short proof must fail closed"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), PrivilegeAuthorizationErrorKind::InvalidProof);
    }

    #[test]
    fn zero_deadline_does_not_open_native_consent() {
        let error = match acquire_macos_authorization_proof(std::time::Duration::ZERO) {
            Ok(_) => panic!("zero deadline must fail before native consent"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), PrivilegeAuthorizationErrorKind::TimedOut);
    }
}

/// Ask polkit to authorize the exact live process retained by `peer`.
///
/// The peer is checked immediately before and after the native call. A caller
/// therefore cannot substitute a self-authored `unix-process` subject, and a
/// process that exits while consent is pending cannot yield authorization.
#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
fn map_peer_error(error: SystemBrokerError, phase: &str) -> PrivilegeAuthorizationError {
    PrivilegeAuthorizationError::new(
        PrivilegeAuthorizationErrorKind::InvalidPeer,
        format!("cannot authenticate system broker peer {phase}: {error}"),
    )
}
