//! Bounded Win32 Unicode clipboard capability.
//! Adapter-private native mechanism selected only by platform::selected.

#![cfg(target_os = "windows")]

use std::{
    fmt, mem, ptr, thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{GlobalFree, HWND},
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
            GetClipboardFormatNameW, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
        },
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
    },
};

const UNICODE_TEXT: u32 = 13;
const RETRY_INTERVAL: Duration = Duration::from_millis(10);
/// How long the type probe waits for another application to release the
/// clipboard before giving up.
const OPEN_TIMEOUT: Duration = Duration::from_millis(500);
/// Most type names one probe reports.
const MAX_CLIPBOARD_TYPES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClipboardError {
    Busy,
    Unavailable,
    TooLarge { limit: usize },
    InvalidUtf16,
    Backend(&'static str),
}

impl fmt::Display for ClipboardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => write!(
                formatter,
                "could not open the Windows clipboard within 500 ms"
            ),
            Self::Unavailable => write!(formatter, "the clipboard does not contain Unicode text"),
            Self::TooLarge { limit } => {
                write!(formatter, "clipboard text exceeds the {limit}-byte limit")
            }
            Self::InvalidUtf16 => write!(formatter, "clipboard text is not valid UTF-16"),
            Self::Backend(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ClipboardError {}

struct OpenClipboardGuard;

impl Drop for OpenClipboardGuard {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
    }
}

fn open(owner: HWND, timeout: Duration) -> Result<OpenClipboardGuard, ClipboardError> {
    let deadline = Instant::now() + timeout;
    loop {
        if unsafe { OpenClipboard(owner) } != 0 {
            return Ok(OpenClipboardGuard);
        }
        if Instant::now() >= deadline {
            return Err(ClipboardError::Busy);
        }
        thread::sleep(RETRY_INTERVAL);
    }
}

pub(crate) fn has_unicode_text() -> bool {
    unsafe { IsClipboardFormatAvailable(UNICODE_TEXT) != 0 }
}

/// Every format currently on the clipboard, in the order Windows offers
/// them -- which is most-preferred first, so a caller can read the list as
/// a preference ranking.
///
/// Registered formats have a name (`GetClipboardFormatNameW`); the
/// standard ones do not, and get the constant's own spelling rather than a
/// bare number, because `CF_BITMAP` says something and `2` does not.
pub(crate) fn available_types() -> Result<Vec<String>, ClipboardError> {
    let _guard = open(std::ptr::null_mut(), OPEN_TIMEOUT)?;
    let mut names = Vec::new();
    let mut format = 0u32;
    loop {
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            break;
        }
        if names.len() >= MAX_CLIPBOARD_TYPES {
            break;
        }
        names.push(format_name(format));
    }
    Ok(names)
}

fn format_name(format: u32) -> String {
    if let Some(standard) = standard_format_name(format) {
        return standard.to_owned();
    }
    let mut buffer = [0u16; 256];
    let written =
        unsafe { GetClipboardFormatNameW(format, buffer.as_mut_ptr(), buffer.len() as i32) };
    if written > 0 {
        return String::from_utf16_lossy(&buffer[..written as usize]);
    }
    format!("CF_{format}")
}

/// The predefined formats worth naming. Anything else registered by an
/// application answers `GetClipboardFormatNameW`.
fn standard_format_name(format: u32) -> Option<&'static str> {
    Some(match format {
        1 => "CF_TEXT",
        2 => "CF_BITMAP",
        3 => "CF_METAFILEPICT",
        6 => "CF_TIFF",
        7 => "CF_OEMTEXT",
        8 => "CF_DIB",
        13 => "CF_UNICODETEXT",
        14 => "CF_ENHMETAFILE",
        15 => "CF_HDROP",
        17 => "CF_DIBV5",
        _ => return None,
    })
}

fn set_text_with_owner(owner: HWND, text: &str, timeout: Duration) -> Result<(), ClipboardError> {
    let _guard = open(owner, timeout)?;
    if unsafe { EmptyClipboard() } == 0 {
        return Err(ClipboardError::Backend(
            "could not clear the Windows clipboard",
        ));
    }

    let encoded_units = text
        .encode_utf16()
        .count()
        .checked_add(1)
        .ok_or(ClipboardError::Backend("clipboard text is too large"))?;
    let allocation_bytes = encoded_units
        .checked_mul(mem::size_of::<u16>())
        .ok_or(ClipboardError::Backend("clipboard text is too large"))?;
    let allocation = unsafe { GlobalAlloc(GMEM_MOVEABLE, allocation_bytes) };
    if allocation.is_null() {
        return Err(ClipboardError::Backend("could not allocate clipboard text"));
    }
    let destination = unsafe { GlobalLock(allocation) } as *mut u16;
    if destination.is_null() {
        unsafe { GlobalFree(allocation) };
        return Err(ClipboardError::Backend("could not lock clipboard text"));
    }
    unsafe {
        // SAFETY: GlobalAlloc reserved exactly encoded_units writable u16s;
        // encode_utf16 emits encoded_units - 1 values and the final write owns
        // the required clipboard NUL terminator.
        for (index, unit) in text.encode_utf16().enumerate() {
            destination.add(index).write(unit);
        }
        destination.add(encoded_units - 1).write(0);
        GlobalUnlock(allocation);
    }
    if unsafe { SetClipboardData(UNICODE_TEXT, allocation) }.is_null() {
        unsafe { GlobalFree(allocation) };
        return Err(ClipboardError::Backend("could not publish clipboard text"));
    }
    Ok(())
}

