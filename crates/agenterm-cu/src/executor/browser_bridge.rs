use std::{
    collections::BTreeSet,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Map, Value, json};

use crate::{
    browser_bridge::{
        BridgeRequest, BridgeStatus, ConnectionId, DEBUG_FILES_MAX_FILES, DebugFile,
        DebugFilesRequest, DebugInvokeRequest, DebugReadRequest, DebugTarget, DebugTypeRequest,
        ProfileInstanceId, ReloadResult, TabsResult, install_for_current_user,
        list_live_connections, send_to_connection, send_to_connection_with_timeout,
    },
    reply::CuError,
};

use super::{error_payload, map_mechanism_err, windows::resolve_inventory_focus};

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

/// Profile-wide tab inventory through the one exact live MV3 connection.
/// This is deliberately stricter than `browser-bridge-tabs CONNECTION_ID`:
/// it proves profile-connection uniqueness, rejects truncated inventories,
/// preserves background-tab URLs, and brackets desktop focus.
pub(super) fn browser_tabs_payload(
    profile_selector: Option<&str>,
    requested_connection: Option<&ConnectionId>,
    match_text: Option<&str>,
    tab_id: Option<u32>,
) -> Result<Value, CuError> {
    // The focus bracket owns the complete bridge interaction, including the
    // connection/status scan. Taking it later could hide a presentation change
    // caused by profile discovery itself.
    let focus_before = desktop_focus_handle()?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let inventory = list_live_connections().map_err(host_error)?;
    if inventory.connections.is_empty() {
        if profile_selector.is_some() || requested_connection.is_some() {
            return Err(browser_tabs_bridge_selectors_unavailable(
                profile_selector,
                requested_connection,
            ));
        }
        if cfg!(target_os = "linux") {
            return browser_tabs_via_cdp_linux(match_text, tab_id, focus_before, deadline);
        }
        return Err(CuError::new(
            "browser_bridge_profile_connection_not_found",
            "no live browser profile bridge connection is available",
        ));
    }
    if inventory.truncated {
        return Err(CuError::new(
            "browser_bridge_connection_inventory_truncated",
            "the live bridge connection inventory is incomplete; profile uniqueness cannot be proven",
        ));
    }
    browser_tabs_via_bridge(
        profile_selector,
        requested_connection,
        match_text,
        tab_id,
        focus_before,
        deadline,
        &inventory,
    )
}

