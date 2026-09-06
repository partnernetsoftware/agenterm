//! Portable PNG encoding shared by Linux and macOS adapters.

use std::{
    fs::File,
    io::{self, BufWriter, Write},
};

use png::{BitDepth, ColorType, Encoder};

use crate::{
    contract::ui_screenshot::{ScreenshotWriteResult, UiScreenshotError, XrgbFrame},
    screenshot::checked_frame,
};

pub(crate) fn write_xrgb_png(
    frame: XrgbFrame<'_>,
) -> Result<ScreenshotWriteResult, UiScreenshotError> {
    checked_frame(&frame)?;
    let file = File::create(frame.path())
        .map_err(|error| UiScreenshotError::failed("screenshot_io_error", error.to_string()))?;
    encode_xrgb_png(frame, BufWriter::new(file))
}

fn encode_xrgb_png<W: Write>(
    frame: XrgbFrame<'_>,
    output: W,
) -> Result<ScreenshotWriteResult, UiScreenshotError> {
    let (x, y, output_width, output_height, output_pixels) = checked_frame(&frame)?;
    let mut first_error = None;
    let result = (|| {
        // png can flush its final IDAT chunk from Drop. Remember errors even when
        // the library discards them, then check after every encoder owner drops.
        let output = StickyWriter {
            inner: output,
            first_error: &mut first_error,
        };
        let mut encoder = Encoder::new(output, output_width, output_height);
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(encode_error)?;
        // The encoder buffers filter/compression state itself. Convert one row
        // at a time so a screenshot never creates another whole-frame RGBA Vec.
        let mut rgba = vec![0u8; output_width as usize * 4];
        {
            let mut stream = writer.stream_writer().map_err(encode_error)?;
            for row in y..y + output_height {
                let row_start = row as usize * frame.width() as usize + x as usize;
                for (column, output) in rgba.chunks_exact_mut(4).enumerate() {
                    let pixel = frame.pixels()[row_start + column];
                    output.copy_from_slice(&[
                        ((pixel >> 16) & 0xFF) as u8,
                        ((pixel >> 8) & 0xFF) as u8,
                        (pixel & 0xFF) as u8,
                        255,
                    ]);
                }
                stream.write_all(&rgba).map_err(encode_error)?;
            }
            stream.finish().map_err(encode_error)?;
        }
        writer.finish().map_err(encode_error)?;

        Ok(ScreenshotWriteResult {
            frame_width: frame.width(),
            frame_height: frame.height(),
            output_width,
            output_height,
            output_pixels,
        })
    })();
    match first_error {
        Some(error) => Err(encode_error(error)),
        None => result,
    }
}

struct StickyWriter<'a, W> {
    inner: W,
    first_error: &'a mut Option<io::Error>,
}

fn copy_io_error(error: &io::Error) -> io::Error {
    match error.raw_os_error() {
        Some(code) => io::Error::from_raw_os_error(code),
        None => io::Error::new(error.kind(), error.to_string()),
    }
}

impl<W: Write> StickyWriter<'_, W> {
    fn remember<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        if let Err(error) = &result
            && error.kind() != io::ErrorKind::Interrupted
        {
            *self.first_error = Some(copy_io_error(error));
        }
        result
    }
}

impl<W: Write> Write for StickyWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(error) = self.first_error.as_ref() {
            return Err(copy_io_error(error));
        }
        let result = match self.inner.write(bytes) {
            Ok(0) if !bytes.is_empty() => Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "screenshot output accepted no bytes",
            )),
            result => result,
        };
        self.remember(result)
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(error) = self.first_error.as_ref() {
            return Err(copy_io_error(error));
        }
        let result = self.inner.flush();
        self.remember(result)
    }
}