pub(crate) fn set_text(text: &str, timeout: Duration) -> Result<(), ClipboardError> {
    set_text_with_owner(ptr::null_mut(), text, timeout)
}

pub(crate) fn get_text(max_utf8_bytes: usize, timeout: Duration) -> Result<String, ClipboardError> {
    let _guard = open(ptr::null_mut(), timeout)?;
    if !has_unicode_text() {
        return Err(ClipboardError::Unavailable);
    }
    let allocation = unsafe { GetClipboardData(UNICODE_TEXT) };
    if allocation.is_null() {
        return Err(ClipboardError::Backend(
            "could not read Unicode clipboard data",
        ));
    }
    let allocation_size = unsafe { GlobalSize(allocation) };
    if allocation_size == 0 {
        return Err(ClipboardError::Backend(
            "Unicode clipboard data has no readable allocation",
        ));
    }
    let maximum_utf16_allocation = max_utf8_bytes
        .saturating_add(1)
        .saturating_mul(mem::size_of::<u16>());
    if allocation_size > maximum_utf16_allocation {
        return Err(ClipboardError::TooLarge {
            limit: max_utf8_bytes,
        });
    }

    let source = unsafe { GlobalLock(allocation) } as *const u16;
    if source.is_null() {
        return Err(ClipboardError::Backend(
            "could not lock Unicode clipboard data",
        ));
    }
    let units =
        unsafe { std::slice::from_raw_parts(source, allocation_size / mem::size_of::<u16>()) };
    let length = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    let decoded = String::from_utf16(&units[..length]).map_err(|_| ClipboardError::InvalidUtf16);
    unsafe { GlobalUnlock(allocation) };
    let decoded = decoded?;
    if decoded.len() > max_utf8_bytes {
        return Err(ClipboardError::TooLarge {
            limit: max_utf8_bytes,
        });
    }
    Ok(decoded)
}

pub(crate) fn get_type(
    type_name: &str,
    max_bytes: usize,
    timeout: Duration,
) -> Result<Vec<u8>, ClipboardError> {
    let format = format_from_name(type_name)?;
    if matches!(format, 2 | 3 | 14) {
        return Err(ClipboardError::Backend(
            "CF_BITMAP/CF_METAFILEPICT/CF_ENHMETAFILE are GDI handles, not byte payloads; use CF_DIB or a registered PNG format",
        ));
    }
    let _guard = open(ptr::null_mut(), timeout)?;
    if unsafe { IsClipboardFormatAvailable(format) } == 0 {
        return Err(ClipboardError::Backend(
            "the clipboard does not carry that format",
        ));
    }
    let allocation = unsafe { GetClipboardData(format) };
    if allocation.is_null() {
        return Err(ClipboardError::Backend(
            "could not read clipboard format data",
        ));
    }
    let allocation_size = unsafe { GlobalSize(allocation) };
    if allocation_size == 0 {
        return Err(ClipboardError::Backend(
            "clipboard format data has no readable allocation",
        ));
    }
    if allocation_size > max_bytes {
        return Err(ClipboardError::TooLarge { limit: max_bytes });
    }
    let source = unsafe { GlobalLock(allocation) } as *const u8;
    if source.is_null() {
        return Err(ClipboardError::Backend(
            "could not lock clipboard format data",
        ));
    }
    let bytes = unsafe { std::slice::from_raw_parts(source, allocation_size) }.to_vec();
    unsafe { GlobalUnlock(allocation) };
    Ok(bytes)
}

