//! XRGB screenshot facade.

use crate::{
    contract::ui_screenshot::{ScreenshotWriteResult, UiScreenshotError, XrgbClip},
    selected,
};

pub use crate::contract::ui_screenshot::{NativeCaptureArea, ScreenshotWindowHandle, XrgbFrame};

/// Maximum accepted framebuffer side length in pixels.
pub const MAX_FRAME_SIDE: u32 = 16_384;

/// Maximum accepted framebuffer pixel count.
pub const MAX_FRAME_PIXELS: usize = 64 * 1024 * 1024;

/// Owned snapshot pixels for asynchronous encoding.
///
/// Snapshot storage is an anonymous mapping rather than a malloc allocation.
/// Dropping the final owner releases it directly, including after an encoding
/// failure; large frames cannot linger in the allocator's free-block cache.
#[derive(Debug)]
pub struct OwnedXrgbPixels {
    mapping: memmap2::Mmap,
}

impl OwnedXrgbPixels {
    /// Copy a complete, bounded frame while the caller's pixels are borrowed.
    /// The resulting immutable owner may be moved to an encoding worker.
    pub fn copy_from(width: u32, height: u32, pixels: &[u32]) -> Result<Self, UiScreenshotError> {
        let count = checked_pixel_count(width, height)?;
        if pixels.len() != count {
            return Err(UiScreenshotError::failed(
                "screenshot_buffer_size_mismatch",
                "snapshot pixel count must match its dimensions",
            ));
        }
        // checked_pixel_count limits this to 64M u32 pixels (256 MiB), below
        // isize::MAX on every supported target and divisible by sizeof(u32).
        let mut mapping = memmap2::MmapMut::map_anon(count * std::mem::size_of::<u32>())
            .map_err(snapshot_storage_error)?;
        for (destination, pixel) in mapping.chunks_exact_mut(4).zip(pixels) {
            destination.copy_from_slice(&pixel.to_ne_bytes());
        }
        let mapping = mapping.make_read_only().map_err(snapshot_storage_error)?;
        Ok(Self { mapping })
    }

    pub fn pixels(&self) -> &[u32] {
        // SAFETY: mmap storage is page-aligned, initialized above in native
        // endian u32 units, immutable, and bounded below isize::MAX. All u32
        // bit patterns are valid, and the borrow cannot outlive its owner.
        unsafe {
            std::slice::from_raw_parts(
                self.mapping.as_ptr().cast::<u32>(),
                self.mapping.len() / std::mem::size_of::<u32>(),
            )
        }
    }
}

fn snapshot_storage_error(error: std::io::Error) -> UiScreenshotError {
    UiScreenshotError::failed("screenshot_storage_error", error.to_string())
}

/// Encode a caller-owned little-endian `0x00RRGGBB` framebuffer as an RGBA PNG.
pub fn write_xrgb_png(frame: XrgbFrame<'_>) -> Result<ScreenshotWriteResult, UiScreenshotError> {
    selected::ui_screenshot::write_xrgb_png(frame)
}

/// Capture a native window or a strict client-area rectangle into a PNG.
pub fn capture_native_window_png(
    window: ScreenshotWindowHandle,
    path: &std::path::Path,
    area: NativeCaptureArea,
) -> Result<ScreenshotWriteResult, UiScreenshotError> {
    selected::ui_screenshot::capture_native_window_png(window, path, area)
}

pub(crate) fn checked_frame(
    frame: &XrgbFrame<'_>,
) -> Result<(u32, u32, u32, u32, usize), UiScreenshotError> {
    if frame.path().as_os_str().is_empty() {
        return Err(UiScreenshotError::failed(
            "screenshot_empty_path",
            "screenshot output path is empty",
        ));
    }
    let frame_pixels = checked_pixel_count(frame.width(), frame.height())?;
    if frame.pixels().len() < frame_pixels {
        return Err(UiScreenshotError::failed(
            "screenshot_buffer_too_small",
            format!(
                "screenshot requires {frame_pixels} pixels but received {}",
                frame.pixels().len()
            ),
        ));
    }
    let (x, y, output_width, output_height) =
        checked_clip(frame.width(), frame.height(), frame.clip())?;
    let output_pixels = checked_pixel_count(output_width, output_height)?;
    Ok((x, y, output_width, output_height, output_pixels))
}

