use std::{
    collections::BTreeSet,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Map, Value, json};

use crate::{
    browser_bridge::{
        BridgeRequest, BridgeStatus, ConnectionId, DebugReadRequest, ProfileInstanceId,
        ReloadResult, TabsResult, install_for_current_user, list_live_connections,
        send_to_connection, send_to_connection_with_timeout,
    },
    reply::CuError,
};

use super::{map_mechanism_err, windows::resolve_inventory_focus};

pub(super) fn browser_bridge_setup_payload() -> Result<Value, CuError> {
    let executable = std::env::current_exe().map_err(|_| {
        CuError::new(
            "browser_bridge_current_executable_unavailable",
            "the running agenterm-cu executable could not be resolved",
        )
    })?;
    let receipt = install_for_current_user(&executable).map_err(|error| {
        let mut typed = CuError::new(error.code, "browser bridge setup failed");
        if let Some(receipt) = error.receipt {
            typed = typed.with_detail(json!({ "receipt": receipt }));
        }
        typed
    })?;
    serde_json::to_value(receipt).map_err(|_| {
        CuError::new(
            "browser_bridge_install_receipt_invalid",
            "the browser bridge setup receipt could not be serialized",
        )
    })
}

pub(super) fn browser_bridge_connections_payload() -> Result<Value, CuError> {
    let inventory = list_live_connections().map_err(host_error)?;
    serde_json::to_value(inventory).map_err(|_| {
        CuError::new(
            "browser_bridge_response_invalid",
            "the bounded browser bridge connection inventory could not be serialized",
        )
    })
}

pub(super) fn browser_bridge_debug_read_payload(
    connection_id: &ConnectionId,
    tab_id: u32,
    max_frames: u16,
    max_depth: u8,
    max_scan: u32,
    max_results: u16,
) -> Result<Value, CuError> {
    let request = DebugReadRequest {
        tab_id,
        max_frames,
        max_depth,
        max_scan,
        max_results,
    };
    request
        .validate()
        .map_err(|error| CuError::new(error.code, error.message))?;
    let args = serde_json::to_value(request).map_err(|_| {
        CuError::new(
            "browser_bridge_request_invalid",
            "the bounded debug-read request could not be serialized",
        )
    })?;
    let Value::Object(args) = args else {
        return Err(CuError::new(
            "browser_bridge_request_invalid",
            "the bounded debug-read request was not an object",
        ));
    };
    browser_bridge_request_payload(connection_id, "debug-read", args)
}

const BRIDGE_REQUEST_MAX_TIMEOUT: Duration = Duration::from_secs(35);
const PRESENTATION_SETTLE: Duration = Duration::from_millis(500);
const LOCK_DEADLINE_MARGIN_MS: u64 = 5_000;

fn remaining_bridge_timeout(deadline: Instant) -> Result<Duration, CuError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining < Duration::from_millis(1) {
        return Err(CuError::new(
            "browser_bridge_operation_timeout",
            "the browser bridge operation exhausted its overall deadline",
        ));
    }
    Ok(remaining.min(BRIDGE_REQUEST_MAX_TIMEOUT))
}

fn bridge_status_until(
    connection_id: &ConnectionId,
    deadline: Instant,
) -> Result<BridgeStatus, CuError> {
    let result = browser_bridge_request_result_with_timeout(
        connection_id,
        "status",
        Map::new(),
        remaining_bridge_timeout(deadline)?,
    )?;
    serde_json::from_value(result).map_err(|_| {
        CuError::new(
            "browser_bridge_response_invalid",
            "the browser bridge status result was not the closed protocol shape",
        )
    })
}

fn bridge_tabs_until(
    connection_id: &ConnectionId,
    deadline: Instant,
) -> Result<TabsResult, CuError> {
    let result = browser_bridge_request_result_with_timeout(
        connection_id,
        "tabs",
        Map::new(),
        remaining_bridge_timeout(deadline)?,
    )?;
    serde_json::from_value(result).map_err(|_| {
        CuError::new(
            "browser_bridge_response_invalid",
            "the browser bridge tab result was not the closed protocol shape",
        )
    })
}

