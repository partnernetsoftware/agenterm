//! Linux selection for caller-owned XRGB screenshot encoding, plus X11
//! window capture.

use std::{borrow::Cow, path::Path};

use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};

use crate::contract::ui_screenshot::{
    NativeCaptureArea, ScreenshotWindowHandle, ScreenshotWriteResult, UiScreenshotError, XrgbFrame,
};

/// Largest window this captures, in pixels. A capture is a caller-owned
/// buffer, so an unbounded one is a memory decision made by whatever
/// window happens to be on screen.
const MAX_CAPTURE_PIXELS: usize = 64 * 1024 * 1024 / 4;

pub(crate) fn write_xrgb_png(
    frame: XrgbFrame<'_>,
) -> Result<ScreenshotWriteResult, UiScreenshotError> {
    crate::selected::portable_png::write_xrgb_png(frame)
}

fn failed(message: impl ToString) -> UiScreenshotError {
    UiScreenshotError::Failed {
        code: Cow::Borrowed("screenshot_failed"),
        message: message.to_string(),
    }
}

/// Capture one X11 window's pixels with `GetImage` and hand them to the
/// shared PNG writer.
///
/// `GetImage` reads the window's current contents from the server, so an
/// obscured or off-screen region may come back as whatever the server
/// happens to hold -- X11 does not promise backing store. That is a
/// property of the protocol, not something this code can paper over, and
/// it is why a screenshot never replaces the accessibility tree.
///
/// Only the ordinary TrueColor 32-bits-per-pixel case is converted. A
/// visual this does not understand is refused typed, naming what it found,
/// rather than reinterpreting the bytes and writing a plausible-looking
/// wrong image.
pub(crate) fn capture_native_window_png(
    window: ScreenshotWindowHandle,
    path: &Path,
    area: NativeCaptureArea,
) -> Result<ScreenshotWriteResult, UiScreenshotError> {
    let raw = window.raw();
    let window_id = u32::try_from(raw)
        .map_err(|_| failed(format!("window handle {raw} is not a valid XID")))?;
    let (connection, _) = x11rb::connect(None).map_err(|error| UiScreenshotError::Unsupported {
        reason: Cow::Owned(format!("X11 display could not be opened: {error}")),
    })?;
    let geometry = connection
        .get_geometry(window_id)
        .map_err(|error| failed(format!("GetGeometry could not be sent: {error}")))?
        .reply()
        .map_err(|error| failed(format!("GetGeometry failed: {error}")))?;
    let (x, y, width, height) = match area {
        NativeCaptureArea::Window => (0i16, 0i16, geometry.width, geometry.height),
        NativeCaptureArea::Client {
            left,
            top,
            width,
            height,
        } => (
            i16::try_from(left).map_err(|_| failed("client left is out of range"))?,
            i16::try_from(top).map_err(|_| failed("client top is out of range"))?,
            u16::try_from(width).map_err(|_| failed("client width is out of range"))?,
            u16::try_from(height).map_err(|_| failed("client height is out of range"))?,
        ),
    };
    if width == 0 || height == 0 {
        return Err(failed(format!(
            "the requested area is empty ({width}x{height})"
        )));
    }
    let pixels = usize::from(width) * usize::from(height);
    if pixels > MAX_CAPTURE_PIXELS {
        return Err(failed(format!(
            "the requested area is {pixels} pixels, over the {MAX_CAPTURE_PIXELS} bound"
        )));
    }
    let image = connection
        .get_image(
            ImageFormat::Z_PIXMAP,
            window_id,
            x,
            y,
            width,
            height,
            u32::MAX,
        )
        .map_err(|error| failed(format!("GetImage could not be sent: {error}")))?
        .reply()
        .map_err(|error| failed(format!("GetImage failed: {error}")))?;
    if image.depth != 24 && image.depth != 32 {
        return Err(UiScreenshotError::Unsupported {
            reason: Cow::Owned(format!(
                "X11 visual depth {} is not the 24/32-bit TrueColor case this converts",
                image.depth
            )),
        });
    }
    let expected = pixels
        .checked_mul(4)
        .ok_or_else(|| failed("capture size overflow"))?;
    if image.data.len() < expected {
        return Err(failed(format!(
            "GetImage returned {} bytes for {pixels} pixels at 4 bytes each",
            image.data.len()
        )));
    }
    // Z_PIXMAP at 32 bits per pixel on a little-endian server is B, G, R, X.
    let mut buffer = Vec::with_capacity(pixels);
    for chunk in image.data.chunks_exact(4).take(pixels) {
        let blue = u32::from(chunk[0]);
        let green = u32::from(chunk[1]);
        let red = u32::from(chunk[2]);
        buffer.push((red << 16) | (green << 8) | blue);
    }
    write_xrgb_png(XrgbFrame::new(
        path,
        u32::from(width),
        u32::from(height),
        &buffer,
    ))
}
