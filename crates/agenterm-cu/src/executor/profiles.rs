//! Chromium-family profiles on the *running* browser: `browser profiles`
//! (read `Local State`, join to the window inventory) and `browser open`
//! (`open -na <app> --args --profile-directory=<dir> [url]` on the running
//! instance, then poll the inventory until the profile's window is there).
//! The pure parsing / resolution lives in `crate::browser_profiles`.

use super::*;

use crate::browser_profiles::{
    self as profiles, AppResolveError, BrowserApp, ProfileEntry, ProfileMatchError,
};

/// How long `browser open` waits for the window by default, and the
/// ceiling a caller may raise it to.
pub(super) const BROWSER_OPEN_TIMEOUT: Duration = Duration::from_millis(8_000);

pub(super) const BROWSER_OPEN_MAX_TIMEOUT: Duration = Duration::from_millis(120_000);

pub(super) const BROWSER_OPEN_POLL: Duration = Duration::from_millis(250);

/// One browser window with the profile it belongs to (the AX-root /
/// title join `windows` reports as `browser_profile`).
pub(super) struct ProfileWindow {
    pub handle: isize,
    pub title: String,
    pub profile: String,
}

impl ProfileWindow {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "handle": self.handle,
            "title": self.title,
            "browser_profile": self.profile,
        })
    }
}

/// Every window of `app` that carries a profile name, in inventory order.
pub(super) fn profile_windows(app: &str) -> Result<Vec<ProfileWindow>, CuError> {
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    Ok(windows
        .iter()
        .filter(|window| window.app_name == app)
        .filter_map(|window| {
            window_browser_profile(window).map(|profile| ProfileWindow {
                handle: window.handle,
                title: window.title.clone(),
                profile,
            })
        })
        .collect())
}

/// Distinct `app_name`s of the inventory (which catalog browser is up).
fn running_app_names() -> Result<Vec<String>, CuError> {
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let mut names: Vec<String> = Vec::new();
    for window in windows {
        if !names.contains(&window.app_name) {
            names.push(window.app_name);
        }
    }
    Ok(names)
}

fn resolve_app(requested: Option<&str>) -> Result<&'static BrowserApp, CuError> {
    let running = running_app_names()?;
    profiles::resolve_app(requested, &running).map_err(|error| match error {
        AppResolveError::Unsupported { requested } => CuError::new(
            "unsupported",
            format!(
                "{requested:?} is not a Chromium-family application whose Local State this binary reads; supported: {}",
                profiles::app_names().join(", ")
            ),
        )
        .with_detail(serde_json::json!({
            "reason": "browser_app_unsupported",
            "requested": requested,
            "supported": profiles::app_names(),
        })),
        AppResolveError::Ambiguous { candidates } => CuError::new(
            "browser_app_ambiguous",
            format!(
                "--app {} names more than one application; refusing to guess",
                requested.map(|s| format!("{s:?}")).unwrap_or_else(|| "(none)".into())
            ),
        )
        .with_detail(serde_json::json!({ "candidates": candidates, "running": running })),
        AppResolveError::NotRunning => CuError::new(
            "browser_app_not_found",
            format!(
                "no window of a supported browser is in the inventory; pass --app ({})",
                profiles::app_names().join(" | ")
            ),
        )
        .with_detail(serde_json::json!({ "supported": profiles::app_names(), "running": running })),
    })
}

/// `Local State` of `app`, parsed. The path is reported in `~/...` form so
/// a reply quoted into a document carries no account name.
struct LocalState {
    entries: Vec<ProfileEntry>,
    display_path: String,
}

fn home_dir() -> Result<PathBuf, CuError> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| CuError::new("unsupported", "HOME is not set; cannot locate Local State"))
}

fn display_path(home: &std::path::Path, path: &std::path::Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn load_local_state(app: &BrowserApp) -> Result<LocalState, CuError> {
    let home = home_dir()?;
    let Some(path) = app.local_state_path(&home) else {
        return Err(CuError::new(
            "unsupported",
            format!(
                "profiles of {} are read from the macOS / Linux user data directory; this OS is not mapped",
                app.name
            ),
        )
        .with_detail(serde_json::json!({ "os": crate::mcu_surface::host_os(), "app": app.name })));
    };
    let display_path = display_path(&home, &path);
    let text = std::fs::read_to_string(&path).map_err(|error| {
        CuError::new(
            "browser_local_state_not_found",
            format!("could not read {display_path}: {error}"),
        )
        .with_detail(serde_json::json!({ "app": app.name, "path": display_path }))
    })?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        CuError::new(
            "browser_local_state_invalid",
            format!("{display_path} is not JSON: {error}"),
        )
    })?;
    let entries = profiles::parse_local_state(&value).map_err(|reason| {
        CuError::new(
            "browser_local_state_invalid",
            format!("{display_path}: {reason}"),
        )
    })?;
    Ok(LocalState {
        entries,
        display_path,
    })
}

