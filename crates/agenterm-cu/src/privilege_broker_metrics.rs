//! Root-owned, bounded counters for the fixed privilege broker.
//!
//! These counters are operational evidence, not authorization state. Every
//! increment is atomically published before the observed consent/effect
//! boundary, so an unavailable counter store fails closed rather than allowing
//! an unmeasured privileged action.

use std::{fs, path::PathBuf};

use agenterm_platform::filesystem::write_private_atomic;
use serde::{Deserialize, Serialize};

use crate::{CuError, privilege_broker::PrivilegeBrokerEvent};

const SCHEMA_VERSION: u32 = 1;
const MAX_METRICS_BYTES: u64 = 16 * 1024;
#[cfg(target_os = "linux")]
pub(crate) const BROKER_METRICS_PATH: &str = "/var/lib/agenterm/cu-privilege/broker-metrics.json";

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrivilegeBrokerMetrics {
    schema_version: u32,
    requests: u64,
    replay_finalized: u64,
    replay_outcome_unknown: u64,
    request_conflicts: u64,
    native_consent_calls: u64,
    effect_attempts: u64,
    reply_completed: u64,
    reply_consent_canceled: u64,
    reply_refused: u64,
    reply_failed_before_effect: u64,
    reply_failed_after_effect: u64,
    reply_outcome_unknown: u64,
    reply_write_failures: u64,
}

impl PrivilegeBrokerMetrics {
    fn empty() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            ..Self::default()
        }
    }

    fn increment(&mut self, event: PrivilegeBrokerEvent) -> Result<(), CuError> {
        let counter = match event {
            PrivilegeBrokerEvent::RequestAccepted => &mut self.requests,
            PrivilegeBrokerEvent::ReplayFinalized => &mut self.replay_finalized,
            PrivilegeBrokerEvent::ReplayOutcomeUnknown => &mut self.replay_outcome_unknown,
            PrivilegeBrokerEvent::RequestConflict => &mut self.request_conflicts,
            PrivilegeBrokerEvent::NativeConsentStarted => &mut self.native_consent_calls,
            PrivilegeBrokerEvent::EffectAttemptStarted => &mut self.effect_attempts,
            PrivilegeBrokerEvent::ReplyCompleted => &mut self.reply_completed,
            PrivilegeBrokerEvent::ReplyConsentCanceled => &mut self.reply_consent_canceled,
            PrivilegeBrokerEvent::ReplyRefused => &mut self.reply_refused,
            PrivilegeBrokerEvent::ReplyFailedBeforeEffect => &mut self.reply_failed_before_effect,
            PrivilegeBrokerEvent::ReplyFailedAfterEffect => &mut self.reply_failed_after_effect,
            PrivilegeBrokerEvent::ReplyOutcomeUnknown => &mut self.reply_outcome_unknown,
            #[cfg(target_os = "linux")]
            PrivilegeBrokerEvent::ReplyWriteFailed => &mut self.reply_write_failures,
        };
        *counter = counter.checked_add(1).ok_or_else(|| {
            CuError::new(
                "privilege_broker_metrics_overflow",
                "privilege broker evidence counter exhausted",
            )
        })?;
        Ok(())
    }
}

pub(crate) struct PrivilegeBrokerMetricsStore {
    path: PathBuf,
}

impl PrivilegeBrokerMetricsStore {
    #[cfg(target_os = "linux")]
    pub(crate) fn fixed() -> Self {
        Self {
            path: PathBuf::from(BROKER_METRICS_PATH),
        }
    }

    #[cfg(test)]
    fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn record(&self, event: PrivilegeBrokerEvent) -> Result<(), CuError> {
        let mut metrics = self.read()?;
        metrics.increment(event)?;
        let bytes = serde_json::to_vec(&metrics).map_err(metrics_error)?;
        if bytes.len() as u64 > MAX_METRICS_BYTES {
            return Err(CuError::new(
                "privilege_broker_metrics_invalid",
                "privilege broker evidence exceeds its byte ceiling",
            ));
        }
        write_private_atomic(&self.path, &bytes).map_err(metrics_error)
    }

    fn read(&self) -> Result<PrivilegeBrokerMetrics, CuError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PrivilegeBrokerMetrics::empty());
            }
            Err(error) => return Err(metrics_error(error)),
        };
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_METRICS_BYTES
        {
            return Err(CuError::new(
                "privilege_broker_metrics_invalid",
                "privilege broker evidence path is not one bounded regular file",
            ));
        }
        let bytes = fs::read(&self.path).map_err(metrics_error)?;
        let metrics: PrivilegeBrokerMetrics =
            serde_json::from_slice(&bytes).map_err(metrics_error)?;
        if metrics.schema_version != SCHEMA_VERSION {
            return Err(CuError::new(
                "privilege_broker_metrics_invalid",
                "privilege broker evidence schema is unsupported",
            ));
        }
        Ok(metrics)
    }
}

fn metrics_error(error: impl std::fmt::Display) -> CuError {
    CuError::new(
        "privilege_broker_metrics_unavailable",
        format!("privilege broker evidence could not be persisted: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PathBuf, PrivilegeBrokerMetricsStore) {
        let root = std::env::temp_dir().join(format!(
            "agenterm-cu-privilege-metrics-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let path = root.join("metrics.json");
        (root, PrivilegeBrokerMetricsStore::at(path))
    }

    #[test]
    fn counters_are_exact_and_atomically_replaced() {
        let (root, store) = fixture();
        store.record(PrivilegeBrokerEvent::RequestAccepted).unwrap();
        store
            .record(PrivilegeBrokerEvent::NativeConsentStarted)
            .unwrap();
        store
            .record(PrivilegeBrokerEvent::EffectAttemptStarted)
            .unwrap();
        store.record(PrivilegeBrokerEvent::ReplyCompleted).unwrap();
        store.record(PrivilegeBrokerEvent::ReplayFinalized).unwrap();
        let metrics = store.read().unwrap();
        assert_eq!(metrics.requests, 1);
        assert_eq!(metrics.native_consent_calls, 1);
        assert_eq!(metrics.effect_attempts, 1);
        assert_eq!(metrics.reply_completed, 1);
        assert_eq!(metrics.replay_finalized, 1);
        assert!(
            fs::read_dir(&root)
                .unwrap()
                .all(|entry| { entry.unwrap().file_name().to_string_lossy() == "metrics.json" })
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn linked_or_malformed_counter_state_fails_closed() {
        use std::os::unix::fs::symlink;

        let (root, store) = fixture();
        let target = root.join("target");
        fs::write(&target, b"{}").unwrap();
        symlink(&target, &store.path).unwrap();
        assert_eq!(
            store
                .record(PrivilegeBrokerEvent::RequestAccepted)
                .unwrap_err()
                .code,
            "privilege_broker_metrics_invalid"
        );
        fs::remove_file(&store.path).unwrap();
        fs::write(&store.path, b"not-json").unwrap();
        assert_eq!(
            store
                .record(PrivilegeBrokerEvent::RequestAccepted)
                .unwrap_err()
                .code,
            "privilege_broker_metrics_unavailable"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
