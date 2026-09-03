//! Linux X11 keyboard bridge: bundle `libxkbcommon-x11` and its XCB-XKB
//! dependency when the host omits them.
//!
//! Winit loads this library through `xkbcommon-dl` at runtime, so the ELF import
//! table never lists it. When the system package is absent we stage our own copy
//! and re-exec once with `LD_LIBRARY_PATH` primed, because the dynamic loader
//! only honors that variable at process start.

use std::ffi::{CString, OsStr, c_char, c_int, c_void};
use std::io::Write as _;
use std::path::{Path, PathBuf};

const XKB_X11_SONAME: &str = "libxkbcommon-x11.so.0";
const XCB_XKB_SONAME: &str = "libxcb-xkb.so.1";
#[cfg(target_arch = "x86_64")]
const BUNDLED_XKB_X11: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../vendor/linux/x86_64/libxkbcommon-x11.so.0"
));
#[cfg(target_arch = "x86_64")]
const BUNDLED_XCB_XKB: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../vendor/linux/x86_64/libxcb-xkb.so.1"
));
#[cfg(target_arch = "aarch64")]
const BUNDLED_XKB_X11: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../vendor/linux/aarch64/libxkbcommon-x11.so.0"
));
#[cfg(target_arch = "aarch64")]
const BUNDLED_XCB_XKB: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../vendor/linux/aarch64/libxcb-xkb.so.1"
));
const STAGED_ENV: &str = "PLATFORM_XKB_X11_STAGED";
const RTLD_NOW: c_int = 2;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

struct DynamicLibrary(*mut c_void);

impl Drop for DynamicLibrary {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful `dlopen` in this module
        // and is closed exactly once by this owner.
        unsafe {
            let _ = dlclose(self.0);
        }
    }
}

pub(crate) fn preflight() -> Result<(), String> {
    let display = std::env::var_os("DISPLAY");
    let wayland = std::env::var_os("WAYLAND_DISPLAY");
    let backend = std::env::var_os("WINIT_UNIX_BACKEND");
    preflight_with(
        display.as_deref(),
        wayland.as_deref(),
        backend.as_deref(),
        system_has_xkb_x11,
        stage_bundled_xkb_x11,
        reexec_with_library_path,
    )
}

fn preflight_with(
    display: Option<&OsStr>,
    wayland: Option<&OsStr>,
    backend: Option<&OsStr>,
    probe: impl FnOnce() -> bool,
    stage: impl FnOnce() -> Result<PathBuf, String>,
    reexec: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    if !requires_x11(display, wayland, backend) {
        return Ok(());
    }
    if probe() {
        return Ok(());
    }
    if std::env::var_os(STAGED_ENV).is_some_and(|value| !value.is_empty()) {
        return Err(bundled_library_unavailable());
    }
    let library_dir = stage()?;
    reexec(&library_dir)
}

fn requires_x11(display: Option<&OsStr>, wayland: Option<&OsStr>, backend: Option<&OsStr>) -> bool {
    if backend.is_some_and(|value| value.eq_ignore_ascii_case("x11")) {
        return true;
    }
    if backend.is_some_and(|value| value.eq_ignore_ascii_case("wayland")) {
        return false;
    }
    display.is_some_and(|value| !value.is_empty())
        && !wayland.is_some_and(|value| !value.is_empty())
}

fn system_has_xkb_x11() -> bool {
    let soname = CString::new(XKB_X11_SONAME).expect("static SONAME contains no NUL");
    // SAFETY: this probes the same versioned SONAME winit will dlopen next.
    let handle = unsafe { dlopen(soname.as_ptr(), RTLD_NOW) };
    if handle.is_null() {
        return false;
    }
    let _library = DynamicLibrary(handle);
    true
}

fn stage_bundled_xkb_x11() -> Result<PathBuf, String> {
    let root = cache_root()?;
    std::fs::create_dir_all(&root).map_err(|error| {
        format!(
            "could not create bundled XKB cache at {}: {error}",
            root.display()
        )
    })?;
    for (soname, bytes) in [
        (XKB_X11_SONAME, BUNDLED_XKB_X11),
        (XCB_XKB_SONAME, BUNDLED_XCB_XKB),
    ] {
        let destination = root.join(soname);
        if !library_matches(&destination, bytes) {
            write_bundled_library(&destination, soname, bytes)?;
        }
    }
    Ok(root)
}

