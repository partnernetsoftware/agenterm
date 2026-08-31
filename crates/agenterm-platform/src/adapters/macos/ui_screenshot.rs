//! macOS selection for caller-owned XRGB screenshot encoding.

use std::{borrow::Cow, path::Path};

use crate::contract::ui_screenshot::{
    NativeCaptureArea, ScreenshotWindowHandle, ScreenshotWriteResult, UiScreenshotError, XrgbClip,
    XrgbFrame,
};

pub(crate) fn write_xrgb_png(
    frame: XrgbFrame<'_>,
) -> Result<ScreenshotWriteResult, UiScreenshotError> {
    crate::selected::portable_png::write_xrgb_png(frame)
}

/// Capture one window's pixels.
///
/// `CGWindowListCreateImage` is **not in the SDK any more** -- the compiler
/// refuses it outright ("obsoleted in macOS 15.0 - Please use
/// ScreenCaptureKit instead"). It is still in the framework binary, and
/// measured on macOS 26.5 it still captures: a probe against a live window
/// returned a 1120x944 32bpp image with real content, not a stub's NULL.
/// So it is resolved by `dlsym`, the same way this product already reaches
/// SkyLight for Space attribution.
///
/// That is a deliberate choice with a real cost, and the reply names the
/// route so nobody has to guess: an obsoleted symbol can disappear in any
/// future macOS. When it does, this refuses typed with the replacement
/// named -- it does not crash, and it does not silently degrade to a
/// full-screen grab. Nothing here ever captures the desktop instead of the
/// window: a screenshot of the wrong thing is worse than a typed refusal,
/// and the product's rule is that a screenshot never replaces the tree.
pub(crate) fn capture_native_window_png(
    window: ScreenshotWindowHandle,
    path: &Path,
    area: NativeCaptureArea,
) -> Result<ScreenshotWriteResult, UiScreenshotError> {
    let window_id = u32::try_from(window.raw()).map_err(|_| UiScreenshotError::Failed {
        code: Cow::Borrowed("invalid_input"),
        message: "window handle is not a CGWindowID".to_owned(),
    })?;
    let image = capture_window_image(window_id)?;
    let pixels = image.xrgb_pixels()?;
    let frame = XrgbFrame::new(path, image.width, image.height, &pixels);
    let frame = match area {
        NativeCaptureArea::Window => frame,
        // A client-area request is a clip of the same capture; the encoder
        // already owns clipping, so this never re-captures.
        NativeCaptureArea::Client {
            left,
            top,
            width,
            height,
        } => {
            let clip = XrgbClip::new(
                u32::try_from(left.max(0)).unwrap_or(0),
                u32::try_from(top.max(0)).unwrap_or(0),
                u32::try_from(width.max(0)).unwrap_or(0),
                u32::try_from(height.max(0)).unwrap_or(0),
            );
            frame.with_clip(clip)
        }
    };
    write_xrgb_png(frame)
}

type CfTypeRef = *const std::ffi::c_void;
type CgImageRef = *const std::ffi::c_void;
type CfDataRef = *const std::ffi::c_void;

/// `kCGWindowListOptionIncludingWindow`.
const WINDOW_LIST_INCLUDING_WINDOW: u32 = 1 << 3;
/// `kCGWindowImageBoundsIgnoreFraming`.
const WINDOW_IMAGE_IGNORE_FRAMING: u32 = 1 << 0;

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

/// `CGRectNull`, which asks for the window's own bounds.
const CG_RECT_NULL: CgRect = CgRect {
    origin: CgPoint {
        x: f64::INFINITY,
        y: f64::INFINITY,
    },
    size: CgSize {
        width: 0.0,
        height: 0.0,
    },
};

