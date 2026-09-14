//! Call-scoped robustness controls supplied by an embedder.
//!
//! The probe is borrowed for one synchronous `Executor` call. It is not
//! authority, is never persisted, and must never outlive that call.

use std::{
    thread,
    time::{Duration, Instant},
};

use crate::CuError;

pub(crate) const OBSERVE_CANCEL_SLICE: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Default)]
pub struct ExecutionControl<'a> {
    cancelled: Option<&'a dyn Fn() -> bool>,
}

impl<'a> ExecutionControl<'a> {
    pub const fn none() -> Self {
        Self { cancelled: None }
    }

    pub const fn with_cancel_probe(cancelled: &'a dyn Fn() -> bool) -> Self {
        Self {
            cancelled: Some(cancelled),
        }
    }

    pub fn is_cancelled(self) -> bool {
        self.cancelled.is_some_and(|probe| probe())
    }

    pub fn check_observe(self) -> Result<(), CuError> {
        if self.is_cancelled() {
            Err(CuError::new(
                "cancelled",
                "the observe-only operation was cancelled before completion",
            )
            .with_detail(serde_json::json!({
                "effect": "not_performed",
                "phase": "observe_wait",
            })))
        } else {
            Ok(())
        }
    }

    /// Sleeps no later than `deadline`, observing the borrowed token between bounded
    /// slices and once at the boundary.
    ///
    /// This is mechanism only: it publishes no error and does not decide whether a
    /// deadline, an authority reply, or cancellation owns the product outcome. The
    /// caller retains that policy and receives only the pending-signal bit.
    pub(crate) fn sleep_until_cancelled(self, deadline: Instant) -> bool {
        while Instant::now() < deadline {
            if self.is_cancelled() {
                return true;
            }
            thread::sleep(
                OBSERVE_CANCEL_SLICE.min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        self.is_cancelled()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;

    #[test]
    fn an_already_set_token_returns_without_consuming_the_bound() {
        let started = Instant::now();
        let cancelled = ExecutionControl::with_cancel_probe(&|| true)
            .sleep_until_cancelled(started + Duration::from_secs(1));
        assert!(cancelled);
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn a_token_set_during_sleep_is_observed_before_the_deadline() {
        let token = Arc::new(AtomicBool::new(false));
        let raised = Arc::clone(&token);
        let trigger = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            raised.store(true, Ordering::Release);
        });
        let probe = || token.load(Ordering::Acquire);
        let started = Instant::now();
        let cancelled = ExecutionControl::with_cancel_probe(&probe)
            .sleep_until_cancelled(started + Duration::from_secs(1));
        trigger.join().expect("cancel trigger");

        assert!(cancelled);
        assert!(started.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn an_unset_token_sleeps_to_the_requested_boundary() {
        let started = Instant::now();
        let cancelled = ExecutionControl::with_cancel_probe(&|| false)
            .sleep_until_cancelled(started + Duration::from_millis(25));
        assert!(!cancelled);
        assert!(started.elapsed() >= Duration::from_millis(20));
    }

    #[test]
    fn a_past_deadline_returns_promptly_without_sleeping() {
        let started = Instant::now();
        let cancelled = ExecutionControl::with_cancel_probe(&|| false)
            .sleep_until_cancelled(started - Duration::from_millis(1));
        assert!(!cancelled);
        assert!(started.elapsed() < Duration::from_millis(100));
    }
}
