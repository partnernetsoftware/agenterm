//! macOS selection for caller-owned XRGB screenshot encoding.

use std::{borrow::Cow, path::Path};

use crate::contract::ui_screenshot::{
    NativeCaptureArea, ScreenshotWindowHandle, ScreenshotWriteResult, UiScreenshotError, XrgbFrame,
};

pub(crate) fn write_xrgb_png(
    frame: XrgbFrame<'_>,
) -> Result<ScreenshotWriteResult, UiScreenshotError> {
    crate::selected::portable_png::write_xrgb_png(frame)
}

/// macOS window capture is not wired, and the reason is worth naming: the
/// API this would have used, `CGWindowListCreateImage`, was **obsoleted in
/// macOS 15.0 and removed from the SDK** (measured on this toolchain:
/// "'CGWindowListCreateImage' is unavailable: obsoleted in macOS 15.0 -
/// Please use ScreenCaptureKit instead"). The replacement is
/// ScreenCaptureKit -- `SCShareableContent` to find the window, an
/// `SCContentFilter`, then `SCScreenshotManager`, all block-based and all
/// gated on the Screen Recording permission, which is a different TCC
/// grant from the Accessibility one the rest of this adapter needs.
///
/// Nothing here degrades to a full-screen grab or a desktop image: a
/// screenshot of the wrong thing is worse than a typed refusal, and the
/// product's own rule is that a screenshot never replaces the tree.
pub(crate) fn capture_native_window_png(
    _window: ScreenshotWindowHandle,
    _path: &Path,
    _area: NativeCaptureArea,
) -> Result<ScreenshotWriteResult, UiScreenshotError> {
    Err(UiScreenshotError::Unsupported {
        reason: Cow::Borrowed(
            "native-window-capture-needs-screencapturekit: CGWindowListCreateImage was obsoleted in macOS 15.0",
        ),
    })
}
