//! Bounded application-level lifecycle observation.
//!
//! Application selectors are frozen through `app-facts`; process rows are
//! admitted only when their complete executable path matches that frozen
//! identity. Events describe the aggregate zero/non-zero instance boundary,
//! not helper-process churn.

use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime},
};

use serde_json::{Value, json};

use crate::CuError;

use super::app_facts::{app_facts_error, query_app_facts};

const DEFAULT_INTERVAL_MS: u64 = 1_000;
const DEFAULT_MAX_EVENTS: usize = 256;
const DEFAULT_MAX_PROCESSES: usize = 1_000;
const MAX_DURATION_MS: u64 = 86_400_000;
const MAX_INTERVAL_MS: u64 = 60_000;
const MAX_EVENTS: usize = 4_096;
const MAX_PROCESSES: usize = 5_000;

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    length: u64,
    modified: Option<SystemTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AppIdentity {
    name: Option<String>,
    bundle: Option<String>,
    path: Option<String>,
    executable: String,
    file: FileIdentity,
}

#[derive(Clone, Debug)]
struct AppBinding {
    index: usize,
    selector: String,
    executable_path: PathBuf,
    executable_name: String,
    identity: AppIdentity,
}

#[derive(Clone, Debug)]
struct AppProcess {
    pid: u32,
    executable_name: String,
    start_identity: String,
}

type ProcessKey = (u32, String);
type Instances = BTreeMap<ProcessKey, AppProcess>;

struct AppSnapshot {
    instances: Vec<Instances>,
    excluded_unidentified: usize,
}

impl AppProcess {
    fn key(&self) -> ProcessKey {
        (self.pid, self.start_identity.clone())
    }

    fn json(&self) -> Value {
        json!({
            "pid": self.pid,
            "start_identity": self.start_identity,
            "executable_name": self.executable_name,
        })
    }
}

fn path_key(path: &Path) -> String {
    if cfg!(windows) {
        path.as_os_str().to_string_lossy().to_uppercase()
    } else {
        path.as_os_str().to_string_lossy().into_owned()
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}

fn same_executable_name(left: &str, right: &str) -> bool {
    if cfg!(windows) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

fn matching_binding<'a>(
    bindings: &'a [AppBinding],
    executable_name: &str,
    executable_path: &Path,
) -> Option<&'a AppBinding> {
    bindings.iter().find(|binding| {
        same_executable_name(executable_name, &binding.executable_name)
            && same_path(executable_path, &binding.executable_path)
    })
}

fn file_identity(path: &Path) -> Result<FileIdentity, CuError> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        CuError::new(
            "app_watch_executable_unavailable",
            format!("application executable metadata is unavailable: {error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(CuError::new(
            "app_watch_executable_unavailable",
            "application executable is not a regular file",
        ));
    }
    let modified = metadata.modified().map_err(|error| {
        CuError::new(
            "app_watch_executable_unavailable",
            format!("application executable modification identity is unavailable: {error}"),
        )
    })?;
    Ok(FileIdentity {
        length: metadata.len(),
        modified: Some(modified),
    })
}

fn resolve_binding(index: usize, selector: &str) -> Result<AppBinding, CuError> {
    let facts = query_app_facts(
        selector,
        agenterm_platform::app_facts::AppFactsOptions::default(),
    )
    .map_err(app_facts_error)?;
    let executable = facts.executable.value.clone().ok_or_else(|| {
        CuError::new(
            "app_watch_executable_unavailable",
            "application selector has no canonical executable identity",
        )
        .with_detail(json!({
            "selector": selector,
            "status": format!("{:?}", facts.executable.status),
            "reason": facts.executable.reason,
        }))
    })?;
    let executable_path = PathBuf::from(&executable);
    let executable_name = executable_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            CuError::new(
                "app_watch_executable_unavailable",
                "application executable has no UTF-8 basename",
            )
            .with_detail(json!({ "selector": selector }))
        })?
        .to_owned();
    let file = file_identity(&executable_path).map_err(|error| {
        error.with_detail(json!({
            "selector": selector,
            "executable": executable,
        }))
    })?;
    Ok(AppBinding {
        index,
        selector: selector.to_owned(),
        executable_path,
        executable_name,
        identity: AppIdentity {
            name: facts.name.value,
            bundle: facts.bundle.value,
            path: facts.path.value,
            executable,
            file,
        },
    })
}

