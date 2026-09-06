//! macOS proof of the current effective user's completed console login session.

use std::{fs, os::unix::fs::MetadataExt as _, path::Path};

use crate::{
    CapabilityStatus,
    contract::current_target_binding::{CurrentTargetBindingError, CurrentTargetBindingErrorKind},
    login_session::{LoginSessionError, LoginSessionErrorKind, LoginSessionInventory},
};

pub(crate) struct NativeCurrentSessionFacts(Vec<u8>);

impl NativeCurrentSessionFacts {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

pub(crate) fn capability_status() -> CapabilityStatus {
    CapabilityStatus::Available
}

pub(crate) fn current_session_facts() -> Result<NativeCurrentSessionFacts, CurrentTargetBindingError>
{
    let identity = crate::user_identity::current_user_identity().map_err(|_| {
        native(
            "effective-user-unavailable",
            "the current effective user could not be determined",
        )
    })?;
    let credentials = identity.posix_credentials().ok_or_else(|| {
        native(
            "effective-user-unavailable",
            "the current identity is not a POSIX user",
        )
    })?;
    let inventory = crate::login_session::inventory().map_err(map_inventory_error)?;
    session_facts_from_inventory(credentials.effective_user_id, &inventory)
}

fn session_facts_from_inventory(
    effective_user_id: u32,
    inventory: &LoginSessionInventory,
) -> Result<NativeCurrentSessionFacts, CurrentTargetBindingError> {
    let session = inventory.console_session().ok_or_else(|| {
        unsupported(
            "console-session-missing",
            "macOS reports no current console login session",
        )
    })?;
    if !session.on_console || !session.login_complete {
        return Err(unsupported(
            "console-session-incomplete",
            "the current console session has not completed login",
        ));
    }
    if session.user_id != effective_user_id {
        return Err(unsupported(
            "console-session-user-mismatch",
            "the current effective user does not own the console session",
        ));
    }

    // Do not retain the native UUID, username, or display name. The login-session
    // facade has already reduced all native session identifiers to this opaque,
    // domain-separated identity. Numeric fields make native identity transitions
    // explicit while the euid binds the proof to this process.
    let mut facts = Vec::with_capacity(80);
    push_bytes(&mut facts, 1, session.identity.as_bytes());
    push_bytes(&mut facts, 2, &effective_user_id.to_le_bytes());
    push_bytes(&mut facts, 3, &session.native_session_id.to_le_bytes());
    push_bytes(
        &mut facts,
        4,
        &session.native_security_session_id.to_le_bytes(),
    );
    push_bytes(&mut facts, 5, &session.native_audit_id.to_le_bytes());
    Ok(NativeCurrentSessionFacts(facts))
}

pub(crate) fn validate_private_key_file(path: &Path) -> Result<(), CurrentTargetBindingError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        permission(
            "install-key-metadata-unavailable",
            "installation key metadata could not be verified",
        )
    })?;
    let identity = crate::user_identity::current_user_identity().map_err(|_| {
        native(
            "install-key-owner-unavailable",
            "the current effective user could not be determined",
        )
    })?;
    let credentials = identity.posix_credentials().ok_or_else(|| {
        native(
            "install-key-owner-unavailable",
            "the current identity is not a POSIX user",
        )
    })?;
    if !metadata.file_type().is_file()
        || metadata.uid() != credentials.effective_user_id
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(permission(
            "install-key-permissions",
            "installation key must be a singly linked regular file owned by the current effective user with mode 0600",
        ));
    }
    Ok(())
}

fn push_bytes(output: &mut Vec<u8>, tag: u8, value: &[u8]) {
    output.push(tag);
    output.extend_from_slice(&(value.len() as u32).to_le_bytes());
    output.extend_from_slice(value);
}

