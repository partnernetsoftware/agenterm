//! Bounded peripheral inventory with installation-scoped opaque identities.

use agenterm_platform::device_inventory::{
    DeviceIdentityContinuity, DeviceInventory, DeviceInventoryError, DeviceInventoryErrorKind,
    DeviceKind, DeviceRecord, DeviceSelector, ProviderState,
};
use std::{
    collections::BTreeMap,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

use crate::{DeviceInventorySelector, reply::CuError, target_binding::CurrentIdentityProvider};

const RESPONSE_CEILING_BYTES: usize = 1024 * 1024;
const RESPONSE_HEADROOM_BYTES: usize = 4096;

pub(super) fn device_inventory_payload(
    selector: DeviceInventorySelector,
    max: usize,
) -> Result<Value, CuError> {
    let provider = CurrentIdentityProvider::default_for_current_user().map_err(|_| {
        CuError::new(
            "device_identity_unavailable",
            "the private installation identity directory is unavailable",
        )
    })?;
    let inventory = agenterm_platform::device_inventory::enumerate(
        provider.private_state_dir(),
        platform_selector(selector),
        max,
    )
    .map_err(device_inventory_error)?;
    inventory_value(inventory)
}

pub(super) fn device_watch_payload(
    selector: DeviceInventorySelector,
    max: usize,
    duration_ms: u64,
    interval_ms: u64,
    event_max: usize,
) -> Result<Value, CuError> {
    let provider = CurrentIdentityProvider::default_for_current_user().map_err(|_| {
        CuError::new(
            "device_identity_unavailable",
            "the private installation identity directory is unavailable",
        )
    })?;
    let started = Instant::now();
    let duration = Duration::from_millis(duration_ms);
    let deadline = started.checked_add(duration).ok_or_else(|| {
        CuError::new(
            "device_watch_deadline_overflow",
            "device watch deadline could not be represented",
        )
    })?;
    let platform_selector = platform_selector(selector);
    let first = enumerate_watch_sample(
        provider.private_state_dir(),
        platform_selector,
        max,
        remaining(deadline)?,
    )?;
    let mut state = WatchState::new(platform_selector, first);

    while Instant::now() < deadline && state.events.len() < event_max {
        let sleep_for = Duration::from_millis(interval_ms)
            .min(deadline.saturating_duration_since(Instant::now()));
        if sleep_for.is_zero() {
            break;
        }
        thread::sleep(sleep_for);
        let remaining = deadline.saturating_duration_since(Instant::now());
        let minimum_sample_budget = Duration::from_millis(interval_ms).min(Duration::from_secs(1));
        if remaining < minimum_sample_budget {
            break;
        }
        let sample = enumerate_watch_sample(
            provider.private_state_dir(),
            platform_selector,
            max,
            remaining,
        )?;
        state.observe(sample, event_max);
    }

    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    state.into_value(
        selector,
        max,
        duration_ms,
        interval_ms,
        event_max,
        elapsed_ms,
    )
}

fn remaining(deadline: Instant) -> Result<Duration, CuError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(CuError::new(
            "device_watch_timeout",
            "device watch exhausted its overall deadline before inventory completed",
        ));
    }
    Ok(remaining)
}

fn enumerate_watch_sample(
    private_state_dir: &std::path::Path,
    selector: DeviceSelector,
    max: usize,
    timeout: Duration,
) -> Result<DeviceInventory, CuError> {
    agenterm_platform::device_inventory::enumerate_with_timeout(
        private_state_dir,
        selector,
        max,
        timeout,
    )
    .map_err(device_inventory_error)
}

#[derive(Debug)]
struct DeviceWatchEvent {
    event: &'static str,
    at_ms: u64,
    device: DeviceRecord,
}

#[derive(Debug)]
struct WatchState {
    selector: DeviceSelector,
    previous: BTreeMap<DeviceKind, Option<BTreeMap<String, DeviceRecord>>>,
    providers: Vec<agenterm_platform::device_inventory::DeviceProviderStatus>,
    events: Vec<DeviceWatchEvent>,
    samples: usize,
    suppressed_provider_samples: usize,
    coverage_complete: bool,
    truncated: bool,
}

impl WatchState {
    fn new(selector: DeviceSelector, first: DeviceInventory) -> Self {
        let (previous, suppressed) = complete_rows(selector, &first);
        Self {
            selector,
            providers: first.providers,
            previous,
            events: Vec::new(),
            samples: 1,
            suppressed_provider_samples: suppressed,
            coverage_complete: suppressed == 0,
            truncated: first.truncated,
        }
    }

