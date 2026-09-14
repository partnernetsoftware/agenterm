//! Bounded peripheral inventory with installation-scoped opaque identities.

use agenterm_platform::device_inventory::{
    DeviceIdentityContinuity, DeviceInventory, DeviceInventoryError, DeviceInventoryErrorKind,
    DeviceKind, DeviceRecord, DeviceSelector, ProviderState,
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

use crate::execution_control::ExecutionControl;
use crate::{DeviceInventorySelector, reply::CuError, target_binding::CurrentIdentityProvider};

const RESPONSE_CEILING_BYTES: usize = 1024 * 1024;
const RESPONSE_HEADROOM_BYTES: usize = 4096;

#[derive(Clone, Copy)]
struct DeviceWatchRequest {
    selector: DeviceInventorySelector,
    max: usize,
    duration_ms: u64,
    interval_ms: u64,
    event_max: usize,
}

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
    control: ExecutionControl<'_>,
) -> Result<Value, CuError> {
    let provider = CurrentIdentityProvider::default_for_current_user().map_err(|_| {
        CuError::new(
            "device_identity_unavailable",
            "the private installation identity directory is unavailable",
        )
    })?;
    let request = DeviceWatchRequest {
        selector,
        max,
        duration_ms,
        interval_ms,
        event_max,
    };
    let platform_selector = platform_selector(selector);
    let private_state_dir = provider.private_state_dir().to_path_buf();
    device_watch_with_sampler(request, control, &move |timeout| {
        enumerate_watch_sample(&private_state_dir, platform_selector, max, timeout)
    })
}