unsafe extern "C" {
    fn dlopen(path: *const std::ffi::c_char, mode: i32) -> *mut std::ffi::c_void;
    fn dlsym(handle: *mut std::ffi::c_void, symbol: *const std::ffi::c_char)
    -> *mut std::ffi::c_void;
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

/// A captured image, released on drop.
struct WindowImage {
    image: CgImageRef,
    width: u32,
    height: u32,
}

impl Drop for WindowImage {
    fn drop(&mut self) {
        if !self.image.is_null() {
            unsafe { CFRelease(self.image as CfTypeRef) };
        }
    }
}

fn capture_window_image(window_id: u32) -> Result<WindowImage, UiScreenshotError> {
    type ListImage = unsafe extern "C" fn(CgRect, u32, u32, u32) -> CgImageRef;

    let core_graphics = c"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics";
    // RTLD_LAZY
    let handle = unsafe { dlopen(core_graphics.as_ptr(), 1) };
    if handle.is_null() {
        return Err(UiScreenshotError::Unsupported {
            reason: Cow::Borrowed("CoreGraphics.framework is not loadable"),
        });
    }
    let symbol = unsafe { dlsym(handle, c"CGWindowListCreateImage".as_ptr()) };
    if symbol.is_null() {
        return Err(UiScreenshotError::Unsupported {
            reason: Cow::Borrowed(
                "CGWindowListCreateImage is gone from this macOS; window capture needs a ScreenCaptureKit route",
            ),
        });
    }
    let list_image: ListImage = unsafe { std::mem::transmute(symbol) };
    let image = unsafe {
        list_image(
            CG_RECT_NULL,
            WINDOW_LIST_INCLUDING_WINDOW,
            window_id,
            WINDOW_IMAGE_IGNORE_FRAMING,
        )
    };
    if image.is_null() {
        // The window is gone, or Screen Recording is not granted -- the API
        // reports both the same way, so the message names both rather than
        // asserting one.
        return Err(UiScreenshotError::Failed {
            code: Cow::Borrowed("screenshot_window_unavailable"),
            message: format!(
                "no image for window {window_id}: it is gone, or this process lacks Screen Recording"
            ),
        });
    }
    let width = unsafe { CGImageGetWidth(image) };
    let height = unsafe { CGImageGetHeight(image) };
    let ok = u32::try_from(width).is_ok() && u32::try_from(height).is_ok() && width > 0 && height > 0;
    if !ok {
        unsafe { CFRelease(image as CfTypeRef) };
        return Err(UiScreenshotError::Failed {
            code: Cow::Borrowed("screenshot_window_unavailable"),
            message: format!("window {window_id} captured as {width}x{height}"),
        });
    }
    Ok(WindowImage {
        image,
        width: width as u32,
        height: height as u32,
    })
}

impl WindowImage {
    /// The capture as `0x00RRGGBB`, row by row.
    ///
    /// `bytesPerRow` is not always `width * 4` -- CoreGraphics pads rows --
    /// so the stride is read rather than assumed; using the width would
    /// shear the image on any window whose row size is padded.
    fn xrgb_pixels(&self) -> Result<Vec<u32>, UiScreenshotError> {
        let bits = unsafe { CGImageGetBitsPerPixel(self.image) };
        if bits != 32 {
            return Err(UiScreenshotError::Failed {
                code: Cow::Borrowed("screenshot_format_unsupported"),
                message: format!("capture is {bits} bits per pixel; only 32 is decoded here"),
            });
        }
        let stride = unsafe { CGImageGetBytesPerRow(self.image) };
        let provider = unsafe { CGImageGetDataProvider(self.image) };
        if provider.is_null() {
            return Err(UiScreenshotError::Failed {
                code: Cow::Borrowed("screenshot_format_unsupported"),
                message: "the capture carries no pixel data".to_owned(),
            });
        }
        let data = unsafe { CGDataProviderCopyData(provider) };
        if data.is_null() {
            return Err(UiScreenshotError::Failed {
                code: Cow::Borrowed("screenshot_format_unsupported"),
                message: "the capture's pixel data could not be copied".to_owned(),
            });
        }
        let bytes = unsafe { CFDataGetBytePtr(data) };
        let length = unsafe { CFDataGetLength(data) };
        let length = usize::try_from(length).unwrap_or(0);
        let width = self.width as usize;
        let height = self.height as usize;
        if bytes.is_null() || stride < width * 4 || length < stride * height {
            unsafe { CFRelease(data) };
            return Err(UiScreenshotError::Failed {
                code: Cow::Borrowed("screenshot_format_unsupported"),
                message: format!(
                    "capture buffer is {length} bytes for {width}x{height} at stride {stride}"
                ),
            });
        }
        let mut pixels = Vec::with_capacity(width * height);
        for row in 0..height {
            let start = row * stride;
            for column in 0..width {
                let at = start + column * 4;
                // BGRA little-endian in memory: byte 0 is blue.
                let blue = unsafe { *bytes.add(at) } as u32;
                let green = unsafe { *bytes.add(at + 1) } as u32;
                let red = unsafe { *bytes.add(at + 2) } as u32;
                pixels.push((red << 16) | (green << 8) | blue);
            }
        }
        unsafe { CFRelease(data) };
        Ok(pixels)
    }
}