fn exact_tab_until(
    connection_id: &ConnectionId,
    tab_id: u32,
    deadline: Instant,
) -> Result<crate::browser_bridge::BrowserTab, CuError> {
    let inventory = bridge_tabs_until(connection_id, deadline)?;
    inventory
        .tabs
        .into_iter()
        .find(|tab| tab.tab_id == tab_id)
        .ok_or_else(|| {
            CuError::new(
                "browser_bridge_tab_not_found",
                "the exact tab is not exposed by this bridge connection",
            )
            .with_detail(json!({
                "connection_id": connection_id,
                "tab_id": tab_id,
                "inventory_truncated": inventory.truncated,
            }))
        })
}

fn tab_identity(tab: &crate::browser_bridge::BrowserTab) -> Value {
    json!({
        "tab_id": tab.tab_id,
        "window_id": tab.window_id,
        "active": tab.active,
    })
}

fn same_tab_identity_and_presentation(
    left: &crate::browser_bridge::BrowserTab,
    right: &crate::browser_bridge::BrowserTab,
) -> bool {
    left.tab_id == right.tab_id && left.window_id == right.window_id && left.active == right.active
}

fn profile_connections_until(
    expected: &ProfileInstanceId,
    deadline: Instant,
    ignore_unavailable: Option<&ConnectionId>,
) -> Result<(Vec<ConnectionId>, BTreeSet<ConnectionId>), CuError> {
    let inventory = list_live_connections().map_err(host_error)?;
    if inventory.truncated {
        return Err(CuError::new(
            "browser_bridge_connection_inventory_truncated",
            "a complete connection inventory is required to prove unique Profile ownership",
        )
        .with_detail(json!({
            "visited": inventory.visited,
        })));
    }
    let live = inventory
        .connections
        .iter()
        .map(|entry| entry.connection_id.clone())
        .collect::<BTreeSet<_>>();
    let mut matches = Vec::new();
    for entry in inventory.connections {
        match bridge_status_until(&entry.connection_id, deadline) {
            Ok(status) if &status.profile_instance_id == expected => {
                matches.push(entry.connection_id)
            }
            Ok(_) => {}
            Err(_) if ignore_unavailable == Some(&entry.connection_id) => {}
            Err(error) => {
                return Err(CuError::new(
                    "browser_bridge_profile_inventory_unverified",
                    "one live bridge connection could not be classified before the deadline",
                )
                .with_detail(json!({
                    "connection_id": entry.connection_id,
                    "cause": error,
                })));
            }
        }
    }
    Ok((matches, live))
}

fn require_unique_profile_connection(
    expected_connection: &ConnectionId,
    expected_profile: &ProfileInstanceId,
    deadline: Instant,
) -> Result<(), CuError> {
    let (matches, _) = profile_connections_until(expected_profile, deadline, None)?;
    if matches.len() != 1 || matches.first() != Some(expected_connection) {
        return Err(CuError::new(
            "browser_bridge_profile_connection_ambiguous",
            "the Profile instance must have exactly one live Native Messaging connection",
        )
        .with_detail(json!({ "matching_connections": matches })));
    }
    Ok(())
}

fn tab_lock_target(profile: &ProfileInstanceId, tab_id: u32) -> String {
    format!(
        "browser:native:{}:profile:{}:tab:{tab_id}",
        crate::browser_bridge::ACU_EXTENSION_ID,
        profile.as_str()
    )
}

