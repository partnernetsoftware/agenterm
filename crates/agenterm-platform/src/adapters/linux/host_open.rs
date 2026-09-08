use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use crate::host_open::{
    HostOpenError, HostOpenErrorDetail, HostOpenErrorKind, HostOpenOptions, HostOpenReceipt,
};

const XDG_OPEN_PATHS: &[&str] = &["/usr/bin/xdg-open", "/bin/xdg-open"];
const GIO_PATHS: &[&str] = &["/usr/bin/gio", "/bin/gio"];
const GTK_LAUNCH_PATHS: &[&str] = &["/usr/bin/gtk-launch", "/bin/gtk-launch"];
const MAX_DIRECTORY_DEPTH: usize = 8;
const MAX_DESKTOP_ENTRIES: usize = 4_096;

const LINUX_HOST_OPEN_ALTERNATIVES: &[&str] = &[
    "host-open TARGET (xdg-open default handler)",
    "browser-open --profile NAME [--url URL] (Chromium-family profiles)",
];

pub(crate) fn open(
    target: &str,
    options: HostOpenOptions<'_>,
) -> Result<HostOpenReceipt, HostOpenError> {
    if options.background {
        return Err(background_unsupported());
    }
    if let Some(application) = options.application {
        return open_with_application(target, application);
    }
    open_default(target)
}

fn background_unsupported() -> HostOpenError {
    HostOpenError::with_detail(
        HostOpenErrorKind::Unsupported,
        "Linux host-open does not claim background activation semantics",
        HostOpenErrorDetail {
            os: "linux",
            required_mechanism: "freedesktop-background-launch",
            alternatives: LINUX_HOST_OPEN_ALTERNATIVES,
        },
    )
}

fn application_unsupported(message: impl Into<String>) -> HostOpenError {
    HostOpenError::with_detail(
        HostOpenErrorKind::Unsupported,
        message,
        HostOpenErrorDetail {
            os: "linux",
            required_mechanism: "freedesktop-application-launch",
            alternatives: LINUX_HOST_OPEN_ALTERNATIVES,
        },
    )
}

fn open_default(target: &str) -> Result<HostOpenReceipt, HostOpenError> {
    let launcher = find_system_binary(XDG_OPEN_PATHS).ok_or_else(|| {
        HostOpenError::new(
            HostOpenErrorKind::LauncherUnavailable,
            "xdg-open is not installed at a system path",
        )
    })?;
    let mut child = std::process::Command::new(&launcher)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            HostOpenError::new(
                HostOpenErrorKind::Native,
                format!("xdg-open could not start: {error}"),
            )
        })?;
    dispatch(&mut child, "linux-xdg-open", "xdg-open")
}

fn open_with_application(
    target: &str,
    application: &str,
) -> Result<HostOpenReceipt, HostOpenError> {
    if let Some(desktop) = resolve_desktop_entry(application)? {
        if let Some(receipt) = launch_desktop_entry(&desktop, target)? {
            return Ok(receipt);
        }
    }
    let Some(executable) = resolve_executable(application) else {
        return Err(application_unsupported(format!(
            "application selector {application:?} matched no desktop entry id and no executable on PATH"
        )));
    };
    launch_executable(&executable, target)
}

fn launch_desktop_entry(
    desktop: &DesktopEntry,
    target: &str,
) -> Result<Option<HostOpenReceipt>, HostOpenError> {
    if let Some(gio) = find_system_binary(GIO_PATHS) {
        let mut child = std::process::Command::new(&gio)
            .arg("launch")
            .arg(&desktop.path)
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                HostOpenError::new(
                    HostOpenErrorKind::Native,
                    format!("gio launch could not start: {error}"),
                )
            })?;
        return Ok(Some(dispatch(
            &mut child,
            "linux-gio-launch",
            "gio launch",
        )?));
    }
    if let Some(gtk_launch) = find_system_binary(GTK_LAUNCH_PATHS) {
        let mut child = std::process::Command::new(&gtk_launch)
            .arg(&desktop.launch_id)
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                HostOpenError::new(
                    HostOpenErrorKind::Native,
                    format!("gtk-launch could not start: {error}"),
                )
            })?;
        return Ok(Some(dispatch(
            &mut child,
            "linux-gtk-launch",
            "gtk-launch",
        )?));
    }
    Ok(None)
}

fn launch_executable(executable: &Path, target: &str) -> Result<HostOpenReceipt, HostOpenError> {
    let mut child = std::process::Command::new(executable)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            HostOpenError::new(
                HostOpenErrorKind::Native,
                format!("application launcher could not start: {error}"),
            )
        })?;
    dispatch(&mut child, "linux-app-exec", "application exec")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DesktopEntry {
    launch_id: String,
    path: PathBuf,
}

