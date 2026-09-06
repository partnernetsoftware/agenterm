//! Bounded current-host login-session inventory and shell-free lock delivery.
//!
//! Approval, expiry, same-user policy, durable receipts and postcondition
//! verification belong to the caller. This facade exposes native mechanism
//! only; in particular, [`lock_console`] reports event delivery, not a verified
//! transition to the locked state.

#[cfg(any(target_os = "macos", test))]
use sha2::{Digest as _, Sha256};

pub use crate::contract::login_session::{
    LOGIN_SESSION_DISPLAY_NAME_MAX_BYTES, LOGIN_SESSION_MAX_ROWS, LOGIN_SESSION_USERNAME_MAX_BYTES,
    LoginSession, LoginSessionError, LoginSessionErrorKind, LoginSessionIdentity,
    LoginSessionInventory, LoginSessionProvider,
};

#[cfg(any(target_os = "macos", test))]
const IDENTITY_DOMAIN: &[u8] = b"agenterm-platform/login-session-identity/v1\0";

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Debug)]
pub(crate) struct NativeLoginSessionRow {
    pub(crate) uuid: String,
    pub(crate) session_id: u64,
    pub(crate) security_session_id: u64,
    pub(crate) audit_id: u64,
    pub(crate) user_id: u64,
    pub(crate) group_id: u64,
    pub(crate) username: String,
    pub(crate) display_name: String,
    pub(crate) on_console: bool,
    pub(crate) login_complete: bool,
}

#[must_use]
pub fn capability_status() -> crate::CapabilityStatus {
    crate::capability_status(crate::Capability::LoginSession)
}

pub fn inventory() -> Result<LoginSessionInventory, LoginSessionError> {
    crate::selected::login_session::inventory()
}