pub(super) fn browser_bridge_attach_payload(
    connection_id: &ConnectionId,
    tab_id: u32,
    session_id: &str,
    lease: &str,
    ttl_seconds: u64,
    timeout_ms: u64,
) -> Result<Value, CuError> {
    if ttl_seconds.saturating_mul(1_000) < timeout_ms.saturating_add(LOCK_DEADLINE_MARGIN_MS) {
        return Err(CuError::new(
            "browser_bridge_lock_ttl_invalid",
            "the tab lock TTL must cover the overall attach deadline plus 5000ms",
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let before_focus = desktop_focus_handle()?;
    let status = bridge_status_until(connection_id, deadline)?;
    let tab = exact_tab_until(connection_id, tab_id, deadline)?;
    require_unique_profile_connection(connection_id, &status.profile_instance_id, deadline)?;
    let lock_target = tab_lock_target(&status.profile_instance_id, tab_id);
    let lock = super::runtime::lock_acquire_payload(session_id, lease, &lock_target, ttl_seconds)?;
    let verification = (|| {
        let after_status = bridge_status_until(connection_id, deadline)?;
        let after_tab = exact_tab_until(connection_id, tab_id, deadline)?;
        require_unique_profile_connection(connection_id, &status.profile_instance_id, deadline)?;
        if after_status.profile_instance_id != status.profile_instance_id
            || !same_tab_identity_and_presentation(&tab, &after_tab)
        {
            return Err(CuError::new(
                "browser_bridge_attach_identity_changed",
                "the exact Profile connection or tab changed while its target lock was published",
            ));
        }
        verify_focus_unchanged(before_focus, deadline)?;
        Ok(after_tab)
    })();
    let after_tab = match verification {
        Ok(tab) => tab,
        Err(error) => return Err(rollback_new_attach_lock(lock, lease, error)),
    };
    Ok(json!({
        "connection_id": connection_id,
        "profile_instance_id": status.profile_instance_id,
        "extension_version": status.extension_version,
        "tab": tab_identity(&after_tab),
        "lock": lock,
        "focus_changed": false,
        "verified": true,
    }))
}

pub(super) fn browser_bridge_reload_payload(
    connection_id: &ConnectionId,
    tab_id: u32,
    session_id: &str,
    lease: &str,
    ttl_seconds: u64,
    timeout_ms: u64,
) -> Result<Value, CuError> {
    if ttl_seconds.saturating_mul(1_000) < timeout_ms.saturating_add(LOCK_DEADLINE_MARGIN_MS) {
        return Err(CuError::new(
            "browser_bridge_lock_ttl_invalid",
            "the tab lock TTL must cover the overall reload deadline plus 5000ms",
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let before_focus = desktop_focus_handle()?;
    let before_status = bridge_status_until(connection_id, deadline)?;
    let before_tab = exact_tab_until(connection_id, tab_id, deadline)?;
    require_unique_profile_connection(connection_id, &before_status.profile_instance_id, deadline)?;
    let lock_target = tab_lock_target(&before_status.profile_instance_id, tab_id);
    let lock_before =
        super::runtime::lock_acquire_payload(session_id, lease, &lock_target, ttl_seconds)?;
    let (_, baseline) =
        profile_connections_until(&before_status.profile_instance_id, deadline, None)?;
    let reload_args =
        serde_json::from_value(json!({ "tab_id": tab_id })).expect("reload args are an object");
    let acknowledgement: ReloadResult =
        serde_json::from_value(browser_bridge_request_result_with_timeout(
            connection_id,
            "reload",
            reload_args,
            remaining_bridge_timeout(deadline)?,
        )?)
        .map_err(|_| {
            CuError::new(
                "browser_bridge_response_invalid",
                "the bridge reload acknowledgement was not the closed protocol shape",
            )
        })?;
    acknowledgement
        .validate_for(&before_status.profile_instance_id)
        .map_err(|error| {
            reload_effect_error(
                CuError::new(error.code, error.message),
                connection_id,
                None,
                false,
            )
        })?;

    let mut observed_new = BTreeSet::new();
    let (new_connection, after_status) = loop {
        let (profile_matches, live) = profile_connections_until(
            &before_status.profile_instance_id,
            deadline,
            Some(connection_id),
        )
        .map_err(|error| reload_effect_error(error, connection_id, None, false))?;
        let old_gone = !live.contains(connection_id);
        let mut new_matches = profile_matches
            .into_iter()
            .filter(|candidate| !baseline.contains(candidate))
            .collect::<Vec<_>>();
        observed_new.extend(new_matches.iter().cloned());
        if new_matches.len() > 1 {
            return Err(reload_effect_error(
                CuError::new(
                    "browser_bridge_reconnect_ambiguous",
                    "more than one new bridge connection claimed the same profile instance",
                )
                .with_detail(json!({
                    "matching_connections": new_matches,
                })),
                connection_id,
                None,
                old_gone,
            ));
        }
        if old_gone && new_matches.len() == 1 {
            let candidate = new_matches.remove(0);
            let status = bridge_status_until(&candidate, deadline).map_err(|error| {
                reload_effect_error(error, connection_id, Some(&candidate), true)
            })?;
            break (candidate, status);
        }
        if Instant::now() >= deadline {
            return Err(reload_effect_error(CuError::new(
                "browser_bridge_reconnect_timeout",
                "the bridge did not publish one unique replacement connection before the deadline",
            )
            .with_detail(json!({
                "observed_new_connections": observed_new,
            })), connection_id, None, old_gone));
        }
        thread::sleep(Duration::from_millis(50));
    };
    let after_tab = exact_tab_until(&new_connection, tab_id, deadline)
        .map_err(|error| reload_effect_error(error, connection_id, Some(&new_connection), true))?;
    if !same_tab_identity_and_presentation(&before_tab, &after_tab) {
        return Err(reload_effect_error(
            CuError::new(
                "browser_bridge_reload_tab_changed",
                "the exact attached tab identity or presentation changed across bridge reload",
            )
            .with_detail(json!({
                "before": tab_identity(&before_tab),
                "after": tab_identity(&after_tab),
            })),
            connection_id,
            Some(&new_connection),
            true,
        ));
    }
    let lock_after =
        super::runtime::lock_acquire_payload(session_id, lease, &lock_target, ttl_seconds)
            .map_err(|error| {
                reload_effect_error(error, connection_id, Some(&new_connection), true)
            })?;
    verify_focus_unchanged(before_focus, deadline)
        .map_err(|error| reload_effect_error(error, connection_id, Some(&new_connection), true))?;
    Ok(json!({
        "old_connection_id": connection_id,
        "connection_id": new_connection,
        "profile_instance_id": after_status.profile_instance_id,
        "extension_version": after_status.extension_version,
        "reload_scope": acknowledgement.reload_scope,
        "tab": tab_identity(&after_tab),
        "lock_before": lock_before,
        "lock_after": lock_after,
        "old_connection_gone": true,
        "unique_reconnect": true,
        "focus_changed": false,
        "focus_restored": false,
        "verified": true,
    }))
}

fn rollback_new_attach_lock(lock: Value, lease: &str, cause: CuError) -> CuError {
    let newly_acquired = lock.get("idempotent").and_then(Value::as_bool) == Some(false);
    let lock_id = lock
        .get("lock")
        .and_then(|value| value.get("lock_id"))
        .and_then(Value::as_str);
    if !newly_acquired {
        return CuError::new(cause.code, cause.message).with_detail(json!({
            "cause": cause.detail,
            "effect": "existing_target_lock_retained",
            "lock_released": false,
        }));
    }
    let Some(lock_id) = lock_id else {
        return CuError::new(
            "browser_bridge_attach_cleanup_uncertain",
            "attach verification failed after lock publication and its new lock identity is unavailable",
        )
        .with_detail(json!({
            "cause": cause,
            "effect": "target_lock_acquired",
            "lock_released": false,
        }));
    };
    match super::runtime::lock_release_payload(lock_id, lease) {
        Ok(_) => CuError::new(cause.code, cause.message).with_detail(json!({
            "cause": cause.detail,
            "effect": "target_lock_rolled_back",
            "lock_released": true,
        })),
        Err(cleanup) => CuError::new(
            "browser_bridge_attach_cleanup_uncertain",
            "attach verification failed and the newly published target lock could not be released",
        )
        .with_detail(json!({
            "cause": cause,
            "cleanup": cleanup,
            "effect": "target_lock_acquired",
            "lock_released": false,
        })),
    }
}

fn reload_effect_error(
    cause: CuError,
    old_connection: &ConnectionId,
    new_connection: Option<&ConnectionId>,
    old_connection_gone: bool,
) -> CuError {
    CuError::new(cause.code, cause.message).with_detail(json!({
        "cause": cause.detail,
        "effect": "native_connection_reload_accepted",
        "old_connection_id": old_connection,
        "connection_id": new_connection,
        "old_connection_gone": old_connection_gone,
        "retry_safe": false,
    }))
}

fn verify_focus_unchanged(
    expected: Option<isize>,
    operation_deadline: Instant,
) -> Result<(), CuError> {
    let settle_deadline = Instant::now() + PRESENTATION_SETTLE;
    loop {
        let observed = desktop_focus_handle()?;
        if observed != expected {
            return Err(CuError::new(
                "browser_bridge_presentation_changed",
                "desktop focus identity changed during the background browser operation",
            )
            .with_detail(json!({
                "before": expected,
                "observed": observed,
                "focus_changed": true,
                "focus_restored": false,
            })));
        }
        if Instant::now() >= settle_deadline {
            return Ok(());
        }
        if Instant::now() >= operation_deadline {
            return Err(CuError::new(
                "browser_bridge_operation_timeout",
                "the overall deadline expired while proving unchanged desktop focus",
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

pub(super) fn browser_bridge_request_payload(
    connection_id: &ConnectionId,
    command: &str,
    args: Map<String, Value>,
) -> Result<Value, CuError> {
    let result = browser_bridge_request_result(connection_id, command, args)?;
    Ok(json!({
        "connection_id": connection_id,
        "result": result,
    }))
}

fn browser_bridge_request_result(
    connection_id: &ConnectionId,
    command: &str,
    args: Map<String, Value>,
) -> Result<Value, CuError> {
    browser_bridge_request_result_with_timeout(
        connection_id,
        command,
        args,
        BRIDGE_REQUEST_MAX_TIMEOUT,
    )
}

fn browser_bridge_request_result_with_timeout(
    connection_id: &ConnectionId,
    command: &str,
    args: Map<String, Value>,
    timeout: Duration,
) -> Result<Value, CuError> {
    let request = BridgeRequest {
        protocol: crate::browser_bridge::PROTOCOL_VERSION,
        id: request_id()?,
        command: command.to_owned(),
        args,
    };
    let response = if timeout == BRIDGE_REQUEST_MAX_TIMEOUT {
        send_to_connection(connection_id, &request)
    } else {
        send_to_connection_with_timeout(connection_id, &request, timeout)
    }
    .map_err(host_error)?;
    if let Some(error) = response.error {
        let detail = json!({
            "connection_id": connection_id,
            "tab_id": error.tab_id,
            "detach": error.detach,
        });
        return Err(CuError::new(
            error.code,
            "the browser extension refused the typed bridge request",
        )
        .with_detail(detail));
    }
    let result = response.result.ok_or_else(|| {
        CuError::new(
            "browser_bridge_response_invalid",
            "the browser bridge omitted its successful result",
        )
    })?;
    Ok(result)
}

pub(super) fn browser_bridge_window_state_payload(
    connection_id: &ConnectionId,
    args: Map<String, Value>,
) -> Result<Value, CuError> {
    let before = desktop_focus_handle()?.ok_or_else(|| {
        CuError::new(
            "browser_bridge_desktop_focus_unavailable",
            "no exact desktop foreground window is available for focus restoration",
        )
    })?;
    let bridge = browser_bridge_request_payload(connection_id, "window-state", args);
    let after_effect = desktop_focus_handle()?;
    let restored = preserve_desktop_focus(before, bridge.as_ref().err())?;
    let after = desktop_focus_handle()?;
    if after != Some(before) {
        return Err(CuError::new(
            "browser_bridge_desktop_focus_restore_failed",
            "the exact previous desktop foreground window did not remain focused after the browser state settled",
        )
        .with_detail(json!({
            "before": before,
            "after_effect": after_effect,
            "after": after,
            "bridge_error": bridge.as_ref().err(),
        })));
    }
    let desktop_focus = json!({
        "before": before,
        "after_effect": after_effect,
        "after": after,
        "restored": restored,
        "verified": true,
    });
    match bridge {
        Ok(mut value) => {
            value["desktop_focus"] = desktop_focus;
            Ok(value)
        }
        Err(error) => Err(CuError::new(error.code, error.message).with_detail(json!({
            "bridge": error.detail,
            "desktop_focus": desktop_focus,
        }))),
    }
}

fn preserve_desktop_focus(
    expected: isize,
    bridge_error: Option<&CuError>,
) -> Result<bool, CuError> {
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut restored = false;
    while Instant::now() < deadline {
        let observed = desktop_focus_handle()?;
        if observed != Some(expected) {
            crate::mechanism::window_op::activate(expected).map_err(|error| {
                CuError::new(
                    "browser_bridge_desktop_focus_restore_failed",
                    "the browser state request changed desktop focus and the exact previous window could not be restored",
                )
                .with_detail(json!({
                    "before": expected,
                    "observed": observed,
                    "mechanism": map_mechanism_err(error),
                    "bridge_error": bridge_error,
                }))
            })?;
            wait_for_desktop_focus(expected)?;
            restored = true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    Ok(restored)
}

fn desktop_focus_handle() -> Result<Option<isize>, CuError> {
    let mut windows =
        crate::mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let stacking = crate::mechanism::window_enumerate::stacking().unwrap_or_default();
    Ok(resolve_inventory_focus(&mut windows, &stacking).handle)
}

fn wait_for_desktop_focus(expected: isize) -> Result<(), CuError> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(1_500) {
        if desktop_focus_handle()? == Some(expected) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(CuError::new(
        "browser_bridge_desktop_focus_restore_failed",
        "the exact previous desktop foreground window was not observed after restoration",
    )
    .with_detail(json!({"expected": expected})))
}

fn request_id() -> Result<String, CuError> {
    let random = agenterm_platform::entropy::secure_random_array::<32>().map_err(|_| {
        CuError::new(
            "browser_bridge_entropy_unavailable",
            "a fresh browser bridge request identity could not be generated",
        )
    })?;
    let mut id = String::with_capacity(68);
    id.push_str("acu-");
    for byte in random {
        use std::fmt::Write as _;
        write!(id, "{byte:02x}").expect("writing hexadecimal text to String cannot fail");
    }
    Ok(id)
}

fn host_error(error: crate::browser_bridge::BridgeHostError) -> CuError {
    CuError::new(
        error.code,
        "the exact browser bridge connection could not complete the request",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(title: &str, url: &str) -> crate::browser_bridge::BrowserTab {
        crate::browser_bridge::BrowserTab {
            tab_id: 7,
            window_id: 11,
            active: false,
            title: title.into(),
            url: url.into(),
        }
    }

    #[test]
    fn generated_request_identity_is_closed_and_bounded() {
        let id = request_id().expect("request id");
        assert_eq!(id.len(), 68);
        assert!(id.starts_with("acu-"));
        assert!(
            id[4..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }

    #[test]
    fn attached_tab_identity_ignores_dynamic_content_but_not_presentation() {
        let before = tab("before", "https://example.invalid/before");
        let after = tab("after", "https://example.invalid/after");
        assert!(same_tab_identity_and_presentation(&before, &after));
        assert_eq!(
            tab_identity(&after),
            json!({"tab_id": 7, "window_id": 11, "active": false})
        );
        let mut moved = after.clone();
        moved.window_id = 12;
        assert!(!same_tab_identity_and_presentation(&before, &moved));
        let mut activated = after;
        activated.active = true;
        assert!(!same_tab_identity_and_presentation(&before, &activated));
    }

    #[test]
    fn post_ack_error_discloses_effect_and_replacement_identity() {
        let old = ConnectionId::parse(&"1".repeat(64)).unwrap();
        let new = ConnectionId::parse(&"2".repeat(64)).unwrap();
        let error = reload_effect_error(
            CuError::new("browser_bridge_reload_tab_changed", "changed"),
            &old,
            Some(&new),
            true,
        );
        let detail = error.detail.unwrap();
        assert_eq!(detail["effect"], "native_connection_reload_accepted");
        assert_eq!(detail["old_connection_gone"], true);
        assert_eq!(detail["connection_id"], new.as_str());
        assert_eq!(detail["retry_safe"], false);
    }

    #[test]
    fn debug_read_bounds_fail_before_connection_io() {
        let connection_id = ConnectionId::parse(&"1".repeat(64)).unwrap();
        let error = browser_bridge_debug_read_payload(
            &connection_id,
            0,
            crate::browser_bridge::DEBUG_READ_MAX_FRAMES,
            crate::browser_bridge::DEBUG_READ_MAX_DEPTH,
            crate::browser_bridge::DEBUG_READ_MAX_SCAN,
            crate::browser_bridge::DEBUG_READ_MAX_RESULTS,
        )
        .unwrap_err();
        assert_eq!(error.code, "browser_bridge_debug_read_limit_invalid");
    }
}
