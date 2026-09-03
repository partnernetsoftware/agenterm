//! Typed-error vocabulary shared by every verb family: the mechanism ->
//! `CuError` mapping, the `invalid_input` shorthand, the error payload a
//! receipt or `detail` embeds, and the OS permission repair paths.

use super::*;

/// Where the macOS Accessibility permission is granted. Quoted in the typed
/// `denied` reply so an agent can relay the repair path without guessing.
pub const ACCESSIBILITY_REPAIR_PATH: &str = "System Settings > Privacy & Security > Accessibility: enable the process that runs agenterm-cu (or its parent terminal / launcher), then rerun";

/// Where a person turns on the grant window capture needs. Separate from
/// the Accessibility one: macOS gates reading a window's pixels on Screen
/// Recording, and granting one does not grant the other.
pub const SCREEN_RECORDING_REPAIR_PATH: &str = "System Settings > Privacy & Security > Screen & System Audio Recording: enable the process that runs agenterm-cu (or its parent terminal / launcher), then rerun";

pub(super) fn invalid_input(message: String) -> CuError {
    CuError::new("invalid_input", message)
}

pub(super) fn error_payload(error: &CuError) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "code": error.code,
        "message": error.message,
    });
    if let Some(detail) = &error.detail {
        payload["detail"] = detail.clone();
    }
    payload
}