fn checked_pixel_count(width: u32, height: u32) -> Result<usize, UiScreenshotError> {
    if width == 0 || height == 0 {
        return Err(UiScreenshotError::failed(
            "screenshot_invalid_dimensions",
            "screenshot dimensions must be non-zero",
        ));
    }
    if width > MAX_FRAME_SIDE || height > MAX_FRAME_SIDE {
        return Err(frame_too_large(width, height));
    }
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| frame_too_large(width, height))?;
    if pixels > MAX_FRAME_PIXELS {
        return Err(frame_too_large(width, height));
    }
    Ok(pixels)
}

fn frame_too_large(width: u32, height: u32) -> UiScreenshotError {
    UiScreenshotError::failed(
        "screenshot_too_large",
        format!(
            "screenshot {width}x{height} exceeds side limit {MAX_FRAME_SIDE} or pixel limit {MAX_FRAME_PIXELS}"
        ),
    )
}

pub(crate) fn checked_clip(
    width: u32,
    height: u32,
    clip: Option<XrgbClip>,
) -> Result<(u32, u32, u32, u32), UiScreenshotError> {
    let Some(clip) = clip else {
        return Ok((0, 0, width, height));
    };
    let right = clip.x.checked_add(clip.width);
    let bottom = clip.y.checked_add(clip.height);
    if clip.width == 0
        || clip.height == 0
        || clip.x >= width
        || clip.y >= height
        || right.is_none_or(|right| right > width)
        || bottom.is_none_or(|bottom| bottom > height)
    {
        return Err(UiScreenshotError::failed(
            "screenshot_invalid_clip",
            format!(
                "screenshot clip {}x{} at ({},{}) is outside {}x{} frame",
                clip.width, clip.height, clip.x, clip.y, width, height
            ),
        ));
    }
    Ok((clip.x, clip.y, clip.width, clip.height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn dimensions_buffers_and_clip_fail_before_encoding() {
        assert_eq!(
            write_xrgb_png(XrgbFrame::new(Path::new("unused.png"), 0, 2, &[]))
                .expect_err("zero width")
                .code(),
            "screenshot_invalid_dimensions"
        );
        assert_eq!(
            write_xrgb_png(XrgbFrame::new(Path::new("unused.png"), 2, 2, &[0; 3]))
                .expect_err("short buffer")
                .code(),
            "screenshot_buffer_too_small"
        );
        assert_eq!(
            write_xrgb_png(
                XrgbFrame::new(Path::new("unused.png"), 2, 2, &[0; 4])
                    .with_clip(XrgbClip::new(1, 1, 2, 2))
            )
            .expect_err("outside clip")
            .code(),
            "screenshot_invalid_clip"
        );
    }

    #[test]
    fn snapshot_owns_pixels_across_source_changes_and_worker_transfer() {
        let mut source = [0xDEFF0000, 0x0000FF00, 0x000000FF, 0xFFFFFFFF];
        let snapshot = OwnedXrgbPixels::copy_from(2, 2, &source).expect("snapshot");
        source.fill(0);
        std::thread::spawn(move || {
            assert_eq!(
                snapshot.pixels(),
                &[0xDEFF0000, 0x0000FF00, 0x000000FF, 0xFFFFFFFF]
            );
        })
        .join()
        .expect("worker");
        assert!(OwnedXrgbPixels::copy_from(0, 1, &[]).is_err());
        assert!(OwnedXrgbPixels::copy_from(2, 2, &[0; 3]).is_err());
        assert!(OwnedXrgbPixels::copy_from(u32::MAX, 2, &[]).is_err());
    }

    #[test]
    fn writes_clipped_rgba_png() {
        let path = std::env::temp_dir().join(format!(
            "agenterm-platform-screenshot-{}.png",
            std::process::id()
        ));
        let pixels = [0x00FF00u32, 0x0000FFu32, 0xFF0000u32, 0xFFFFFFu32];
        let result = write_xrgb_png(
            XrgbFrame::new(&path, 2, 2, &pixels).with_clip(XrgbClip::new(1, 0, 1, 2)),
        )
        .expect("PNG");
        assert_eq!(result.output_width, 1);
        assert_eq!(result.output_height, 2);
        assert_eq!(result.output_pixels, 2);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let decoder = png::Decoder::new(std::io::BufReader::new(
                std::fs::File::open(&path).expect("PNG"),
            ));
            let mut reader = decoder.read_info().expect("header");
            let mut rgba = vec![0; reader.output_buffer_size().expect("bounded output")];
            let info = reader.next_frame(&mut rgba).expect("decode");
            assert_eq!(info.color_type, png::ColorType::Rgba);
            assert_eq!(
                &rgba[..info.buffer_size()],
                &[0, 0, 255, 255, 255, 255, 255, 255]
            );
        }
        let _ = std::fs::remove_file(path);
    }
}
