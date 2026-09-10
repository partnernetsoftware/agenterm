//! Lightweight single-process liveness and start-identity observation.

pub use crate::contract::process_observation::{IdentityVerdict, ProcessObservation};

/// Observe one process without claiming ownership or changing its state.
///
/// `Unknown` is fail-closed evidence: callers must not infer that a process is
/// dead from permission errors, parse failures, or incomplete native queries.
pub fn observe(pid: u32) -> ProcessObservation {
    crate::selected::process_observation::observe(pid)
}

/// Classify one already-observed process against an exact frozen identity.
///
/// This is pure so identity-sensitive product code can exhaustively test every
/// observation state without depending on a live native process.
pub fn classify_identity(
    observation: &ProcessObservation,
    expected_start_identity: &str,
) -> IdentityVerdict {
    match observation {
        ProcessObservation::Live {
            start_identity: Some(actual),
        } if actual == expected_start_identity => IdentityVerdict::Live,
        ProcessObservation::Live {
            start_identity: Some(_),
        } => IdentityVerdict::PidReused,
        ProcessObservation::Live {
            start_identity: None,
        } => IdentityVerdict::IdentityUnavailable,
        ProcessObservation::Dead { .. } => IdentityVerdict::Dead,
        ProcessObservation::Unknown { .. } => IdentityVerdict::Unobservable,
    }
}

/// Observe and classify one PID against an exact frozen identity.
pub fn verify_identity(pid: u32, expected_start_identity: &str) -> IdentityVerdict {
    classify_identity(&observe(pid), expected_start_identity)
}

/// Read the start identity of a process that the caller already knows it may
/// accept or reject as one operation. Do not use this lossy convenience for
/// liveness, absence, ownership, or cleanup decisions; use `verify_identity`
/// and exhaustively handle every `IdentityVerdict` instead.
pub fn start_identity(pid: u32) -> Result<String, String> {
    match observe(pid) {
        ProcessObservation::Live {
            start_identity: Some(identity),
        } => Ok(identity),
        ProcessObservation::Live {
            start_identity: None,
        } => Err("process is live but its start identity is unavailable".to_owned()),
        ProcessObservation::Dead { reason } | ProcessObservation::Unknown { reason } => Err(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_live_with_a_start_identity() {
        assert!(matches!(
            observe(std::process::id()),
            ProcessObservation::Live {
                start_identity: Some(identity)
            } if !identity.is_empty()
        ));
    }

    #[test]
    fn portable_missing_pid_is_dead() {
        assert!(matches!(
            observe(i32::MAX as u32),
            ProcessObservation::Dead { .. }
        ));
    }

    #[test]
    fn identity_classification_preserves_every_fail_closed_state() {
        assert_eq!(
            classify_identity(
                &ProcessObservation::Live {
                    start_identity: Some("expected".to_owned()),
                },
                "expected",
            ),
            IdentityVerdict::Live
        );
        assert_eq!(
            classify_identity(
                &ProcessObservation::Live {
                    start_identity: Some("other".to_owned()),
                },
                "expected",
            ),
            IdentityVerdict::PidReused
        );
        assert_eq!(
            classify_identity(
                &ProcessObservation::Live {
                    start_identity: None,
                },
                "expected",
            ),
            IdentityVerdict::IdentityUnavailable
        );
        assert_eq!(
            classify_identity(
                &ProcessObservation::Dead {
                    reason: "missing".to_owned(),
                },
                "expected",
            ),
            IdentityVerdict::Dead
        );
        assert_eq!(
            classify_identity(
                &ProcessObservation::Unknown {
                    reason: "access-denied".to_owned(),
                },
                "expected",
            ),
            IdentityVerdict::Unobservable
        );
    }
}
