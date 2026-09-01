//! Bounded macOS Unicode clipboard capability.
//! Adapter-private native mechanism selected only by platform::selected.

#![cfg(target_os = "macos")]

use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Adapter ceiling for a caller-supplied clipboard deadline.
pub(crate) const HELPER_TIMEOUT: Duration = Duration::from_millis(1_500);

/// Bound for the type-name probe (a list of names, never a paste payload).
const TYPE_LIST_LIMIT_BYTES: usize = 16 * 1024;
/// Most type names one probe reports.
const MAX_CLIPBOARD_TYPES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClipboardError {
    Unavailable { message: String },
    TooLarge { limit: usize },
    Timeout { timeout: Duration },
    Backend { message: String },
}

impl ClipboardError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Unavailable { message } | Self::Backend { message } => message.clone(),
            Self::TooLarge { limit } => {
                format!("clipboard text exceeds the {limit} byte terminal paste limit")
            }
            Self::Timeout { timeout } => {
                format!(
                    "clipboard helper exceeded the {} ms deadline",
                    timeout.as_millis()
                )
            }
        }
    }
}

pub(crate) fn set_text(text: &str, timeout: std::time::Duration) -> Result<(), ClipboardError> {
    write_via_command("pbcopy", text, bounded_helper_timeout(timeout)?)
}

pub(crate) fn get_text(
    max_read_bytes: usize,
    timeout: std::time::Duration,
) -> Result<String, ClipboardError> {
    read_via_command("pbpaste", max_read_bytes, bounded_helper_timeout(timeout)?)
}

fn bounded_helper_timeout(timeout: Duration) -> Result<Duration, ClipboardError> {
    if timeout.is_zero() {
        return Err(ClipboardError::Timeout { timeout });
    }
    Ok(timeout.min(HELPER_TIMEOUT))
}

/// The clipboard's type names, from AppleScript's `clipboard info`.
///
/// That command answers a list of `«class utf8», 211` pairs -- the UTI-ish
/// class name and the byte count -- which is the one route to the type
/// list that does not require linking AppKit into this adapter. The class
/// names are passed through as the system spelled them: a caller matching
/// on `«class PNGf»` is matching on what macOS actually said, not on a
/// vocabulary invented here.
/// AppleScript `as` clause for a clipboard-info class name. Other spellings
/// (UTI, NSPasteboard names MCU uses) are refused rather than interpolated.
fn macos_as_clause_owned(type_name: &str) -> Result<String, ClipboardError> {
    let t = type_name.trim();
    if t.eq_ignore_ascii_case("string") {
        return Ok("string".into());
    }
    if t.eq_ignore_ascii_case("unicode text") {
        return Ok("Unicode text".into());
    }
    let inner = t
        .strip_prefix("«class ")
        .and_then(|rest| rest.strip_suffix('»'));
    if let Some(inner) = inner
        && !inner.is_empty()
        && inner.len() <= 32
        && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ')
    {
        return Ok(format!("«class {inner}»"));
    }
    Err(ClipboardError::Backend {
        message: format!(
            "clipboard type {t:?} is not an AppleScript clipboard-info class (string, Unicode text, or «class XXXX»)"
        ),
    })
}

pub(crate) fn get_type(
    type_name: &str,
    max_bytes: usize,
    timeout: Duration,
) -> Result<Vec<u8>, ClipboardError> {
    let as_clause = macos_as_clause_owned(type_name)?;
    if max_bytes == 0 {
        return Err(ClipboardError::TooLarge { limit: 0 });
    }
    let path = std::env::temp_dir().join(format!(
        "agenterm-clip-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let path_str = path.to_str().ok_or_else(|| ClipboardError::Backend {
        message: "clipboard temp path is not UTF-8".into(),
    })?;
    if path_str.contains('"') || path_str.contains('\\') {
        return Err(ClipboardError::Backend {
            message: "clipboard temp path is not AppleScript-safe".into(),
        });
    }
    let script = format!(
        "set outFile to POSIX file \"{path_str}\"\n\
         set f to open for access outFile with write permission\n\
         set eof of f to 0\n\
         write (the clipboard as {as_clause}) to f\n\
         close access f\n"
    );
    let run = write_via_command_script("osascript", &script, timeout);
    let bytes = std::fs::read(&path);
    let _ = std::fs::remove_file(&path);
    run?;
    let bytes = bytes.map_err(|error| ClipboardError::Backend {
        message: format!("clipboard type file: {error}"),
    })?;
    if bytes.len() > max_bytes {
        return Err(ClipboardError::TooLarge { limit: max_bytes });
    }
    Ok(bytes)
}

