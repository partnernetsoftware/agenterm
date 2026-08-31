//! macOS installed-application inventory and LaunchServices launch.

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use crate::contract::app_inventory::{
    AppInventoryError, InstalledApp, InstalledApps, MAX_APP_PATH_BYTES, MAX_INSTALLED_APPS,
};

type CfTypeRef = *const c_void;
type CfStringRef = *const c_void;
type CfUrlRef = *const c_void;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
/// `kCFURLPOSIXPathStyle`.
const K_CF_URL_POSIX_PATH_STYLE: isize = 0;

#[allow(clippy::duplicated_attributes)]
#[link(name = "CoreFoundation", kind = "framework")]
#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CfTypeRef);
    fn CFStringCreateWithBytes(
        alloc: CfTypeRef,
        bytes: *const u8,
        num_bytes: isize,
        encoding: u32,
        is_external: u8,
    ) -> CfStringRef;
    fn CFURLCreateWithFileSystemPath(
        alloc: CfTypeRef,
        path: CfStringRef,
        style: isize,
        is_directory: u8,
    ) -> CfUrlRef;
    fn LSOpenCFURLRef(url: CfUrlRef, out_launched: *mut CfUrlRef) -> i32;
}

/// Where macOS keeps applications. The user's own folder is included
/// because an application installed there is as real as one in
/// `/Applications`; nothing outside these is scanned, so this never walks
/// the whole disk.
const SEARCH_ROOTS: &[&str] = &[
    "/Applications",
    "/Applications/Utilities",
    "/System/Applications",
    "/System/Applications/Utilities",
];

pub(crate) fn list_installed() -> Result<InstalledApps, AppInventoryError> {
    let mut roots: Vec<PathBuf> = SEARCH_ROOTS.iter().map(PathBuf::from).collect();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(Path::new(&home).join("Applications"));
    }
    let mut apps: Vec<InstalledApp> = Vec::new();
    let mut truncated = false;
    for root in roots {
        // A missing directory is ordinary: not every host has
        // `~/Applications` or the Utilities folders.
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("app") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let Some(path_text) = path.to_str() else {
                continue;
            };
            if apps.len() >= MAX_INSTALLED_APPS {
                truncated = true;
                break;
            }
            apps.push(InstalledApp {
                name: name.to_owned(),
                path: path_text.to_owned(),
            });
        }
        if truncated {
            break;
        }
    }
    apps.sort_by(|left, right| left.name.cmp(&right.name).then(left.path.cmp(&right.path)));
    apps.dedup_by(|left, right| left.path == right.path);
    Ok(InstalledApps { apps, truncated })
}

/// LaunchServices opens the bundle, exactly as a double click would.
///
/// No pid comes back and none is invented: LaunchServices owns the process
/// it starts. A caller that needs the pid looks for the window that
/// appears, which is also the only way to know the application is actually
/// up rather than merely asked to start.
pub(crate) fn launch(path: &str) -> Result<(), AppInventoryError> {
    if path.len() > MAX_APP_PATH_BYTES {
        return Err(AppInventoryError::failed(
            "invalid_input",
            format!("path exceeds {MAX_APP_PATH_BYTES} bytes"),
        ));
    }
    if !Path::new(path).exists() {
        return Err(AppInventoryError::failed(
            "app_not_found",
            format!("nothing exists at {path}"),
        ));
    }
    let url = unsafe {
        let text = CFStringCreateWithBytes(
            std::ptr::null(),
            path.as_ptr(),
            path.len() as isize,
            K_CF_STRING_ENCODING_UTF8,
            0,
        );
        if text.is_null() {
            return Err(AppInventoryError::failed(
                "app_launch_failed",
                "the path could not be encoded for LaunchServices",
            ));
        }
        let url =
            CFURLCreateWithFileSystemPath(std::ptr::null(), text, K_CF_URL_POSIX_PATH_STYLE, 1);
        CFRelease(text as CfTypeRef);
        url
    };
    if url.is_null() {
        return Err(AppInventoryError::failed(
            "app_launch_failed",
            "CFURLCreateWithFileSystemPath returned null",
        ));
    }
    let status = unsafe { LSOpenCFURLRef(url, std::ptr::null_mut()) };
    unsafe { CFRelease(url as CfTypeRef) };
    if status != 0 {
        return Err(AppInventoryError::failed(
            "app_launch_failed",
            format!("LSOpenCFURLRef returned OSStatus {status}"),
        ));
    }
    Ok(())
}
