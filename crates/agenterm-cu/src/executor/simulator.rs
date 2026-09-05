use std::time::Duration;

use agenterm_platform::simulator::{
    self, SimulatorAppAction, SimulatorAppLifecycleReceipt, SimulatorBootReceipt, SimulatorError,
};
use serde_json::{Value, json};

use crate::{
    command::{
        SIMULATOR_RESULTS_MAX, SIMULATOR_TIMEOUT_MS_MAX, validate_simulator_bundle_id,
        validate_simulator_udid,
    },
    reply::CuError,
};

pub(super) fn simulator_devices_payload(max: usize) -> Result<Value, CuError> {
    validate_max(max)?;
    let inventory = simulator::list_devices(max).map_err(platform_error)?;
    Ok(json!({
        "devices": inventory.devices.into_iter().map(|device| {
            let booted = device.is_booted();
            json!({
                "udid": device.udid,
                "runtime": device.runtime,
                "device_type": device.device_type,
                "state": device.state,
                "booted": booted,
            })
        }).collect::<Vec<_>>(),
        "visited": inventory.visited,
        "truncated": inventory.truncated,
    }))
}

pub(super) fn simulator_boot_payload(
    udid: &str,
    timeout_ms: u64,
    expect_booted: bool,
) -> Result<Value, CuError> {
    validate_simulator_udid(udid).map_err(udid_error)?;
    validate_timeout(timeout_ms)?;
    if !expect_booted {
        return Err(CuError::new(
            "simulator_expectation_required",
            "simulator boot requires the explicit --expect booted acknowledgement",
        ));
    }
    let receipt =
        simulator::boot_exact(udid, Duration::from_millis(timeout_ms)).map_err(platform_error)?;
    boot_receipt(receipt, udid)
}

fn boot_receipt(receipt: SimulatorBootReceipt, udid: &str) -> Result<Value, CuError> {
    if receipt.udid != udid || receipt.after_state != "Booted" {
        return Err(CuError::new(
            "simulator_boot_unverified",
            "CoreSimulator did not verify the exact device in Booted state",
        )
        .with_detail(json!({
            "udid": receipt.udid,
            "before_state": receipt.before_state,
            "after_state": receipt.after_state,
            "verified": false,
        })));
    }
    Ok(json!({
        "udid": receipt.udid,
        "before_state": receipt.before_state,
        "after_state": receipt.after_state,
        "already_booted": receipt.already_booted,
        "verified": true,
    }))
}

pub(super) fn simulator_apps_payload(udid: &str, max: usize) -> Result<Value, CuError> {
    validate_simulator_udid(udid).map_err(udid_error)?;
    validate_max(max)?;
    let inventory = simulator::list_apps(udid, max).map_err(platform_error)?;
    if inventory.device_udid != udid {
        return Err(CuError::new(
            "simulator_device_changed",
            "CoreSimulator returned applications for a different device identity",
        ));
    }
    Ok(json!({
        "device_udid": inventory.device_udid,
        "apps": inventory.apps.into_iter().map(|app| json!({
            "bundle_id": app.bundle_id,
            "name": app.name,
            "application_type": app.application_type,
        })).collect::<Vec<_>>(),
        "visited": inventory.visited,
        "truncated": inventory.truncated,
    }))
}

pub(super) fn simulator_app_lifecycle_payload(
    udid: &str,
    bundle_id: &str,
    timeout_ms: u64,
    expect_accepted: bool,
    action: SimulatorAppAction,
) -> Result<Value, CuError> {
    validate_simulator_udid(udid).map_err(udid_error)?;
    validate_simulator_bundle_id(bundle_id).map_err(bundle_id_error)?;
    validate_timeout(timeout_ms)?;
    if !expect_accepted {
        return Err(CuError::new(
            "simulator_expectation_required",
            "simulator app lifecycle requires the explicit --expect accepted acknowledgement",
        ));
    }
    let receipt = match action {
        SimulatorAppAction::Launch => {
            simulator::launch_exact(udid, bundle_id, Duration::from_millis(timeout_ms))
        }
        SimulatorAppAction::Terminate => {
            simulator::terminate_exact(udid, bundle_id, Duration::from_millis(timeout_ms))
        }
    }
    .map_err(platform_error)?;
    lifecycle_receipt(receipt, udid, bundle_id, action)
}

fn lifecycle_receipt(
    receipt: SimulatorAppLifecycleReceipt,
    udid: &str,
    bundle_id: &str,
    expected_action: SimulatorAppAction,
) -> Result<Value, CuError> {
    if receipt.device_udid != udid
        || receipt.bundle_id != bundle_id
        || receipt.action != expected_action
        || !receipt.accepted
        || receipt.verified
    {
        return Err(CuError::new(
            "simulator_lifecycle_receipt_invalid",
            "CoreSimulator returned a lifecycle receipt outside the public accepted-only contract",
        ));
    }
    Ok(json!({
        "device_udid": receipt.device_udid,
        "bundle_id": receipt.bundle_id,
        "action": action_name(receipt.action),
        "device_state_before": receipt.device_state_before,
        "device_state_after": receipt.device_state_after,
        "accepted": true,
        "verified": false,
        "launch_pid": receipt.launch_pid,
    }))
}

