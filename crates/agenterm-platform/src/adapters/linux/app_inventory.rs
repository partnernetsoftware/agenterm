//! Linux installed-application inventory and launch, over XDG desktop entries.
//!
//! An application a window cannot reveal is one nothing is running for, and
//! on Linux the only host-wide record of those is the desktop-entry
//! directories the XDG Base Directory and Desktop Entry specs define. This
//! reads them and nothing else: no package manager, no `$PATH` scan. A bare
//! executable on `$PATH` with no desktop entry is deliberately not listed --
//! it has no display name, and inventing one from a filename would put
//! guesses in a listing callers match names against.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::contract::app_inventory::{
    AppInventoryError, InstalledApp, InstalledApps, MAX_APP_PATH_BYTES, MAX_INSTALLED_APPS,
};

/// Largest desktop entry this will read. Entries are small key/value files;
/// anything larger is not one, and reading it would be unbounded work per
/// candidate.
const MAX_ENTRY_BYTES: u64 = 128 * 1024;

/// The `applications` directories, in XDG precedence order: the first
/// occurrence of a given entry id wins, which is how a user's own entry
/// overrides the system one of the same name.
fn application_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let home_data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    if let Some(home_data) = home_data {
        directories.push(home_data.join("applications"));
    }
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    for entry in system.split(':') {
        let path = PathBuf::from(entry);
        if path.is_absolute() {
            directories.push(path.join("applications"));
        }
    }
    directories
}

/// One parsed desktop entry, reduced to what this contract carries.
struct DesktopEntry {
    name: String,
    exec: Option<String>,
    listed: bool,
    terminal: bool,
}

/// Read the `[Desktop Entry]` group of one entry file.
///
/// Only that group counts: an entry's actions live in their own groups and
/// a key from one of those is not the application's. Localised keys
/// (`Name[de]`) are skipped -- the unlocalised `Name` is the one every
/// entry is required to have.
fn parse_entry(path: &Path) -> Option<DesktopEntry> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_ENTRY_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_group = false;
    let mut name = None;
    let mut exec = None;
    let mut kind = None;
    let mut no_display = false;
    let mut hidden = false;
    let mut terminal = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Name" => name = Some(value.to_owned()),
            "Exec" => exec = Some(value.to_owned()),
            "Type" => kind = Some(value.to_owned()),
            "NoDisplay" => no_display = value.eq_ignore_ascii_case("true"),
            "Hidden" => hidden = value.eq_ignore_ascii_case("true"),
            "Terminal" => terminal = value.eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    // `Type` is required, and only `Application` is a thing to launch --
    // `Link` and `Directory` entries are not applications.
    if kind.as_deref() != Some("Application") {
        return None;
    }
    let name = name.filter(|value| !value.is_empty())?;
    Some(DesktopEntry {
        name,
        exec,
        // `Hidden` means the entry is deleted as far as anything reading it
        // is concerned; `NoDisplay` means it exists but is not something a
        // person picks from a menu (a MIME handler, a settings panel).
        // Neither belongs in a listing an agent chooses from.
        listed: !no_display && !hidden,
        terminal,
    })
}

pub(crate) fn list_installed() -> Result<InstalledApps, AppInventoryError> {
    let directories = application_directories();
    if directories.is_empty() {
        return Err(AppInventoryError::failed(
            "app_inventory_failed",
            "neither XDG_DATA_HOME/HOME nor XDG_DATA_DIRS named a directory to read",
        ));
    }
    // Keyed by entry id (the file name relative to its applications
    // directory), because that is what the spec makes unique and what
    // gives a user's own entry precedence over the system one.
    let mut by_id: HashMap<String, InstalledApp> = HashMap::new();
    let mut read_any = false;
    for directory in &directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        read_any = true;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("desktop") {
                continue;
            }
            let Some(id) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if by_id.contains_key(id) {
                continue;
            }
            let Some(parsed) = parse_entry(&path) else {
                continue;
            };
            if !parsed.listed {
                continue;
            }
            let Some(path) = path.to_str() else {
                continue;
            };
            by_id.insert(
                id.to_owned(),
                InstalledApp {
                    name: parsed.name,
                    path: path.to_owned(),
                },
            );
        }
    }
    // No readable directory at all is a different answer from an empty
    // one: the first means this host keeps its applications somewhere this
    // cannot see, the second means it has none installed.
    if !read_any {
        return Err(AppInventoryError::failed(
            "app_inventory_failed",
            "no XDG applications directory could be read on this host",
        ));
    }
    let mut apps: Vec<InstalledApp> = by_id.into_values().collect();
    apps.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.path.cmp(&right.path))
    });
    let truncated = apps.len() > MAX_INSTALLED_APPS;
    apps.truncate(MAX_INSTALLED_APPS);
    Ok(InstalledApps { apps, truncated })
}