fn map_inventory_error(error: LoginSessionError) -> CurrentTargetBindingError {
    match error.kind() {
        LoginSessionErrorKind::Unsupported => unsupported(
            "login-session-unsupported",
            "macOS login-session inventory is unavailable",
        ),
        LoginSessionErrorKind::ProviderUnavailable => native(
            "login-session-provider-unavailable",
            "macOS login-session inventory could not be read",
        ),
        LoginSessionErrorKind::ProviderShape | LoginSessionErrorKind::AmbiguousConsole => native(
            "login-session-provider-invalid",
            "macOS login-session inventory did not provide an unambiguous native session",
        ),
        LoginSessionErrorKind::InputPermissionDenied | LoginSessionErrorKind::DeliveryFailed => {
            native(
                "login-session-provider-invalid",
                "macOS login-session inventory returned an unexpected failure",
            )
        }
    }
}

fn unsupported(code: &'static str, message: &'static str) -> CurrentTargetBindingError {
    CurrentTargetBindingError::new(CurrentTargetBindingErrorKind::Unsupported, code, message)
}

fn native(code: &'static str, message: &'static str) -> CurrentTargetBindingError {
    CurrentTargetBindingError::new(CurrentTargetBindingErrorKind::Native, code, message)
}

fn permission(code: &'static str, message: &'static str) -> CurrentTargetBindingError {
    CurrentTargetBindingError::new(CurrentTargetBindingErrorKind::Permission, code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::login_session::{LoginSession, LoginSessionIdentity, LoginSessionProvider};

    fn inventory(user_id: u32, login_complete: bool) -> LoginSessionInventory {
        LoginSessionInventory {
            provider: LoginSessionProvider::MacosIoRegistry,
            locked: false,
            sessions: vec![LoginSession {
                identity: LoginSessionIdentity::new([0x5a; 32]),
                native_uuid: "771048D1-EB47-4706-B698-A213D09B0F72".into(),
                native_session_id: 257,
                native_security_session_id: 100_002,
                native_audit_id: 100_003,
                user_id,
                group_id: 20,
                username: "private-name".into(),
                display_name: "Private Display Name".into(),
                on_console: true,
                login_complete,
            }],
            console_session_index: Some(0),
        }
    }

    #[test]
    fn facts_are_stable_and_exclude_plaintext_identity() {
        let first = session_facts_from_inventory(501, &inventory(501, true)).unwrap();
        let second = session_facts_from_inventory(501, &inventory(501, true)).unwrap();
        assert_eq!(first.as_bytes(), second.as_bytes());
        assert!(!first.as_bytes().is_empty());
        assert!(
            !first
                .as_bytes()
                .windows(b"private-name".len())
                .any(|window| window == b"private-name")
        );
        assert!(
            !first
                .as_bytes()
                .windows(36)
                .any(|window| window == b"771048D1-EB47-4706-B698-A213D09B0F72")
        );
    }

    #[test]
    fn facts_require_completed_console_session_owned_by_euid() {
        let mismatch = session_facts_from_inventory(502, &inventory(501, true))
            .err()
            .unwrap();
        assert_eq!(mismatch.code(), "console-session-user-mismatch");

        let incomplete = session_facts_from_inventory(501, &inventory(501, false))
            .err()
            .unwrap();
        assert_eq!(incomplete.code(), "console-session-incomplete");

        let mut missing = inventory(501, true);
        missing.console_session_index = None;
        let missing = session_facts_from_inventory(501, &missing).err().unwrap();
        assert_eq!(missing.code(), "console-session-missing");
    }

    #[test]
    fn native_inventory_proves_a_session_or_fails_closed() {
        assert_eq!(capability_status(), CapabilityStatus::Available);
        match current_session_facts() {
            Ok(facts) => assert!(!facts.as_bytes().is_empty()),
            Err(error) => assert!(matches!(
                error.kind(),
                CurrentTargetBindingErrorKind::Unsupported | CurrentTargetBindingErrorKind::Native
            )),
        }
    }
}