    fn observe(&mut self, sample: DeviceInventory, event_max: usize) {
        let (current, suppressed) = complete_rows(self.selector, &sample);
        self.samples = self.samples.saturating_add(1);
        self.suppressed_provider_samples =
            self.suppressed_provider_samples.saturating_add(suppressed);
        self.coverage_complete &= suppressed == 0;
        self.truncated |= sample.truncated;
        let at_ms = epoch_ms();

        for kind in DeviceKind::ALL {
            if !self.selector.includes(kind) {
                continue;
            }
            let Some(Some(previous)) = self.previous.get(&kind) else {
                continue;
            };
            let Some(Some(current_rows)) = current.get(&kind) else {
                continue;
            };
            append_diff(
                &mut self.events,
                previous,
                current_rows,
                at_ms,
                event_max,
                &mut self.truncated,
            );
            if self.events.len() >= event_max {
                break;
            }
        }
        if self.events.len() >= event_max {
            self.truncated = true;
        }
        self.previous = current;
        self.providers = sample.providers;
    }

    fn into_value(
        self,
        selector: DeviceInventorySelector,
        max: usize,
        duration_ms: u64,
        interval_ms: u64,
        event_max: usize,
        elapsed_ms: u64,
    ) -> Result<Value, CuError> {
        let observed_events = self.events.len();
        let mut encoded_rows = 0usize;
        let row_budget = RESPONSE_CEILING_BYTES - RESPONSE_HEADROOM_BYTES;
        let mut events = Vec::with_capacity(observed_events);
        for event in self.events {
            let row = json!({
                "event": event.event,
                "at_ms": event.at_ms,
                "device": device_value(event.device),
            });
            let bytes = serde_json::to_vec(&row)
                .map_err(|error| CuError::new("device_watch_encode_failed", error.to_string()))?
                .len();
            if encoded_rows.saturating_add(bytes) > row_budget {
                break;
            }
            encoded_rows = encoded_rows.saturating_add(bytes);
            events.push(row);
        }
        let returned = events.len();
        let response_truncated = self.truncated || returned < observed_events;
        let providers = self
            .providers
            .into_iter()
            .map(provider_value)
            .collect::<Vec<_>>();
        let value = json!({
            "selector": selector_name(selector),
            "max": max,
            "duration_ms": duration_ms,
            "interval_ms": interval_ms,
            "event_max": event_max,
            "elapsed_ms": elapsed_ms,
            "samples": self.samples,
            "events": events,
            "providers": providers,
            "returned": returned,
            "truncated": response_truncated,
            "coverage_complete": self.coverage_complete,
            "suppressed_provider_samples": self.suppressed_provider_samples,
            "termination": if returned >= event_max { "event-limit" } else { "duration" },
            "identity_scope": "installation",
            "response_ceiling_bytes": RESPONSE_CEILING_BYTES,
        });
        let encoded = serde_json::to_vec(&value)
            .map_err(|error| CuError::new("device_watch_encode_failed", error.to_string()))?;
        if encoded.len() > RESPONSE_CEILING_BYTES {
            return Err(CuError::new(
                "device_watch_response_limit",
                "device watch response exceeded the 1 MiB ceiling",
            ));
        }
        Ok(value)
    }
}

fn complete_rows(
    selector: DeviceSelector,
    inventory: &DeviceInventory,
) -> (
    BTreeMap<DeviceKind, Option<BTreeMap<String, DeviceRecord>>>,
    usize,
) {
    let mut rows = BTreeMap::new();
    let mut suppressed = 0usize;
    for kind in DeviceKind::ALL {
        if !selector.includes(kind) {
            continue;
        }
        let complete = !inventory.truncated
            && inventory.providers.iter().any(|provider| {
                provider.kind == kind
                    && provider.state == ProviderState::Complete
                    && !provider.truncated
            });
        if complete {
            rows.insert(
                kind,
                Some(
                    inventory
                        .devices
                        .iter()
                        .filter(|device| device.kind == kind)
                        .cloned()
                        .map(|device| (device.id.clone(), device))
                        .collect(),
                ),
            );
        } else {
            suppressed = suppressed.saturating_add(1);
            rows.insert(kind, None);
        }
    }
    (rows, suppressed)
}

