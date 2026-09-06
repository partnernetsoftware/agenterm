//! Product-neutral bounded login-session inventory and lock-delivery types.

pub const LOGIN_SESSION_MAX_ROWS: usize = 64;
pub const LOGIN_SESSION_USERNAME_MAX_BYTES: usize = 256;
pub const LOGIN_SESSION_DISPLAY_NAME_MAX_BYTES: usize = 512;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct LoginSessionIdentity([u8; 32]);

impl LoginSessionIdentity {
    #[cfg(any(target_os = "macos", test))]
    pub(crate) const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for LoginSessionIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LoginSessionIdentity(<opaque>)")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoginSession {
    pub identity: LoginSessionIdentity,
    pub native_uuid: String,
    pub native_session_id: u64,
    pub native_security_session_id: u64,
    pub native_audit_id: u64,
    pub user_id: u32,
    pub group_id: u32,
    pub username: String,
    pub display_name: String,
    pub on_console: bool,
    pub login_complete: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LoginSessionProvider {
    MacosIoRegistry,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoginSessionInventory {
    pub provider: LoginSessionProvider,
    pub locked: bool,
    pub sessions: Vec<LoginSession>,
    pub console_session_index: Option<usize>,
}

impl LoginSessionInventory {
    #[must_use]
    pub fn console_session(&self) -> Option<&LoginSession> {
        self.console_session_index
            .and_then(|index| self.sessions.get(index))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LoginSessionErrorKind {
    Unsupported,
    ProviderUnavailable,
    ProviderShape,
    AmbiguousConsole,
    InputPermissionDenied,
    DeliveryFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoginSessionError {
    kind: LoginSessionErrorKind,
    detail: String,
}

impl LoginSessionError {
    pub(crate) fn new(kind: LoginSessionErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> LoginSessionErrorKind {
        self.kind
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for LoginSessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "login session {:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for LoginSessionError {}
