//! Chromium-family browser profiles (`browser profiles` / `browser open`):
//! the pure half. A Chromium user data directory keeps one `Local State`
//! JSON whose `profile.info_cache` maps each profile *directory* (`Default`,
//! `Profile 3`) to its display *name* (what the window title and the
//! profile menu show) and whose `profile.last_used` names the directory
//! the last window was opened from. One running instance serves every
//! profile of one user data directory, so a profile is opened with
//! `--profile-directory=<dir>` on the running instance's own command line
//! (macOS `open -na <app> --args ...`), never by restarting the browser.
//! The mechanism (reading the file, launching, polling the inventory)
//! lives in `executor`; everything here is pure and unit-tested on
//! `tests/fixtures/local_state.json`.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// One Chromium-family application whose `Local State` this binary can
/// read. `macos_dir` / `linux_dir` are relative to the platform's
/// application-support root (`~/Library/Application Support` /
/// `~/.config`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrowserApp {
    /// The application name the window inventory reports (`app_name`),
    /// and the bundle name `open -a` takes.
    pub name: &'static str,
    pub macos_dir: &'static str,
    pub linux_dir: &'static str,
}

/// The catalog, in resolution order. Anything else is typed `unsupported`.
pub const APPS: &[BrowserApp] = &[
    BrowserApp {
        name: "Brave Origin",
        macos_dir: "BraveSoftware/Brave-Origin",
        linux_dir: "BraveSoftware/Brave-Origin",
    },
    BrowserApp {
        name: "Brave Browser",
        macos_dir: "BraveSoftware/Brave-Browser",
        linux_dir: "BraveSoftware/Brave-Browser",
    },
    BrowserApp {
        name: "Google Chrome",
        macos_dir: "Google/Chrome",
        linux_dir: "google-chrome",
    },
];

pub fn app_names() -> Vec<&'static str> {
    APPS.iter().map(|app| app.name).collect()
}

impl BrowserApp {
    /// `<home>/.../Local State` for this host, or `None` when the host OS
    /// has no mapping (Windows user data lives under `%LOCALAPPDATA%` and
    /// is not mapped here).
    pub fn local_state_path(&self, home: &Path) -> Option<PathBuf> {
        let dir = self.user_data_dir(home)?;
        Some(dir.join("Local State"))
    }

    pub fn user_data_dir(&self, home: &Path) -> Option<PathBuf> {
        if cfg!(target_os = "macos") {
            Some(
                home.join("Library")
                    .join("Application Support")
                    .join(self.macos_dir),
            )
        } else if cfg!(target_os = "linux") {
            Some(home.join(".config").join(self.linux_dir))
        } else {
            None
        }
    }
}

/// Why `--app` did not resolve to one catalog entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppResolveError {
    /// The substring matched nothing in the catalog: typed `unsupported`.
    Unsupported { requested: String },
    /// More than one catalog entry matched and none of them narrowed it.
    Ambiguous { candidates: Vec<&'static str> },
    /// No `--app` and no catalog application has a window.
    NotRunning,
}

/// Resolve `--app SUB` against the catalog: exact (case-insensitive)
/// first, then substring; a substring shared by several entries is
/// narrowed to the ones currently running (`running` = distinct
/// `app_name`s of the window inventory). Without `--app`, the one
/// running catalog application wins; none running is `NotRunning`.
pub fn resolve_app(
    requested: Option<&str>,
    running: &[String],
) -> Result<&'static BrowserApp, AppResolveError> {
    let is_running = |app: &BrowserApp| running.iter().any(|name| name == app.name);
    let Some(requested) = requested.map(str::trim).filter(|s| !s.is_empty()) else {
        let up: Vec<&'static BrowserApp> = APPS.iter().filter(|app| is_running(app)).collect();
        return match up.as_slice() {
            [] => Err(AppResolveError::NotRunning),
            [one] => Ok(one),
            many => Err(AppResolveError::Ambiguous {
                candidates: many.iter().map(|app| app.name).collect(),
            }),
        };
    };
    let wanted = requested.to_lowercase();
    if let Some(exact) = APPS.iter().find(|app| app.name.to_lowercase() == wanted) {
        return Ok(exact);
    }
    let hits: Vec<&'static BrowserApp> = APPS
        .iter()
        .filter(|app| app.name.to_lowercase().contains(&wanted))
        .collect();
    match hits.as_slice() {
        [] => Err(AppResolveError::Unsupported {
            requested: requested.to_owned(),
        }),
        [one] => Ok(one),
        many => {
            let up: Vec<&&'static BrowserApp> = many.iter().filter(|app| is_running(app)).collect();
            match up.as_slice() {
                [one] => Ok(one),
                _ => Err(AppResolveError::Ambiguous {
                    candidates: many.iter().map(|app| app.name).collect(),
                }),
            }
        }
    }
}

