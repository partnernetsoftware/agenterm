//! Public `device-screenshot` product semantics over the platform capture
//! contract. Native discovery remains read-only; successful frame bytes are
//! published atomically to a new caller-owned path and never enter stdout.

use super::*;

#[cfg(target_os = "macos")]
use std::{fs, io::Write as _, path::Path};

#[cfg(target_os = "macos")]
pub(super) const DEFAULT_DEVICE_CAPTURE_TIMEOUT_MS: u64 = 10_000;
#[cfg(target_os = "macos")]
pub(super) const MAX_DEVICE_CAPTURE_TIMEOUT_MS: u64 = 120_000;

#[cfg(target_os = "macos")]
pub(super) fn device_screenshot_payload(
    path: Option<&str>,
    selector: Option<&str>,
    timeout_ms: Option<u64>,
    list: bool,
) -> Result<serde_json::Value, CuError> {
    use std::time::Duration;

    use agenterm_platform::device_capture::{
        DeviceCaptureBackend as _, classify_stream_failure, native_backend, select_device,
    };
    use sha2::{Digest as _, Sha256};

    if list && (path.is_some() || selector.is_some() || timeout_ms.is_some()) {
        return Err(CuError::new(
            "invalid_input",
            "device-screenshot --list cannot be combined with capture fields",
        ));
    }

    let backend = native_backend();
    let evidence = backend.observe();
    if list {
        let inventory = evidence.inventory().map_err(map_capture_error)?;
        return Ok(serde_json::json!({
            "count": inventory.sources.len(),
            "devices": inventory.sources,
            "host_camera_authorization": inventory.host_camera_authorization,
            "usbmux": inventory.usbmux,
        }));
    }

    let destination = path.filter(|value| !value.is_empty()).ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "device-screenshot requires --out PATH (or --list)",
        )
    })?;
    let timeout_ms = timeout_ms.unwrap_or(DEFAULT_DEVICE_CAPTURE_TIMEOUT_MS);
    if !(1..=MAX_DEVICE_CAPTURE_TIMEOUT_MS).contains(&timeout_ms) {
        return Err(CuError::new(
            "invalid_input",
            format!("device-screenshot --timeout-ms must be 1..={MAX_DEVICE_CAPTURE_TIMEOUT_MS}"),
        ));
    }

    let selected = select_device(&evidence, selector).map_err(map_capture_error)?;
    let frame = backend
        .capture_selected(&selected, Duration::from_millis(timeout_ms))
        .map_err(|failure| map_capture_error(classify_stream_failure(&selected, failure)))?;

    let bytes = frame.encoded.len();
    let digest = Sha256::digest(&frame.encoded)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    publish_new_file(Path::new(destination), &frame.encoded).map_err(|error| {
        CuError::new(error.code, error.message).with_detail(serde_json::json!({
            "path": destination,
            "published": error.published,
        }))
    })?;

    Ok(serde_json::json!({
        "path": destination,
        "bytes": bytes,
        "sha256": digest,
        "width": frame.width,
        "height": frame.height,
        "device": frame.source,
        "content_read": false,
        "replaced": false,
        "host_camera_authorization": evidence.host_camera_authorization,
        "status_bar": "ios_capture_placeholder",
        "status_bar_note": "iOS may replace the status bar while a device capture session is live; application pixels are the capture evidence",
    }))
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct PublishFailure {
    code: &'static str,
    message: String,
    published: bool,
}