fn append_diff(
    events: &mut Vec<DeviceWatchEvent>,
    previous: &BTreeMap<String, DeviceRecord>,
    current: &BTreeMap<String, DeviceRecord>,
    at_ms: u64,
    event_max: usize,
    truncated: &mut bool,
) {
    for (event, device) in previous
        .iter()
        .filter(|(id, _)| !current.contains_key(*id))
        .map(|(_, device)| ("removed", device))
        .chain(
            current
                .iter()
                .filter(|(id, _)| !previous.contains_key(*id))
                .map(|(_, device)| ("added", device)),
        )
        .chain(current.iter().filter_map(|(id, device)| {
            previous
                .get(id)
                .filter(|old| *old != device)
                .map(|_| ("changed", device))
        }))
    {
        if events.len() >= event_max {
            *truncated = true;
            return;
        }
        events.push(DeviceWatchEvent {
            event,
            at_ms,
            device: device.clone(),
        });
    }
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn platform_selector(selector: DeviceInventorySelector) -> DeviceSelector {
    match selector {
        DeviceInventorySelector::Usb => DeviceSelector::Usb,
        DeviceInventorySelector::Bluetooth => DeviceSelector::Bluetooth,
        DeviceInventorySelector::Audio => DeviceSelector::Audio,
        DeviceInventorySelector::Camera => DeviceSelector::Camera,
        DeviceInventorySelector::Gpu => DeviceSelector::Gpu,
        DeviceInventorySelector::All => DeviceSelector::All,
    }
}

fn selector_name(selector: DeviceInventorySelector) -> &'static str {
    match selector {
        DeviceInventorySelector::Usb => "usb",
        DeviceInventorySelector::Bluetooth => "bluetooth",
        DeviceInventorySelector::Audio => "audio",
        DeviceInventorySelector::Camera => "camera",
        DeviceInventorySelector::Gpu => "gpu",
        DeviceInventorySelector::All => "all",
    }
}

fn inventory_value(inventory: DeviceInventory) -> Result<Value, CuError> {
    let observed = inventory.devices.len();
    let mut devices = Vec::with_capacity(inventory.devices.len());
    let mut encoded_rows = 0usize;
    let row_budget = RESPONSE_CEILING_BYTES - RESPONSE_HEADROOM_BYTES;
    for device in inventory.devices {
        let row = device_value(device);
        let bytes = serde_json::to_vec(&row)
            .map_err(|error| CuError::new("device_inventory_encode_failed", error.to_string()))?
            .len();
        if encoded_rows.saturating_add(bytes) > row_budget {
            break;
        }
        encoded_rows += bytes;
        devices.push(row);
    }
    let returned = devices.len();
    let response_truncated = inventory.truncated || returned < observed;
    let providers = inventory
        .providers
        .into_iter()
        .map(provider_value)
        .collect::<Vec<_>>();
    Ok(json!({
        "devices": devices,
        "providers": providers,
        "returned": returned,
        "truncated": response_truncated,
        "complete": inventory.complete && !response_truncated,
        "identity_scope": "installation",
        "response_ceiling_bytes": RESPONSE_CEILING_BYTES,
    }))
}

fn provider_value(provider: agenterm_platform::device_inventory::DeviceProviderStatus) -> Value {
    json!({
        "kind": kind_name(provider.kind),
        "state": provider_state_name(provider.state),
        "provider": provider.provider,
        "visited": provider.visited,
        "read_errors": provider.read_errors,
        "truncated": provider.truncated,
        "code": provider.code,
    })
}

fn device_value(device: DeviceRecord) -> Value {
    json!({
        "id": device.id,
        "identity_continuity": match device.identity_continuity {
            DeviceIdentityContinuity::ProviderStable => "provider-stable",
            DeviceIdentityContinuity::Topology => "topology",
        },
        "kind": kind_name(device.kind),
        "name": device.name,
        "vendor": device.vendor,
        "model": device.model,
        "transport": device.transport,
    })
}

fn kind_name(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Usb => "usb",
        DeviceKind::Bluetooth => "bluetooth",
        DeviceKind::Audio => "audio",
        DeviceKind::Camera => "camera",
        DeviceKind::Gpu => "gpu",
    }
}

fn provider_state_name(state: ProviderState) -> &'static str {
    match state {
        ProviderState::Complete => "complete",
        ProviderState::Partial => "partial",
        ProviderState::Unavailable => "unavailable",
    }
}