/// The bounded device-watch loop, parameterized over the inventory sampler.
///
/// The sampler is a borrowed closure so the cancellation cases can drive the
/// real loop; production passes `enumerate_watch_sample` through the closure
/// above, so the tested loop is the shipped loop.
fn device_watch_with_sampler(
    request: DeviceWatchRequest,
    control: ExecutionControl<'_>,
    sampler: &dyn Fn(Duration) -> Result<DeviceInventory, CuError>,
) -> Result<Value, CuError> {
    let started = Instant::now();
    let duration = Duration::from_millis(request.duration_ms);
    let deadline = started.checked_add(duration).ok_or_else(|| {
        CuError::new(
            "device_watch_deadline_overflow",
            "device watch deadline could not be represented",
        )
    })?;
    let platform_selector = platform_selector(request.selector);
    // PRE-FIRST-SAMPLE CANCEL: checked before the first platform inventory call,
    // so a cancelled watch here has issued zero samples and the existing
    // `effect: not_performed` claim is exactly true.
    control.check_observe()?;
    let first = sampler(remaining(deadline)?)?;
    let mut state = WatchState::new(platform_selector, first);

    while Instant::now() < deadline && state.events.len() < request.event_max {
        let sleep_for = Duration::from_millis(request.interval_ms)
            .min(deadline.saturating_duration_since(Instant::now()));
        if sleep_for.is_zero() {
            break;
        }
        // The pause observes the token through 10 ms slices; the total pause and
        // the overall deadline semantics are unchanged. A token set during the
        // previous round's authority call is noticed here, before the next
        // sample, and never displaces that round's result.
        //
        // ORDERING: a normal outcome that is ALREADY reached outranks the token. If
        // the pause consumed the rest of the duration, the watch ended by duration --
        // not by cancellation -- so a token that only became visible at the end of
        // the final pause must not rewrite an already-reached bound into a
        // cancellation. Only a watch still inside its normal bound may cancel.
        // (This branch is a defence in depth: the same-round case, where a sample
        // returns at the deadline, is the one covered by an owning test.)
        let paused_cancelled = control.sleep_until_cancelled(Instant::now() + sleep_for);
        if Instant::now() >= deadline {
            break;
        }
        if paused_cancelled {
            return cancelled_after_samples(state, request, started);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let minimum_sample_budget =
            Duration::from_millis(request.interval_ms).min(Duration::from_secs(1));
        if remaining < minimum_sample_budget {
            break;
        }
        // LAST-MOMENT CANCEL: only before a LATER authority call, and only while the
        // watch is still inside its normal bound. A same-round sample result,
        // provider error, event-limit or deadline below stays authoritative for
        // this round.
        if Instant::now() >= deadline {
            break;
        }
        if control.is_cancelled() {
            return cancelled_after_samples(state, request, started);
        }
        let sample = sampler(remaining)?;
        state.observe(sample, request.event_max);
    }

    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    // `into_value` remains the SOLE encoder: public device projection, row
    // truncation and the 1 MiB ceiling all happen here before the value is
    // returned or attached to a cancellation error.
    state.into_value(request, elapsed_ms, None)
}

/// The post-sample cancellation outcome: a named `cancelled` failure whose
/// structured detail carries the COMPLETE public partial watch payload.
///
/// The payload is produced by the same `WatchState::into_value` encoder as a
/// normal watch, so it has passed the installation-scoped public device
/// projection, row truncation and the 1 MiB ceiling before it is attached. No
/// raw `DeviceRecord` is ever serialized on this path. `effect` deliberately
/// says an observation was partially performed, because claiming
/// `not_performed` after a sample would be false.
fn cancelled_after_samples(
    state: WatchState,
    request: DeviceWatchRequest,
    started: Instant,
) -> Result<Value, CuError> {
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let partial = state.into_value(request, elapsed_ms, Some(WatchTermination::Cancelled))?;
    Err(CuError::new(
        "cancelled",
        "the device watch was cancelled after observation began",
    )
    .with_detail(serde_json::json!({
        "effect": "partially_performed",
        "phase": "observe_wait",
        "partial_observation": partial,
    })))
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
        request: DeviceWatchRequest,
        elapsed_ms: u64,
        termination_override: Option<WatchTermination>,
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
        // The normal outcome is derived from the SAME `returned` the payload
        // publishes, exactly as before. Only a cancelled partial overrides it, so
        // the nested evidence never claims `duration` for a watch that stopped for
        // cancellation, and the normal wire bytes are unchanged.
        let termination = termination_override
            .unwrap_or_else(|| WatchTermination::completed(returned, request.event_max));
        let value = json!({
            "selector": selector_name(request.selector),
            "max": request.max,
            "duration_ms": request.duration_ms,
            "interval_ms": request.interval_ms,
            "event_max": request.event_max,
            "elapsed_ms": elapsed_ms,
            "samples": self.samples,
            "events": events,
            "providers": providers,
            "returned": returned,
            "truncated": response_truncated,
            "coverage_complete": self.coverage_complete,
            "suppressed_provider_samples": self.suppressed_provider_samples,
            "termination": termination.as_str(),
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

/// Why a bounded watch stopped. The normal paths keep exactly the two existing
/// wire values; `Cancelled` is used only for the nested partial observation
/// inside a `cancelled` error detail, so the top-level success contract is
/// unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WatchTermination {
    Duration,
    EventLimit,
    Cancelled,
}

impl WatchTermination {
    fn as_str(self) -> &'static str {
        match self {
            WatchTermination::Duration => "duration",
            WatchTermination::EventLimit => "event-limit",
            WatchTermination::Cancelled => "cancelled",
        }
    }

    /// The normal observation outcome, derived exactly as before.
    fn completed(returned: usize, event_max: usize) -> Self {
        if returned >= event_max {
            WatchTermination::EventLimit
        } else {
            WatchTermination::Duration
        }
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

    // ---- cooperative cancellation -------------------------------------------------

    /// Drives the REAL bounded loop (`device_watch_with_sampler`) with an injected
    /// sampler. Production reaches the same function through
    /// `device_watch_payload`, so these cases cannot pass against a decision the
    /// shipped loop would not take.
    fn run_watch(
        duration_ms: u64,
        interval_ms: u64,
        event_max: usize,
        control: ExecutionControl<'_>,
        sampler: &dyn Fn(Duration) -> Result<DeviceInventory, CuError>,
    ) -> Result<Value, CuError> {
        device_watch_with_sampler(
            DeviceWatchRequest {
                selector: DeviceInventorySelector::Usb,
                max: 50,
                duration_ms,
                interval_ms,
                event_max,
            },
            control,
            sampler,
        )
    }

    #[test]
    fn pre_first_sample_cancel_issues_zero_inventory_calls() {
        // A token already set must stop the watch before the first platform
        // inventory call, with the existing `not_performed` shape.
        let calls = std::cell::Cell::new(0usize);
        let probe = || true;
        let sampler = |_: Duration| -> Result<DeviceInventory, CuError> {
            calls.set(calls.get() + 1);
            Ok(sample(
                ProviderState::Complete,
                false,
                vec![record("a", "one")],
            ))
        };
        let error = run_watch(
            60_000,
            50,
            8,
            ExecutionControl::with_cancel_probe(&probe),
            &sampler,
        )
        .expect_err("a pre-first-sample cancel must refuse the watch");
        assert_eq!(error.code, "cancelled");
        assert_eq!(calls.get(), 0, "no platform inventory call may be issued");
        let detail = error.detail.expect("the cancellation carries detail");
        assert_eq!(detail["effect"], "not_performed");
        assert!(
            detail.get("partial_observation").is_none(),
            "nothing was observed, so no partial payload may be claimed"
        );
    }

    #[test]
    fn post_sample_cancel_reports_shaped_partial_evidence_after_a_real_event() {
        // Round 1 returns a baseline and leaves the token clear, so a second round
        // really is attempted. Round 2 returns a CHANGED sample that produces an
        // `added` event, and only then sets the token. The following pause must
        // observe it, over an observation that genuinely contains an event, so the
        // nested privacy shaping is actually exercised.
        let token = std::cell::Cell::new(false);
        let calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let sampler = |_: Duration| -> Result<DeviceInventory, CuError> {
            let n = calls.get() + 1;
            calls.set(n);
            let inventory = if n == 1 {
                sample(ProviderState::Complete, false, vec![record("a", "one")])
            } else {
                sample(
                    ProviderState::Complete,
                    false,
                    vec![record("a", "one"), record("b", "two")],
                )
            };
            // Round 2 is the one that produces the event AND flips the token, so the
            // observation being preserved genuinely contains an event.
            if n == 2 {
                token.set(true);
            }
            Ok(inventory)
        };
        let error = run_watch(
            60_000,
            50,
            8,
            ExecutionControl::with_cancel_probe(&probe),
            &sampler,
        )
        .expect_err("a post-sample cancel must refuse the watch");
        assert_eq!(error.code, "cancelled");
        assert!(token.get(), "the sampler really did flip the token");
        assert_eq!(calls.get(), 2, "the token must stop the third sample");

        let detail = error.detail.expect("the cancellation carries detail");
        assert_eq!(detail["effect"], "partially_performed");
        assert_eq!(detail["phase"], "observe_wait");

        let partial = &detail["partial_observation"];
        // The nested evidence names the REAL reason it stopped.
        assert_eq!(partial["termination"], "cancelled");
        // And carries the complete shaped bounded payload from both samples.
        assert_eq!(partial["samples"], 2);
        assert_eq!(partial["selector"], "usb");
        assert_eq!(partial["max"], 50);
        assert_eq!(partial["identity_scope"], "installation");
        assert_eq!(partial["response_ceiling_bytes"], 1_048_576);
        assert_eq!(partial["coverage_complete"], true);
        assert_eq!(partial["suppressed_provider_samples"], 0);
        // The event from round 2 survived the cancellation, and it was shaped by
        // `device_value`, so no raw provider identity leaked.
        let events = partial["events"].as_array().expect("events array");
        assert_eq!(events.len(), 1, "the same-round event must be preserved");
        assert_eq!(events[0]["event"], "added");
        let device = &events[0]["device"];
        assert!(device.get("id").is_some(), "a public opaque id is expected");
        assert!(device.get("serial").is_none());
        assert!(device.get("path").is_none());
        assert!(device.get("address").is_none());
        assert!(device.get("instance_id").is_none());
        // Revealing identity continuity is the public projection, not a raw record.
        assert!(device.get("name").is_some());
        assert!(device.get("vendor").is_some());
    }

    #[test]
    fn a_same_round_event_limit_win_over_a_token_that_round_flipped() {
        // Round 1 supplies one device and leaves the token clear. Round 2 flips the
        // token AND returns a sample that reaches the event ceiling. The
        // pre-second-sample check therefore saw false, and the same-round
        // event-limit result must win with the event still present.
        let token = std::cell::Cell::new(false);
        let calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let sampler = |_: Duration| -> Result<DeviceInventory, CuError> {
            let n = calls.get() + 1;
            calls.set(n);
            let inventory = if n == 1 {
                sample(ProviderState::Complete, false, vec![record("a", "one")])
            } else {
                sample(
                    ProviderState::Complete,
                    false,
                    vec![record("a", "one"), record("b", "two")],
                )
            };
            if n == 2 {
                token.set(true);
            }
            Ok(inventory)
        };
        // event_max = 1, so round 2 reaches the ceiling and the loop ends normally.
        let value = run_watch(
            60_000,
            50,
            1,
            ExecutionControl::with_cancel_probe(&probe),
            &sampler,
        )
        .expect("the same-round event-limit result must win over the flipped token");
        assert!(token.get(), "the sampler really did flip the token");
        assert_eq!(calls.get(), 2, "round 2 really happened");
        assert_eq!(value["termination"], "event-limit");
        let events = value["events"].as_array().expect("events array");
        assert_eq!(events.len(), 1, "the same-round event must remain present");
        assert_eq!(events[0]["event"], "added");
    }

    #[test]
    fn a_same_round_provider_error_win_over_a_token_that_round_flipped() {
        // Round 1 leaves the token clear, so round 2 is attempted. Round 2 flips the
        // token and then fails. The provider error must be the named outcome.
        let token = std::cell::Cell::new(false);
        let calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let sampler = |_: Duration| -> Result<DeviceInventory, CuError> {
            let n = calls.get() + 1;
            calls.set(n);
            if n == 1 {
                return Ok(sample(
                    ProviderState::Complete,
                    false,
                    vec![record("a", "one")],
                ));
            }
            token.set(true);
            Err(CuError::new(
                "device_watch_fixture_provider_error",
                "the fixture provider refused the later sample",
            ))
        };
        let error = run_watch(
            60_000,
            50,
            8,
            ExecutionControl::with_cancel_probe(&probe),
            &sampler,
        )
        .expect_err("the provider error must surface");
        assert!(token.get(), "the sampler really did flip the token");
        assert_eq!(error.code, "device_watch_fixture_provider_error");
        assert_eq!(calls.get(), 2, "the second sample really was attempted");
    }

    #[test]
    fn a_same_round_sample_returning_at_the_deadline_beats_its_late_token() {
        // A same-round authority result that returns at or after the overall deadline
        // ends the watch by duration, not by the token that was set during that same
        // call. The sleep is inside the FIRST sampler, so on return the loop
        // condition is already false and no pause runs: this pins the
        // sample-return-versus-deadline case, not the end-of-pause case. Production
        // additionally re-checks the deadline immediately after each pause.
        let token = std::cell::Cell::new(false);
        let calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let sampler = |timeout: Duration| -> Result<DeviceInventory, CuError> {
            let n = calls.get() + 1;
            calls.set(n);
            if n == 1 {
                // Burn the whole allowance inside the authority call, then flip the
                // token, so the result and the token arrive in the same round.
                std::thread::sleep(timeout);
                token.set(true);
            }
            Ok(sample(
                ProviderState::Complete,
                false,
                vec![record("a", "one")],
            ))
        };
        let value = run_watch(
            30,
            60_000,
            8,
            ExecutionControl::with_cancel_probe(&probe),
            &sampler,
        )
        .expect("the reached duration bound must win over the late token");
        assert!(token.get(), "the token really was set");
        assert_eq!(calls.get(), 1, "no second sample was attempted");
        assert_eq!(value["termination"], "duration");
        assert_eq!(value["samples"], 1);
    }

    #[test]
    fn an_uncancelled_watch_keeps_its_normal_shape_and_termination() {
        // No token ever: the ordinary duration outcome and wire shape are unchanged.
        let calls = std::cell::Cell::new(0usize);
        let sampler = |_: Duration| -> Result<DeviceInventory, CuError> {
            calls.set(calls.get() + 1);
            Ok(sample(
                ProviderState::Complete,
                false,
                vec![record("a", "one")],
            ))
        };
        let value = run_watch(60, 50, 8, ExecutionControl::none(), &sampler)
            .expect("an uncancelled watch must succeed");
        assert_eq!(value["termination"], "duration");
        assert_eq!(value["selector"], "usb");
        assert_eq!(value["samples"], 1);
        assert_eq!(value["max"], 50);
        assert_eq!(value["identity_scope"], "installation");
        assert_eq!(value["response_ceiling_bytes"], 1_048_576);
        assert_eq!(value["returned"], 0);
    }

    #[test]
    fn an_uncancelled_watch_reaching_the_event_ceiling_still_says_event_limit() {
        // The event-limit value is unchanged for a normal watch.
        let calls = std::cell::Cell::new(0usize);
        let sampler = |_: Duration| -> Result<DeviceInventory, CuError> {
            let n = calls.get() + 1;
            calls.set(n);
            if n == 1 {
                Ok(sample(
                    ProviderState::Complete,
                    false,
                    vec![record("a", "one")],
                ))
            } else {
                Ok(sample(
                    ProviderState::Complete,
                    false,
                    vec![record("a", "one"), record("b", "two")],
                ))
            }
        };
        let value = run_watch(60_000, 50, 1, ExecutionControl::none(), &sampler)
            .expect("the event ceiling ends the watch normally");
        assert_eq!(value["termination"], "event-limit");
        assert_eq!(value["samples"], 2);
    }
}