fn cache_root() -> Result<PathBuf, String> {
    if let Some(runtime) = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return Ok(runtime.join("agenterm").join("xkb-x11"));
    }
    if let Some(home) = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return Ok(home.join(".cache").join("agenterm").join("xkb-x11"));
    }
    let fallback = std::env::temp_dir().join(format!("agenterm-xkb-x11-{}", std::process::id()));
    Ok(fallback)
}

fn library_matches(path: &Path, expected: &[u8]) -> bool {
    std::fs::read(path)
        .ok()
        .is_some_and(|bytes| bytes == expected)
}

fn write_bundled_library(path: &Path, soname: &str, bytes: &[u8]) -> Result<(), String> {
    let mut file = std::fs::File::create(path).map_err(|error| {
        format!(
            "could not write bundled {soname} to {}: {error}",
            path.display()
        )
    })?;
    file.write_all(bytes).map_err(|error| {
        format!(
            "could not write bundled {soname} to {}: {error}",
            path.display()
        )
    })?;
    let mut permissions = std::fs::metadata(path)
        .map_err(|error| format!("could not stat {}: {error}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).map_err(|error| {
        format!(
            "could not chmod bundled {soname} at {}: {error}",
            path.display()
        )
    })
}

fn reexec_with_library_path(directory: &Path) -> Result<(), String> {
    let directory = directory
        .canonicalize()
        .map_err(|error| format!("could not resolve bundled library directory: {error}"))?;
    let mut library_path = directory.to_string_lossy().into_owned();
    if let Ok(existing) = std::env::var("LD_LIBRARY_PATH")
        && !existing.is_empty()
    {
        library_path.push(':');
        library_path.push_str(&existing);
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not resolve current executable: {error}"))?;
    let status = std::process::Command::new(executable)
        .args(std::env::args().skip(1))
        .env(STAGED_ENV, "1")
        .env("LD_LIBRARY_PATH", library_path)
        .status()
        .map_err(|error| format!("could not restart AgenTerm with bundled XKB support: {error}"))?;
    std::process::exit(status.code().unwrap_or(1));
}

fn bundled_library_unavailable() -> String {
    format!(
        "Linux X11 runtime dependencies unavailable: {XKB_X11_SONAME} / {XCB_XKB_SONAME}. \
         AgenTerm staged its bundled copies but the dynamic loader still could not use them."
    )
}

#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt as _;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("test lock")
    }

    #[test]
    fn bundled_bytes_are_non_empty() {
        assert!(!BUNDLED_XKB_X11.is_empty());
        assert!(!BUNDLED_XCB_XKB.is_empty());
    }

    #[test]
    fn wayland_only_does_not_stage_the_x11_bridge() {
        assert_eq!(
            preflight_with(
                None,
                Some(OsStr::new("wayland-0")),
                None,
                || { panic!("must not probe") },
                || Err("must not stage".to_owned()),
                |_| Ok(()),
            ),
            Ok(())
        );
    }

    #[test]
    fn explicit_backend_selection_wins_over_ambient_variables() {
        assert!(requires_x11(
            None,
            Some(OsStr::new("wayland-0")),
            Some(OsStr::new("x11"))
        ));
        assert!(!requires_x11(
            Some(OsStr::new(":0")),
            None,
            Some(OsStr::new("wayland"))
        ));
    }

    #[test]
    fn staging_writes_the_bundled_library_bytes() {
        let _guard = lock();
        let scratch = std::env::temp_dir().join(format!(
            "agenterm-xkb-stage-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&scratch).expect("scratch dir");
        let previous_cache = std::env::var_os("XDG_CACHE_HOME");
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", &scratch);
        }
        let library_dir = stage_bundled_xkb_x11().expect("staging succeeds");
        let library = library_dir.join(XKB_X11_SONAME);
        let transitive = library_dir.join(XCB_XKB_SONAME);
        if let Some(value) = previous_cache {
            unsafe {
                std::env::set_var("XDG_CACHE_HOME", value);
            }
        } else {
            unsafe {
                std::env::remove_var("XDG_CACHE_HOME");
            }
        }
        assert!(library.is_file());
        assert_eq!(
            std::fs::read(&library).expect("read staged library"),
            BUNDLED_XKB_X11
        );
        assert_eq!(
            std::fs::read(&transitive).expect("read staged transitive library"),
            BUNDLED_XCB_XKB
        );
        let _ = std::fs::remove_dir_all(scratch);
    }
}
