//! Lightweight resource observation without process inventory or ownership APIs.

pub use crate::contract::process_metrics::{
    PageFaultCounters, ProcessBackgroundPolicy, ProcessMetrics, ProcessMetricsError,
    ProcessMetricsErrorKind,
};

/// Read cumulative CPU time, resident memory and page faults for one host process.
///
/// The caller owns PID selection, aggregation, sampling intervals and policy.
pub fn metrics(pid: u32) -> Result<ProcessMetrics, ProcessMetricsError> {
    crate::selected::process_metrics::metrics(pid)
}

/// Read the host's Unix nice value for one process when that scheduling model
/// exists. Windows reports [`ProcessMetricsErrorKind::Unsupported`] rather
/// than inventing a lossy mapping from priority classes.
pub fn nice(pid: u32) -> Result<i32, ProcessMetricsError> {
    crate::selected::process_metrics::nice(pid)
}

/// Reports whether one process is stopped by the host scheduler. This is a
/// point observation used to verify suspend/resume; it is not process identity.
pub fn is_stopped(pid: u32) -> Result<bool, ProcessMetricsError> {
    crate::selected::process_metrics::is_stopped(pid)
}

/// Read the native process-background policy when the host exposes the exact
/// Darwin flag model. No mutation or cross-platform semantic substitution is
/// performed.
pub fn background_policy(pid: u32) -> Result<ProcessBackgroundPolicy, ProcessMetricsError> {
    crate::selected::process_metrics::background_policy(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observes_the_current_process() {
        let sample = metrics(std::process::id()).expect("observe current process");
        assert!(sample.resident_bytes > 0);
        assert!(sample.page_faults.total > 0);
        #[cfg(windows)]
        assert_eq!(
            (sample.page_faults.soft, sample.page_faults.hard),
            (None, None)
        );
        #[cfg(target_os = "linux")]
        assert!(sample.page_faults.soft.is_some() && sample.page_faults.hard.is_some());
        #[cfg(target_os = "macos")]
        assert!(sample.page_faults.soft.is_none() && sample.page_faults.hard.is_some());
    }

    #[test]
    fn page_fault_counter_advances_when_new_pages_are_touched() {
        let before = metrics(std::process::id()).expect("sample before page touches");
        let mut pages = vec![0_u8; 16 * 1024 * 1024];
        for page in pages.chunks_mut(4096) {
            // Volatile access forces physical backing without relying on optimizer behavior.
            unsafe { page.as_mut_ptr().write_volatile(1) };
        }
        std::hint::black_box(&pages);
        let after = metrics(std::process::id()).expect("sample after page touches");
        let delta = after
            .page_faults
            .checked_delta_since(before.page_faults)
            .expect("short-lived cumulative counter must not wrap");
        assert!(
            delta.total > 0,
            "touching new pages produced no page faults"
        );
    }

    #[test]
    fn zero_is_not_a_single_process() {
        let error = metrics(0).expect_err("reject PID zero");
        assert_eq!(error.kind(), ProcessMetricsErrorKind::InvalidId);
    }

    #[test]
    fn observes_current_process_priority_or_types_the_host_model() {
        let observed = nice(std::process::id());
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert!((-20..=20).contains(&observed.expect("observe current nice value")));
        #[cfg(windows)]
        assert_eq!(
            observed.expect_err("Windows has no Unix nice model").kind(),
            ProcessMetricsErrorKind::Unsupported
        );
    }

    #[test]
    fn current_process_is_not_reported_stopped_or_types_the_host_model() {
        let observed = is_stopped(std::process::id());
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert!(!observed.expect("observe current run state"));
        #[cfg(windows)]
        assert_eq!(
            observed
                .expect_err("Windows state model is not implemented")
                .kind(),
            ProcessMetricsErrorKind::Unsupported
        );
    }

    #[test]
    fn current_background_policy_is_native_or_typed_not_applicable() {
        let observed = background_policy(std::process::id());
        #[cfg(target_os = "macos")]
        {
            let policy = observed.expect("observe current Darwin background policy");
            assert_eq!(
                policy.background(),
                policy.darwin_background || policy.external_background
            );
        }
        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            observed
                .expect_err("non-Darwin hosts have no Darwin policy model")
                .kind(),
            ProcessMetricsErrorKind::Unsupported
        );
    }

    #[test]
    fn a_missing_process_is_distinct_from_an_observation_failure() {
        let error = metrics(i32::MAX as u32).expect_err("maximum portable PID must not exist");
        assert_eq!(error.kind(), ProcessMetricsErrorKind::NotFound);
    }
}