fn browser_tabs_via_bridge(
    profile_selector: Option<&str>,
    requested_connection: Option<&ConnectionId>,
    match_text: Option<&str>,
    tab_id: Option<u32>,
    focus_before: Option<isize>,
    deadline: Instant,
    inventory: &crate::browser_bridge::ConnectionInventory,
) -> Result<Value, CuError> {
    let selector = profile_selector.map(str::trim);
    if selector.is_some_and(|value| {
        value.is_empty()
            || value.len() > 32
            || value
                .bytes()
                .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    }) {
        return Err(CuError::new(
            "browser_bridge_profile_identity_invalid",
            "profile instance selector must be a 1..=32 lowercase hexadecimal prefix",
        ));
    }
    let mut statuses = Vec::with_capacity(inventory.connections.len());
    for entry in inventory.connections.iter() {
        let raw = browser_bridge_request_result_with_timeout(
            &entry.connection_id,
            "status",
            Map::new(),
            remaining_bridge_timeout(deadline)?,
        )
        .map_err(|cause| {
            CuError::new(
                "browser_bridge_profile_inventory_unverified",
                "one live connection did not publish a verifiable profile identity",
            )
            .with_detail(json!({
                "connection_id": entry.connection_id,
                "cause": error_payload(&cause),
            }))
        })?;
        let status: BridgeStatus = serde_json::from_value(raw).map_err(|_| {
            CuError::new(
                "browser_bridge_profile_inventory_unverified",
                "one live connection returned an invalid profile status",
            )
        })?;
        statuses.push((entry.connection_id.clone(), status));
    }
    let selected = select_profile_connection(&statuses, selector, requested_connection)?;
    // A direct connection id is not enough authority to hide another live
    // connection for the same profile instance.
    let same_profile = statuses
        .iter()
        .filter(|(_, status)| status.profile_instance_id == selected.1.profile_instance_id)
        .count();
    if same_profile != 1 {
        return Err(CuError::new(
            "browser_bridge_profile_connection_ambiguous",
            "the selected profile instance has more than one live native connection",
        )
        .with_count(same_profile));
    }

    let raw_tabs = browser_bridge_request_result_with_timeout(
        &selected.0,
        "tabs",
        Map::new(),
        remaining_bridge_timeout(deadline)?,
    )?;
    let result: TabsResult = serde_json::from_value(raw_tabs).map_err(|_| {
        CuError::new(
            "browser_bridge_response_invalid",
            "the selected profile returned an invalid bounded tab inventory",
        )
    })?;
    result
        .validate()
        .map_err(|error| CuError::new(error.code, error.message))?;
    if result.truncated {
        return Err(CuError::new(
            "browser_bridge_tabs_inventory_truncated",
            "the selected profile has more tabs than the complete inventory bound",
        ));
    }
    require_unique_profile_connection(&selected.0, &selected.1.profile_instance_id, deadline)?;
    verify_focus_unchanged(focus_before, deadline)?;

    let tabs = filter_profile_tabs(result.tabs, match_text, tab_id);
    Ok(json!({
        "mechanism": "mv3-native-messaging",
        "connection_id": selected.0,
        "profile_instance_id": selected.1.profile_instance_id,
        "selection": {
            "profile_instance_id": selector,
            "connection_id": requested_connection,
            "match": match_text,
            "tab_id": tab_id,
        },
        "returned": tabs.len(),
        "truncated": false,
        "tabs": tabs,
        "focus_changed": false,
        "verified": true,
    }))
}

#[cfg(target_os = "linux")]
fn browser_tabs_via_cdp_linux(
    match_text: Option<&str>,
    tab_id: Option<u32>,
    focus_before: Option<isize>,
    deadline: Instant,
) -> Result<Value, CuError> {
    let candidates = super::browser::discover_linux_cdp_ports()?;
    if candidates.is_empty() {
        return Err(browser_tabs_inventory_unsupported());
    }
    let mut live = Vec::new();
    for (pid, port) in candidates {
        match crate::cdp::targets::list_targets(port) {
            Ok(targets) => live.push((pid, port, targets)),
            Err(error) if error.code == "unsupported" => {}
            Err(error) => {
                return Err(CuError::new(error.code, error.message).with_detail(error.detail));
            }
        }
    }
    if live.is_empty() {
        return Err(browser_tabs_inventory_unsupported());
    }
    let selector = match_text.map(str::trim).filter(|value| !value.is_empty());
    let selected = if selector.is_some() || tab_id.is_some() {
        let matching: Vec<(
            u32,
            u16,
            Vec<crate::cdp::targets::PageTarget>,
            Vec<crate::browser_bridge::BrowserTab>,
        )> = live
            .iter()
            .filter_map(|(pid, port, targets)| {
                let pages: Vec<_> = targets.iter().filter(|target| target.is_page()).collect();
                let tabs = cdp_page_targets_to_tabs(&pages);
                let filtered = filter_profile_tabs(tabs, match_text, tab_id);
                if filtered.is_empty() {
                    None
                } else {
                    Some((*pid, *port, targets.clone(), filtered))
                }
            })
            .collect();
        match matching.as_slice() {
            [] => {
                verify_focus_unchanged(focus_before, deadline)?;
                let pages: Vec<_> = live[0].2.iter().filter(|target| target.is_page()).collect();
                return Ok(cdp_tabs_payload(
                    live[0].0,
                    live[0].1,
                    &pages,
                    match_text,
                    tab_id,
                    Vec::new(),
                ));
            }
            [one] => one.clone(),
            many => {
                return Err(CuError::new(
                    "browser_tabs_cdp_ambiguous",
                    "more than one live Chromium CDP listener matches the requested tab filter; refusing to guess",
                )
                .with_count(many.len())
                .with_detail(json!({
                    "instances": many
                        .iter()
                        .map(|(pid, port, _, tabs)| json!({
                            "pid": pid,
                            "port": port,
                            "returned": tabs.len(),
                        }))
                        .collect::<Vec<_>>(),
                })));
            }
        }
    } else {
        match live.as_slice() {
            [(pid, port, targets)] => {
                let pages: Vec<_> = targets.iter().filter(|target| target.is_page()).collect();
                let tabs = cdp_page_targets_to_tabs(&pages);
                let filtered = filter_profile_tabs(tabs, match_text, tab_id);
                (*pid, *port, targets.clone(), filtered)
            }
            many => {
                return Err(CuError::new(
                    "browser_tabs_cdp_ambiguous",
                    "more than one live Chromium instance publishes a remote debugging port; refusing to guess",
                )
                .with_count(many.len())
                .with_detail(json!({
                    "instances": many
                        .iter()
                        .map(|(pid, port, _)| json!({ "pid": pid, "port": port }))
                        .collect::<Vec<_>>(),
                })));
            }
        }
    };
    verify_focus_unchanged(focus_before, deadline)?;
    let pages: Vec<_> = selected
        .2
        .iter()
        .filter(|target| target.is_page())
        .collect();
    Ok(cdp_tabs_payload(
        selected.0, selected.1, &pages, match_text, tab_id, selected.3,
    ))
}

