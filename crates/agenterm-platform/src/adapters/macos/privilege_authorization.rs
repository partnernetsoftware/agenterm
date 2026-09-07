//! macOS Authorization Services consent proof boundary.
//!
//! The interactive client obtains only the fixed right. The root broker later
//! internalizes its opaque 32-byte proof and checks that same right without
//! interaction or extension. This module never executes a privileged program.

use std::{
    ptr,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    time::Duration,
};

use security_framework_sys::authorization;

use crate::privilege_authorization::{
    PRIVILEGE_ACTION_ID, PrivilegeAuthorizationDecision, PrivilegeAuthorizationError,
    PrivilegeAuthorizationErrorKind, PrivilegeAuthorizationResult,
};

pub(super) const EXTERNAL_FORM_LENGTH: usize = authorization::kAuthorizationExternalFormLength;
const _: [(); 32] = [(); EXTERNAL_FORM_LENGTH];

type WorkerReply = PrivilegeAuthorizationResult<([u8; EXTERNAL_FORM_LENGTH], Sender<()>)>;

struct AuthorizationOwner(authorization::AuthorizationRef);

// Authorization Services owns the opaque reference. It never leaves the
// consent worker that created it or the broker thread that internalized it.
impl Drop for AuthorizationOwner {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this owner holds the sole live reference and requests
            // destruction of all rights before releasing it.
            let _ = unsafe {
                authorization::AuthorizationFree(
                    self.0,
                    authorization::kAuthorizationFlagDestroyRights,
                )
            };
            self.0 = ptr::null_mut();
        }
    }
}

impl AuthorizationOwner {
    fn destroy(mut self) -> PrivilegeAuthorizationResult<()> {
        // SAFETY: this owner holds the sole live reference.
        let status = unsafe {
            authorization::AuthorizationFree(self.0, authorization::kAuthorizationFlagDestroyRights)
        };
        self.0 = ptr::null_mut();
        if status == authorization::errAuthorizationSuccess {
            Ok(())
        } else {
            Err(native_error(
                PrivilegeAuthorizationErrorKind::InvalidNativeResult,
                "AuthorizationFree(DestroyRights) failed",
                status,
            ))
        }
    }
}

struct ExternalFormGuard(authorization::AuthorizationExternalForm);