#[cfg(target_os = "macos")]
fn publish_new_file(destination: &Path, bytes: &[u8]) -> Result<(), PublishFailure> {
    let file_name = destination.file_name().ok_or_else(|| PublishFailure {
        code: "device_capture_invalid_output",
        message: "device-screenshot --out requires a final file name".to_owned(),
        published: false,
    })?;
    let parent = destination
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent).map_err(|error| PublishFailure {
        code: "device_capture_output_unavailable",
        message: format!("device screenshot output parent is unavailable: {error}"),
        published: false,
    })?;
    let destination = parent.join(file_name);
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(PublishFailure {
            code: "device_capture_destination_exists",
            message: "device screenshot output already exists; it was not replaced".to_owned(),
            published: false,
        });
    }

    let mut temporary = None;
    let mut file = None;
    for _ in 0..64 {
        let random = agenterm_platform::entropy::secure_random_array::<16>().map_err(|_| {
            PublishFailure {
                code: "device_capture_output_unavailable",
                message: "secure output staging identity is unavailable".to_owned(),
                published: false,
            }
        })?;
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let candidate = parent.join(format!(
            ".{}.device-capture-{suffix}",
            file_name.to_string_lossy()
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(opened) => {
                temporary = Some(candidate);
                file = Some(opened);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(PublishFailure {
                    code: "device_capture_output_unavailable",
                    message: format!(
                        "device screenshot staging file could not be created: {error}"
                    ),
                    published: false,
                });
            }
        }
    }
    let temporary = temporary.ok_or_else(|| PublishFailure {
        code: "device_capture_output_unavailable",
        message: "unique device screenshot staging attempts were exhausted".to_owned(),
        published: false,
    })?;
    let mut file = file.expect("temporary and file are created together");
    let staged = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = staged {
        let _ = fs::remove_file(&temporary);
        return Err(PublishFailure {
            code: "device_capture_write_failed",
            message: format!("device screenshot staging write failed: {error}"),
            published: false,
        });
    }

    // Same-parent hard linking is the portable no-clobber publication
    // primitive: destination creation is atomic and fails if any entry,
    // including a symlink, won the race. The staging name is then removed.
    if let Err(error) = fs::hard_link(&temporary, &destination) {
        let _ = fs::remove_file(&temporary);
        let (code, message) = if error.kind() == std::io::ErrorKind::AlreadyExists {
            (
                "device_capture_destination_exists",
                "device screenshot output appeared concurrently; it was not replaced".to_owned(),
            )
        } else {
            (
                "device_capture_publish_failed",
                format!("device screenshot output could not be published: {error}"),
            )
        };
        return Err(PublishFailure {
            code,
            message,
            published: false,
        });
    }
    let _ = fs::remove_file(&temporary);
    if let Err(error) = fs::File::open(&parent).and_then(|directory| directory.sync_all()) {
        return Err(PublishFailure {
            code: "device_capture_durability_uncertain",
            message: format!(
                "device screenshot is complete, but parent durability could not be confirmed: {error}"
            ),
            published: true,
        });
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(super) fn device_screenshot_payload(
    _path: Option<&str>,
    _selector: Option<&str>,
    _timeout_ms: Option<u64>,
    _list: bool,
) -> Result<serde_json::Value, CuError> {
    Err(CuError::new(
        "device_capture_unsupported",
        "wired device capture is currently implemented on macOS hosts only",
    ))
}

#[cfg(target_os = "macos")]
fn map_capture_error(error: agenterm_platform::device_capture::DeviceCaptureError) -> CuError {
    let code = error.code();
    let mut detail = serde_json::json!({});
    if let Some(fix) = error.fix {
        detail["fix"] = serde_json::json!(fix);
    }
    if matches!(
        error.kind,
        agenterm_platform::device_capture::DeviceCaptureErrorKind::HostTccDenied
            | agenterm_platform::device_capture::DeviceCaptureErrorKind::HostTccRestricted
            | agenterm_platform::device_capture::DeviceCaptureErrorKind::HostTccConsentRequired
    ) {
        detail["permission"] = serde_json::json!("camera");
        detail["stable_identity_required"] = serde_json::json!(true);
    }
    CuError::new(code, error.message).with_detail(detail)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn fixture_directory(label: &str) -> std::path::PathBuf {
        let random = agenterm_platform::entropy::secure_random_array::<8>().expect("entropy");
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = std::env::temp_dir().join(format!(
            "agenterm-device-capture-{label}-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("fixture directory");
        path
    }

    #[test]
    fn publication_is_complete_and_never_overwrites() {
        let directory = fixture_directory("publish");
        let destination = directory.join("frame.png");
        publish_new_file(&destination, b"first-complete-frame").expect("first publish");
        assert_eq!(
            std::fs::read(&destination).expect("published frame"),
            b"first-complete-frame"
        );

        let error = publish_new_file(&destination, b"replacement").expect_err("no clobber");
        assert_eq!(error.code, "device_capture_destination_exists");
        assert!(!error.published);
        assert_eq!(
            std::fs::read(&destination).expect("original frame"),
            b"first-complete-frame"
        );
        let names = std::fs::read_dir(&directory)
            .expect("fixture inventory")
            .map(|entry| entry.expect("fixture entry").file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![std::ffi::OsString::from("frame.png")]);
        std::fs::remove_dir_all(directory).expect("fixture cleanup");
    }

    #[test]
    fn concurrent_publish_has_exactly_one_winner() {
        let directory = fixture_directory("race");
        let destination = directory.join("frame.png");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut workers = Vec::new();
        for value in [b"left".as_slice(), b"right".as_slice()] {
            let destination = destination.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                publish_new_file(&destination, value)
            }));
        }
        let outcomes = workers
            .into_iter()
            .map(|worker| worker.join().expect("publisher"))
            .collect::<Vec<_>>();
        assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            outcomes
                .iter()
                .filter_map(|result| result.as_ref().err())
                .filter(|error| error.code == "device_capture_destination_exists")
                .count(),
            1
        );
        let bytes = std::fs::read(&destination).expect("winner frame");
        assert!(bytes == b"left" || bytes == b"right");
        std::fs::remove_dir_all(directory).expect("fixture cleanup");
    }
}
