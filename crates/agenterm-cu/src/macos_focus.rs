//! macOS foreground reads for the window inventory: which application is
//! frontmost (NSWorkspace) and which of its windows holds focus (that
//! application's own `AXFocusedWindow`).
//!
//! Why this exists beside the platform mechanism's `focused` mark: the
//! mechanism resolves focus through the *system-wide* accessibility
//! element (`AXFocusedApplication` -> `AXFrontmost` -> `AXFocusedWindow`),
//! and that first read answers `kAXErrorCannotComplete` (-25204) from a
//! process that is not a descendant of the GUI session's front process --
//! a tmux server, an SSH login, a remote agent bridge -- while the
//! per-application element created from the frontmost pid answers every
//! step (measured 2026-09-03: Brave frontmost per NSWorkspace, system-wide
//! read -25204, app element -> window 22778). So `windows --focused`
//! returned no rows although an application plainly had a focused window.
//! `NSWorkspace.frontmostApplication` does not go through accessibility
//! messaging at all, and the app element is addressed by pid, so this
//! chain works wherever the inventory itself does.
//!
//! Reads only: nothing here activates, raises, or focuses anything.

#![cfg(target_os = "macos")]

use std::ffi::c_void;

use objc2_app_kit::NSWorkspace;

use crate::observe::FrontmostApp;

type CfTypeRef = *const c_void;
type CfStringRef = *const c_void;
type AxUiElementRef = *const c_void;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const AX_SUCCESS: i32 = 0;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CfTypeRef);
    fn CFStringCreateWithCString(alloc: CfTypeRef, c_str: *const i8, encoding: u32) -> CfStringRef;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AxUiElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AxUiElementRef,
        attribute: CfStringRef,
        value: *mut CfTypeRef,
    ) -> i32;
    /// Private but stable since 10.x; the platform adapter uses the same
    /// symbol to map an AX window to its CGWindowID.
    fn _AXUIElementGetWindow(element: AxUiElementRef, out: *mut u32) -> i32;
}

/// The application NSWorkspace reports as frontmost (the one owning the
/// menu bar), or `None` when there is none (no GUI session).
pub fn frontmost_app() -> Option<FrontmostApp> {
    // SAFETY: `sharedWorkspace` is a process-wide singleton; the returned
    // running-application object is retained for the duration of the
    // reads and every accessor is a plain property read.
    unsafe {
        let workspace = NSWorkspace::sharedWorkspace();
        let app = workspace.frontmostApplication()?;
        // `processIdentifier` is behind objc2-app-kit's `libc` feature;
        // the selector itself is a plain `pid_t` read.
        let pid: i32 = objc2::msg_send![&*app, processIdentifier];
        if pid <= 0 {
            return None;
        }
        Some(FrontmostApp {
            name: app
                .localizedName()
                .map(|name| name.to_string())
                .unwrap_or_default(),
            pid: pid as u32,
            bundle_id: app.bundleIdentifier().map(|id| id.to_string()),
        })
    }
}

/// The CGWindowID of `pid`'s focused window through that application's
/// own accessibility element, or `None` when the application does not
/// answer (no focused window, not an AX client, or the read failed).
pub fn focused_window_of(pid: u32) -> Option<isize> {
    // SAFETY: every CF object created here is released on every path; the
    // AX calls are reads with out-pointers to locals.
    unsafe {
        let app = AXUIElementCreateApplication(pid as i32);
        if app.is_null() {
            return None;
        }
        let key = CFStringCreateWithCString(
            std::ptr::null(),
            c"AXFocusedWindow".as_ptr(),
            K_CF_STRING_ENCODING_UTF8,
        );
        let mut window: CfTypeRef = std::ptr::null();
        let status = AXUIElementCopyAttributeValue(app, key, &mut window);
        CFRelease(key);
        CFRelease(app);
        if status != AX_SUCCESS || window.is_null() {
            return None;
        }
        let mut id = 0u32;
        let status = _AXUIElementGetWindow(window, &mut id);
        CFRelease(window);
        (status == AX_SUCCESS && id != 0).then_some(id as isize)
    }
}
