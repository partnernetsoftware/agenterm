//! Fragment-wise native-stream I/O under one absolute deadline.
//!
//! Every primitive recomputes the caller's remaining budget from a single
//! absolute `Instant` before it touches the stream, so a slow peer can never
//! multiply a nominal timeout by the number of fragments it dribbles. On Unix
//! the platform facade's nonblocking readiness waits (`poll`) enforce that
//! budget; on Windows each fragment carries the same remaining budget in the
//! named pipe's overlapped-I/O timeout.
//!
//! Callers keep their own typed error vocabulary. This module reports only the
//! classification — deadline, EOF, declared-size, or other native I/O — so one
//! deadline discipline serves the browser bridge and the managed-job client
//! without either inheriting the other's codes.

use std::{
    io::{self, Read, Write},
    time::{Duration, Instant},
};

use agenterm_platform::ipc::NativeStream;

/// Why one deadline-bounded primitive stopped.
#[derive(Debug)]
pub(crate) enum FrameIoError {
    /// The caller's absolute deadline expired.
    Deadline,
    /// The peer closed the stream before the expected bytes arrived.
    Eof,
    /// A declared frame length was empty or above the caller's ceiling.
    FrameSize,
    /// Any other native I/O failure.
    Io(io::Error),
}

pub(crate) type FrameIoResult<T> = Result<T, FrameIoError>;

/// Declared-size bounds for one length-prefixed frame.
#[derive(Clone, Copy)]
pub(crate) struct FrameLimits {
    pub max_bytes: usize,
    /// `true` for the little-endian Native Messaging prefix, `false` for the
    /// managed-job big-endian prefix.
    pub little_endian: bool,
}

/// Encodes the one length prefix both frame paths share.
pub(crate) fn frame_header(length: u32, little_endian: bool) -> [u8; 4] {
    if little_endian {
        length.to_le_bytes()
    } else {
        length.to_be_bytes()
    }
}

/// Narrows one body length into the wire's `u32` prefix without truncation.
///
/// The frame writers must not cast with `as u32`: a body longer than the
/// ceiling would be published with a wrapped length and read back as a
/// different frame. Callers check the policy ceiling and then this conversion.
pub(crate) fn frame_length_u32(length: usize) -> FrameIoResult<u32> {
    u32::try_from(length).map_err(|_| FrameIoError::FrameSize)
}

/// Decodes the one length prefix both frame paths share.
pub(crate) fn frame_length(header: [u8; 4], little_endian: bool) -> usize {
    let length = if little_endian {
        u32::from_le_bytes(header)
    } else {
        u32::from_be_bytes(header)
    };
    length as usize
}

/// Puts an already-connected stream into deadline-bounded I/O mode.
///
/// Unix gives up the blocking socket timeout in favour of readiness waits;
/// Windows keeps its overlapped-I/O timeout and needs no mode change.
pub(crate) fn prepare(stream: &NativeStream) -> io::Result<()> {
    #[cfg(unix)]
    {
        stream.set_nonblocking(true)
    }
    #[cfg(windows)]
    {
        let _ = stream;
        Ok(())
    }
}

pub(crate) fn remaining(deadline: Instant) -> FrameIoResult<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(FrameIoError::Deadline)
    } else {
        Ok(remaining)
    }
}

/// Re-arms the native per-operation timeout with the budget left.
///
/// Unix deliberately does nothing here: repeatedly resetting `SO_RCVTIMEO` /
/// `SO_SNDTIMEO` on an already-connected stream is not portable, and its
/// readiness waits own the deadline instead.
fn arm_remaining_timeout(stream: &mut NativeStream, deadline: Instant) -> FrameIoResult<()> {
    let timeout = remaining(deadline)?;
    #[cfg(unix)]
    {
        let _ = (stream, timeout);
        Ok(())
    }
    #[cfg(windows)]
    {
        stream
            .set_io_timeout(timeout)
            .map_err(|_| FrameIoError::Io(io::Error::other("deadline I/O timeout not accepted")))
    }
}

fn is_retryable(kind: io::ErrorKind) -> bool {
    matches!(kind, io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock)
}

pub(crate) fn wait_readable(stream: &mut NativeStream, deadline: Instant) -> FrameIoResult<()> {
    arm_remaining_timeout(stream, deadline)?;
    #[cfg(unix)]
    {
        if !stream
            .wait_readable(remaining(deadline)?)
            .map_err(FrameIoError::Io)?
        {
            return Err(FrameIoError::Deadline);
        }
    }
    Ok(())
}

pub(crate) fn wait_writable(stream: &mut NativeStream, deadline: Instant) -> FrameIoResult<()> {
    arm_remaining_timeout(stream, deadline)?;
    #[cfg(unix)]
    {
        if !stream
            .wait_writable(remaining(deadline)?)
            .map_err(FrameIoError::Io)?
        {
            return Err(FrameIoError::Deadline);
        }
    }
    Ok(())
}