fn reject_duplicate_executables(bindings: &[AppBinding]) -> Result<(), CuError> {
    for (index, binding) in bindings.iter().enumerate() {
        if let Some(previous) = bindings[..index]
            .iter()
            .find(|previous| same_path(&previous.executable_path, &binding.executable_path))
        {
            return Err(CuError::new(
                "app_watch_selector_duplicate",
                "two application selectors resolved to the same canonical executable",
            )
            .with_detail(json!({
                "selector": binding.selector,
                "previous_selector": previous.selector,
                "executable": binding.identity.executable,
            })));
        }
    }
    Ok(())
}

fn resolve_bindings(selectors: &[String]) -> Result<Vec<AppBinding>, CuError> {
    let bindings = selectors
        .iter()
        .enumerate()
        .map(|(index, selector)| resolve_binding(index, selector))
        .collect::<Result<Vec<_>, _>>()?;
    reject_duplicate_executables(&bindings)?;
    Ok(bindings)
}

fn app_snapshot(bindings: &[AppBinding], max_processes: usize) -> Result<AppSnapshot, CuError> {
    let rows = agenterm_platform::process::list().map_err(|error| {
        CuError::new("app_watch_inventory_failed", error.to_string()).with_detail(json!({
            "kind": format!("{:?}", error.kind()),
        }))
    })?;
    let candidates = rows
        .into_iter()
        .filter(|row| {
            bindings
                .iter()
                .any(|binding| same_executable_name(&row.executable_name, &binding.executable_name))
        })
        .collect::<Vec<_>>();
    if candidates.len() > max_processes {
        return Err(CuError::new(
            "app_watch_inventory_too_large",
            "matched process candidate inventory exceeds --max-processes",
        )
        .with_detail(json!({
            "matched_candidates": candidates.len(),
            "max_processes": max_processes,
        })));
    }

    let mut instances = vec![BTreeMap::new(); bindings.len()];
    let mut excluded_unidentified = 0usize;
    for row in candidates {
        let process_path = match agenterm_platform::process_image::executable_path(row.id) {
            Ok(path) => path,
            Err(error)
                if error.kind()
                    == agenterm_platform::process_image::ProcessImageErrorKind::NotFound =>
            {
                continue;
            }
            Err(_) => {
                excluded_unidentified = excluded_unidentified.saturating_add(1);
                continue;
            }
        };
        let Some(binding) = matching_binding(bindings, &row.executable_name, &process_path) else {
            continue;
        };
        let start_identity = match agenterm_platform::process_observation::observe(row.id) {
            agenterm_platform::process_observation::ProcessObservation::Live {
                start_identity: Some(identity),
            } => identity,
            agenterm_platform::process_observation::ProcessObservation::Dead { .. } => continue,
            _ => {
                excluded_unidentified = excluded_unidentified.saturating_add(1);
                continue;
            }
        };
        let process = AppProcess {
            pid: row.id,
            executable_name: row.executable_name,
            start_identity,
        };
        instances[binding.index].insert(process.key(), process);
    }
    Ok(AppSnapshot {
        instances,
        excluded_unidentified,
    })
}

fn append_transitions(
    previous: &[Instances],
    current: &[Instances],
    t_ms: u64,
    max_events: usize,
    events: &mut Vec<Value>,
) -> bool {
    for (app_index, (before, after)) in previous.iter().zip(current).enumerate() {
        let transition = if before.is_empty() && !after.is_empty() {
            after.values().next().map(|process| ("launched", process))
        } else if !before.is_empty() && after.is_empty() {
            before.values().next_back().map(|process| ("quit", process))
        } else {
            None
        };
        let Some((kind, process)) = transition else {
            continue;
        };
        if events.len() == max_events {
            return true;
        }
        events.push(json!({
            "t_ms": t_ms,
            "app_index": app_index,
            "kind": kind,
            "instances": after.len(),
            "process": process.json(),
            "process_role": "boundary-representative",
        }));
        if events.len() == max_events {
            return true;
        }
    }
    false
}

fn revalidate_bindings(bindings: &[AppBinding]) -> Result<(), CuError> {
    for original in bindings {
        let current = resolve_binding(original.index, &original.selector).map_err(|error| {
            CuError::new(
                "app_watch_identity_drift",
                "application identity became unavailable during lifecycle watch",
            )
            .with_detail(json!({
                "selector": original.selector,
                "cause": { "code": error.code, "message": error.message },
            }))
        })?;
        if current.identity != original.identity {
            return Err(CuError::new(
                "app_watch_identity_drift",
                "application identity changed during lifecycle watch",
            )
            .with_detail(json!({
                "selector": original.selector,
                "expected_executable": original.identity.executable,
                "observed_executable": current.identity.executable,
                "expected_length": original.identity.file.length,
                "observed_length": current.identity.file.length,
            })));
        }
    }
    Ok(())
}

