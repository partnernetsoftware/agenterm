//! Discover Chromium-family catalog applications without a live window.
//!
//! Every mapped host first checks its catalog user-data directory. Linux
//! additionally checks PATH / known install paths and Freedesktop `.desktop`
//! entries; macOS additionally checks `/Applications/*.app` bundles.

use std::path::{Path, PathBuf};

use super::{APPS, BrowserApp};

/// Catalog browsers that appear installed for `home`.
pub fn installed_catalog_apps(home: &Path) -> Vec<&'static BrowserApp> {
    APPS.iter()
        .filter(|app| app_is_installed(app, home))
        .collect()
}

fn app_is_installed(app: &BrowserApp, home: &Path) -> bool {
    if user_data_root_exists(app, home) {
        return true;
    }
    if cfg!(target_os = "linux") {
        linux_binary_installed(app) || linux_desktop_entry_installed(app)
    } else if cfg!(target_os = "macos") {
        macos_bundle_installed(app)
    } else {
        false
    }
}

fn user_data_root_exists(app: &BrowserApp, home: &Path) -> bool {
    app.user_data_dir(home)
        .is_some_and(|path| path.is_dir() && !path.is_symlink())
}

#[cfg(target_os = "linux")]
fn linux_binary_installed(app: &BrowserApp) -> bool {
    linux_binary_names(app)
        .iter()
        .any(|name| path_has_executable(name) || known_linux_path(name).is_some())
}

#[cfg(not(target_os = "linux"))]
fn linux_binary_installed(_app: &BrowserApp) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn linux_desktop_entry_installed(app: &BrowserApp) -> bool {
    linux_desktop_entry_installed_for_home(app, None)
}

#[cfg(target_os = "linux")]
fn linux_desktop_entry_installed_for_home(app: &BrowserApp, home: Option<&Path>) -> bool {
    for dir in linux_desktop_dirs(home) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if desktop_entry_matches_app(&text, app) {
                return true;
            }
        }
    }
    false
}

#[cfg(not(target_os = "linux"))]
fn linux_desktop_entry_installed(_app: &BrowserApp) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn macos_bundle_installed(app: &BrowserApp) -> bool {
    Path::new("/Applications")
        .join(format!("{}.app", app.name))
        .is_dir()
}

#[cfg(not(target_os = "macos"))]
fn macos_bundle_installed(_app: &BrowserApp) -> bool {
    false
}

/// Candidate launcher basenames for `browser open`, in preference order.
#[cfg(target_os = "linux")]
pub(crate) fn linux_launch_binary_names(app: &BrowserApp) -> &'static [&'static str] {
    linux_binary_names(app)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn linux_launch_binary_names(_app: &BrowserApp) -> &'static [&'static str] {
    &[]
}

/// First executable path for a Linux launcher basename.
#[cfg(target_os = "linux")]
pub(crate) fn resolve_linux_executable(name: &str) -> Option<PathBuf> {
    if let Some(path) = known_linux_path(name) {
        return Some(path);
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        return None;
    };
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| is_launchable_file(candidate))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn resolve_linux_executable(_name: &str) -> Option<PathBuf> {
    None
}

#[cfg(target_os = "linux")]
fn is_launchable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .is_some()
}

#[cfg(target_os = "linux")]
fn linux_binary_names(app: &BrowserApp) -> &'static [&'static str] {
    match app.name {
        "Google Chrome" => &["google-chrome", "google-chrome-stable", "chrome"],
        "Brave Browser" => &["brave-browser", "brave"],
        "Brave Origin" => &["brave-browser", "brave"],
        _ => &[],
    }
}

#[cfg(target_os = "linux")]
fn known_linux_path(name: &str) -> Option<PathBuf> {
    let candidates: &[PathBuf] = match name {
        "google-chrome" => &[
            PathBuf::from("/usr/bin/google-chrome"),
            PathBuf::from("/opt/google/chrome/google-chrome"),
        ],
        "google-chrome-stable" => &[
            PathBuf::from("/usr/bin/google-chrome-stable"),
            PathBuf::from("/usr/bin/google-chrome"),
            PathBuf::from("/opt/google/chrome/google-chrome"),
        ],
        "chrome" => &[PathBuf::from("/opt/google/chrome/google-chrome")],
        "chromium" | "chromium-browser" => &[
            PathBuf::from("/usr/bin/chromium"),
            PathBuf::from("/usr/bin/chromium-browser"),
        ],
        "brave-browser" | "brave" => &[PathBuf::from("/usr/bin/brave-browser")],
        _ => return None,
    };
    candidates
        .iter()
        .find(|path| is_launchable_file(path))
        .cloned()
}

#[cfg(target_os = "linux")]
fn path_has_executable(name: &str) -> bool {
    resolve_linux_executable(name).is_some()
}

#[cfg(target_os = "linux")]
fn linux_desktop_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/usr/share/applications")];
    if let Some(home) = home {
        dirs.push(home.join(".local/share/applications"));
    } else if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    dirs.push(PathBuf::from("/var/lib/snapd/desktop/applications"));
    dirs
}

#[cfg(target_os = "linux")]
fn desktop_entry_matches_app(text: &str, app: &BrowserApp) -> bool {
    let wanted = app.name.to_ascii_lowercase();
    let mut name = None;
    let mut exec = None;
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            match key.trim() {
                "Name" => name = Some(value.trim()),
                "Exec" => exec = Some(value.trim()),
                _ => {}
            }
        }
    }
    if name.is_some_and(|name| name.eq_ignore_ascii_case(app.name)) {
        return true;
    }
    exec.is_some_and(|exec| {
        linux_binary_names(app)
            .iter()
            .any(|binary| exec.split_whitespace().any(|token| token.ends_with(binary)))
            || exec.to_ascii_lowercase().contains(&wanted)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    fn fixture(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agenterm-cu-browser-discovery-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::canonicalize(root).unwrap()
    }

    #[test]
    fn user_data_root_marks_catalog_app_installed() {
        let home = fixture("user-data");
        let chrome = APPS
            .iter()
            .find(|app| app.name == "Google Chrome")
            .copied()
            .unwrap();
        let root = chrome.user_data_dir(&home).unwrap();
        assert!(!user_data_root_exists(&chrome, &home));
        fs::create_dir_all(&root).unwrap();
        assert!(user_data_root_exists(&chrome, &home));
        fs::remove_dir_all(home).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn desktop_entry_name_marks_google_chrome_installed() {
        let home = fixture("desktop");
        let desktop_dir = home.join(".local/share/applications");
        fs::create_dir_all(&desktop_dir).unwrap();
        fs::write(
            desktop_dir.join("google-chrome.desktop"),
            "Name=Google Chrome\nExec=/usr/bin/google-chrome %U\n",
        )
        .unwrap();
        let chrome = APPS
            .iter()
            .find(|app| app.name == "Google Chrome")
            .copied()
            .unwrap();
        assert!(linux_desktop_entry_installed_for_home(&chrome, Some(&home)));
        fs::remove_dir_all(home).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn path_binary_marks_google_chrome_installed_when_present() {
        if !Path::new("/usr/bin/google-chrome").is_file() {
            return;
        }
        let home = fixture("path-only");
        let installed = installed_catalog_apps(&home);
        assert!(
            installed.iter().any(|app| app.name == "Google Chrome"),
            "{installed:?}"
        );
        fs::remove_dir_all(home).unwrap();
    }
}