/// Strip the field codes a desktop entry's `Exec` line may carry.
///
/// `%f`/`%F`/`%u`/`%U` stand for the files or URLs being opened and there
/// are none here; `%i`/`%c`/`%k` expand to the entry's own icon, name and
/// path. Passing any of them through literally would hand the application
/// an argument that means nothing. `%%` is an escaped percent sign.
fn expand_exec(exec: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in exec.split_whitespace() {
        let token = token.trim_matches('"');
        if matches!(token, "%f" | "%F" | "%u" | "%U" | "%i" | "%c" | "%k") {
            continue;
        }
        out.push(token.replace("%%", "%"));
    }
    out
}

/// Start the application a desktop entry names.
///
/// The entry's `Exec` line is the launch mechanism the spec defines, so
/// that is what this runs -- it is not a shell line, and it is not run
/// through one: the tokens go straight to the process. The child is
/// detached, so this returns as soon as it is started and never waits for
/// a GUI application to exit.
pub(crate) fn launch(path: &str) -> Result<(), AppInventoryError> {
    if path.len() > MAX_APP_PATH_BYTES {
        return Err(AppInventoryError::failed(
            "invalid_input",
            format!("path exceeds {MAX_APP_PATH_BYTES} bytes"),
        ));
    }
    let entry_path = Path::new(path);
    if !entry_path.exists() {
        return Err(AppInventoryError::failed(
            "app_not_found",
            format!("nothing exists at {path}"),
        ));
    }
    let Some(entry) = parse_entry(entry_path) else {
        return Err(AppInventoryError::failed(
            "invalid_input",
            format!("{path} is not a Type=Application desktop entry"),
        ));
    };
    let Some(exec) = entry.exec.filter(|value| !value.trim().is_empty()) else {
        return Err(AppInventoryError::failed(
            "app_launch_failed",
            format!("{path} has no Exec line to run"),
        ));
    };
    // An entry that asks for a terminal needs one supplied, and which
    // terminal that is is the desktop's policy, not this crate's. Starting
    // it without one would leave a process with no usable stdio and report
    // success for it.
    // This is a property of the entry, not of the host, so it is a typed
    // failure rather than `Unsupported`: the launch mechanism is here and
    // works, and reporting "mechanism unavailable on this host" would send
    // a caller looking for something to install.
    if entry.terminal {
        return Err(AppInventoryError::failed(
            "app_launch_needs_terminal",
            format!(
                "{path} sets Terminal=true; which terminal emulator to run it in is the desktop's policy, not this crate's"
            ),
        ));
    }
    let argv = expand_exec(&exec);
    let Some((program, arguments)) = argv.split_first() else {
        return Err(AppInventoryError::failed(
            "app_launch_failed",
            format!("{path} has an Exec line with no command"),
        ));
    };
    std::process::Command::new(program)
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| {
            AppInventoryError::failed(
                "app_launch_failed",
                format!("could not start {program}: {error}"),
            )
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_field_codes_are_dropped_not_passed_through() {
        assert_eq!(expand_exec("gedit %U"), vec!["gedit".to_owned()]);
        assert_eq!(
            expand_exec("app --flag %f --other"),
            vec!["app".to_owned(), "--flag".to_owned(), "--other".to_owned()]
        );
        assert_eq!(
            expand_exec("app 100%%"),
            vec!["app".to_owned(), "100%".to_owned()]
        );
    }

    #[test]
    fn only_the_desktop_entry_group_counts_and_only_applications_are_listed() {
        let dir = std::env::temp_dir().join(format!("agenterm-desktop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let app = dir.join("a.desktop");
        std::fs::write(
            &app,
            "[Desktop Entry]\nType=Application\nName=Real App\nExec=real %U\n\n\
             [Desktop Action New]\nName=Not The App\nExec=other\n",
        )
        .unwrap();
        let parsed = parse_entry(&app).expect("an application entry");
        // The action group's Name must not win over the entry's own.
        assert_eq!(parsed.name, "Real App");
        assert_eq!(parsed.exec.as_deref(), Some("real %U"));
        assert!(parsed.listed && !parsed.terminal);

        let link = dir.join("b.desktop");
        std::fs::write(&link, "[Desktop Entry]\nType=Link\nName=Somewhere\nURL=x\n").unwrap();
        assert!(parse_entry(&link).is_none(), "a Link is not an application");

        let hidden = dir.join("c.desktop");
        std::fs::write(
            &hidden,
            "[Desktop Entry]\nType=Application\nName=Hidden\nExec=x\nNoDisplay=true\n",
        )
        .unwrap();
        assert!(!parse_entry(&hidden).expect("parsed").listed);

        std::fs::remove_dir_all(&dir).ok();
    }
}