fn signal_ready() -> Result<(), CuError> {
    let Some(path) = std::env::var_os("AGENTERM_CU_APP_WATCH_READY_PATH") else {
        return Ok(());
    };
    let publish = || -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(b"ready\n")
    };
    publish().map_err(|error| {
        CuError::new(
            "app_watch_ready_signal_failed",
            format!("application watch readiness signal could not be published: {error}"),
        )
    })
}

fn binding_json(binding: &AppBinding) -> Value {
    json!({
        "index": binding.index,
        "selector": binding.selector,
        "app": {
            "name": binding.identity.name,
            "bundle": binding.identity.bundle,
            "path": binding.identity.path,
            "executable": binding.identity.executable,
        },
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn app_watch_payload(
    selectors: &[String],
    duration_ms: u64,
    interval_ms: Option<u64>,
    max_events: Option<usize>,
    max_processes: Option<usize>,
    control: crate::execution_control::ExecutionControl<'_>,
) -> Result<Value, CuError> {
    let interval_ms = interval_ms.unwrap_or(DEFAULT_INTERVAL_MS);
    let max_events = max_events.unwrap_or(DEFAULT_MAX_EVENTS);
    let max_processes = max_processes.unwrap_or(DEFAULT_MAX_PROCESSES);
    if selectors.is_empty()
        || selectors.len() > 16
        || selectors.iter().any(|selector| {
            selector.trim().is_empty()
                || selector.len() > agenterm_platform::app_facts::MAX_APP_FACTS_SELECTOR_BYTES
                || selector.as_bytes().contains(&0)
        })
        || !(1..=MAX_DURATION_MS).contains(&duration_ms)
        || !(1..=MAX_INTERVAL_MS).contains(&interval_ms)
        || !(1..=MAX_EVENTS).contains(&max_events)
        || !(1..=MAX_PROCESSES).contains(&max_processes)
    {
        return Err(CuError::new(
            "invalid_input",
            "app-watch requires 1..=16 non-empty selectors, duration-ms in 1..=86400000, interval-ms in 1..=60000, max-events in 1..=4096 and max-processes in 1..=5000",
        ));
    }

    control.check_observe()?;
    let bindings = resolve_bindings(selectors)?;
    control.check_observe()?;
    // From here on a baseline may exist, so NO direct `check_observe` may escape:
    // a post-sample cancellation is a partial observation, not a pre-effect stop.
    let request = AppWatchRequest {
        duration_ms,
        interval_ms,
        max_events,
        max_processes,
    };
    app_watch_with_providers(
        &bindings,
        request,
        control,
        app_snapshot,
        revalidate_bindings,
    )
}

/// The bounded app-watch request and its bounds, so the loop and the ONE encoder
/// share a description instead of repeating four parameters.
#[derive(Clone, Copy)]
struct AppWatchRequest {
    duration_ms: u64,
    interval_ms: u64,
    max_events: usize,
    max_processes: usize,
}

/// Everything the encoder needs about ONE bounded observation, normal or
/// cancelled. Both paths publish through `into_value`, so a cancelled
/// observation cannot bypass the identity projection or the bounds.
struct AppWatchState {
    /// The ORIGINAL baseline rows, captured once. Kept explicitly rather than
    /// derived from the advancing `previous`, so the published baseline always
    /// matches the one the caller was promised.
    baseline: Vec<Value>,
    excluded_unidentified: usize,
    events: Vec<Value>,
    truncated: bool,
}

/// The bounded app-watch loop, generic over the snapshot and revalidation
/// providers.
///
/// These are GENERIC parameters rather than trait objects or type aliases, so a
/// caller's closure may borrow its own locals and the compiler keeps the higher
/// ranked borrow intact. Production passes the real `app_snapshot` and
/// `revalidate_bindings`, so the tested loop is the shipped loop.
fn app_watch_with_providers<S, R>(
    bindings: &[AppBinding],
    request: AppWatchRequest,
    control: crate::execution_control::ExecutionControl<'_>,
    snapshot: S,
    revalidate: R,
) -> Result<Value, CuError>
where
    S: Fn(&[AppBinding], usize) -> Result<AppSnapshot, CuError>,
    R: Fn(&[AppBinding]) -> Result<(), CuError>,
{
    let AppWatchRequest {
        duration_ms,
        interval_ms,
        max_events,
        max_processes,
    } = request;
    let initial = snapshot(bindings, max_processes)?;
    let mut previous = initial.instances;
    let mut excluded_unidentified = initial.excluded_unidentified;
    let baseline = previous
        .iter()
        .enumerate()
        .flat_map(|(app_index, instances)| {
            instances.values().map(move |process| {
                json!({
                    "app_index": app_index,
                    "pid": process.pid,
                    "start_identity": process.start_identity,
                })
            })
        })
        .collect::<Vec<_>>();
    signal_ready()?;
    let started = Instant::now();
    let deadline = started + Duration::from_millis(duration_ms);
    let mut events = Vec::with_capacity(max_events.min(DEFAULT_MAX_EVENTS));
    let mut truncated = false;

    while Instant::now() < deadline {
        // POST-BASELINE PAUSE: a valid cancellation POINT, but it may only report a
        // private signal. It must never build a `not_performed` error, because the
        // baseline above already ran real work. The deadline is re-checked first so
        // a bound that has already been reached stays the authoritative outcome.
        if app_watch_pause(control, interval_ms, deadline) {
            if Instant::now() >= deadline {
                break;
            }
            return app_watch_cancelled(
                AppWatchState {
                    baseline,
                    excluded_unidentified,
                    events,
                    truncated,
                },
                bindings,
                request,
                &revalidate,
            );
        }
        if Instant::now() >= deadline {
            break;
        }
        // LAST-MOMENT CHECK before a later authority call, same private signal shape.
        if control.is_cancelled() {
            return app_watch_cancelled(
                AppWatchState {
                    baseline,
                    excluded_unidentified,
                    events,
                    truncated,
                },
                bindings,
                request,
                &revalidate,
            );
        }
        // The snapshot is consumed UNCONDITIONALLY once it returns; there is no
        // cancellation check between this call and the transition derivation below,
        // so a token flipped inside the authority call cannot discard the round.
        let next = snapshot(bindings, max_processes)?;
        excluded_unidentified = excluded_unidentified.max(next.excluded_unidentified);
        let current = next.instances;
        let t_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        truncated = append_transitions(&previous, &current, t_ms, max_events, &mut events);
        previous = current;
        if truncated {
            break;
        }
    }

    // FINAL BINDING AUTHORITY: revalidation runs on the normal path too, and its
    // failure outranks nothing here because nothing is pending. On the cancelled
    // path it runs BEFORE the cancellation outcome is built, so drift wins.
    revalidate(bindings)?;
    AppWatchState {
        baseline,
        excluded_unidentified,
        events,
        truncated,
    }
    .into_value(bindings, request, None)
}

/// Slice width for the inter-round pause. The token is observed before each slice
/// and once at the end. Returns `true` when cancelled; it never builds an error.
const APP_WATCH_CANCEL_SLICE: Duration = Duration::from_millis(10);

fn app_watch_pause(
    control: crate::execution_control::ExecutionControl<'_>,
    interval_ms: u64,
    deadline: Instant,
) -> bool {
    let sleep_deadline = Instant::now()
        + Duration::from_millis(interval_ms)
            .min(deadline.saturating_duration_since(Instant::now()));
    while Instant::now() < sleep_deadline {
        if control.is_cancelled() {
            return true;
        }
        thread::sleep(
            APP_WATCH_CANCEL_SLICE.min(sleep_deadline.saturating_duration_since(Instant::now())),
        );
    }
    control.is_cancelled()
}

/// The post-baseline cancellation outcome.
///
/// ORDERING IS THE POINT: binding revalidation runs FIRST and its failure wins,
/// because `app_watch_identity_drift`/unavailable is an authoritative statement
/// about whether the observation is still attributable to the same applications,
/// while cancellation is only a request to stop. Only after revalidation succeeds
/// is the shaped partial observation published.
fn app_watch_cancelled<R>(
    state: AppWatchState,
    bindings: &[AppBinding],
    request: AppWatchRequest,
    revalidate: &R,
) -> Result<Value, CuError>
where
    R: Fn(&[AppBinding]) -> Result<(), CuError>,
{
    revalidate(bindings)?;
    let partial = state.into_value(bindings, request, Some("cancelled"))?;
    Err(CuError::new(
        "cancelled",
        "the app watch was cancelled after observation began",
    )
    .with_detail(json!({
        "effect": "partially_performed",
        "phase": "observe_wait",
        "partial_observation": partial,
    })))
}

impl AppWatchState {
    /// The ONE encoder for normal and cancelled watches. `termination_override` is
    /// `None` on the normal path, so that wire shape gains NO new field; the
    /// cancelled path adds a nested `termination` and forces `completed: false`,
    /// while `coverage_complete` keeps its existing provider-coverage meaning.
    fn into_value(
        self,
        bindings: &[AppBinding],
        request: AppWatchRequest,
        termination_override: Option<&'static str>,
    ) -> Result<Value, CuError> {
        let emitted = self.events.len();
        let cancelled = termination_override.is_some();
        let selector_rows = bindings.iter().map(binding_json).collect::<Vec<_>>();
        let mut value = json!({
            "selectors": selector_rows,
            "baseline": self.baseline,
            "events": self.events,
            "emitted": emitted,
            "completed": !self.truncated && !cancelled,
            "truncated": self.truncated,
            "coverage_complete": self.excluded_unidentified == 0,
            "excluded_unidentified": self.excluded_unidentified,
            "duration_ms": request.duration_ms,
            "interval_ms": request.interval_ms,
            "max_events": request.max_events,
            "max_processes": request.max_processes,
            "identity": "app-selector+canonical-executable+pid+start-identity",
            "verified": true,
        });
        if let Some(termination) = termination_override {
            value["termination"] = json!(termination);
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, identity: &str) -> AppProcess {
        AppProcess {
            pid,
            executable_name: "fixture".into(),
            start_identity: identity.into(),
        }
    }

    fn instances(rows: &[(u32, &str)]) -> Instances {
        rows.iter()
            .map(|(pid, identity)| {
                let process = process(*pid, identity);
                (process.key(), process)
            })
            .collect()
    }

    // ---- cooperative cancellation: production-driver cases --------------------

    /// Drives the REAL loop (`app_watch_with_providers`) with injected snapshot and
    /// revalidation closures, so these cases cannot pass against a decision the
    /// shipped loop would not take.
    fn run_with_providers<S, R>(
        duration_ms: u64,
        interval_ms: u64,
        max_events: usize,
        control: crate::execution_control::ExecutionControl<'_>,
        snapshot: S,
        revalidate: R,
    ) -> Result<Value, CuError>
    where
        S: Fn(&[AppBinding], usize) -> Result<AppSnapshot, CuError>,
        R: Fn(&[AppBinding]) -> Result<(), CuError>,
    {
        app_watch_with_providers(
            &[fixture_binding()],
            AppWatchRequest {
                duration_ms,
                interval_ms,
                max_events,
                max_processes: 8,
            },
            control,
            snapshot,
            revalidate,
        )
    }

    /// A binding that never has to resolve a real application, because the seam
    /// injects both providers.
    fn fixture_binding() -> AppBinding {
        AppBinding {
            index: 0,
            selector: "fixture".into(),
            executable_path: PathBuf::from("/fixture/App"),
            executable_name: "App".into(),
            identity: AppIdentity {
                name: Some("Fixture".into()),
                bundle: None,
                path: Some("/fixture/App".into()),
                executable: "App".into(),
                file: FileIdentity {
                    length: 1,
                    modified: None,
                },
            },
        }
    }

    fn snapshot_with(rows: Vec<Instances>) -> AppSnapshot {
        AppSnapshot {
            instances: rows,
            excluded_unidentified: 0,
        }
    }

    /// A revalidation provider that always succeeds and counts its calls.
    fn ok_revalidate(
        calls: &std::cell::Cell<usize>,
    ) -> impl Fn(&[AppBinding]) -> Result<(), CuError> + '_ {
        move |_| {
            calls.set(calls.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn a_post_baseline_pause_cancel_reports_partial_evidence_and_stops_the_next_snapshot() {
        // Round 1 establishes the baseline and leaves the token clear; the token is
        // then raised from a test thread while the watch sits in a long pause. So the
        // cancellation lands AFTER the baseline, no round-2 snapshot runs, and the
        // outcome must be a truthful partial observation with complete baseline
        // evidence, not `not_performed`.
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let token = Arc::new(AtomicBool::new(false));
        let raised = Arc::clone(&token);
        let snapshots = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&snapshots);
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(25));
            raised.store(true, Ordering::Release);
        });
        let probe = || token.load(Ordering::Acquire);
        let revalidations = std::cell::Cell::new(0usize);
        let snapshot = move |_: &[AppBinding], _: usize| -> Result<AppSnapshot, CuError> {
            counted.fetch_add(1, Ordering::AcqRel);
            Ok(snapshot_with(vec![instances(&[(10, "a")])]))
        };
        let error = run_with_providers(
            60_000,
            60_000,
            8,
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            snapshot,
            ok_revalidate(&revalidations),
        )
        .expect_err("a post-baseline cancel must refuse the watch");
        trigger.join().expect("cancel trigger");

        assert_eq!(error.code, "cancelled");
        assert_eq!(
            snapshots.load(Ordering::Acquire),
            1,
            "the token must stop the second snapshot"
        );
        // Revalidation really ran before the cancellation outcome was built.
        assert_eq!(revalidations.get(), 1);
        let detail = error.detail.expect("detail");
        assert_eq!(detail["effect"], "partially_performed");
        assert_eq!(detail["phase"], "observe_wait");
        let partial = &detail["partial_observation"];
        assert_eq!(partial["termination"], "cancelled");
        assert_eq!(partial["completed"], false);
        assert_eq!(partial["verified"], true);
        assert_eq!(
            partial["identity"],
            "app-selector+canonical-executable+pid+start-identity"
        );
        assert_eq!(partial["coverage_complete"], true);
        // The complete shaped baseline survives the cancellation.
        let baseline = partial["baseline"].as_array().expect("baseline array");
        assert_eq!(baseline.len(), 1);
        assert_eq!(baseline[0]["pid"], 10);
        assert_eq!(partial["emitted"], 0);
        assert_eq!(partial["selectors"][0]["selector"], "fixture");
    }

    #[test]
    fn a_same_round_event_ceiling_win_over_a_token_that_round_flipped() {
        // Round 2 flips the token AND returns a snapshot whose transition reaches the
        // event ceiling. The normal truncated result must win and the event survive.
        let token = std::cell::Cell::new(false);
        let calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let revalidations = std::cell::Cell::new(0usize);
        let snapshot = |_: &[AppBinding], _: usize| -> Result<AppSnapshot, CuError> {
            let n = calls.get() + 1;
            calls.set(n);
            let rows = if n == 1 {
                Instances::new()
            } else {
                instances(&[(10, "a")])
            };
            if n == 2 {
                token.set(true);
            }
            Ok(snapshot_with(vec![rows]))
        };
        let value = run_with_providers(
            60_000,
            50,
            1,
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            snapshot,
            ok_revalidate(&revalidations),
        )
        .expect("the same-round event ceiling must win over the flipped token");
        assert!(token.get(), "the provider really did flip the token");
        assert_eq!(calls.get(), 2);
        let events = value["events"].as_array().expect("events array");
        assert_eq!(events.len(), 1, "the same-round event must remain present");
        assert_eq!(events[0]["kind"], "launched");
        // Shipped ceiling semantics: truncated while the deadline has not passed.
        assert_eq!(value["truncated"], true);
        assert_eq!(value["completed"], false);
        assert!(
            value.get("termination").is_none(),
            "the normal payload must not gain a termination field"
        );
        assert_eq!(revalidations.get(), 1, "normal path revalidates once");
    }

    #[test]
    fn a_same_round_snapshot_error_win_over_a_token_that_round_flipped() {
        // Round 2 flips the token and then fails. The provider error must surface and
        // revalidation must not have replaced it.
        let token = std::cell::Cell::new(false);
        let calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let revalidations = std::cell::Cell::new(0usize);
        let snapshot = |_: &[AppBinding], _: usize| -> Result<AppSnapshot, CuError> {
            let n = calls.get() + 1;
            calls.set(n);
            if n == 1 {
                return Ok(snapshot_with(vec![Instances::new()]));
            }
            token.set(true);
            Err(CuError::new(
                "app_watch_fixture_provider_error",
                "the fixture provider refused the later snapshot",
            ))
        };
        let error = run_with_providers(
            60_000,
            50,
            8,
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            snapshot,
            ok_revalidate(&revalidations),
        )
        .expect_err("the provider error must surface");
        assert!(token.get());
        assert_eq!(error.code, "app_watch_fixture_provider_error");
        assert_eq!(calls.get(), 2);
        // The authority error returned before any final binding work was attempted.
        assert_eq!(
            revalidations.get(),
            0,
            "a same-round provider error must not reach revalidation"
        );
    }

    #[test]
    fn a_cancel_after_an_event_below_the_ceiling_keeps_the_event_in_partial_evidence() {
        // Round 2 flips the token INSIDE its own snapshot and returns one transition,
        // staying below the ceiling. The token is observed in the NEXT pause, so the
        // partial evidence must still contain that already-observed event.
        let token = std::cell::Cell::new(false);
        let calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let revalidations = std::cell::Cell::new(0usize);
        let snapshot = |_: &[AppBinding], _: usize| -> Result<AppSnapshot, CuError> {
            let n = calls.get() + 1;
            calls.set(n);
            if n == 2 {
                token.set(true);
                return Ok(snapshot_with(vec![instances(&[(10, "a")])]));
            }
            Ok(snapshot_with(vec![Instances::new()]))
        };
        let error = run_with_providers(
            60_000,
            50,
            8,
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            snapshot,
            ok_revalidate(&revalidations),
        )
        .expect_err("a cancel after an event must return partial evidence");
        assert_eq!(calls.get(), 2);
        assert_eq!(revalidations.get(), 1);
        let detail = error.detail.expect("detail");
        assert_eq!(detail["effect"], "partially_performed");
        let partial = &detail["partial_observation"];
        assert_eq!(partial["termination"], "cancelled");
        assert_eq!(partial["completed"], false);
        let events = partial["events"].as_array().expect("events array");
        assert_eq!(events.len(), 1, "the observed event must be preserved");
        assert_eq!(events[0]["kind"], "launched");
        assert_eq!(partial["emitted"], 1);
    }

    #[test]
    fn binding_drift_wins_over_a_pending_cancellation() {
        // The token becomes pending only AFTER authority began: the initial snapshot
        // closure sets it while returning the baseline. So this models a real
        // production late cancellation, which the wrapper prechecks would not have
        // rejected, and the following pause observes it. Drift must still win.
        let token = std::cell::Cell::new(false);
        let calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let revalidations = std::cell::Cell::new(0usize);
        let snapshot = |_: &[AppBinding], _: usize| -> Result<AppSnapshot, CuError> {
            calls.set(calls.get() + 1);
            token.set(true);
            Ok(snapshot_with(vec![instances(&[(10, "a")])]))
        };
        let drift = |_: &[AppBinding]| -> Result<(), CuError> {
            revalidations.set(revalidations.get() + 1);
            Err(CuError::new(
                "app_watch_identity_drift",
                "application identity became unavailable during lifecycle watch",
            ))
        };
        let error = run_with_providers(
            60_000,
            60_000,
            8,
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            snapshot,
            drift,
        )
        .expect_err("drift must win over the pending cancellation");
        assert!(token.get(), "the initial snapshot really did set the token");
        assert_eq!(error.code, "app_watch_identity_drift");
        assert_eq!(revalidations.get(), 1, "revalidation runs exactly once");
        // The baseline snapshot still ran; the point is that no partial cancellation
        // replaced the drift outcome.
        assert_eq!(calls.get(), 1);
        let detail = error.detail.unwrap_or(Value::Null);
        assert_ne!(detail["effect"], "partially_performed");
    }

    #[test]
    fn a_same_round_deadline_win_over_a_token_that_round_flipped() {
        // Round 2 flips the token AND burns the remaining duration inside its own
        // snapshot call, so the deadline has passed when it returns. The normal
        // completion must win, and the token must really have been flipped rather than
        // the run merely timing out on an unflipped token.
        let token = std::cell::Cell::new(false);
        let calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let revalidations = std::cell::Cell::new(0usize);
        let snapshot = |_: &[AppBinding], _: usize| -> Result<AppSnapshot, CuError> {
            let n = calls.get() + 1;
            calls.set(n);
            if n == 2 {
                std::thread::sleep(Duration::from_millis(200));
                token.set(true);
                return Ok(snapshot_with(vec![instances(&[(10, "a")])]));
            }
            Ok(snapshot_with(vec![Instances::new()]))
        };
        let value = run_with_providers(
            200,
            20,
            8,
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            snapshot,
            ok_revalidate(&revalidations),
        )
        .expect("the reached deadline must win over the token flipped in that round");
        // Proves the token really flipped inside round 2.
        assert!(token.get(), "the provider really did flip the token");
        assert_eq!(calls.get(), 2, "round 2 really happened");
        assert_eq!(revalidations.get(), 1);
        // A normal result, not a cancellation.
        assert!(value.get("termination").is_none());
        assert_eq!(value["completed"], true);
        let events = value["events"].as_array().expect("events array");
        assert_eq!(events.len(), 1, "the round-2 event was derived before exit");
        assert_eq!(events[0]["kind"], "launched");
    }

    #[test]
    fn an_uncancelled_watch_keeps_its_normal_shape_and_has_no_termination() {
        let revalidations = std::cell::Cell::new(0usize);
        let snapshot = |_: &[AppBinding], _: usize| -> Result<AppSnapshot, CuError> {
            Ok(snapshot_with(vec![instances(&[(10, "a")])]))
        };
        let value = run_with_providers(
            60,
            50,
            8,
            crate::execution_control::ExecutionControl::none(),
            snapshot,
            ok_revalidate(&revalidations),
        )
        .expect("an uncancelled watch must succeed");
        assert_eq!(value["emitted"], 0);
        assert_eq!(value["truncated"], false);
        assert_eq!(value["completed"], true);
        assert_eq!(value["coverage_complete"], true);
        assert_eq!(value["verified"], true);
        assert_eq!(
            value["identity"],
            "app-selector+canonical-executable+pid+start-identity"
        );
        assert_eq!(value["duration_ms"], 60);
        assert_eq!(value["interval_ms"], 50);
        assert_eq!(value["max_events"], 8);
        assert_eq!(value["max_processes"], 8);
        assert_eq!(value["selectors"][0]["selector"], "fixture");
        assert_eq!(value["baseline"].as_array().expect("baseline").len(), 1);
        assert!(
            value.get("termination").is_none(),
            "the normal payload must not gain a termination field"
        );
        assert_eq!(revalidations.get(), 1);
    }

    #[test]
    fn aggregate_emits_only_zero_boundary_transitions() {
        let empty = vec![Instances::new()];
        let one = vec![instances(&[(10, "a")])];
        let two = vec![instances(&[(10, "a"), (11, "b")])];
        let mut events = Vec::new();
        assert!(!append_transitions(&empty, &one, 10, 8, &mut events));
        assert!(!append_transitions(&one, &two, 20, 8, &mut events));
        assert!(!append_transitions(&two, &one, 30, 8, &mut events));
        assert!(!append_transitions(&one, &empty, 40, 8, &mut events));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["kind"], "launched");
        assert_eq!(events[0]["instances"], 1);
        assert_eq!(events[1]["kind"], "quit");
        assert_eq!(events[1]["instances"], 0);
    }

    #[test]
    fn event_ceiling_truncates_on_the_boundary() {
        let previous = vec![Instances::new(), Instances::new()];
        let current = vec![instances(&[(10, "a")]), instances(&[(11, "b")])];
        let mut events = Vec::new();
        assert!(append_transitions(&previous, &current, 10, 1, &mut events));
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn duplicate_selector_check_uses_canonical_executable_identity() {
        let identity = AppIdentity {
            name: Some("Fixture".into()),
            bundle: None,
            path: Some("fixture.app".into()),
            executable: "fixture-exe".into(),
            file: FileIdentity {
                length: 1,
                modified: None,
            },
        };
        let bindings = [
            AppBinding {
                index: 0,
                selector: "first".into(),
                executable_path: PathBuf::from("fixture-exe"),
                executable_name: "fixture-exe".into(),
                identity: identity.clone(),
            },
            AppBinding {
                index: 1,
                selector: "second".into(),
                executable_path: PathBuf::from("fixture-exe"),
                executable_name: "fixture-exe".into(),
                identity,
            },
        ];
        let error = reject_duplicate_executables(&bindings).unwrap_err();
        assert_eq!(error.code, "app_watch_selector_duplicate");
    }

    #[test]
    fn complete_path_disambiguates_apps_with_the_same_executable_basename() {
        let make_binding = |index, path: &str| AppBinding {
            index,
            selector: format!("selector-{index}"),
            executable_path: PathBuf::from(path),
            executable_name: "fixture".into(),
            identity: AppIdentity {
                name: Some(format!("Fixture {index}")),
                bundle: None,
                path: None,
                executable: path.into(),
                file: FileIdentity {
                    length: 1,
                    modified: None,
                },
            },
        };
        let bindings = [
            make_binding(0, "first/fixture"),
            make_binding(1, "second/fixture"),
        ];
        assert_eq!(
            matching_binding(&bindings, "fixture", Path::new("second/fixture"))
                .map(|binding| binding.index),
            Some(1)
        );
    }

    #[test]
    fn identity_comparison_includes_path_length_and_mtime() {
        let original = AppIdentity {
            name: Some("Fixture".into()),
            bundle: Some("org.example.Fixture".into()),
            path: Some("fixture.app".into()),
            executable: "fixture-exe".into(),
            file: FileIdentity {
                length: 10,
                modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
            },
        };
        let mut drifted = original.clone();
        drifted.file.length += 1;
        assert_ne!(original, drifted);
        let mut drifted = original.clone();
        drifted.file.modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(2));
        assert_ne!(original, drifted);
        let mut drifted = original.clone();
        drifted.executable.push_str("-new");
        assert_ne!(original, drifted);
    }
}