/// One profile of a user data directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileEntry {
    /// Display name (`profile.info_cache.<dir>.name`); what a window
    /// title's ` - <App> - <name>` suffix carries.
    pub name: String,
    /// Directory key (`Default`, `Profile 3`); what
    /// `--profile-directory=` takes.
    pub directory: String,
    /// `profile.last_used == directory`.
    pub last_used: bool,
}

impl ProfileEntry {
    pub fn json(&self) -> Value {
        json!({
            "name": self.name,
            "directory": self.directory,
            "last_used": self.last_used,
        })
    }
}

/// The profiles of one parsed `Local State`, in `profiles_order` when the
/// file carries it (directory-key order otherwise). A profile without a
/// `name` is listed under its directory name so nothing is hidden.
pub fn parse_local_state(local_state: &Value) -> Result<Vec<ProfileEntry>, String> {
    let profile = local_state
        .get("profile")
        .ok_or("Local State has no `profile` object")?;
    let cache = profile
        .get("info_cache")
        .and_then(Value::as_object)
        .ok_or("Local State has no `profile.info_cache` object")?;
    let last_used = profile.get("last_used").and_then(Value::as_str);
    let mut directories: Vec<String> = profile
        .get("profiles_order")
        .and_then(Value::as_array)
        .map(|order| {
            order
                .iter()
                .filter_map(Value::as_str)
                .filter(|dir| cache.contains_key(*dir))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    for dir in cache.keys() {
        if !directories.contains(dir) {
            directories.push(dir.clone());
        }
    }
    Ok(directories
        .into_iter()
        .map(|directory| {
            let name = cache[&directory]
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(&directory)
                .to_owned();
            ProfileEntry {
                name,
                last_used: last_used == Some(directory.as_str()),
                directory,
            }
        })
        .collect())
}

/// Why `--profile NAME` did not name exactly one profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileMatchError {
    NotFound,
    Ambiguous { candidates: Vec<ProfileEntry> },
}

/// `NAME` -> one profile: an exact name match wins (case-sensitive, then
/// case-insensitive); otherwise a case-insensitive substring of the name
/// or the directory, which must be unique.
pub fn resolve_profile<'a>(
    entries: &'a [ProfileEntry],
    name: &str,
) -> Result<&'a ProfileEntry, ProfileMatchError> {
    let name = name.trim();
    if let Some(exact) = entries.iter().find(|entry| entry.name == name) {
        return Ok(exact);
    }
    let wanted = name.to_lowercase();
    if let Some(exact) = entries
        .iter()
        .find(|entry| entry.name.to_lowercase() == wanted)
    {
        return Ok(exact);
    }
    let hits: Vec<&ProfileEntry> = entries
        .iter()
        .filter(|entry| {
            entry.name.to_lowercase().contains(&wanted) || entry.directory.to_lowercase() == wanted
        })
        .collect();
    match hits.as_slice() {
        [] => Err(ProfileMatchError::NotFound),
        [one] => Ok(one),
        many => Err(ProfileMatchError::Ambiguous {
            candidates: many.iter().map(|entry| (*entry).clone()).collect(),
        }),
    }
}

/// The macOS launch line: `open -na <app> --args --profile-directory=<dir>
/// [url]`. `-n` asks Launch Services for a fresh process, which the
/// Chromium process singleton hands to the running instance (a new window
/// or, with a URL, a new tab in that profile's window) and exits; the
/// user's browser is never quit or restarted.
pub fn open_argv(app: &BrowserApp, directory: &str, url: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "open".to_owned(),
        "-na".to_owned(),
        app.name.to_owned(),
        "--args".to_owned(),
        format!("--profile-directory={directory}"),
    ];
    if let Some(url) = url {
        argv.push(url.to_owned());
    }
    argv
}

