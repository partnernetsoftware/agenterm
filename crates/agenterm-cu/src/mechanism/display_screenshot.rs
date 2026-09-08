//! Full-display screenshot when `screenshot` omits `--window`.
//!
//! Window capture stays on `agt_screenshot_capture_window`; display capture
//! resolves one native desktop surface per host and reuses the same PNG
//! writer so the CU reply shape stays identical.

use std::path::Path;

use super::{MechanismError, screenshot::ScreenshotWriteResult};

#[cfg(target_os = "macos")]
use super::map_status;

/// Capture the primary display (or the sole X11 root) to `path`.
pub fn capture_native_display_png(path: &Path) -> Result<ScreenshotWriteResult, MechanismError> {
    #[cfg(target_os = "macos")]
    {
        capture_macos_display_png(path)
    }
    #[cfg(any(target_os = "linux", windows))]
    {
        let handle = display_capture_handle()?;
        super::screenshot::capture_native_window_png(handle, path)
    }
}

#[cfg(target_os = "linux")]
fn display_capture_handle() -> Result<isize, MechanismError> {
    use x11rb::connection::Connection;

    let (connection, screen) =
        x11rb::connect(None).map_err(|error| MechanismError::Unsupported {
            reason: format!("X11 display could not be opened: {error}"),
        })?;
    let root = connection
        .setup()
        .roots
        .get(screen)
        .ok_or_else(|| MechanismError::Failed {
            code: "display_unavailable".to_owned(),
            message: "configured X11 screen does not exist".to_owned(),
        })?
        .root;
    Ok(root as isize)
}

#[cfg(windows)]
fn display_capture_handle() -> Result<isize, MechanismError> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetDesktopWindow;
    // SAFETY: `GetDesktopWindow` has no pointer arguments and returns a
    // process-independent pseudo-window handle which we validate below.
    let hwnd = unsafe { GetDesktopWindow() };
    if hwnd.is_null() {
        return Err(MechanismError::Failed {
            code: "display_unavailable".to_owned(),
            message: "GetDesktopWindow returned null".to_owned(),
        });
    }
    Ok(hwnd as isize)
}

#[cfg(target_os = "macos")]
fn capture_macos_display_png(path: &Path) -> Result<ScreenshotWriteResult, MechanismError> {
    use std::ffi::CString;

    let screens = super::window_enumerate::list_screens()?;
    let primary = screens
        .iter()
        .find(|screen| screen.primary)
        .or_else(|| screens.first())
        .ok_or_else(|| MechanismError::Failed {
            code: "display_unavailable".to_owned(),
            message: "no displays are available".to_owned(),
        })?;
    let frame = &primary.frame;
    if frame.width == 0 || frame.height == 0 {
        return Err(MechanismError::Failed {
            code: "display_unavailable".to_owned(),
            message: format!(
                "primary display frame is empty ({}x{})",
                frame.width, frame.height
            ),
        });
    }

    let pixels = macos::capture_on_screen_rect(frame.x, frame.y, frame.width, frame.height)?;
    let width = frame.width;
    let height = frame.height;

    let path_c =
        CString::new(path.to_string_lossy().as_bytes()).map_err(|_| MechanismError::Failed {
            code: "bad_path".to_owned(),
            message: "path contains an interior NUL byte".to_owned(),
        })?;
    let f = super::call_sym::<super::ScreenshotWritePng>(b"agt_screenshot_write_png")?;
    let status = unsafe {
        f(
            path_c.as_ptr(),
            pixels.as_ptr(),
            pixels.len(),
            width,
            height,
        )
    };
    map_status("agt_screenshot_write_png", status)?;
    let output_pixels = width as usize * height as usize;
    Ok(ScreenshotWriteResult {
        frame_width: width,
        frame_height: height,
        output_width: width,
        output_height: height,
        output_pixels,
    })
}

#[cfg(target_os = "macos")]
mod macos {
    use super::MechanismError;

    type CfTypeRef = *const std::ffi::c_void;
    type CgImageRef = *const std::ffi::c_void;
    type CfDataRef = *const std::ffi::c_void;

    /// `kCGWindowListOptionOnScreenOnly`
    const WINDOW_LIST_ON_SCREEN_ONLY: u32 = 1 << 11;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CgPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CgSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CgRect {
        origin: CgPoint,
        size: CgSize,
    }