/// Writes every byte, re-checking the same absolute deadline per fragment.
pub(crate) fn write_all_before(
    stream: &mut NativeStream,
    mut bytes: &[u8],
    deadline: Instant,
) -> FrameIoResult<()> {
    while !bytes.is_empty() {
        wait_writable(stream, deadline)?;
        match stream.write(bytes) {
            Ok(0) => return Err(FrameIoError::Eof),
            Ok(written) => bytes = &bytes[written..],
            Err(error) if is_retryable(error.kind()) => {}
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                return Err(FrameIoError::Deadline);
            }
            Err(error) => return Err(FrameIoError::Io(error)),
        }
    }
    Ok(())
}

pub(crate) fn flush_before(stream: &mut NativeStream, deadline: Instant) -> FrameIoResult<()> {
    // A retryable flush is not a completed flush: keep re-arming the same
    // absolute deadline and retry instead of reporting success, so the caller's
    // single budget still bounds the whole operation.
    loop {
        wait_writable(stream, deadline)?;
        match stream.flush() {
            Ok(()) => return Ok(()),
            Err(error) if is_retryable(error.kind()) => {}
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                return Err(FrameIoError::Deadline);
            }
            Err(error) => return Err(FrameIoError::Io(error)),
        }
    }
}

/// Fills `bytes` completely, re-checking the same absolute deadline per fragment.
pub(crate) fn read_exact_before(
    stream: &mut NativeStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> FrameIoResult<()> {
    while !bytes.is_empty() {
        wait_readable(stream, deadline)?;
        match stream.read(bytes) {
            Ok(0) => return Err(FrameIoError::Eof),
            Ok(read) => bytes = &mut bytes[read..],
            Err(error) if is_retryable(error.kind()) => {}
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                return Err(FrameIoError::Deadline);
            }
            Err(error) => return Err(FrameIoError::Io(error)),
        }
    }
    Ok(())
}

/// Reads one length-prefixed frame body under the same absolute deadline.
pub(crate) fn read_frame_before(
    stream: &mut NativeStream,
    deadline: Instant,
    limits: FrameLimits,
) -> FrameIoResult<Vec<u8>> {
    let mut header = [0_u8; 4];
    read_exact_before(stream, &mut header, deadline)?;
    let length = frame_length(header, limits.little_endian);
    if length == 0 || length > limits.max_bytes {
        return Err(FrameIoError::FrameSize);
    }
    let mut body = vec![0_u8; length];
    read_exact_before(stream, &mut body, deadline)?;
    Ok(body)
}

/// Writes one length-prefixed frame under the same absolute deadline.
///
/// The prefix and the body are separate fragments, so each one recomputes the
/// remaining budget from the caller's single absolute deadline. Flushing stays
/// the caller's explicit step, matching the legacy frame writer.
pub(crate) fn write_frame_before(
    stream: &mut NativeStream,
    bytes: &[u8],
    deadline: Instant,
    limits: FrameLimits,
) -> FrameIoResult<()> {
    if bytes.is_empty() || bytes.len() > limits.max_bytes {
        return Err(FrameIoError::FrameSize);
    }
    let header = frame_header(frame_length_u32(bytes.len())?, limits.little_endian);
    write_all_before(stream, &header, deadline)?;
    write_all_before(stream, bytes, deadline)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exhausted_deadline_reports_deadline_not_io() {
        let past = Instant::now() - Duration::from_millis(1);
        assert!(matches!(remaining(past), Err(FrameIoError::Deadline)));
        let ahead = Instant::now() + Duration::from_secs(1);
        assert!(remaining(ahead).is_ok());
    }

    #[test]
    fn retryable_kinds_are_interrupted_and_would_block_only() {
        assert!(is_retryable(io::ErrorKind::Interrupted));
        assert!(is_retryable(io::ErrorKind::WouldBlock));
        assert!(!is_retryable(io::ErrorKind::TimedOut));
        assert!(!is_retryable(io::ErrorKind::ConnectionReset));
    }

    /// Both carriage forms round-trip, and the big-endian form is the one the
    /// managed-job wire has always used.
    #[test]
    fn frame_prefixes_round_trip_in_both_byte_orders() {
        assert_eq!(frame_header(0x0102_0304, false), [1, 2, 3, 4]);
        assert_eq!(frame_header(0x0102_0304, true), [4, 3, 2, 1]);
        assert_eq!(frame_length([1, 2, 3, 4], false), 0x0102_0304);
        assert_eq!(frame_length([4, 3, 2, 1], true), 0x0102_0304);
        assert_eq!(frame_length(frame_header(7, false), false), 7);
        assert_eq!(frame_length(frame_header(7, true), true), 7);
    }

    /// The wire prefix is `u32`: a body past that ceiling must be refused rather
    /// than published with a wrapped length.
    #[test]
    fn frame_length_narrowing_is_checked() {
        assert!(matches!(frame_length_u32(u32::MAX as usize), Ok(u32::MAX)));
        assert!(matches!(
            frame_length_u32(u32::MAX as usize + 1),
            Err(FrameIoError::FrameSize)
        ));
    }
}