/// Deliver the host's standard lock-screen chord without requesting a new
/// input-monitoring permission prompt.
///
/// Success only means that native event posting was admitted. The caller must
/// read a fresh inventory and decide whether the requested lock occurred.
pub fn lock_console() -> Result<(), LoginSessionError> {
    crate::selected::login_session::lock_console()
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn finish_inventory(
    locked: bool,
    rows: Vec<NativeLoginSessionRow>,
) -> Result<LoginSessionInventory, LoginSessionError> {
    if rows.len() > LOGIN_SESSION_MAX_ROWS {
        return Err(shape("native inventory exceeds the session row ceiling"));
    }
    let mut sessions = Vec::new();
    sessions
        .try_reserve(rows.len())
        .map_err(|_| shape("session inventory allocation failed"))?;
    for row in rows {
        validate_uuid(&row.uuid)?;
        validate_text(
            "username",
            &row.username,
            1,
            LOGIN_SESSION_USERNAME_MAX_BYTES,
        )?;
        validate_text(
            "display name",
            &row.display_name,
            0,
            LOGIN_SESSION_DISPLAY_NAME_MAX_BYTES,
        )?;
        let user_id = u32::try_from(row.user_id)
            .map_err(|_| shape("native user id exceeds the portable range"))?;
        let group_id = u32::try_from(row.group_id)
            .map_err(|_| shape("native group id exceeds the portable range"))?;
        let native_uuid = row.uuid.to_ascii_uppercase();
        let identity = session_identity(
            &native_uuid,
            row.session_id,
            row.security_session_id,
            row.audit_id,
            user_id,
        );
        sessions.push(LoginSession {
            identity,
            native_uuid,
            native_session_id: row.session_id,
            native_security_session_id: row.security_session_id,
            native_audit_id: row.audit_id,
            user_id,
            group_id,
            username: row.username,
            display_name: row.display_name,
            on_console: row.on_console,
            login_complete: row.login_complete,
        });
    }
    sessions.sort_by_key(|session| session.native_session_id);
    for adjacent in sessions.windows(2) {
        if adjacent[0].native_session_id == adjacent[1].native_session_id
            || adjacent[0].identity == adjacent[1].identity
        {
            return Err(shape(
                "native inventory contains duplicate session identity",
            ));
        }
    }
    let console_sessions: Vec<usize> = sessions
        .iter()
        .enumerate()
        .filter_map(|(index, session)| session.on_console.then_some(index))
        .collect();
    if console_sessions.len() > 1 {
        return Err(LoginSessionError::new(
            LoginSessionErrorKind::AmbiguousConsole,
            "native inventory reports more than one console session",
        ));
    }
    Ok(LoginSessionInventory {
        provider: LoginSessionProvider::MacosIoRegistry,
        locked,
        sessions,
        console_session_index: console_sessions.first().copied(),
    })
}

#[cfg(any(target_os = "macos", test))]
fn validate_text(
    field: &'static str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), LoginSessionError> {
    if value.len() < minimum
        || value.len() > maximum
        || value
            .as_bytes()
            .iter()
            .any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
    {
        return Err(shape(format!(
            "native {field} must be {minimum}..={maximum} UTF-8 bytes without NUL or newline"
        )));
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn validate_uuid(uuid: &str) -> Result<(), LoginSessionError> {
    if uuid.len() != 36
        || !uuid.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
    {
        return Err(shape("native session UUID is malformed"));
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn session_identity(
    uuid: &str,
    session_id: u64,
    security_session_id: u64,
    audit_id: u64,
    user_id: u32,
) -> LoginSessionIdentity {
    let mut digest = Sha256::new();
    digest.update(IDENTITY_DOMAIN);
    digest.update((uuid.len() as u64).to_le_bytes());
    digest.update(uuid.as_bytes());
    digest.update(session_id.to_le_bytes());
    digest.update(security_session_id.to_le_bytes());
    digest.update(audit_id.to_le_bytes());
    digest.update(user_id.to_le_bytes());
    LoginSessionIdentity::new(digest.finalize().into())
}

#[cfg(any(target_os = "macos", test))]
fn shape(detail: impl Into<String>) -> LoginSessionError {
    LoginSessionError::new(LoginSessionErrorKind::ProviderShape, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(session_id: u64) -> NativeLoginSessionRow {
        NativeLoginSessionRow {
            uuid: "771048D1-EB47-4706-B698-A213D09B0F72".into(),
            session_id,
            security_session_id: 100_002,
            audit_id: 100_002,
            user_id: 501,
            group_id: 20,
            username: "fixture".into(),
            display_name: "Fixture User".into(),
            on_console: true,
            login_complete: true,
        }
    }

    #[test]
    fn rows_are_bounded_sorted_and_have_domain_separated_opaque_identity() {
        let mut later = row(300);
        later.on_console = false;
        later.uuid = "AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE".into();
        let inventory = finish_inventory(false, vec![later, row(257)]).unwrap();
        assert_eq!(inventory.sessions[0].native_session_id, 257);
        assert_eq!(inventory.console_session().unwrap().username, "fixture");
        assert_ne!(
            inventory.sessions[0].identity,
            inventory.sessions[1].identity
        );
        assert_eq!(inventory.sessions[0].identity.as_bytes().len(), 32);
        assert_eq!(
            format!("{:?}", inventory.sessions[0].identity),
            "LoginSessionIdentity(<opaque>)"
        );
    }

    #[test]
    fn malformed_rows_and_ambiguous_console_fail_closed() {
        let mut malformed = row(1);
        malformed.uuid = "not-a-uuid".into();
        assert_eq!(
            finish_inventory(false, vec![malformed]).unwrap_err().kind(),
            LoginSessionErrorKind::ProviderShape
        );
        let mut second = row(2);
        second.uuid = "AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE".into();
        assert_eq!(
            finish_inventory(false, vec![row(1), second])
                .unwrap_err()
                .kind(),
            LoginSessionErrorKind::AmbiguousConsole
        );
    }

    #[test]
    fn text_and_count_limits_are_strict() {
        let mut bad_name = row(1);
        bad_name.username = "bad\nname".into();
        assert!(finish_inventory(false, vec![bad_name]).is_err());
        let rows = (0..=LOGIN_SESSION_MAX_ROWS)
            .map(|index| {
                let mut value = row(index as u64);
                value.on_console = false;
                value
            })
            .collect();
        assert_eq!(
            finish_inventory(false, rows).unwrap_err().kind(),
            LoginSessionErrorKind::ProviderShape
        );

        let mut oversized = row(2);
        oversized.username = "u".repeat(LOGIN_SESSION_USERNAME_MAX_BYTES + 1);
        assert_eq!(
            finish_inventory(false, vec![oversized]).unwrap_err().kind(),
            LoginSessionErrorKind::ProviderShape
        );

        let mut out_of_range = row(3);
        out_of_range.user_id = u64::from(u32::MAX) + 1;
        assert_eq!(
            finish_inventory(false, vec![out_of_range])
                .unwrap_err()
                .kind(),
            LoginSessionErrorKind::ProviderShape
        );

        let mut duplicate = row(4);
        duplicate.on_console = false;
        assert_eq!(
            finish_inventory(false, vec![row(4), duplicate])
                .unwrap_err()
                .kind(),
            LoginSessionErrorKind::ProviderShape
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_inventory_is_read_only_and_coherent() {
        assert_eq!(capability_status(), crate::CapabilityStatus::Available);
        let inventory = inventory().expect("read native login-session inventory");
        assert!(inventory.sessions.len() <= LOGIN_SESSION_MAX_ROWS);
        assert!(
            inventory
                .console_session_index
                .is_none_or(|index| index < inventory.sessions.len())
        );
    }
}