/// `browser profiles [--app SUB]`: every profile of the application's user
/// data directory with the inventory windows that belong to it.
pub(super) fn browser_profiles_payload(app: Option<&str>) -> Result<serde_json::Value, CuError> {
    let app = resolve_app(app)?;
    let state = load_local_state(app)?;
    let windows = profile_windows(app.name)?;
    let rows: Vec<serde_json::Value> = state
        .entries
        .iter()
        .map(|entry| {
            let mut row = entry.json();
            let handles: Vec<isize> = windows
                .iter()
                .filter(|window| window.profile == entry.name)
                .map(|window| window.handle)
                .collect();
            row["windows"] = serde_json::json!(handles);
            row
        })
        .collect();
    let orphan: Vec<serde_json::Value> = windows
        .iter()
        .filter(|window| {
            !state
                .entries
                .iter()
                .any(|entry| entry.name == window.profile)
        })
        .map(ProfileWindow::json)
        .collect();
    Ok(serde_json::json!({
        "mechanism": "local-state+window-inventory",
        "app": app.name,
        "local_state": state.display_path,
        "join": "window browser_profile == profile name (title / AX-root suffix)",
        "returned": rows.len(),
        "profiles": rows,
        // Browser windows whose profile name is not in Local State (a
        // guest / ephemeral window): listed, never folded into a row.
        "unlisted_windows": orphan,
    }))
}

fn profile_error(error: ProfileMatchError, requested: &str, entries: &[ProfileEntry]) -> CuError {
    match error {
        ProfileMatchError::NotFound => CuError::new(
            "browser_profile_not_found",
            format!(
                "no profile named (or containing) {requested:?}; {} profile(s) in Local State",
                entries.len()
            ),
        )
        .with_detail(serde_json::json!({
            "requested": requested,
            "candidates": entries.iter().map(ProfileEntry::json).collect::<Vec<_>>(),
        })),
        ProfileMatchError::Ambiguous { candidates } => CuError::new(
            "browser_profile_ambiguous",
            format!(
                "{} profile names contain {requested:?}; refusing to guess",
                candidates.len()
            ),
        )
        .with_count(candidates.len())
        .with_detail(serde_json::json!({
            "requested": requested,
            "candidates": candidates.iter().map(ProfileEntry::json).collect::<Vec<_>>(),
        })),
    }
}