impl Drop for ExternalFormGuard {
    fn drop(&mut self) {
        for byte in &mut self.0.bytes {
            // SAFETY: each pointer comes from this live exclusive array.
            unsafe { ptr::write_volatile(byte, 0) };
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

pub(super) fn acquire(
    timeout: Duration,
) -> PrivilegeAuthorizationResult<([u8; EXTERNAL_FORM_LENGTH], Sender<()>)> {
    if timeout.is_zero() {
        return Err(PrivilegeAuthorizationError::new(
            PrivilegeAuthorizationErrorKind::TimedOut,
            "macOS authorization deadline must be greater than zero",
        ));
    }
    // SAFETY: geteuid takes no arguments and has no failure contract.
    if unsafe { libc::geteuid() } == 0 {
        return Err(PrivilegeAuthorizationError::new(
            PrivilegeAuthorizationErrorKind::InvalidPeer,
            "macOS authorization proof must be requested by an ordinary user",
        ));
    }

    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("agenterm-mac-consent".into())
        .spawn(move || consent_worker(reply_tx))
        .map_err(|error| {
            PrivilegeAuthorizationError::new(
                PrivilegeAuthorizationErrorKind::TransportFailed,
                format!("cannot start the macOS authorization worker: {error}"),
            )
        })?;
    receive_worker_reply(&reply_rx, timeout)
}

fn consent_worker(reply: mpsc::SyncSender<WorkerReply>) {
    match acquire_on_current_thread() {
        Ok((bytes, authorization)) => {
            let (cleanup_tx, cleanup_rx) = mpsc::channel();
            if reply.send(Ok((bytes, cleanup_tx))).is_ok() {
                // The proof owner signals after the broker reply or on local
                // failure. A disconnected owner has the same cleanup effect.
                let _ = cleanup_rx.recv();
            }
            drop(authorization);
        }
        Err(error) => {
            let _ = reply.send(Err(error));
        }
    }
}

fn receive_worker_reply(
    receiver: &Receiver<WorkerReply>,
    timeout: Duration,
) -> PrivilegeAuthorizationResult<([u8; EXTERNAL_FORM_LENGTH], Sender<()>)> {
    match receiver.recv_timeout(timeout) {
        Ok(reply) => reply,
        Err(RecvTimeoutError::Timeout) => Err(PrivilegeAuthorizationError::new(
            PrivilegeAuthorizationErrorKind::TimedOut,
            "macOS Authorization Services exceeded its deadline",
        )),
        Err(RecvTimeoutError::Disconnected) => Err(PrivilegeAuthorizationError::new(
            PrivilegeAuthorizationErrorKind::TransportFailed,
            "macOS authorization worker exited without a result",
        )),
    }
}

fn acquire_on_current_thread()
-> PrivilegeAuthorizationResult<([u8; EXTERNAL_FORM_LENGTH], AuthorizationOwner)> {
    let mut reference = ptr::null_mut();
    // SAFETY: output is valid writable storage; no rights/environment means
    // this call itself performs no interaction.
    let status = unsafe {
        authorization::AuthorizationCreate(
            ptr::null(),
            ptr::null(),
            authorization::kAuthorizationFlagDefaults,
            &raw mut reference,
        )
    };
    if status != authorization::errAuthorizationSuccess || reference.is_null() {
        return Err(map_client_status(status, "AuthorizationCreate"));
    }
    let owner = AuthorizationOwner(reference);
    let mut right = fixed_right();
    let rights = authorization::AuthorizationRights {
        count: 1,
        items: &raw mut right,
    };
    let flags = authorization::kAuthorizationFlagInteractionAllowed
        | authorization::kAuthorizationFlagExtendRights
        | authorization::kAuthorizationFlagPreAuthorize;
    // SAFETY: owner is live; rights points to one fixed, pointer-free item;
    // authorizedRights is null because only the status is consumed.
    let status = unsafe {
        authorization::AuthorizationCopyRights(
            owner.0,
            &raw const rights,
            ptr::null(),
            flags,
            ptr::null_mut(),
        )
    };
    if status != authorization::errAuthorizationSuccess {
        return Err(map_client_status(status, "AuthorizationCopyRights"));
    }
    let mut form = ExternalFormGuard(authorization::AuthorizationExternalForm {
        bytes: [0; EXTERNAL_FORM_LENGTH],
    });
    // SAFETY: owner is live and form is complete writable storage.
    let status = unsafe { authorization::AuthorizationMakeExternalForm(owner.0, &raw mut form.0) };
    if status != authorization::errAuthorizationSuccess {
        return Err(map_client_status(status, "AuthorizationMakeExternalForm"));
    }
    let bytes = form.0.bytes.map(|byte| byte.to_ne_bytes()[0]);
    Ok((bytes, owner))
}

pub(super) fn verify(
    bytes: &mut [u8; EXTERNAL_FORM_LENGTH],
) -> PrivilegeAuthorizationResult<PrivilegeAuthorizationDecision> {
    verify_with_root_requirement(bytes, true)
}

fn verify_with_root_requirement(
    bytes: &mut [u8; EXTERNAL_FORM_LENGTH],
    require_root: bool,
) -> PrivilegeAuthorizationResult<PrivilegeAuthorizationDecision> {
    // SAFETY: geteuid takes no arguments and has no failure contract.
    if require_root && unsafe { libc::geteuid() } != 0 {
        return Err(PrivilegeAuthorizationError::new(
            PrivilegeAuthorizationErrorKind::InvalidPeer,
            "macOS authorization proof can only be verified by the root broker",
        ));
    }
    let form = ExternalFormGuard(authorization::AuthorizationExternalForm {
        bytes: bytes.map(|byte| libc::c_char::from_ne_bytes([byte])),
    });
    wipe(bytes);
    let mut reference = ptr::null_mut();
    // SAFETY: form has the exact public fixed layout and output is writable.
    let status = unsafe {
        authorization::AuthorizationCreateFromExternalForm(&raw const form.0, &raw mut reference)
    };
    if status != authorization::errAuthorizationSuccess || reference.is_null() {
        return Err(map_proof_status(
            status,
            "AuthorizationCreateFromExternalForm",
        ));
    }
    let owner = AuthorizationOwner(reference);
    let mut right = fixed_right();
    let rights = authorization::AuthorizationRights {
        count: 1,
        items: &raw mut right,
    };
    // No interaction, no right extension and no caller-provided environment.
    // SAFETY: owner is live and rights points to one fixed item.
    let status = unsafe {
        authorization::AuthorizationCopyRights(
            owner.0,
            &raw const rights,
            ptr::null(),
            authorization::kAuthorizationFlagDefaults,
            ptr::null_mut(),
        )
    };
    if status != authorization::errAuthorizationSuccess {
        return Err(map_broker_status(status));
    }
    owner.destroy()?;
    Ok(PrivilegeAuthorizationDecision::Authorized)
}

fn fixed_right() -> authorization::AuthorizationItem {
    let name = c"com.partnernetsoftware.agenterm.cu.privilege.process-signal";
    debug_assert_eq!(name.to_bytes(), PRIVILEGE_ACTION_ID.as_bytes());
    authorization::AuthorizationItem {
        name: name.as_ptr(),
        valueLength: 0,
        value: ptr::null_mut(),
        flags: 0,
    }
}

fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        // SAFETY: each pointer comes from this live exclusive slice.
        unsafe { ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

fn map_client_status(status: i32, phase: &str) -> PrivilegeAuthorizationError {
    let kind = match status {
        authorization::errAuthorizationCanceled => {
            PrivilegeAuthorizationErrorKind::AuthorizationCanceled
        }
        authorization::errAuthorizationDenied
        | authorization::errAuthorizationInteractionNotAllowed => {
            PrivilegeAuthorizationErrorKind::NotAuthorized
        }
        authorization::errAuthorizationExternalizeNotAllowed => {
            PrivilegeAuthorizationErrorKind::Unsupported
        }
        authorization::errAuthorizationInvalidSet
        | authorization::errAuthorizationInvalidRef
        | authorization::errAuthorizationInvalidPointer
        | authorization::errAuthorizationInvalidFlags => {
            PrivilegeAuthorizationErrorKind::InvalidNativeResult
        }
        _ => PrivilegeAuthorizationErrorKind::TransportFailed,
    };
    native_error(kind, phase, status)
}

fn map_proof_status(status: i32, phase: &str) -> PrivilegeAuthorizationError {
    let kind = match status {
        authorization::errAuthorizationInternalizeNotAllowed
        | authorization::errAuthorizationInternal
        | authorization::errAuthorizationDenied
        | authorization::errAuthorizationInvalidRef
        | authorization::errAuthorizationInvalidPointer
        | authorization::errAuthorizationInvalidSet
        | authorization::errAuthorizationInvalidFlags => {
            PrivilegeAuthorizationErrorKind::InvalidProof
        }
        _ => PrivilegeAuthorizationErrorKind::TransportFailed,
    };
    native_error(kind, phase, status)
}

fn map_broker_status(status: i32) -> PrivilegeAuthorizationError {
    let kind = match status {
        authorization::errAuthorizationCanceled => {
            PrivilegeAuthorizationErrorKind::AuthorizationCanceled
        }
        authorization::errAuthorizationDenied
        | authorization::errAuthorizationInteractionNotAllowed => {
            PrivilegeAuthorizationErrorKind::NotAuthorized
        }
        authorization::errAuthorizationInvalidRef
        | authorization::errAuthorizationInvalidSet
        | authorization::errAuthorizationInvalidPointer
        | authorization::errAuthorizationInvalidFlags => {
            PrivilegeAuthorizationErrorKind::InvalidProof
        }
        _ => PrivilegeAuthorizationErrorKind::TransportFailed,
    };
    native_error(kind, "broker AuthorizationCopyRights", status)
}

fn native_error(
    kind: PrivilegeAuthorizationErrorKind,
    phase: &str,
    status: i32,
) -> PrivilegeAuthorizationError {
    PrivilegeAuthorizationError::new(kind, format!("{phase} failed with OSStatus {status}"))
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use super::{
        EXTERNAL_FORM_LENGTH, map_client_status, receive_worker_reply, verify_with_root_requirement,
    };
    use crate::privilege_authorization::PrivilegeAuthorizationErrorKind;
    use security_framework_sys::authorization;

    #[test]
    fn authorization_statuses_remain_typed() {
        assert_eq!(
            map_client_status(authorization::errAuthorizationCanceled, "test").kind(),
            PrivilegeAuthorizationErrorKind::AuthorizationCanceled
        );
        assert_eq!(
            map_client_status(authorization::errAuthorizationDenied, "test").kind(),
            PrivilegeAuthorizationErrorKind::NotAuthorized
        );
        assert_eq!(
            map_client_status(authorization::errAuthorizationInteractionNotAllowed, "test").kind(),
            PrivilegeAuthorizationErrorKind::NotAuthorized
        );
    }

    #[test]
    fn bounded_worker_wait_reports_timeout_without_native_interaction() {
        let (_sender, receiver) = mpsc::sync_channel(1);
        let error = receive_worker_reply(&receiver, Duration::from_millis(1))
            .expect_err("empty live worker must time out");
        assert_eq!(error.kind(), PrivilegeAuthorizationErrorKind::TimedOut);
    }

    #[test]
    fn invalid_external_form_is_refused_without_interaction_and_cleared() {
        let mut bytes = [0_u8; EXTERNAL_FORM_LENGTH];
        let error = verify_with_root_requirement(&mut bytes, false)
            .expect_err("zero external form must be invalid");
        assert_eq!(
            error.kind(),
            PrivilegeAuthorizationErrorKind::InvalidProof,
            "{}",
            error.message()
        );
        assert_eq!(bytes, [0_u8; EXTERNAL_FORM_LENGTH]);
    }
}
