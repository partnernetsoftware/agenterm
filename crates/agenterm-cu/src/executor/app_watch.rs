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
    let initial = app_snapshot(&bindings, max_processes)?;
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
        let remaining = deadline.saturating_duration_since(Instant::now());
        let sleep_deadline = Instant::now() + Duration::from_millis(interval_ms).min(remaining);
        while Instant::now() < sleep_deadline {
            control.check_observe()?;
            thread::sleep(
                Duration::from_millis(10)
                    .min(sleep_deadline.saturating_duration_since(Instant::now())),
            );
        }
        control.check_observe()?;
        let next = app_snapshot(&bindings, max_processes)?;
        excluded_unidentified = excluded_unidentified.max(next.excluded_unidentified);
        let current = next.instances;
        let t_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        truncated = append_transitions(&previous, &current, t_ms, max_events, &mut events);
        previous = current;
        if truncated {
            break;
        }
    }

    control.check_observe()?;
    revalidate_bindings(&bindings)?;
    let selector_rows = bindings.iter().map(binding_json).collect::<Vec<_>>();
    Ok(json!({
        "selectors": selector_rows,
        "baseline": baseline,
        "events": events,
        "emitted": events.len(),
        "completed": !truncated,
        "truncated": truncated,
        "coverage_complete": excluded_unidentified == 0,
        "excluded_unidentified": excluded_unidentified,
        "duration_ms": duration_ms,
        "interval_ms": interval_ms,
        "max_events": max_events,
        "max_processes": max_processes,
        "identity": "app-selector+canonical-executable+pid+start-identity",
        "verified": true,
    }))
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