pub(super) fn map_mechanism_err(error: mechanism::MechanismError) -> CuError {
    match error {
        mechanism::MechanismError::Unsupported { reason } => CuError::new("unsupported", reason),
        // An OS permission refusal is the PRD 31 `denied` vocabulary, with
        // the mechanism code and repair path kept in `detail` so a caller
        // never has to parse prose to know what to fix.
        mechanism::MechanismError::Failed { code, message } if code == "a11y_permission_denied" => {
            CuError::new("denied", message).with_detail(serde_json::json!({
                "reason": code,
                "permission": "accessibility",
                "repair": ACCESSIBILITY_REPAIR_PATH,
            }))
        }
        mechanism::MechanismError::Failed { code, message } => CuError::new(code, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_denial_is_typed_denied_with_repair_path() {
        let error = map_mechanism_err(mechanism::MechanismError::Failed {
            code: "a11y_permission_denied".into(),
            message: "AXIsProcessTrusted() is false".into(),
        });
        assert_eq!(error.code, "denied");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["reason"], "a11y_permission_denied");
        assert_eq!(detail["permission"], "accessibility");
        assert_eq!(detail["repair"], ACCESSIBILITY_REPAIR_PATH);
        // Every other mechanism code passes through unchanged.
        let other = map_mechanism_err(mechanism::MechanismError::Failed {
            code: "a11y_tree_empty".into(),
            message: "no nodes".into(),
        });
        assert_eq!(other.code, "a11y_tree_empty");
        assert!(other.detail.is_none());
    }

    /// Every mechanism error code libagenterm, the platform adapters and
    /// `mechanism.rs` can produce (collected 2026-09-03), plus one unknown
    /// spelling so pass-through stays covered.
    const MECHANISM_ERROR_CODES: &[&str] = &[
        "a11y_access_denied",
        "a11y_action_no_effect",
        "a11y_action_timeout",
        "a11y_action_unavailable",
        "a11y_allocation_failed",
        "a11y_app_not_found",
        "a11y_app_visibility_not_applied",
        "a11y_backend_failed",
        "a11y_caret_no_effect",
        "a11y_caret_unavailable",
        "a11y_connect_failed",
        "a11y_depth_limit",
        "a11y_extents_unavailable",
        "a11y_focus_outside_window",
        "a11y_focus_unavailable",
        "a11y_invalid_node_id",
        "a11y_invalid_operation",
        "a11y_invalid_safearray",
        "a11y_key_injection_failed",
        "a11y_key_limit",
        "a11y_key_timeout",
        "a11y_key_unavailable",
        "a11y_menu_item_ambiguous",
        "a11y_menu_item_disabled",
        "a11y_menu_item_not_found",
        "a11y_menu_item_not_leaf",
        "a11y_menu_unavailable",
        "a11y_node_disabled",
        "a11y_node_id_limit",
        "a11y_node_id_truncated",
        "a11y_node_limit",
        "a11y_node_not_found",
        "a11y_node_recycled",
        "a11y_null_interface",
        "a11y_observe_unavailable",
        "a11y_option_ambiguous",
        "a11y_option_not_found",
        "a11y_pattern_unsupported",
        "a11y_permission_denied",
        "a11y_property_invalid",
        "a11y_publish_connect",
        "a11y_publish_embed",
        "a11y_publish_export",
        "a11y_publish_thread",
        "a11y_registry_read_failed",
        "a11y_runtime_id_duplicate",
        "a11y_runtime_id_invalid",
        "a11y_safearray_limit",
        "a11y_scroll_no_effect",
        "a11y_scroll_unavailable",
        "a11y_selection_no_effect",
        "a11y_selection_unavailable",
        "a11y_string_limit",
        "a11y_text_limit",
        "a11y_text_read_only",
        "a11y_text_timeout",
        "a11y_text_unavailable",
        "a11y_timeout",
        "a11y_tree_empty",
        "a11y_tree_timeout",
        "a11y_uia_failed",
        "a11y_window_gone",
        "app_inventory_failed",
        "app_launch_failed",
        "app_launch_needs_terminal",
        "app_list_failed",
        "app_not_found",
        "bad_action",
        "bad_area",
        "bad_button",
        "bad_dimensions",
        "bad_encoding",
        "bad_env",
        "bad_field",
        "bad_handle",
        "bad_index",
        "bad_path",
        "bad_pid",
        "bad_pointer",
        "bad_program",
        "bad_shortcut",
        "bad_size",
        "bad_state",
        "bad_text",
        "clipboard_backend_error",
        "clipboard_busy",
        "clipboard_failed",
        "clipboard_timeout",
        "clipboard_too_large",
        "clipboard_unavailable",
        "clipboard_worker_disconnected",
        "clipboard_worker_panicked",
        "clipboard_worker_start",
        "desktop_host_add_icon",
        "desktop_host_append_menu",
        "desktop_host_bad_action_count",
        "desktop_host_bad_action_id",
        "desktop_host_bad_label",
        "desktop_host_bad_native_action",
        "desktop_host_bad_shortcut",
        "desktop_host_closed",
        "desktop_host_create_menu",
        "desktop_host_create_window",
        "desktop_host_cursor_position",
        "desktop_host_duplicate_action_id",
        "desktop_host_duplicate_hotkey",
        "desktop_host_failed",
        "desktop_host_get_message",
        "desktop_host_get_module",
        "desktop_host_hotkey_unavailable",
        "desktop_host_null",
        "desktop_host_register_class",
        "desktop_host_register_taskbar_created",
        "desktop_host_set_timer",
        "desktop_host_wrong_thread",
        "dylib_load",
        "dylib_symbol",
        "input_failed",
        "input_inject_failed",
        "invalid_input",
        "invalid_logical_extent",
        "invalid_physical_extent",
        "invalid_scale_factor",
        "invalid_state",
        "mechanism_failed",
        "pointer_position_failed",
        "screen_churn",
        "screenshot_allocation_failed",
        "screenshot_bitmap_create",
        "screenshot_buffer_too_small",
        "screenshot_capture_failed",
        "screenshot_dc_unavailable",
        "screenshot_empty_path",
        "screenshot_encode_error",
        "screenshot_failed",
        "screenshot_file_error",
        "screenshot_format_unsupported",
        "screenshot_gdiplus_startup",
        "screenshot_invalid_bounds",
        "screenshot_invalid_clip",
        "screenshot_invalid_dimensions",
        "screenshot_io_error",
        "screenshot_too_large",
        "screenshot_unsupported",
        "screenshot_window_unavailable",
        "unexpected_status",
        "window_churn",
        "window_constraints_invalid",
        "window_enum_failed",
        "window_failed",
        "window_identity_invalid",
        "window_identity_unknown",
        "window_inspect_access_denied",
        "window_inspect_failed",
        "window_metadata_invalid",
        "window_not_found",
        "window_op_failed",
        "window_stale",
        "failed",
        "unknown_future_code",
    ];

    /// Snapshot gate for `map_mechanism_err`: every `MechanismError`
    /// variant (`Unsupported`, and `Failed` for each known code) maps to
    /// exactly the `CuError` recorded in
    /// `tests/fixtures/mechanism_error_map.json`. Regenerate deliberately
    /// with `AGENTERM_CU_WRITE_ERROR_MAP_SNAPSHOT=1`.
    #[test]
    fn mechanism_error_map_matches_snapshot() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mechanism_error_map.json"
        );
        let mut cases = vec![mechanism::MechanismError::Unsupported {
            reason: "host adapter unavailable".into(),
        }];
        cases.extend(
            MECHANISM_ERROR_CODES
                .iter()
                .map(|code| mechanism::MechanismError::Failed {
                    code: (*code).to_owned(),
                    message: format!("message for {code}"),
                }),
        );
        let rows: Vec<serde_json::Value> = cases
            .into_iter()
            .map(|input| {
                let input_json = match &input {
                    mechanism::MechanismError::Unsupported { reason } => {
                        serde_json::json!({ "variant": "Unsupported", "reason": reason })
                    }
                    mechanism::MechanismError::Failed { code, message } => {
                        serde_json::json!({ "variant": "Failed", "code": code, "message": message })
                    }
                };
                let mapped = serde_json::to_value(map_mechanism_err(input)).expect("CuError json");
                serde_json::json!({ "input": input_json, "mapped": mapped })
            })
            .collect();
        let actual = serde_json::Value::Array(rows);
        if std::env::var_os("AGENTERM_CU_WRITE_ERROR_MAP_SNAPSHOT").is_some() {
            let text = serde_json::to_string_pretty(&actual).expect("snapshot json") + "\n";
            std::fs::write(path, text).expect("write snapshot fixture");
        }
        let expected: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(path).expect("read tests/fixtures/mechanism_error_map.json"),
        )
        .expect("snapshot fixture is JSON");
        assert_eq!(
            actual, expected,
            "map_mechanism_err changed; every mechanism error -> CuError pair must be preserved"
        );
    }
}