fn write_via_command_script(
    program: &str,
    script: &str,
    timeout: Duration,
) -> Result<(), ClipboardError> {
    let mut child = Command::new(program)
        .arg("-e")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| unavailable_or_backend(program, error))?;
    wait_child(&mut child, timeout, timeout)
}

pub(crate) fn available_types() -> Result<Vec<String>, ClipboardError> {
    let listing = read_via_command_with_args(
        "osascript",
        &["-e", "clipboard info"],
        TYPE_LIST_LIMIT_BYTES,
        HELPER_TIMEOUT,
    )?;
    Ok(parse_clipboard_info(&listing))
}

/// `«class utf8», 211, string, 6` -> ["«class utf8»", "string"].
///
/// The list alternates name and byte count, so the counts are dropped by
/// position rather than by trying to tell a numeric name from a number.
fn parse_clipboard_info(listing: &str) -> Vec<String> {
    listing
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .enumerate()
        .filter(|(index, _)| index % 2 == 0)
        .map(|(_, name)| name.to_owned())
        .take(MAX_CLIPBOARD_TYPES)
        .collect()
}

pub(crate) fn has_unicode_text() -> bool {
    match read_via_command("pbpaste", 1, HELPER_TIMEOUT) {
        Ok(text) => !text.is_empty(),
        Err(ClipboardError::TooLarge { .. }) => true,
        Err(_) => false,
    }
}

pub(crate) fn map_error(error: ClipboardError) -> crate::contract::clipboard::ClipboardError {
    match &error {
        ClipboardError::Unavailable { .. } => {
            crate::contract::clipboard::ClipboardError::unsupported("clipboard-unavailable")
        }
        ClipboardError::TooLarge { .. } => crate::contract::clipboard::ClipboardError::failed(
            "clipboard_too_large",
            error.message(),
        ),
        ClipboardError::Timeout { .. } => {
            crate::contract::clipboard::ClipboardError::failed("clipboard_timeout", error.message())
        }
        ClipboardError::Backend { .. } => crate::contract::clipboard::ClipboardError::failed(
            "clipboard_backend_error",
            error.message(),
        ),
    }
}

fn write_via_command(program: &str, text: &str, timeout: Duration) -> Result<(), ClipboardError> {
    let mut child = Command::new(program)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| unavailable_or_backend(program, error))?;
    let mut stdin = child.stdin.take().ok_or_else(|| ClipboardError::Backend {
        message: "clipboard helper stdin is unavailable".to_owned(),
    })?;
    let text = text.as_bytes().to_vec();
    let writer = thread::spawn(move || {
        let result = stdin
            .write_all(&text)
            .map_err(|error| ClipboardError::Backend {
                message: error.to_string(),
            });
        drop(stdin);
        result
    });
    let deadline = Instant::now() + timeout;
    while !writer.is_finished() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = writer.join();
            return Err(ClipboardError::Timeout { timeout });
        }
        thread::sleep(Duration::from_millis(5));
    }
    match writer.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ClipboardError::Backend {
                message: "clipboard writer thread panicked".to_owned(),
            });
        }
    }
    wait_child(
        &mut child,
        deadline.saturating_duration_since(Instant::now()),
        timeout,
    )
}

fn read_via_command(
    program: &str,
    limit: usize,
    timeout: Duration,
) -> Result<String, ClipboardError> {
    read_via_command_with_args(program, &[], limit, timeout)
}

fn read_via_command_with_args(
    program: &str,
    args: &[&str],
    limit: usize,
    timeout: Duration,
) -> Result<String, ClipboardError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| unavailable_or_backend(program, error))?;
    let mut stdout = child.stdout.take().ok_or_else(|| ClipboardError::Backend {
        message: "clipboard helper stdout is unavailable".to_owned(),
    })?;
    let reader = thread::spawn(move || read_stdout_bounded(&mut stdout, limit));
    let deadline = Instant::now() + timeout;
    while !reader.is_finished() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(ClipboardError::Timeout { timeout });
        }
        thread::sleep(Duration::from_millis(5));
    }
    let bytes = match reader.join() {
        Ok(result) => result,
        Err(_) => Err(ClipboardError::Backend {
            message: "clipboard reader thread panicked".to_owned(),
        }),
    };
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    wait_child(
        &mut child,
        deadline.saturating_duration_since(Instant::now()),
        timeout,
    )?;
    String::from_utf8(bytes).map_err(|error| ClipboardError::Backend {
        message: error.to_string(),
    })
}

