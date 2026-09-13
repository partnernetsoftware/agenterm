//! Product-neutral single-process liveness and start-identity facts.

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProcessObservation {
    Live { start_identity: Option<String> },
    Dead { reason: String },
    Unknown { reason: String },
}

/// The parent relationship observed for one exact process id.
///
/// A missing relationship is distinct from an unreadable one so callers do
/// not turn incomplete native evidence into a false process-tree claim.
/// `Live` means that the native record supplied a parent id; it does not prove
/// that the process is still running. Use [`ProcessObservation`] for liveness.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ParentProcessObservation {
    Live { parent_id: u32 },
    Dead { reason: String },
    Unknown { reason: String },
}

/// Exact relationship between one observation and a previously frozen process
/// identity.
///
/// Only `Dead` and `PidReused` prove that the frozen process is absent.
/// `IdentityUnavailable` and `Unobservable` are deliberately separate so a
/// caller cannot turn missing evidence into an absence claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityVerdict {
    Live,
    PidReused,
    Dead,
    IdentityUnavailable,
    Unobservable,
}
