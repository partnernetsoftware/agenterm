//! Product-neutral identity for one operating-system boot.

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct HostBootIdentity([u8; 32]);

impl HostBootIdentity {
    pub(crate) const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for HostBootIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HostBootIdentity(<opaque>)")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HostBootIdentityErrorKind {
    Query,
    InvalidNativeValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostBootIdentityError {
    kind: HostBootIdentityErrorKind,
    detail: String,
}

impl HostBootIdentityError {
    pub(crate) fn new(kind: HostBootIdentityErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub const fn kind(&self) -> HostBootIdentityErrorKind {
        self.kind
    }
}

impl std::fmt::Display for HostBootIdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "host boot identity {:?}: {}",
            self.kind, self.detail
        )
    }
}

impl std::error::Error for HostBootIdentityError {}