/// A `--url` this binary is willing to hand to `open`: non-empty, a single
/// line, and not another switch (`--incognito` on the browser's own
/// command line is not a URL).
pub fn validate_url(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("browser open --url must not be empty".into());
    }
    if trimmed.starts_with('-') {
        return Err(format!(
            "browser open --url {url:?} looks like a switch, not a URL"
        ));
    }
    if trimmed.contains('\n') || trimmed.contains('\0') {
        return Err("browser open --url must be one line".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/local_state.json");

    fn fixture() -> Vec<ProfileEntry> {
        parse_local_state(&serde_json::from_str(FIXTURE).expect("fixture json")).expect("parse")
    }

    #[test]
    fn local_state_lists_every_profile_in_order_with_last_used() {
        let entries = fixture();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Beta Work", "beta-home", "Person 4"]);
        let dirs: Vec<&str> = entries.iter().map(|e| e.directory.as_str()).collect();
        assert_eq!(dirs, ["Default", "Profile 1", "Profile 2", "Profile 3"]);
        let last: Vec<&str> = entries
            .iter()
            .filter(|e| e.last_used)
            .map(|e| e.directory.as_str())
            .collect();
        assert_eq!(last, ["Profile 1"]);
        assert_eq!(
            entries[0].json(),
            json!({ "name": "alpha", "directory": "Default", "last_used": false })
        );
        // No profiles_order: directory-key order, unnamed falls back to the key.
        let bare = parse_local_state(&json!({
            "profile": { "info_cache": { "Profile 9": {}, "Default": { "name": "x" } } }
        }))
        .expect("parse");
        assert_eq!(bare.len(), 2);
        assert!(
            bare.iter()
                .any(|e| e.name == "Profile 9" && e.directory == "Profile 9")
        );
        assert!(bare.iter().all(|e| !e.last_used));
        assert!(parse_local_state(&json!({})).is_err());
        assert!(parse_local_state(&json!({ "profile": { "last_used": "Default" } })).is_err());
    }

    #[test]
    fn profile_resolves_exact_then_case_insensitive_then_unique_substring() {
        let entries = fixture();
        assert_eq!(
            resolve_profile(&entries, "alpha").unwrap().directory,
            "Default"
        );
        assert_eq!(
            resolve_profile(&entries, "BETA WORK").unwrap().directory,
            "Profile 1"
        );
        assert_eq!(
            resolve_profile(&entries, "home").unwrap().directory,
            "Profile 2"
        );
        // The directory key is an exact alternative spelling.
        assert_eq!(
            resolve_profile(&entries, "profile 3").unwrap().name,
            "Person 4"
        );
        match resolve_profile(&entries, "beta") {
            Err(ProfileMatchError::Ambiguous { candidates }) => {
                let names: Vec<&str> = candidates.iter().map(|e| e.name.as_str()).collect();
                assert_eq!(names, ["Beta Work", "beta-home"]);
            }
            other => panic!("two beta profiles must be ambiguous: {other:?}"),
        }
        assert_eq!(
            resolve_profile(&entries, "nowhere"),
            Err(ProfileMatchError::NotFound)
        );
    }

    #[test]
    fn app_resolves_from_catalog_and_running_inventory() {
        let running = vec!["Brave Origin".to_owned(), "Terminal".to_owned()];
        assert_eq!(resolve_app(None, &running).unwrap().name, "Brave Origin");
        assert_eq!(
            resolve_app(Some("brave origin"), &[]).unwrap().name,
            "Brave Origin"
        );
        // A shared substring narrows to the running one.
        assert_eq!(
            resolve_app(Some("Brave"), &running).unwrap().name,
            "Brave Origin"
        );
        assert_eq!(
            resolve_app(Some("Brave"), &[]),
            Err(AppResolveError::Ambiguous {
                candidates: vec!["Brave Origin", "Brave Browser"]
            })
        );
        assert_eq!(
            resolve_app(Some("chrome"), &[]).unwrap().name,
            "Google Chrome"
        );
        assert_eq!(
            resolve_app(Some("Safari"), &running),
            Err(AppResolveError::Unsupported {
                requested: "Safari".into()
            })
        );
        assert_eq!(resolve_app(None, &[]), Err(AppResolveError::NotRunning));
        assert_eq!(
            resolve_app(None, &["Brave Origin".into(), "Google Chrome".into()]),
            Err(AppResolveError::Ambiguous {
                candidates: vec!["Brave Origin", "Google Chrome"]
            })
        );
    }

    #[test]
    fn local_state_path_is_under_the_platform_support_root() {
        let home = Path::new("/synthetic-home");
        let path = APPS[0].local_state_path(home);
        if cfg!(target_os = "macos") {
            assert_eq!(
                path,
                Some(PathBuf::from(
                    "/synthetic-home/Library/Application Support/BraveSoftware/Brave-Origin/Local State"
                ))
            );
            assert!(
                APPS[2]
                    .local_state_path(home)
                    .unwrap()
                    .ends_with("Google/Chrome/Local State")
            );
        } else if cfg!(target_os = "linux") {
            assert_eq!(
                path,
                Some(PathBuf::from(
                    "/synthetic-home/.config/BraveSoftware/Brave-Origin/Local State"
                ))
            );
        } else {
            assert_eq!(path, None);
        }
    }

    #[test]
    fn open_argv_is_the_open_na_line_and_urls_are_validated() {
        let argv = open_argv(&APPS[0], "Profile 2", Some("https://example.com/"));
        assert_eq!(
            argv,
            [
                "open",
                "-na",
                "Brave Origin",
                "--args",
                "--profile-directory=Profile 2",
                "https://example.com/"
            ]
        );
        assert_eq!(open_argv(&APPS[2], "Default", None).len(), 5);
        assert!(validate_url("https://example.com/").is_ok());
        assert!(validate_url("data:text/html,<h1>x</h1>").is_ok());
        assert!(validate_url("").is_err());
        assert!(validate_url("--incognito").is_err());
        assert!(validate_url("a\nb").is_err());
    }
}