    unsafe extern "C" {
        fn dlopen(path: *const std::ffi::c_char, mode: i32) -> *mut std::ffi::c_void;
        fn dlsym(
            handle: *mut std::ffi::c_void,
            symbol: *const std::ffi::c_char,
        ) -> *mut std::ffi::c_void;
        fn CFRelease(cf: CfTypeRef);
        fn CGImageGetWidth(image: CgImageRef) -> usize;
        fn CGImageGetHeight(image: CgImageRef) -> usize;
        fn CGImageGetBitsPerPixel(image: CgImageRef) -> usize;
        fn CGImageGetBytesPerRow(image: CgImageRef) -> usize;
        fn CGImageGetDataProvider(image: CgImageRef) -> CfTypeRef;
        fn CGDataProviderCopyData(provider: CfTypeRef) -> CfDataRef;
        fn CFDataGetBytePtr(data: CfDataRef) -> *const u8;
        fn CFDataGetLength(data: CfDataRef) -> isize;
    }

    pub(super) fn capture_on_screen_rect(
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<Vec<u32>, MechanismError> {
        type ListImage = unsafe extern "C" fn(CgRect, u32, u32, u32) -> CgImageRef;

        let core_graphics = c"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics";
        let handle = unsafe { dlopen(core_graphics.as_ptr(), 1) };
        if handle.is_null() {
            return Err(MechanismError::Unsupported {
                reason: "CoreGraphics.framework is not loadable".to_owned(),
            });
        }
        let symbol = unsafe { dlsym(handle, c"CGWindowListCreateImage".as_ptr()) };
        if symbol.is_null() {
            return Err(MechanismError::Unsupported {
                reason:
                    "CGWindowListCreateImage is unavailable; display capture needs ScreenCaptureKit"
                        .to_owned(),
            });
        }
        let list_image: ListImage = unsafe { std::mem::transmute(symbol) };
        let rect = CgRect {
            origin: CgPoint {
                x: f64::from(x),
                y: f64::from(y),
            },
            size: CgSize {
                width: f64::from(width),
                height: f64::from(height),
            },
        };
        let image = unsafe { list_image(rect, WINDOW_LIST_ON_SCREEN_ONLY, 0, 0) };
        if image.is_null() {
            return Err(MechanismError::Failed {
                code: "screenshot_failed".to_owned(),
                message:
                    "display capture returned no image: Screen Recording may be denied or the display is unavailable"
                        .to_owned(),
            });
        }
        let result = xrgb_pixels(image);
        unsafe { CFRelease(image as CfTypeRef) };
        result
    }

    fn xrgb_pixels(image: CgImageRef) -> Result<Vec<u32>, MechanismError> {
        let width = unsafe { CGImageGetWidth(image) };
        let height = unsafe { CGImageGetHeight(image) };
        let bpp = unsafe { CGImageGetBitsPerPixel(image) };
        if width == 0 || height == 0 {
            return Err(MechanismError::Failed {
                code: "screenshot_failed".to_owned(),
                message: "display capture image has zero dimensions".to_owned(),
            });
        }
        if bpp != 32 {
            return Err(MechanismError::Unsupported {
                reason: format!(
                    "display capture returned {bpp} bits per pixel, not the 32-bit case this converts"
                ),
            });
        }
        let provider = unsafe { CGImageGetDataProvider(image) };
        if provider.is_null() {
            return Err(MechanismError::Failed {
                code: "screenshot_failed".to_owned(),
                message: "display capture image has no data provider".to_owned(),
            });
        }
        let data = unsafe { CGDataProviderCopyData(provider) };
        if data.is_null() {
            return Err(MechanismError::Failed {
                code: "screenshot_failed".to_owned(),
                message: "display capture image data could not be copied".to_owned(),
            });
        }
        let ptr = unsafe { CFDataGetBytePtr(data) };
        let len = unsafe { CFDataGetLength(data) } as usize;
        let row_bytes = unsafe { CGImageGetBytesPerRow(image) };
        let mut buffer = Vec::with_capacity(width * height);
        for row in 0..height {
            let start = row * row_bytes;
            if start + width * 4 > len {
                unsafe { CFRelease(data as CfTypeRef) };
                return Err(MechanismError::Failed {
                    code: "screenshot_failed".to_owned(),
                    message: "display capture image row bytes are truncated".to_owned(),
                });
            }
            for col in 0..width {
                let offset = start + col * 4;
                let b = u32::from(unsafe { *ptr.add(offset) });
                let g = u32::from(unsafe { *ptr.add(offset + 1) });
                let r = u32::from(unsafe { *ptr.add(offset + 2) });
                buffer.push((r << 16) | (g << 8) | b);
            }
        }
        unsafe { CFRelease(data as CfTypeRef) };
        Ok(buffer)
    }
}
