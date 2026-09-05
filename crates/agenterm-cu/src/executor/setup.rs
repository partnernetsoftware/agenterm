//! Stable current-user CLI entrypoint setup.

use std::path::PathBuf;

use crate::{
    command::SetupAction,
    managed_job_store::{ManagedJobRefreshBlockers, ManagedJobStore},
    reply::CuError,
    runtime_coordinator::RuntimeCoordinator,
    setup_entrypoint::{self, SetupMode},
    target_binding::{CurrentIdentityProvider, enroll_current_installation},
};

pub(super) fn setup_payload(
    action: SetupAction,
    bin_dir: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let source = std::env::current_exe().map_err(|error| {
        CuError::new(
            "setup_entrypoint_source_invalid",
            format!("resolve current agenterm-cu executable failed: {error}"),
        )
    })?;
    let bin_dir = match bin_dir {
        Some(path) => PathBuf::from(path),
        None => setup_entrypoint::default_bin_dir()?,
    };
    match action {
        SetupAction::Check => {
            let blockers = ManagedJobStore::refresh_blockers_read_only()
                .map_err(runtime_refresh_preflight_error)?;
            let mut setup = setup_entrypoint::run(&source, &bin_dir, SetupMode::Check)?;
            attach_runtime_refresh(&mut setup, SetupAction::Check, blockers)?;
            Ok(setup)
        }
        SetupAction::Apply => {
            let runtime = RuntimeCoordinator::open().map_err(runtime_refresh_preflight_error)?;
            let _refresh_fence = runtime.acquire_refresh_fence()?;
            let store = ManagedJobStore::open().map_err(runtime_refresh_preflight_error)?;
            let blockers = store
                .refresh_blockers()
                .map_err(runtime_refresh_preflight_error)?;
            let mut setup = setup_entrypoint::run(&source, &bin_dir, SetupMode::Apply)?;
            attach_installation_identity(&mut setup)?;
            attach_runtime_refresh(&mut setup, SetupAction::Apply, blockers)?;
            Ok(setup)
        }
    }
}

fn attach_installation_identity(setup: &mut serde_json::Value) -> Result<(), CuError> {
    let provider = CurrentIdentityProvider::default_for_current_user().map_err(|_| {
        setup_identity_error(
            setup,
            "the private installation identity directory is unavailable",
        )
    })?;
    let performed = enroll_current_installation(&provider).map_err(|_| {
        setup_identity_error(
            setup,
            "the private installation identity could not be enrolled",
        )
    })?;
    let object = setup.as_object_mut().ok_or_else(|| {
        CuError::new(
            "setup_entrypoint_serialization_failed",
            "setup result is not a JSON object",
        )
    })?;
    let launcher_performed = object
        .get("action")
        .and_then(|value| value.get("performed"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    object.insert(
        "installation_identity".into(),
        serde_json::json!({
            "schema": 1,
            "scope": "installation",
            "status": "ready",
            "action": {
                "performed": performed,
                "outcome": if performed { "enrolled" } else { "unchanged" },
            }
        }),
    );
    if performed && !launcher_performed {
        object.insert(
            "action".into(),
            serde_json::json!({ "performed": true, "outcome": "identity-enrolled" }),
        );
        object.insert("effect".into(), serde_json::json!("committed"));
    }
    Ok(())
}

fn setup_identity_error(setup: &serde_json::Value, message: &'static str) -> CuError {
    let launcher_performed = setup
        .get("action")
        .and_then(|value| value.get("performed"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    CuError::new("setup_identity_enrollment_failed", message).with_detail(serde_json::json!({
        "effect": if launcher_performed { "committed" } else { "none" },
        "launcher": setup,
    }))
}

fn runtime_refresh_preflight_error(error: CuError) -> CuError {
    CuError::new(
        "runtime_refresh_preflight_uncertain",
        "runtime resource inventory could not be proven before setup refresh",
    )
    .with_detail(serde_json::json!({
        "effect": "none",
        "cause": error.code,
    }))
}

fn attach_runtime_refresh(
    setup: &mut serde_json::Value,
    mode: SetupAction,
    blockers: ManagedJobRefreshBlockers,
) -> Result<(), CuError> {
    let object = setup.as_object_mut().ok_or_else(|| {
        CuError::new(
            "setup_entrypoint_serialization_failed",
            "setup result is not a JSON object",
        )
    })?;
    let launcher_ready = object.get("status").and_then(serde_json::Value::as_str) == Some("ready");
    let launcher_performed = object
        .get("action")
        .and_then(|value| value.get("performed"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let ready = launcher_ready && blockers.blocking == 0;
    object.insert(
        "runtime_refresh".into(),
        serde_json::json!({
            "schema": 1,
            "architecture": "on-demand-coordinator-with-resource-owners",
            "mode": match mode { SetupAction::Check => "check", SetupAction::Apply => "apply" },
            "status": if ready { "ready" } else { "deferred" },
            "global_daemon": { "present": false, "required": false },
            "future_activation": {
                "provider": "direct-launcher",
                "aligned": launcher_ready,
                "action": if launcher_performed { "published" } else { "unchanged" },
            },
            "owned_resources": {
                "managed_jobs": {
                    "blocking": blockers.blocking,
                    "states": {
                        "start_intent": blockers.start_intent,
                        "starting": blockers.starting,
                        "running": blockers.running,
                        "orphaned_uncertain": blockers.orphaned_uncertain,
                    }
                },
                "device_leases": {
                    "provider": "unavailable",
                    "active": 0,
                    "authority": "no-native-claim-surface"
                }
            },
            "preservation": {
                "verified": true,
                "stopped": 0,
                "restarted": 0,
                "released": 0
            },
            "action": {
                "performed": launcher_performed && blockers.blocking == 0,
                "effect": if launcher_performed && blockers.blocking == 0 {
                    "future_activation_published"
                } else {
                    "none"
                },
                "outcome": if ready {
                    if launcher_performed { "refreshed" } else { "unchanged" }
                } else {
                    "deferred"
                }
            }
        }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_value(status: &str, performed: bool) -> serde_json::Value {
        serde_json::json!({
            "status": status,
            "action": { "performed": performed }
        })
    }

    #[test]
    fn runtime_refresh_is_ready_only_for_an_aligned_launcher_and_no_blockers() {
        let mut value = setup_value("ready", true);
        attach_runtime_refresh(
            &mut value,
            SetupAction::Apply,
            ManagedJobRefreshBlockers::default(),
        )
        .unwrap();
        assert_eq!(value["runtime_refresh"]["status"], "ready");
        assert_eq!(value["runtime_refresh"]["action"]["performed"], true);
        assert_eq!(value["runtime_refresh"]["global_daemon"]["present"], false);
    }

    #[test]
    fn runtime_refresh_defers_without_disturbing_a_resident_owner() {
        let mut value = setup_value("ready", true);
        attach_runtime_refresh(
            &mut value,
            SetupAction::Apply,
            ManagedJobRefreshBlockers {
                blocking: 1,
                running: 1,
                ..ManagedJobRefreshBlockers::default()
            },
        )
        .unwrap();
        assert_eq!(value["runtime_refresh"]["status"], "deferred");
        assert_eq!(value["runtime_refresh"]["action"]["performed"], false);
        assert_eq!(value["runtime_refresh"]["preservation"]["stopped"], 0);
    }
}