fn encode_error(error: impl std::fmt::Display) -> UiScreenshotError {
    UiScreenshotError::failed("screenshot_encode_error", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, path::Path, rc::Rc};

    // One-shot faults: later writes would succeed without sticky propagation.
    struct FaultWriter {
        written: usize,
        fail_at: Option<usize>,
        fail_flush: bool,
        triggered: Rc<Cell<bool>>,
    }

    impl Write for FaultWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.is_empty() {
                return Ok(0);
            }
            if !self.triggered.get()
                && let Some(limit) = self.fail_at
            {
                if self.written == limit {
                    self.triggered.set(true);
                    return Err(io::Error::other("injected final IDAT write failure"));
                }
                let accepted = bytes.len().min(limit - self.written);
                self.written += accepted;
                return Ok(accepted);
            }
            self.written += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.fail_flush && !self.triggered.replace(true) {
                return Err(io::Error::other("injected final output flush failure"));
            }
            Ok(())
        }
    }

    fn frame() -> XrgbFrame<'static> {
        XrgbFrame::new(
            Path::new("unused.png"),
            2,
            2,
            &[0x00ff0000, 0x0000ff00, 0x000000ff, 0x00ffffff],
        )
    }

    #[test]
    fn final_idat_write_failure_is_reported() {
        let mut encoded = Vec::new();
        encode_xrgb_png(frame(), &mut encoded).expect("reference PNG");
        let mut offset = 8;
        let mut final_idat_crc = None;
        while offset < encoded.len() {
            let length =
                u32::from_be_bytes(encoded[offset..offset + 4].try_into().unwrap()) as usize;
            if &encoded[offset + 4..offset + 8] == b"IDAT" {
                final_idat_crc = Some(offset + 8 + length);
            }
            offset += 12 + length;
        }
        let fail_at = final_idat_crc.expect("final IDAT chunk");
        // A raw writer exposes png's final chunk Drop; BufWriter also checks
        // the real file-writer shape used by the public screenshot entrypoint.
        for buffered in [false, true] {
            let triggered = Rc::new(Cell::new(false));
            let sink = FaultWriter {
                written: 0,
                fail_at: Some(fail_at),
                fail_flush: false,
                triggered: triggered.clone(),
            };
            let result = if buffered {
                encode_xrgb_png(frame(), BufWriter::new(sink))
            } else {
                encode_xrgb_png(frame(), sink)
            };
            assert!(triggered.get(), "tail fault must actually fire");
            assert_eq!(
                result.expect_err("tail failure must escape Drop").code(),
                "screenshot_encode_error"
            );
        }
    }

    #[test]
    fn final_output_flush_failure_is_reported() {
        let triggered = Rc::new(Cell::new(false));
        let sink = FaultWriter {
            written: 0,
            fail_at: None,
            fail_flush: true,
            triggered: triggered.clone(),
        };
        let error =
            encode_xrgb_png(frame(), BufWriter::new(sink)).expect_err("final flush must fail");
        assert!(triggered.get());
        assert_eq!(error.code(), "screenshot_encode_error");
    }
    struct TransientWriter {
        zero: bool,
        calls: usize,
    }

    impl Write for TransientWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.calls == 1 {
                if self.zero {
                    return Ok(0);
                }
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn interrupted_write_remains_retryable() {
        let mut error = None;
        let mut writer = StickyWriter {
            inner: TransientWriter {
                zero: false,
                calls: 0,
            },
            first_error: &mut error,
        };
        assert_eq!(
            writer.write(&[1]).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        // Assert before write_all: a regression caching Interrupted must fail
        // boundedly, rather than letting write_all retry that error forever.
        assert!(writer.first_error.is_none());
        writer.write_all(&[1, 2]).expect("retry succeeds");
        assert_eq!(writer.inner.calls, 2);
        assert!(writer.first_error.is_none());
    }

    #[test]
    fn zero_length_progress_is_a_sticky_write_error() {
        let mut error = None;
        let mut writer = StickyWriter {
            inner: TransientWriter {
                zero: true,
                calls: 0,
            },
            first_error: &mut error,
        };
        assert_eq!(
            writer.write_all(&[1]).unwrap_err().kind(),
            io::ErrorKind::WriteZero
        );
        assert_eq!(
            writer.first_error.as_ref().unwrap().kind(),
            io::ErrorKind::WriteZero
        );
        assert_eq!(writer.flush().unwrap_err().kind(), io::ErrorKind::WriteZero);
        assert_eq!(writer.inner.calls, 1);
    }
}