pub(crate) fn set_type(
    type_name: &str,
    bytes: &[u8],
    timeout: Duration,
) -> Result<(), ClipboardError> {
    let format = format_from_name(type_name)?;
    if matches!(format, 2 | 3 | 14) {
        return Err(ClipboardError::Backend(
            "CF_BITMAP/CF_METAFILEPICT/CF_ENHMETAFILE are GDI handles, not byte payloads",
        ));
    }
    if bytes.len() > crate::contract::clipboard::MAX_CLIPBOARD_TYPE_BYTES {
        return Err(ClipboardError::TooLarge {
            limit: crate::contract::clipboard::MAX_CLIPBOARD_TYPE_BYTES,
        });
    }
    let _guard = open(ptr::null_mut(), timeout)?;
    if unsafe { EmptyClipboard() } == 0 {
        return Err(ClipboardError::Backend(
            "could not clear the Windows clipboard",
        ));
    }
    let allocation = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) };
    if allocation.is_null() {
        return Err(ClipboardError::Backend(
            "could not allocate clipboard format data",
        ));
    }
    let destination = unsafe { GlobalLock(allocation) } as *mut u8;
    if destination.is_null() {
        unsafe { GlobalFree(allocation) };
        return Err(ClipboardError::Backend(
            "could not lock clipboard format data",
        ));
    }
    unsafe {
        if !bytes.is_empty() {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len());
        }
        GlobalUnlock(allocation);
    }
    if unsafe { SetClipboardData(format, allocation) }.is_null() {
        unsafe { GlobalFree(allocation) };
        return Err(ClipboardError::Backend(
            "could not publish clipboard format data",
        ));
    }
    Ok(())
}

pub(crate) fn set_file(path: &str, timeout: Duration) -> Result<(), ClipboardError> {
    if !std::path::Path::new(path).exists() {
        return Err(ClipboardError::Backend("clipboard file does not exist"));
    }
    let absolute = std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_owned());
    let wide: Vec<u16> = absolute
        .encode_utf16()
        .chain(std::iter::once(0))
        .chain(std::iter::once(0))
        .collect();
    let header = 20usize;
    let total = header + wide.len() * 2;
    let mut payload = vec![0u8; total];
    payload[0..4].copy_from_slice(&(header as u32).to_le_bytes());
    payload[16..20].copy_from_slice(&1u32.to_le_bytes()); // fWide
    let dest = &mut payload[header..];
    for (i, unit) in wide.iter().enumerate() {
        dest[i * 2..i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
    }
    set_type("CF_HDROP", &payload, timeout)
}

pub(crate) fn clear(timeout: Duration) -> Result<(), ClipboardError> {
    let _guard = open(ptr::null_mut(), timeout)?;
    if unsafe { EmptyClipboard() } == 0 {
        return Err(ClipboardError::Backend(
            "could not clear the Windows clipboard",
        ));
    }
    Ok(())
}

fn format_from_name(name: &str) -> Result<u32, ClipboardError> {
    if let Some(standard) = standard_format_id(name) {
        return Ok(standard);
    }
    let mut wide: Vec<u16> = name.encode_utf16().collect();
    wide.push(0);
    let format = unsafe {
        windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW(wide.as_ptr())
    };
    if format == 0 {
        return Err(ClipboardError::Backend(
            "could not resolve clipboard format name",
        ));
    }
    Ok(format)
}

fn standard_format_id(name: &str) -> Option<u32> {
    Some(match name {
        "CF_TEXT" => 1,
        "CF_BITMAP" => 2,
        "CF_METAFILEPICT" => 3,
        "CF_TIFF" => 6,
        "CF_OEMTEXT" => 7,
        "CF_DIB" => 8,
        "CF_UNICODETEXT" => 13,
        "CF_ENHMETAFILE" => 14,
        "CF_HDROP" => 15,
        "CF_DIBV5" => 17,
        _ => return None,
    })
}

pub(crate) fn map_error(error: ClipboardError) -> crate::contract::clipboard::ClipboardError {
    match &error {
        ClipboardError::Unavailable => {
            crate::contract::clipboard::ClipboardError::unsupported("clipboard-unavailable")
        }
        ClipboardError::Busy => {
            crate::contract::clipboard::ClipboardError::failed("clipboard_busy", error.to_string())
        }
        ClipboardError::TooLarge { .. } => crate::contract::clipboard::ClipboardError::failed(
            "clipboard_too_large",
            error.to_string(),
        ),
        ClipboardError::InvalidUtf16 => crate::contract::clipboard::ClipboardError::failed(
            "clipboard_invalid_utf16",
            error.to_string(),
        ),
        ClipboardError::Backend(_) => crate::contract::clipboard::ClipboardError::failed(
            "clipboard_backend_error",
            error.to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_errors_are_stable_and_typed() {
        assert_eq!(
            ClipboardError::TooLarge { limit: 128 }.to_string(),
            "clipboard text exceeds the 128-byte limit"
        );
        assert_eq!(
            ClipboardError::InvalidUtf16.to_string(),
            "clipboard text is not valid UTF-16"
        );
        assert!(
            map_error(ClipboardError::Busy)
                .to_string()
                .contains("500 ms")
        );
    }

    #[test]
    fn allocation_bound_covers_the_largest_valid_utf16_input() {
        let limit = 64_usize;
        let maximum = limit
            .saturating_add(1)
            .saturating_mul(mem::size_of::<u16>());
        assert_eq!(maximum, 130);
    }
}