/// `browser open --profile NAME [--url URL] [--app SUB] [--timeout-ms N]`.
pub(super) fn browser_open_payload(
    profile: &str,
    url: Option<&str>,
    app: Option<&str>,
    timeout_ms: Option<u64>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    if profile.trim().is_empty() {
        return Err(invalid_input(
            "browser open requires --profile NAME (a name from `browser profiles`)".into(),
        ));
    }
    if let Some(url) = url {
        profiles::validate_url(url).map_err(invalid_input)?;
    }
    let timeout = match timeout_ms {
        None => BROWSER_OPEN_TIMEOUT,
        Some(0) => {
            return Err(invalid_input(
                "browser open --timeout-ms must be at least 1".into(),
            ));
        }
        Some(ms) => {
            let wanted = Duration::from_millis(ms);
            if wanted > BROWSER_OPEN_MAX_TIMEOUT {
                return Err(invalid_input(format!(
                    "browser open --timeout-ms {ms} exceeds the {} ms ceiling",
                    BROWSER_OPEN_MAX_TIMEOUT.as_millis()
                )));
            }
            wanted
        }
    };
    if !cfg!(target_os = "macos") {
        return Err(CuError::new(
            "unsupported",
            "browser open launches through macOS `open -na <app> --args --profile-directory=...`; not mapped on this OS",
        )
        .with_detail(serde_json::json!({ "os": crate::mcu_surface::host_os() })));
    }
    let app = resolve_app(app)?;
    let state = load_local_state(app)?;
    let entry = profiles::resolve_profile(&state.entries, profile)
        .map_err(|error| profile_error(error, profile, &state.entries))?
        .clone();
    let before: Vec<ProfileWindow> = profile_windows(app.name)?
        .into_iter()
        .filter(|window| window.profile == entry.name)
        .collect();
    let before_json: Vec<serde_json::Value> = before.iter().map(ProfileWindow::json).collect();
    let argv = profiles::open_argv(app, &entry.directory, url);
    // The reply's `created` field says which of the two postconditions
    // can close the loop: a new window of the profile, or (a URL into a
    // profile that already has a window) that window's title changing.
    let expect_title_change = url.is_some() && !before.is_empty();
    let ticket = receipts.reserve(
        "browser-open",
        0,
        serde_json::json!({
            "action": "open",
            "app": app.name,
            "profile": entry.json(),
            "url": url,
            "argv": argv,
            "postcondition": if expect_title_change { "profile window appears or an existing one's title changes" } else { "profile window appears" },
            "before": before_json,
            "timeout_ms": timeout.as_millis() as u64,
        }),
    )?;
    let started = Instant::now();
    let launch = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output();
    let launch_error = match launch {
        Ok(output) if output.status.success() => None,
        Ok(output) => Some(CuError::new(
            "browser_open_failed",
            format!(
                "{} exited with {}: {}",
                argv.join(" "),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        )),
        Err(error) => Some(CuError::new(
            "browser_open_failed",
            format!("could not run {}: {error}", argv.join(" ")),
        )),
    };
    let mut polls = 0usize;
    let mut hit: Option<(ProfileWindow, bool)> = None;
    let mut readback_error = None;
    if launch_error.is_none() {
        loop {
            polls += 1;
            match profile_windows(app.name) {
                Ok(now) => {
                    let now: Vec<ProfileWindow> = now
                        .into_iter()
                        .filter(|window| window.profile == entry.name)
                        .collect();
                    if let Some(created) = now
                        .iter()
                        .find(|window| !before.iter().any(|old| old.handle == window.handle))
                    {
                        hit = Some((
                            ProfileWindow {
                                handle: created.handle,
                                title: created.title.clone(),
                                profile: created.profile.clone(),
                            },
                            true,
                        ));
                    } else if expect_title_change
                        && let Some(changed) = now.iter().find(|window| {
                            before
                                .iter()
                                .any(|old| old.handle == window.handle && old.title != window.title)
                        })
                    {
                        hit = Some((
                            ProfileWindow {
                                handle: changed.handle,
                                title: changed.title.clone(),
                                profile: changed.profile.clone(),
                            },
                            false,
                        ));
                    }
                }
                Err(error) => {
                    readback_error = Some(error);
                    break;
                }
            }
            if hit.is_some() || started.elapsed() >= timeout {
                break;
            }
            thread::sleep(BROWSER_OPEN_POLL.min(timeout.saturating_sub(started.elapsed())));
        }
    }
    let verified = hit.is_some() && launch_error.is_none() && readback_error.is_none();
    let reason = if launch_error.is_some() {
        Some("launch_failed")
    } else if readback_error.is_some() {
        Some("readback_failed")
    } else if hit.is_none() {
        Some("window_not_found")
    } else {
        None
    };
    let verification = serde_json::json!({
        "method": "window-inventory",
        "reason": reason,
        "polls": polls,
        "elapsed_ms": started.elapsed().as_millis() as u64,
        "timeout_ms": timeout.as_millis() as u64,
    });
    let handle = hit.as_ref().map(|(window, _)| window.handle).unwrap_or(0);
    receipts.complete(
        &ticket,
        "browser-open",
        handle,
        verified,
        serde_json::json!({
            "performed": launch_error.is_none(),
            "after": hit.as_ref().map(|(window, created)| {
                let mut row = window.json();
                row["created"] = serde_json::json!(created);
                row
            }),
            "verification": verification,
            "error": launch_error.as_ref().or(readback_error.as_ref()).map(error_payload),
        }),
    )?;
    let receipt = serde_json::json!({
        "addressing": "browser-profile",
        "mechanism": "open -na",
        "app": app.name,
        "profile": entry.json(),
        "url": url,
        "argv": argv,
        "performed": launch_error.is_none(),
        "verified": verified,
        "verification": verification,
        "before": before_json,
        "receipt": ticket.json(),
    });
    if let Some(error) = launch_error.or(readback_error) {
        return Err(error.with_detail(serde_json::json!({ "receipt": receipt })));
    }
    let Some((window, created)) = hit else {
        return Err(CuError::new(
            "browser_window_not_found",
            format!(
                "no window of profile {:?} {} within {} ms ({} polls); the browser may still be opening it",
                entry.name,
                if expect_title_change { "appeared or changed its title" } else { "appeared" },
                timeout.as_millis(),
                polls
            ),
        )
        .with_detail(serde_json::json!({ "reason": "timeout", "receipt": receipt })));
    };
    let mut payload = receipt;
    if let Some(object) = payload.as_object_mut() {
        object.insert("handle".into(), serde_json::json!(window.handle));
        object.insert("browser_profile".into(), serde_json::json!(window.profile));
        object.insert("title".into(), serde_json::json!(window.title));
        object.insert("created".into(), serde_json::json!(created));
        object.insert(
            "next_actions".into(),
            serde_json::json!([
                format!("tab list --window {}", window.handle),
                format!("page text --window {}", window.handle),
            ]),
        );
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_open_validates_its_inputs_before_touching_anything() {
        let empty = actuate_executor().execute(&Command::BrowserOpen {
            target: TargetRef::Current,
            profile: "  ".into(),
            url: None,
            app: None,
            timeout_ms: None,
        });
        assert!(!empty.ok);
        assert_eq!(empty.command, "browser-open");
        assert_eq!(empty.error.as_ref().unwrap().code, "invalid_input");
        let switch = actuate_executor().execute(&Command::BrowserOpen {
            target: TargetRef::Current,
            profile: "work".into(),
            url: Some("--incognito".into()),
            app: None,
            timeout_ms: None,
        });
        assert_eq!(switch.error.as_ref().unwrap().code, "invalid_input");
        let zero = actuate_executor().execute(&Command::BrowserOpen {
            target: TargetRef::Current,
            profile: "work".into(),
            url: None,
            app: None,
            timeout_ms: Some(0),
        });
        assert_eq!(zero.error.as_ref().unwrap().code, "invalid_input");
        let huge = actuate_executor().execute(&Command::BrowserOpen {
            target: TargetRef::Current,
            profile: "work".into(),
            url: None,
            app: None,
            timeout_ms: Some(BROWSER_OPEN_MAX_TIMEOUT.as_millis() as u64 + 1),
        });
        assert_eq!(huge.error.as_ref().unwrap().code, "invalid_input");
        // Observe-only authorization never reaches the launcher.
        let denied = observe_executor().execute(&Command::BrowserOpen {
            target: TargetRef::Current,
            profile: "work".into(),
            url: None,
            app: None,
            timeout_ms: None,
        });
        assert!(!denied.ok);
        assert_ne!(denied.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn unsupported_app_is_typed_before_any_local_state_read() {
        let reply = observe_executor().execute(&Command::BrowserProfiles {
            target: TargetRef::Current,
            app: Some("Safari".into()),
        });
        assert!(!reply.ok);
        assert_eq!(reply.command, "browser-profiles");
        let err = reply.error.as_ref().expect("typed");
        // Either the catalog refuses the name (unsupported) or, on a host
        // whose window inventory cannot be read, the mechanism failure
        // comes first; never `usage` and never a profile-level code.
        assert!(
            matches!(err.code.as_str(), "unsupported" | "a11y_permission_denied")
                || err.code.starts_with("dylib"),
            "{}",
            err.code
        );
        if err.code == "unsupported" {
            assert_eq!(
                err.detail.as_ref().unwrap()["reason"],
                "browser_app_unsupported"
            );
        }
    }

    #[test]
    fn profile_errors_carry_the_candidates() {
        let entries = vec![
            ProfileEntry {
                name: "alpha".into(),
                directory: "Default".into(),
                last_used: true,
            },
            ProfileEntry {
                name: "beta".into(),
                directory: "Profile 1".into(),
                last_used: false,
            },
        ];
        let missing = profile_error(ProfileMatchError::NotFound, "zeta", &entries);
        assert_eq!(missing.code, "browser_profile_not_found");
        assert_eq!(
            missing.detail.as_ref().unwrap()["candidates"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let ambiguous = profile_error(
            ProfileMatchError::Ambiguous {
                candidates: entries.clone(),
            },
            "a",
            &entries,
        );
        assert_eq!(ambiguous.code, "browser_profile_ambiguous");
        assert_eq!(
            ambiguous.detail.as_ref().unwrap()["candidates"][1]["name"],
            "beta"
        );
        assert_eq!(
            display_path(
                std::path::Path::new("/synthetic-home"),
                std::path::Path::new("/synthetic-home/Library/Application Support/X/Local State")
            ),
            "~/Library/Application Support/X/Local State"
        );
    }
}