fn resolve_desktop_entry(application: &str) -> Result<Option<DesktopEntry>, HostOpenError> {
    let normalized = application.strip_suffix(".desktop").unwrap_or(application);
    if normalized.is_empty() {
        return Ok(None);
    }
    let mut visited = 0usize;
    for root in application_directories() {
        if let Some(entry) = scan_desktop_root(&root, normalized, &mut visited)? {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

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
    for path in std::env::split_paths(&system) {
        if path.is_absolute() {
            directories.push(path.join("applications"));
        }
    }
    directories
}

fn scan_desktop_root(
    root: &Path,
    selector: &str,
    visited: &mut usize,
) -> Result<Option<DesktopEntry>, HostOpenError> {
    let metadata = match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() => metadata,
        Ok(_) | Err(_) => return Ok(None),
    };
    if metadata.file_type().is_symlink() {
        return Ok(None);
    }
    scan_desktop_directory(root, root, selector, 0, visited)
}

fn scan_desktop_directory(
    root: &Path,
    directory: &Path,
    selector: &str,
    depth: usize,
    visited: &mut usize,
) -> Result<Option<DesktopEntry>, HostOpenError> {
    if depth > MAX_DIRECTORY_DEPTH {
        return Ok(None);
    }
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return Ok(None),
    };
    for entry in entries {
        *visited = visited.saturating_add(1);
        if *visited > MAX_DESKTOP_ENTRIES {
            return Ok(None);
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        let path = entry.path();
        if file_type.is_dir() && !file_type.is_symlink() {
            if let Some(found) = scan_desktop_directory(root, &path, selector, depth + 1, visited)?
            {
                return Ok(Some(found));
            }
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("desktop") {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let Some(id) = desktop_id(relative) else {
            continue;
        };
        let basename = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if id != selector && basename != selector {
            continue;
        }
        return Ok(Some(DesktopEntry {
            launch_id: basename.to_owned(),
            path: path.canonicalize().unwrap_or(path),
        }));
    }
    Ok(None)
}

fn desktop_id(relative: &Path) -> Option<String> {
    let mut components = Vec::new();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return None;
        };
        components.push(component.to_str()?);
    }
    (!components.is_empty()).then(|| components.join("-"))
}

fn resolve_executable(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let path = PathBuf::from(name);
        return is_launchable_file(&path).then_some(path);
    }
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| is_launchable_file(candidate))
}

fn is_launchable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn find_system_binary(paths: &[&str]) -> Option<String> {
    paths
        .iter()
        .copied()
        .find(|path| Path::new(path).is_file())
        .map(str::to_owned)
}

fn dispatch(
    child: &mut std::process::Child,
    provider: &'static str,
    label: &str,
) -> Result<HostOpenReceipt, HostOpenError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return Ok(HostOpenReceipt {
                    provider,
                    accepted: true,
                });
            }
            Ok(Some(status)) => {
                return Err(HostOpenError::new(
                    HostOpenErrorKind::Rejected,
                    format!("{label} rejected the request with {status}"),
                ));
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HostOpenError::new(
                    HostOpenErrorKind::TimedOut,
                    format!("{label} did not finish within 10 seconds"),
                ));
            }
            Err(error) => {
                return Err(HostOpenError::new(
                    HostOpenErrorKind::Native,
                    format!("{label} status failed: {error}"),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_open::HostOpenOptions;

    #[test]
    fn linux_rejects_background_before_dispatch() {
        let error = open(
            "https://example.com/",
            HostOpenOptions {
                application: None,
                background: true,
            },
        )
        .expect_err("background must stay unsupported");
        assert_eq!(error.kind(), HostOpenErrorKind::Unsupported);
        let detail = error.detail().expect("background unsupported detail");
        assert_eq!(detail.os, "linux");
        assert_eq!(detail.required_mechanism, "freedesktop-background-launch");
        assert!(!detail.alternatives.is_empty());
    }

    #[test]
    fn desktop_id_joins_nested_relative_paths() {
        assert_eq!(
            desktop_id(Path::new("vendor/example.desktop")),
            Some("vendor-example.desktop".to_owned())
        );
        assert_eq!(
            desktop_id(Path::new("firefox.desktop")),
            Some("firefox.desktop".to_owned())
        );
    }

    #[test]
    fn resolve_desktop_entry_finds_by_id_in_fixture_directory() {
        let root = std::env::temp_dir().join(format!(
            "agenterm-host-open-desktop-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        ));
        let applications = root.join("applications");
        std::fs::create_dir_all(&applications).expect("create applications dir");
        std::fs::write(
            applications.join("fixture-browser.desktop"),
            "[Desktop Entry]\nType=Application\nName=Fixture Browser\n",
        )
        .expect("write desktop entry");
        let previous = std::env::var_os("XDG_DATA_HOME");
        // SAFETY: test-only mutation of process environment; restored before return.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &root);
        }
        let resolved = resolve_desktop_entry("fixture-browser").expect("scan desktop entries");
        // SAFETY: test-only mutation of process environment; restored before return.
        unsafe {
            if let Some(previous) = previous {
                std::env::set_var("XDG_DATA_HOME", previous);
            } else {
                std::env::remove_var("XDG_DATA_HOME");
            }
        }
        let _ = std::fs::remove_dir_all(root);
        let entry = resolved.expect("fixture desktop entry");
        assert_eq!(entry.launch_id, "fixture-browser");
        assert!(entry.path.ends_with("fixture-browser.desktop"));
    }

    #[test]
    fn unknown_application_returns_enriched_unsupported() {
        let error = open(
            "https://example.invalid/no-such-app",
            HostOpenOptions {
                application: Some("agenterm-host-open-missing-app-xyzzy"),
                background: false,
            },
        )
        .expect_err("missing application must fail typed");
        assert_eq!(error.kind(), HostOpenErrorKind::Unsupported);
        let detail = error.detail().expect("unsupported detail");
        assert_eq!(detail.os, "linux");
        assert_eq!(detail.required_mechanism, "freedesktop-application-launch");
        assert!(
            detail
                .alternatives
                .iter()
                .any(|alternative| alternative.contains("host-open"))
        );
    }

    #[test]
    fn resolve_executable_finds_a_system_binary() {
        let Some(path) = resolve_executable("true") else {
            return;
        };
        assert!(path.is_absolute());
        assert!(is_launchable_file(&path));
    }

    #[test]
    fn open_with_application_can_dispatch_a_path_binary() {
        let Some(_true_path) = resolve_executable("true") else {
            return;
        };
        let receipt =
            open_with_application("/dev/null", "true").expect("true accepts a path argument");
        assert_eq!(receipt.provider, "linux-app-exec");
        assert!(receipt.accepted);
    }
}