fn read_stdout_bounded(stdout: &mut impl Read, limit: usize) -> Result<Vec<u8>, ClipboardError> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8_192];
    loop {
        let count = stdout
            .read(&mut chunk)
            .map_err(|error| ClipboardError::Backend {
                message: error.to_string(),
            })?;
        if count == 0 {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(count) > limit {
            return Err(ClipboardError::TooLarge { limit });
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}

fn wait_child(
    child: &mut Child,
    remaining: Duration,
    operation_timeout: Duration,
) -> Result<(), ClipboardError> {
    let deadline = Instant::now() + remaining;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(ClipboardError::Backend {
                    message: format!("clipboard helper exited with {status}"),
                });
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ClipboardError::Timeout {
                    timeout: operation_timeout,
                });
            }
            Err(error) => {
                return Err(ClipboardError::Backend {
                    message: error.to_string(),
                });
            }
        }
    }
}

fn unavailable_or_backend(program: &str, error: std::io::Error) -> ClipboardError {
    if error.kind() == std::io::ErrorKind::NotFound {
        ClipboardError::Unavailable {
            message: format!("{program} is unavailable"),
        }
    } else {
        ClipboardError::Backend {
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn failures_have_stable_typed_codes() {
        assert_eq!(
            map_error(ClipboardError::TooLarge { limit: 32 }).to_capability_status(),
            crate::CapabilityStatus::Failed {
                code: "clipboard_too_large".into(),
                message: "clipboard text exceeds the 32 byte terminal paste limit".to_owned(),
            }
        );
        assert_eq!(
            map_error(ClipboardError::Timeout {
                timeout: HELPER_TIMEOUT,
            })
            .to_capability_status(),
            crate::CapabilityStatus::Failed {
                code: "clipboard_timeout".into(),
                message: "clipboard helper exceeded the 1500 ms deadline".to_owned(),
            }
        );
    }

    #[test]
    fn caller_timeout_is_preserved_and_capped_for_helpers() {
        assert_eq!(
            bounded_helper_timeout(Duration::from_millis(75)),
            Ok(Duration::from_millis(75))
        );
        assert_eq!(
            bounded_helper_timeout(Duration::from_secs(30)),
            Ok(HELPER_TIMEOUT)
        );
        assert_eq!(
            bounded_helper_timeout(Duration::ZERO),
            Err(ClipboardError::Timeout {
                timeout: Duration::ZERO
            })
        );
    }

    #[test]
    fn bounded_reader_stops_before_retaining_excess_bytes() {
        let mut input = &b"12345"[..];
        assert_eq!(
            read_stdout_bounded(&mut input, 4),
            Err(ClipboardError::TooLarge { limit: 4 })
        );
    }

    #[test]
    fn bounded_reader_reports_the_caller_supplied_limit() {
        let mut input = &b"12345"[..];
        assert_eq!(
            read_stdout_bounded(&mut input, 4),
            Err(ClipboardError::TooLarge { limit: 4 })
        );
    }

    #[test]
    fn blocked_writer_is_terminated_by_the_helper_deadline() {
        let path = std::env::temp_dir().join(format!(
            "agenterm-macos-blocked-clipboard-{}.sh",
            std::process::id()
        ));
        std::fs::write(&path, "#!/bin/sh\nexec sleep 5\n").expect("write helper");
        let mut permissions = std::fs::metadata(&path)
            .expect("helper metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).expect("make helper executable");

        let text = "x".repeat(2 * 1024 * 1024);
        let started = Instant::now();
        let result = write_via_command(
            path.to_str().expect("UTF-8 helper path"),
            &text,
            Duration::from_millis(50),
        );
        let _ = std::fs::remove_file(path);

        assert_eq!(
            result,
            Err(ClipboardError::Timeout {
                timeout: Duration::from_millis(50)
            })
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn as_clause_accepts_clipboard_info_classes_only() {
        assert_eq!(macos_as_clause_owned("string").unwrap(), "string");
        assert_eq!(
            macos_as_clause_owned("«class PNGf»").unwrap(),
            "«class PNGf»"
        );
        assert!(macos_as_clause_owned("public.png").is_err());
        assert!(macos_as_clause_owned("«class PNGf» & do shell script \"x\"").is_err());
    }

    #[test]
    fn unknown_clipboard_info_class_fails_without_hanging() {
        let started = Instant::now();
        let result = get_type("«class ZZZZ»", 1024, HELPER_TIMEOUT);
        assert!(result.is_err(), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