fn action_name(action: SimulatorAppAction) -> &'static str {
    match action {
        SimulatorAppAction::Launch => "launch",
        SimulatorAppAction::Terminate => "terminate",
    }
}

fn validate_max(max: usize) -> Result<(), CuError> {
    if !(1..=SIMULATOR_RESULTS_MAX).contains(&max) {
        return Err(CuError::new(
            "simulator_limit_invalid",
            "simulator max must be in 1..=200",
        ));
    }
    Ok(())
}

fn validate_timeout(timeout_ms: u64) -> Result<(), CuError> {
    if !(1..=SIMULATOR_TIMEOUT_MS_MAX).contains(&timeout_ms) {
        return Err(CuError::new(
            "simulator_timeout_invalid",
            "simulator timeout_ms must be in 1..=600000",
        ));
    }
    Ok(())
}

fn udid_error(message: &'static str) -> CuError {
    CuError::new("simulator_udid_invalid", message)
}

fn bundle_id_error(message: &'static str) -> CuError {
    CuError::new("simulator_bundle_id_invalid", message)
}

fn platform_error(error: SimulatorError) -> CuError {
    CuError::new(error.code(), error.message())
}

#[cfg(test)]
mod tests {
    use super::*;

    const UDID: &str = "12345678-1234-1234-1234-123456789ABC";

    #[test]
    fn input_bounds_fail_before_platform_dispatch() {
        assert_eq!(
            simulator_devices_payload(0).unwrap_err().code,
            "simulator_limit_invalid"
        );
        assert_eq!(
            simulator_boot_payload("fuzzy", 1, true).unwrap_err().code,
            "simulator_udid_invalid"
        );
        assert_eq!(
            simulator_app_lifecycle_payload(
                UDID,
                "not dotted",
                1,
                true,
                SimulatorAppAction::Launch,
            )
            .unwrap_err()
            .code,
            "simulator_bundle_id_invalid"
        );
        assert_eq!(
            simulator_app_lifecycle_payload(
                UDID,
                "com.example.app",
                1,
                false,
                SimulatorAppAction::Terminate,
            )
            .unwrap_err()
            .code,
            "simulator_expectation_required"
        );
    }

    #[test]
    fn receipts_keep_verified_boot_separate_from_accepted_app_requests() {
        let boot = boot_receipt(
            SimulatorBootReceipt {
                udid: UDID.into(),
                before_state: "Shutdown".into(),
                after_state: "Booted".into(),
                already_booted: false,
            },
            UDID,
        )
        .unwrap();
        assert_eq!(boot["verified"], true);

        let lifecycle = lifecycle_receipt(
            SimulatorAppLifecycleReceipt {
                device_udid: UDID.into(),
                bundle_id: "com.example.app".into(),
                action: SimulatorAppAction::Launch,
                device_state_before: "Booted".into(),
                device_state_after: "Booted".into(),
                accepted: true,
                verified: false,
                launch_pid: Some(42),
            },
            UDID,
            "com.example.app",
            SimulatorAppAction::Launch,
        )
        .unwrap();
        assert_eq!(lifecycle["accepted"], true);
        assert_eq!(lifecycle["verified"], false);
    }

    #[test]
    fn receipts_fail_closed_on_identity_action_or_evidence_drift() {
        let bad_boot = boot_receipt(
            SimulatorBootReceipt {
                udid: UDID.into(),
                before_state: "Shutdown".into(),
                after_state: "Booting".into(),
                already_booted: false,
            },
            UDID,
        )
        .unwrap_err();
        assert_eq!(bad_boot.code, "simulator_boot_unverified");

        for receipt in [
            SimulatorAppLifecycleReceipt {
                device_udid: UDID.into(),
                bundle_id: "com.other.app".into(),
                action: SimulatorAppAction::Launch,
                device_state_before: "Booted".into(),
                device_state_after: "Booted".into(),
                accepted: true,
                verified: false,
                launch_pid: None,
            },
            SimulatorAppLifecycleReceipt {
                device_udid: UDID.into(),
                bundle_id: "com.example.app".into(),
                action: SimulatorAppAction::Terminate,
                device_state_before: "Booted".into(),
                device_state_after: "Booted".into(),
                accepted: true,
                verified: false,
                launch_pid: None,
            },
            SimulatorAppLifecycleReceipt {
                device_udid: UDID.into(),
                bundle_id: "com.example.app".into(),
                action: SimulatorAppAction::Launch,
                device_state_before: "Booted".into(),
                device_state_after: "Booted".into(),
                accepted: true,
                verified: true,
                launch_pid: None,
            },
        ] {
            assert_eq!(
                lifecycle_receipt(receipt, UDID, "com.example.app", SimulatorAppAction::Launch,)
                    .unwrap_err()
                    .code,
                "simulator_lifecycle_receipt_invalid"
            );
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_is_typed_unsupported_after_validation() {
        let error = simulator_devices_payload(1).unwrap_err();
        assert_eq!(error.code, "simulator_unsupported");
    }
}
