//! Call-scoped robustness controls supplied by an embedder.
//!
//! The probe is borrowed for one synchronous `Executor` call. It is not
//! authority, is never persisted, and must never outlive that call.

use crate::CuError;

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
}