#[cfg(target_os = "linux")]
fn cdp_page_targets_to_tabs(
    pages: &[&crate::cdp::targets::PageTarget],
) -> Vec<crate::browser_bridge::BrowserTab> {
    pages
        .iter()
        .enumerate()
        .map(|(index, target)| crate::browser_bridge::BrowserTab {
            tab_id: u32::try_from(index + 1).expect("page target index fits u32"),
            window_id: 1,
            active: false,
            title: target.title.clone(),
            url: target.url.clone(),
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn cdp_tabs_payload(
    pid: u32,
    port: u16,
    pages: &[&crate::cdp::targets::PageTarget],
    match_text: Option<&str>,
    tab_id: Option<u32>,
    filtered: Vec<crate::browser_bridge::BrowserTab>,
) -> Value {
    json!({
        "mechanism": "cdp-json",
        "backend": crate::cdp::backend(),
        "port": port,
        "pid": pid,
        "via": "/json",
        "heuristic": "one CDP listener bound to an exact browser process command line; tab_id is a synthetic 1-based index over page targets because /json carries no MV3 tab identity",
        "selection": {
            "match": match_text,
            "tab_id": tab_id,
        },
        "total_pages": pages.len(),
        "returned": filtered.len(),
        "truncated": false,
        "tabs": filtered,
        "targets": pages
            .iter()
            .map(|target| target.json())
            .collect::<Vec<_>>(),
        "focus_changed": false,
        "verified": true,
    })
}

#[cfg(not(target_os = "linux"))]
fn browser_tabs_via_cdp_linux(
    _match_text: Option<&str>,
    _tab_id: Option<u32>,
    _focus_before: Option<isize>,
    _deadline: Instant,
) -> Result<Value, CuError> {
    unreachable!("browser_tabs_via_cdp_linux is only called on Linux")
}

#[cfg(any(target_os = "linux", test))]
fn browser_tabs_inventory_unsupported() -> CuError {
    CuError::new(
        "unsupported",
        "browser-tabs needs a live MV3 bridge connection or a Chromium process whose command line carries --remote-debugging-port",
    )
    .with_detail(json!({
        "backend": crate::cdp::backend(),
        "mechanisms": ["mv3-native-messaging", "cdp-json"],
        "next_actions": [
            "browser bridge setup; then load the ACU extension in Chrome for profile-wide tabs without a debug port",
            format!(
                "relaunch Chrome with --remote-debugging-port={} bound to 127.0.0.1 (scripts/box-chrome-a11y.sh forwards extra args)",
                crate::cdp::DEFAULT_PORT
            ),
            "page-targets --port N reads the CDP /json inventory when a listener is already answering",
        ],
        "alternatives": ["browser-bridge-setup", "browser-bridge-connections", "page-targets"],
    }))
}

fn browser_tabs_bridge_selectors_unavailable(
    profile_selector: Option<&str>,
    requested_connection: Option<&ConnectionId>,
) -> CuError {
    CuError::new(
        "browser_bridge_profile_connection_not_found",
        "profile_instance_id and connection_id selectors require a live MV3 bridge connection",
    )
    .with_detail(json!({
        "profile_instance_id": profile_selector,
        "connection_id": requested_connection,
        "next_actions": [
            "browser bridge setup; then load the ACU extension and retry with --connection-id or --profile-instance-id",
            format!(
                "omit bridge-only selectors and rely on CDP when Chrome carries --remote-debugging-port={}",
                crate::cdp::DEFAULT_PORT
            ),
        ],
        "alternatives": ["browser-bridge-setup", "browser-bridge-connections", "page-targets"],
    }))
}

fn select_profile_connection<'a>(
    statuses: &'a [(ConnectionId, BridgeStatus)],
    selector: Option<&str>,
    requested_connection: Option<&ConnectionId>,
) -> Result<&'a (ConnectionId, BridgeStatus), CuError> {
    let candidates: Vec<_> = statuses
        .iter()
        .filter(|(connection_id, status)| {
            requested_connection.is_none_or(|wanted| wanted == connection_id)
                && selector
                    .is_none_or(|wanted| status.profile_instance_id.as_str().starts_with(wanted))
        })
        .collect();
    match candidates.as_slice() {
        [one] => Ok(*one),
        [] => Err(CuError::new(
            "browser_bridge_profile_connection_not_found",
            "no live connection matches the requested browser profile identity",
        )
        .with_detail(json!({
            "profile_instance_id": selector,
            "connection_id": requested_connection,
        }))),
        many => Err(CuError::new(
            "browser_bridge_profile_connection_ambiguous",
            "more than one live connection matches the browser profile; refusing to guess",
        )
        .with_count(many.len())
        .with_detail(json!({
            "profile_instance_id": selector,
            "connections": many.iter().map(|(id, _)| id).collect::<Vec<_>>(),
        }))),
    }
}

fn filter_profile_tabs(
    tabs: Vec<crate::browser_bridge::BrowserTab>,
    match_text: Option<&str>,
    tab_id: Option<u32>,
) -> Vec<crate::browser_bridge::BrowserTab> {
    let wanted = match_text.map(str::to_lowercase);
    tabs.into_iter()
        .filter(|tab| tab_id.is_none_or(|wanted| tab.tab_id == wanted))
        .filter(|tab| {
            wanted.as_deref().is_none_or(|needle| {
                tab.title.to_lowercase().contains(needle) || tab.url.to_lowercase().contains(needle)
            })
        })
        .collect()
}

pub(super) fn browser_bridge_debug_read_payload(
    connection_id: &ConnectionId,
    tab_id: u32,
    max_frames: u16,
    max_depth: u8,
    max_scan: u32,
    max_results: u16,
    actionable: bool,
) -> Result<Value, CuError> {
    let request = DebugReadRequest {
        tab_id,
        max_frames,
        max_depth,
        max_scan,
        max_results,
        actionable,
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

pub(super) fn browser_bridge_debug_invoke_payload(
    request_context: &super::JobRequestContext<'_>,
    connection_id: &ConnectionId,
    request: DebugInvokeRequest,
    lock_ttl_seconds: u64,
    timeout_ms: u64,
) -> Result<Value, CuError> {
    execute_locked_debug_request(
        request_context,
        DebugRoute {
            connection_id,
            tab_id: request.tab_id,
            target: &request.target,
            command: "debug-invoke",
            lock_ttl_seconds,
            timeout_ms,
        },
        &request,
    )
}

pub(super) fn browser_bridge_debug_type_payload(
    request_context: &super::JobRequestContext<'_>,
    connection_id: &ConnectionId,
    tab_id: u32,
    target: &DebugTarget,
    text: &str,
    lock_ttl_seconds: u64,
    timeout_ms: u64,
) -> Result<Value, CuError> {
    let request = DebugTypeRequest {
        tab_id,
        target: target.clone(),
        text: text.to_owned(),
    };
    execute_locked_debug_request(
        request_context,
        DebugRoute {
            connection_id,
            tab_id,
            target,
            command: "debug-type",
            lock_ttl_seconds,
            timeout_ms,
        },
        &request,
    )
}

pub(super) fn browser_bridge_debug_files_payload(
    request_context: &super::JobRequestContext<'_>,
    connection_id: &ConnectionId,
    tab_id: u32,
    target: &DebugTarget,
    paths: &[String],
    lock_ttl_seconds: u64,
    timeout_ms: u64,
) -> Result<Value, CuError> {
    let files = validate_debug_files(paths)?;
    let request = DebugFilesRequest {
        tab_id,
        target: target.clone(),
        files,
    };
    execute_locked_debug_request(
        request_context,
        DebugRoute {
            connection_id,
            tab_id,
            target,
            command: "debug-files",
            lock_ttl_seconds,
            timeout_ms,
        },
        &request,
    )
}

fn validate_debug_files(paths: &[String]) -> Result<Vec<DebugFile>, CuError> {
    const MAX_PATH_BYTES: usize = 4_096;
    const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024 * 1024;
    const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024 * 1024;
    if paths.is_empty() || paths.len() > DEBUG_FILES_MAX_FILES {
        return Err(CuError::new(
            "browser_bridge_debug_files_invalid",
            "debug-files requires 1..=32 local files",
        ));
    }
    let mut total = 0_u64;
    let mut files = Vec::with_capacity(paths.len());
    for raw in paths {
        if raw.is_empty() || raw.len() > MAX_PATH_BYTES || raw.contains('\0') {
            return Err(CuError::new(
                "browser_bridge_debug_files_invalid",
                "each file path must be 1..=4096 bytes without NUL",
            ));
        }
        let path = std::path::Path::new(raw);
        if !path.is_absolute() {
            return Err(CuError::new(
                "browser_bridge_debug_files_invalid",
                "file paths must be absolute on the browser host",
            ));
        }
        let metadata = std::fs::symlink_metadata(path).map_err(|_| {
            CuError::new(
                "browser_bridge_debug_file_unavailable",
                "one local file could not be inspected",
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CuError::new(
                "browser_bridge_debug_file_invalid",
                "debug-files accepts regular non-symlink files only",
            ));
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err(CuError::new(
                "browser_bridge_debug_file_too_large",
                "one local file exceeds the 10 GiB bound",
            ));
        }
        total = total.checked_add(metadata.len()).ok_or_else(|| {
            CuError::new(
                "browser_bridge_debug_files_too_large",
                "file byte total overflowed",
            )
        })?;
        if total > MAX_TOTAL_BYTES {
            return Err(CuError::new(
                "browser_bridge_debug_files_too_large",
                "local files exceed the 20 GiB aggregate bound",
            ));
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CuError::new(
                    "browser_bridge_debug_file_invalid",
                    "each local file requires a UTF-8 basename",
                )
            })?;
        files.push(DebugFile {
            path: raw.clone(),
            name: name.to_owned(),
            size: metadata.len(),
        });
    }
    if serde_json::to_vec(&files).map_or(true, |encoded| encoded.len() > 128 * 1024) {
        return Err(CuError::new(
            "browser_bridge_debug_files_too_large",
            "encoded file descriptors exceed the 128 KiB request bound",
        ));
    }
    Ok(files)
}

struct DebugRoute<'a> {
    connection_id: &'a ConnectionId,
    tab_id: u32,
    target: &'a DebugTarget,
    command: &'static str,
    lock_ttl_seconds: u64,
    timeout_ms: u64,
}

fn execute_locked_debug_request(
    request_context: &super::JobRequestContext<'_>,
    route: DebugRoute<'_>,
    request: &impl serde::Serialize,
) -> Result<Value, CuError> {
    let DebugRoute {
        connection_id,
        tab_id,
        target,
        command,
        lock_ttl_seconds,
        timeout_ms,
    } = route;
    if lock_ttl_seconds.saturating_mul(1_000) < timeout_ms.saturating_add(LOCK_DEADLINE_MARGIN_MS) {
        return Err(CuError::new(
            "browser_bridge_lock_ttl_invalid",
            "the tab lock TTL must cover the overall effect deadline plus 5000ms",
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let before_focus = desktop_focus_handle()?;
    let before_status = bridge_status_until(connection_id, deadline)?;
    let before_tab = exact_tab_until(connection_id, tab_id, deadline)?;
    require_unique_profile_connection(connection_id, &before_status.profile_instance_id, deadline)?;
    let lock_target = tab_lock_target(&before_status.profile_instance_id, tab_id);
    let lock = super::runtime::lock_acquire_payload(
        request_context.session_id,
        request_context.session_lease,
        &lock_target,
        lock_ttl_seconds,
    )?;
    let after_lock_status = bridge_status_until(connection_id, deadline)?;
    let after_lock_tab = exact_tab_until(connection_id, tab_id, deadline)?;
    require_unique_profile_connection(connection_id, &before_status.profile_instance_id, deadline)?;
    if after_lock_status.profile_instance_id != before_status.profile_instance_id
        || !same_tab_identity_and_presentation(&before_tab, &after_lock_tab)
    {
        return Err(CuError::new(
            "browser_bridge_debug_identity_changed",
            "the exact profile connection or tab changed before effect delivery",
        )
        .with_detail(json!({ "effect": "not-performed" })));
    }
    if target.frame_id.is_empty() || target.backend_node_id == 0 {
        return Err(CuError::new(
            "browser_bridge_debug_target_invalid",
            "the exact frame and backend node identities are required",
        ));
    }
    let Value::Object(args) = serde_json::to_value(request).map_err(|_| {
        CuError::new(
            "browser_bridge_request_invalid",
            "the closed-tree request could not be serialized",
        )
    })?
    else {
        return Err(CuError::new(
            "browser_bridge_request_invalid",
            "the closed-tree request was not an object",
        ));
    };
    let result = browser_bridge_request_result_with_timeout(
        connection_id,
        command,
        args,
        remaining_bridge_timeout(deadline)?,
    )?;
    let postcheck = (|| {
        let after_status = bridge_status_until(connection_id, deadline)?;
        let after_tab = exact_tab_until(connection_id, tab_id, deadline)?;
        require_unique_profile_connection(
            connection_id,
            &before_status.profile_instance_id,
            deadline,
        )?;
        if after_status.profile_instance_id != before_status.profile_instance_id
            || !same_tab_identity_and_presentation(&before_tab, &after_tab)
        {
            return Err(CuError::new(
                "browser_bridge_debug_identity_changed",
                "the exact profile connection or tab changed after effect delivery",
            ));
        }
        let lock_after = super::runtime::lock_acquire_payload(
            request_context.session_id,
            request_context.session_lease,
            &lock_target,
            lock_ttl_seconds,
        )?;
        verify_focus_unchanged(before_focus, deadline)?;
        Ok(lock_after)
    })();
    let lock_after = postcheck.map_err(debug_effect_unknown)?;
    Ok(json!({
        "connection_id": connection_id,
        "tab": tab_identity(&before_tab),
        "result": result,
        "lock": public_lock(&lock),
        "lock_after": public_lock(&lock_after),
        "focus_changed": false,
        "verified": true,
    }))
}

fn public_lock(lock: &Value) -> Value {
    json!({
        "lock_id": lock.pointer("/lock/lock_id"),
        "session_id": lock.pointer("/lock/session_id"),
        "expires_at_utc_s": lock.pointer("/lock/expires_at_utc_s"),
        "idempotent": lock.get("idempotent"),
        "target_redacted": true,
    })
}

fn debug_effect_unknown(cause: CuError) -> CuError {
    CuError::new(
        "browser_bridge_outcome_unknown",
        "the browser effect completed but its exact postcondition could not be fully proved",
    )
    .with_detail(json!({
        "effect": "unknown",
        "retry_safe": false,
        "cause": { "code": cause.code, "detail": cause.detail },
    }))
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
    match matches.as_slice() {
        [only] if only == expected_connection => Ok(()),
        [] => Err(CuError::new(
            "browser_bridge_profile_connection_not_found",
            "the selected profile connection disappeared before verification",
        )
        .with_detail(json!({ "matching_connections": matches }))),
        [_] => Err(CuError::new(
            "browser_bridge_profile_connection_changed",
            "the selected profile identity moved to a different native connection",
        )
        .with_detail(json!({
            "expected_connection": expected_connection,
            "matching_connections": matches,
        }))),
        _ => Err(CuError::new(
            "browser_bridge_profile_connection_ambiguous",
            "the profile instance has more than one live Native Messaging connection",
        )
        .with_detail(json!({ "matching_connections": matches }))),
    }
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
        let uncertain = error.effect.as_deref() == Some("unknown");
        let cause = error.code.clone();
        let detail = json!({
            "connection_id": connection_id,
            "tab_id": error.tab_id,
            "detach": error.detach,
            "effect": error.effect,
            "cause": cause,
            "retry_safe": !uncertain,
        });
        return Err(CuError::new(
            if uncertain {
                "browser_bridge_outcome_unknown".to_owned()
            } else {
                error.code
            },
            if uncertain {
                "the browser effect may have been delivered but its postcondition or cleanup was not proved"
            } else {
                "the browser extension refused the typed bridge request"
            },
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

pub(super) fn browser_bridge_window_open_payload(
    connection_id: &ConnectionId,
    args: Map<String, Value>,
    focused: bool,
) -> Result<Value, CuError> {
    if focused {
        return browser_bridge_request_payload(connection_id, "window-open", args);
    }
    let before = desktop_focus_handle()?.ok_or_else(|| {
        CuError::new(
            "browser_bridge_desktop_focus_unavailable",
            "no exact desktop foreground window is available before the background window is created",
        )
    })?;
    let bridge = browser_bridge_request_payload(connection_id, "window-open", args);
    let after_effect = desktop_focus_handle()
        .map_err(|cause| window_open_focus_unknown(before, None, bridge.as_ref().err(), cause))?;
    let restored = preserve_desktop_focus(before, bridge.as_ref().err()).map_err(|cause| {
        window_open_focus_unknown(before, after_effect, bridge.as_ref().err(), cause)
    })?;
    let after = desktop_focus_handle().map_err(|cause| {
        window_open_focus_unknown(before, after_effect, bridge.as_ref().err(), cause)
    })?;
    if after != Some(before) {
        return Err(window_open_focus_unknown(
            before,
            after_effect,
            bridge.as_ref().err(),
            CuError::new(
                "browser_bridge_desktop_focus_restore_failed",
                "the exact previous desktop foreground window was not restored after background window creation",
            )
            .with_detail(json!({ "observed": after })),
        ));
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
        Err(error) => {
            let mut detail = match error.detail {
                Some(Value::Object(detail)) => detail,
                Some(bridge) => {
                    let mut detail = Map::new();
                    detail.insert("bridge".into(), bridge);
                    detail
                }
                None => Map::new(),
            };
            detail.insert("desktop_focus".into(), desktop_focus);
            Err(CuError::new(error.code, error.message).with_detail(Value::Object(detail)))
        }
    }
}

fn window_open_focus_unknown(
    before: isize,
    after_effect: Option<isize>,
    bridge_error: Option<&CuError>,
    cause: CuError,
) -> CuError {
    CuError::new(
        "browser_bridge_outcome_unknown",
        "the browser window effect or exact desktop focus restoration could not be fully proved",
    )
    .with_detail(json!({
        "effect": "unknown",
        "retry_safe": false,
        "before": before,
        "after_effect": after_effect,
        "bridge_error": bridge_error,
        "cause": cause,
    }))
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
    let uncertain = error.code == "browser_bridge_outcome_unknown";
    let typed = CuError::new(
        error.code,
        "the exact browser bridge connection could not complete the request",
    );
    if uncertain {
        typed.with_detail(json!({ "effect": "unknown", "retry_safe": false }))
    } else {
        typed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(hex_pair: &str) -> ConnectionId {
        ConnectionId::parse(&hex_pair.repeat(32)).unwrap()
    }

    fn status(profile: &str) -> BridgeStatus {
        BridgeStatus {
            protocol: crate::browser_bridge::PROTOCOL_VERSION,
            extension_id: crate::browser_bridge::ACU_EXTENSION_ID.into(),
            extension_version: crate::browser_bridge::BRIDGE_EXTENSION_VERSION.into(),
            profile_instance_id: crate::browser_bridge::ProfileInstanceId::parse(profile).unwrap(),
            commands: Vec::new(),
        }
    }

    #[test]
    fn profile_tab_selection_is_exact_unique_and_filtering_keeps_urls() {
        let first = connection("12");
        let second = connection("34");
        let statuses = vec![
            (first.clone(), status("abcdef1234567890abcdef1234567890")),
            (second.clone(), status("1234567890abcdef1234567890abcdef")),
        ];
        assert_eq!(
            select_profile_connection(&statuses, Some("abcdef"), None)
                .unwrap()
                .0,
            first
        );
        assert_eq!(
            select_profile_connection(&statuses, None, Some(&second))
                .unwrap()
                .1
                .profile_instance_id
                .as_str(),
            "1234567890abcdef1234567890abcdef"
        );
        assert_eq!(
            select_profile_connection(&statuses, None, None)
                .unwrap_err()
                .code,
            "browser_bridge_profile_connection_ambiguous"
        );
        assert_eq!(
            select_profile_connection(&statuses, Some("ffff"), None)
                .unwrap_err()
                .code,
            "browser_bridge_profile_connection_not_found"
        );

        let tabs = vec![
            crate::browser_bridge::BrowserTab {
                tab_id: 7,
                window_id: 2,
                active: false,
                title: "Background docs".into(),
                url: "https://example.com/Guide".into(),
            },
            crate::browser_bridge::BrowserTab {
                tab_id: 8,
                window_id: 2,
                active: true,
                title: "Inbox".into(),
                url: "https://mail.example/".into(),
            },
        ];
        let by_url = filter_profile_tabs(tabs.clone(), Some("EXAMPLE.COM/GUIDE"), None);
        assert_eq!(by_url.len(), 1);
        assert_eq!(by_url[0].tab_id, 7);
        assert!(!by_url[0].active);
        assert_eq!(
            filter_profile_tabs(tabs, None, Some(8))[0].url,
            "https://mail.example/"
        );
    }

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
    fn uncertain_window_open_focus_is_never_retry_safe() {
        let error = window_open_focus_unknown(
            41,
            Some(42),
            None,
            CuError::new(
                "browser_bridge_desktop_focus_restore_failed",
                "synthetic focus failure",
            ),
        );
        assert_eq!(error.code, "browser_bridge_outcome_unknown");
        let detail = error.detail.unwrap();
        assert_eq!(detail["effect"], "unknown");
        assert_eq!(detail["retry_safe"], false);
        assert_eq!(detail["before"], 41);
        assert_eq!(detail["after_effect"], 42);
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
            false,
        )
        .unwrap_err();
        assert_eq!(error.code, "browser_bridge_debug_read_limit_invalid");
    }

    #[test]
    fn browser_tabs_unsupported_carries_enablement_steps() {
        let error = browser_tabs_inventory_unsupported();
        assert_eq!(error.code, "unsupported");
        let detail = error.detail.expect("detail");
        assert_eq!(
            detail["mechanisms"],
            json!(["mv3-native-messaging", "cdp-json"])
        );
        assert!(
            detail["next_actions"]
                .as_array()
                .is_some_and(|steps| steps.len() >= 2)
        );
        assert!(
            detail["alternatives"]
                .as_array()
                .is_some_and(|items| items.iter().any(|value| value == "page-targets"))
        );
    }

    #[test]
    fn bridge_selectors_without_connection_name_alternatives() {
        let error = browser_tabs_bridge_selectors_unavailable(Some("abc"), None);
        assert_eq!(error.code, "browser_bridge_profile_connection_not_found");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["profile_instance_id"], "abc");
        assert!(
            detail["next_actions"]
                .as_array()
                .is_some_and(|steps| !steps.is_empty())
        );
    }
}