fn device_inventory_error(error: DeviceInventoryError) -> CuError {
    let code = match error.kind() {
        DeviceInventoryErrorKind::InvalidLimit => "device_inventory_invalid_limit",
        DeviceInventoryErrorKind::IdentityMissing => "device_identity_uninitialized",
        DeviceInventoryErrorKind::IdentityInvalid => "device_identity_invalid",
        DeviceInventoryErrorKind::PermissionDenied => "device_inventory_permission_denied",
        DeviceInventoryErrorKind::ProviderFailed => "device_inventory_provider_failed",
        DeviceInventoryErrorKind::Timeout => "device_inventory_timeout",
        DeviceInventoryErrorKind::OutputLimit => "device_inventory_provider_output_limit",
        DeviceInventoryErrorKind::MalformedSnapshot => "device_inventory_malformed_snapshot",
        DeviceInventoryErrorKind::ResourceLimit => "device_inventory_resource_limit",
        DeviceInventoryErrorKind::CleanupFailed => "device_inventory_cleanup_failed",
        _ => "device_inventory_failed",
    };
    CuError::new(code, error.detail())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenterm_platform::device_inventory::DeviceProviderStatus;

    #[test]
    fn public_shape_omits_provider_private_identity() {
        let value = inventory_value(DeviceInventory {
            devices: vec![DeviceRecord {
                id: "agt-device-v1-example".into(),
                identity_continuity: DeviceIdentityContinuity::ProviderStable,
                kind: DeviceKind::Usb,
                name: Some("Fixture".into()),
                vendor: Some("Example".into()),
                model: None,
                transport: Some("usb".into()),
            }],
            providers: vec![DeviceProviderStatus {
                kind: DeviceKind::Usb,
                state: ProviderState::Complete,
                provider: "fixture",
                visited: 1,
                read_errors: 0,
                truncated: false,
                code: None,
            }],
            truncated: false,
            complete: true,
        })
        .unwrap();
        assert_eq!(value["identity_scope"], "installation");
        assert_eq!(value["returned"], 1);
        let row = &value["devices"][0];
        assert!(row.get("serial").is_none());
        assert!(row.get("path").is_none());
        assert!(row.get("address").is_none());
        assert!(row.get("instance_id").is_none());
    }

    fn record(id: &str, name: &str) -> DeviceRecord {
        DeviceRecord {
            id: id.into(),
            identity_continuity: DeviceIdentityContinuity::ProviderStable,
            kind: DeviceKind::Usb,
            name: Some(name.into()),
            vendor: Some("Example".into()),
            model: None,
            transport: Some("usb".into()),
        }
    }

    fn sample(
        state: ProviderState,
        truncated: bool,
        devices: Vec<DeviceRecord>,
    ) -> DeviceInventory {
        DeviceInventory {
            devices,
            providers: vec![DeviceProviderStatus {
                kind: DeviceKind::Usb,
                state,
                provider: "fixture",
                visited: 3,
                read_errors: usize::from(state != ProviderState::Complete),
                truncated,
                code: (state != ProviderState::Complete).then_some("fixture-incomplete"),
            }],
            truncated,
            complete: state == ProviderState::Complete && !truncated,
        }
    }

    #[test]
    fn complete_snapshots_emit_stable_add_remove_and_change_events() {
        let mut state = WatchState::new(
            DeviceSelector::Usb,
            sample(
                ProviderState::Complete,
                false,
                vec![record("a", "old"), record("b", "removed")],
            ),
        );
        state.observe(
            sample(
                ProviderState::Complete,
                false,
                vec![record("a", "changed"), record("c", "added")],
            ),
            10,
        );
        let observed = state
            .events
            .iter()
            .map(|event| (event.event, event.device.id.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            observed,
            vec![("removed", "b"), ("added", "c"), ("changed", "a")]
        );
        assert!(state.coverage_complete);
        assert_eq!(state.suppressed_provider_samples, 0);
    }

    #[test]
    fn incomplete_transition_suppresses_false_events_and_reseeds_baseline() {
        let mut state = WatchState::new(
            DeviceSelector::Usb,
            sample(ProviderState::Complete, false, vec![record("a", "old")]),
        );
        state.observe(sample(ProviderState::Partial, false, Vec::new()), 10);
        state.observe(
            sample(ProviderState::Complete, false, vec![record("b", "new")]),
            10,
        );
        assert!(state.events.is_empty());
        assert!(!state.coverage_complete);
        assert_eq!(state.suppressed_provider_samples, 1);

        state.observe(
            sample(
                ProviderState::Complete,
                false,
                vec![record("b", "new"), record("c", "later")],
            ),
            10,
        );
        assert_eq!(state.events.len(), 1);
        assert_eq!(state.events[0].event, "added");
        assert_eq!(state.events[0].device.id, "c");
    }

    #[test]
    fn event_ceiling_is_fail_closed_and_marked_truncated() {
        let mut state = WatchState::new(
            DeviceSelector::Usb,
            sample(ProviderState::Complete, false, Vec::new()),
        );
        state.observe(
            sample(
                ProviderState::Complete,
                false,
                vec![record("a", "one"), record("b", "two")],
            ),
            1,
        );
        assert_eq!(state.events.len(), 1);
        assert!(state.truncated);
    }

    #[test]
    fn globally_truncated_snapshot_cannot_prove_kind_events() {
        let (rows, suppressed) = complete_rows(
            DeviceSelector::Usb,
            &sample(ProviderState::Complete, true, vec![record("a", "one")]),
        );
        assert_eq!(rows.get(&DeviceKind::Usb), Some(&None));
        assert_eq!(suppressed, 1);
    }
}
