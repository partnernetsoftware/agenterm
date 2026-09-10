//! Product-neutral single-process liveness and start-identity facts.

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProcessObservation {
    Live { start_identity: Option<String> },
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
