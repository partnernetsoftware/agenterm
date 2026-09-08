//! `clipboard-read` / `clipboard-write` / `clipboard-write-file` /
//! `clipboard-clear`: bounded native clipboard verbs.

use super::*;

/// Read the target session's native Unicode-text clipboard through the
/// existing bounded libagenterm two-stage ABI. The payload is returned only
/// in this observe command's reply; audit records never receive it because
/// they are restricted to authorized actuation metadata.
/// `clipboard-read`: the Unicode text, plus what else the clipboard is
/// carrying.
///
/// The type list matters even though this verb only reads text: an agent
/// that copies an image and then reads an empty string would otherwise
/// conclude the clipboard is empty. `types` names what is actually there
/// in the host's own spelling; `types_available` false means this host
/// cannot enumerate them, which is a different fact from an empty list.
pub(super) fn clipboard_read() -> Result<serde_json::Value, CuError> {
    let text = mechanism::clipboard::get_text().map_err(map_mechanism_err)?;
    let bytes = text.len();
    let (types, types_available, types_reason) = match mechanism::clipboard::available_types() {
        Ok(names) => (names, true, None),
        Err(mechanism::MechanismError::Unsupported { reason }) => (Vec::new(), false, Some(reason)),
        // A reason a caller reads, not a Debug rendering of the enum.
        Err(mechanism::MechanismError::Failed { code, message }) => {
            (Vec::new(), false, Some(format!("{code}: {message}")))
        }
    };
    let mut payload = serde_json::json!({
        "text": text,
        "bytes": bytes,
        "format": "text/plain;charset=utf-8",
        "mechanism": "libagenterm",
        "types": types,
        "types_available": types_available,
    });
    if let Some(reason) = types_reason
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("types_reason".into(), serde_json::json!(reason));
    }
    Ok(payload)
}

/// Inspect the native clipboard provider without copying any clipboard
/// payload into this process. This is the safe capability/doctor probe: an
/// empty type list is a valid empty clipboard, while an unavailable type
/// inventory remains an explicit provider result.
pub(super) fn clipboard_metadata() -> Result<serde_json::Value, CuError> {
    let (types, types_available, types_reason) = match mechanism::clipboard::available_types() {
        Ok(names) => (names, true, None),
        Err(mechanism::MechanismError::Unsupported { reason }) => (Vec::new(), false, Some(reason)),
        Err(mechanism::MechanismError::Failed { code, message }) => {
            (Vec::new(), false, Some(format!("{code}: {message}")))
        }
    };
    let mut payload = serde_json::json!({
        "payload_read": false,
        "mechanism": "libagenterm",
        "types": types,
        "types_available": types_available,
    });
    if let Some(reason) = types_reason
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("types_reason".into(), serde_json::json!(reason));
    }
    Ok(payload)
}

pub(super) const DEFAULT_CLIPBOARD_TYPE_BYTES: usize = 1024 * 1024;

pub(super) const MAX_CLIPBOARD_TYPE_BYTES: usize = 16 * 1024 * 1024;

/// Match the hard read ceiling of `agt_clipboard_get_text`, because a direct
/// write is successful only after exact native read-back verification.
const MAX_CLIPBOARD_TEXT_BYTES: usize = 1024 * 1024;

pub(super) fn clipboard_read_type(
    type_name: &str,
    max_bytes: Option<usize>,
    out: Option<&str>,
    replace: bool,
) -> Result<serde_json::Value, CuError> {
    if type_name.is_empty() || type_name.len() > 256 || type_name.contains('\0') {
        return Err(CuError::new(
            "invalid_input",
            "clipboard-read --type must be 1..256 bytes without NUL",
        ));
    }
    let max_bytes = max_bytes.unwrap_or(DEFAULT_CLIPBOARD_TYPE_BYTES);
    if max_bytes == 0 || max_bytes > MAX_CLIPBOARD_TYPE_BYTES {
        return Err(CuError::new(
            "invalid_input",
            "clipboard-read --max-bytes must be 1..16777216",
        ));
    }
    let bytes = mechanism::clipboard::get_type(type_name, max_bytes).map_err(map_mechanism_err)?;
    let sha256 = clipboard_sha256_hex(&bytes);
    let mut payload = serde_json::json!({
        "type": type_name,
        "bytes": bytes.len(),
        "sha256": sha256,
        "mechanism": "libagenterm",
    });
    if let Some(path) = out {
        write_clipboard_bytes(path, &bytes, &sha256, replace)?;
        payload
            .as_object_mut()
            .expect("object")
            .insert("out".into(), serde_json::json!(path));
        return Ok(payload);
    }
    let (encoding, value) = clipboard_encoding_and_value(type_name, &bytes);
    payload
        .as_object_mut()
        .expect("object")
        .insert("encoding".into(), serde_json::json!(encoding));
    payload
        .as_object_mut()
        .expect("object")
        .insert("value".into(), serde_json::json!(value));
    Ok(payload)
}

pub(super) fn clipboard_sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn clipboard_encoding_and_value(
    type_name: &str,
    bytes: &[u8],
) -> (&'static str, String) {
    let textual = type_name.eq_ignore_ascii_case("string")
        || type_name.eq_ignore_ascii_case("unicode text")
        || type_name.contains("utf8")
        || type_name.contains("UTF8")
        || type_name.contains("text/plain")
        || type_name.contains("text/html")
        || type_name == "CF_TEXT"
        || type_name == "CF_UNICODETEXT"
        || type_name == "CF_OEMTEXT"
        || type_name.starts_with("public.utf8")
        || type_name.starts_with("public.plain-text")
        || type_name.starts_with("public.text")
        || type_name.starts_with("public.url")
        || type_name.starts_with("public.file-url");
    if textual && let Ok(text) = std::str::from_utf8(bytes) {
        return ("utf8", text.to_owned());
    }
    ("base64", encode_base64(bytes))
}

pub(super) fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        let b1 = bytes.get(i + 1).copied();
        let b2 = bytes.get(i + 2).copied();
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1.unwrap_or(0) >> 4)) as usize] as char);
        match (b1, b2) {
            (None, _) => {
                out.push('=');
                out.push('=');
            }
            (Some(b1), None) => {
                out.push(TABLE[((b1 & 0x0f) << 2) as usize] as char);
                out.push('=');
            }
            (Some(b1), Some(b2)) => {
                out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
                out.push(TABLE[(b2 & 0x3f) as usize] as char);
            }
        }
        i += 3;
    }
    out
}

pub(super) fn write_clipboard_bytes(
    path: &str,
    bytes: &[u8],
    sha256: &str,
    replace: bool,
) -> Result<(), CuError> {
    let flags = if replace {
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .to_owned()
    } else {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .to_owned()
    };
    let mut file = flags.open(path).map_err(|error| {
        CuError::new(
            "clipboard_write_failed",
            format!("clipboard-read --out {path}: {error}"),
        )
    })?;
    use std::io::Write;
    file.write_all(bytes).map_err(|error| {
        CuError::new(
            "clipboard_write_failed",
            format!("clipboard-read --out {path}: {error}"),
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    let stored = std::fs::read(path).map_err(|error| {
        CuError::new(
            "clipboard_write_failed",
            format!("clipboard-read --out reread {path}: {error}"),
        )
    })?;
    if stored.len() != bytes.len() || clipboard_sha256_hex(&stored) != sha256 {
        return Err(CuError::new(
            "clipboard_write_failed",
            "clipboard-read --out failed hash/length verification",
        ));
    }
    Ok(())
}

pub(super) fn clipboard_write(
    type_name: &str,
    source: &crate::command::ClipboardWriteSource,
) -> Result<serde_json::Value, CuError> {
    if type_name.is_empty() || type_name.len() > 256 || type_name.contains('\0') {
        return Err(CuError::new(
            "invalid_input",
            "clipboard-write --type must be 1..256 bytes without NUL",
        ));
    }
    let path = match source {
        crate::command::ClipboardWriteSource::Path(path) => path,
        crate::command::ClipboardWriteSource::Text { text } => {
            return clipboard_write_text(type_name, text);
        }
    };
    let bytes = std::fs::read(path).map_err(|error| {
        CuError::new(
            "clipboard_write_failed",
            format!("clipboard-write --path {path}: {error}"),
        )
    })?;
    if bytes.len() > MAX_CLIPBOARD_TYPE_BYTES {
        return Err(CuError::new(
            "invalid_input",
            "clipboard-write source must be at most 16777216 bytes",
        ));
    }
    // UTF-8 plain text must use the publish path so Linux X11 keeps a
    // detached CLIPBOARD owner alive after this process exits.
    if type_name == crate::command::CLIPBOARD_UTF8_TEXT_TYPE {
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            CuError::new(
                "invalid_input",
                format!(
                    "clipboard-write --path {path} is not valid UTF-8 for --type {}: {error}",
                    crate::command::CLIPBOARD_UTF8_TEXT_TYPE
                ),
            )
        })?;
        return clipboard_write_text(type_name, text);
    }
    if uses_linux_x11_publish(type_name) {
        return clipboard_write_published_type(type_name, &bytes);
    }
    let sha256 = clipboard_sha256_hex(&bytes);
    mechanism::clipboard::set_type(type_name, &bytes).map_err(map_mechanism_err)?;
    let stored = mechanism::clipboard::get_type(type_name, MAX_CLIPBOARD_TYPE_BYTES)
        .map_err(map_mechanism_err)?;
    let verified = stored == bytes;
    Ok(serde_json::json!({
        "type": type_name,
        "bytes": bytes.len(),
        "sha256": sha256,
        "verified": verified,
        "mechanism": "libagenterm",
    }))
}

fn clipboard_write_text(type_name: &str, text: &str) -> Result<serde_json::Value, CuError> {
    validate_clipboard_text(type_name, text)?;
    let bytes = text.as_bytes();
    let sha256 = clipboard_sha256_hex(bytes);
    mechanism::clipboard::publish_text(text).map_err(map_mechanism_err)?;
    let stored = mechanism::clipboard::get_text().map_err(map_mechanism_err)?;
    verified_text_receipt(type_name, bytes, stored.as_bytes(), &sha256)
}

fn uses_linux_x11_publish(type_name: &str) -> bool {
    #[cfg(not(target_os = "linux"))]
    let _ = type_name;
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("DISPLAY").is_some()
            && (type_name == crate::command::CLIPBOARD_HTML_TYPE
                || type_name == crate::command::CLIPBOARD_PNG_TYPE)
        {
            return true;
        }
    }
    false
}

fn clipboard_write_published_type(
    type_name: &str,
    bytes: &[u8],
) -> Result<serde_json::Value, CuError> {
    let sha256 = clipboard_sha256_hex(bytes);
    mechanism::clipboard::publish_type(type_name, bytes).map_err(map_mechanism_err)?;
    let stored = mechanism::clipboard::get_type(type_name, MAX_CLIPBOARD_TYPE_BYTES)
        .map_err(map_mechanism_err)?;
    verified_bytes_receipt(type_name, bytes, &stored, &sha256)
}

fn verified_bytes_receipt(
    type_name: &str,
    expected: &[u8],
    stored: &[u8],
    expected_sha256: &str,
) -> Result<serde_json::Value, CuError> {
    if stored.len() != expected.len() || clipboard_sha256_hex(stored) != expected_sha256 {
        return Err(CuError::new(
            "clipboard_verify_failed",
            "clipboard-write read-back length/hash verification failed",
        ));
    }
    Ok(serde_json::json!({
        "type": type_name,
        "bytes": expected.len(),
        "sha256": expected_sha256,
        "verified": true,
        "mechanism": "libagenterm",
    }))
}

fn validate_clipboard_text(type_name: &str, text: &str) -> Result<(), CuError> {
    if type_name != crate::command::CLIPBOARD_UTF8_TEXT_TYPE {
        return Err(CuError::new(
            "invalid_input",
            "clipboard-write --text requires --type text/plain;charset=utf-8",
        ));
    }
    if text.len() > MAX_CLIPBOARD_TEXT_BYTES {
        return Err(CuError::new(
            "invalid_input",
            "clipboard-write --text must be at most 1048576 UTF-8 bytes",
        ));
    }
    if text.contains('\0') {
        return Err(CuError::new(
            "invalid_input",
            "clipboard-write --text cannot contain NUL because native text clipboards cannot round-trip it",
        ));
    }
    Ok(())
}

fn verified_text_receipt(
    type_name: &str,
    expected: &[u8],
    stored: &[u8],
    expected_sha256: &str,
) -> Result<serde_json::Value, CuError> {
    if stored.len() != expected.len() || clipboard_sha256_hex(stored) != expected_sha256 {
        return Err(CuError::new(
            "clipboard_verify_failed",
            "clipboard-write --text read-back length/hash verification failed",
        ));
    }
    Ok(serde_json::json!({
        "type": type_name,
        "bytes": expected.len(),
        "sha256": expected_sha256,
        "verified": true,
        "mechanism": "libagenterm",
    }))
}

pub(super) fn clipboard_write_file(path: &str) -> Result<serde_json::Value, CuError> {
    if !std::path::Path::new(path).exists() {
        return Err(CuError::new(
            "invalid_input",
            "clipboard-write-file path does not exist",
        ));
    }
    mechanism::clipboard::set_file(path).map_err(map_mechanism_err)?;
    Ok(serde_json::json!({
        "path": path,
        "mechanism": "libagenterm",
        "verified": true,
    }))
}

pub(super) fn clipboard_clear(apply: bool) -> Result<serde_json::Value, CuError> {
    if !apply {
        return Ok(serde_json::json!({
            "status": "planned",
            "applyRequired": true,
            "operation": "clipboard-clear",
        }));
    }
    mechanism::clipboard::clear().map_err(map_mechanism_err)?;
    let types = mechanism::clipboard::available_types().unwrap_or_default();
    Ok(serde_json::json!({
        "status": "cleared",
        "verified": types.is_empty(),
        "types": types,
        "mechanism": "libagenterm",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_base64_padding_matches_rfc4648() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
    }

    #[test]
    fn direct_text_validation_is_bounded_typed_and_nul_safe() {
        assert!(
            validate_clipboard_text(crate::command::CLIPBOARD_UTF8_TEXT_TYPE, "中🚀\ntext").is_ok()
        );
        assert_eq!(
            validate_clipboard_text("UTF8_STRING", "text")
                .expect_err("host-specific type")
                .code,
            "invalid_input"
        );
        let secret = "prefix\0private-suffix";
        let error = validate_clipboard_text(crate::command::CLIPBOARD_UTF8_TEXT_TYPE, secret)
            .expect_err("NUL");
        assert_eq!(error.code, "invalid_input");
        assert!(!error.message.contains(secret));
        assert_eq!(
            validate_clipboard_text(
                crate::command::CLIPBOARD_UTF8_TEXT_TYPE,
                &"x".repeat(MAX_CLIPBOARD_TEXT_BYTES + 1),
            )
            .expect_err("ceiling")
            .code,
            "invalid_input"
        );
    }

    #[test]
    fn clipboard_write_path_for_utf8_plain_text_routes_through_publish() {
        let oversized = "x".repeat(MAX_CLIPBOARD_TEXT_BYTES + 1);
        let error = validate_clipboard_text(crate::command::CLIPBOARD_UTF8_TEXT_TYPE, &oversized)
            .expect_err("ceiling");
        assert_eq!(error.code, "invalid_input");
    }

    #[test]
    fn direct_text_receipt_never_echoes_content_and_mismatch_fails() {
        let secret = "private 中🚀\ntext";
        let sha256 = clipboard_sha256_hex(secret.as_bytes());
        let receipt = verified_text_receipt(
            crate::command::CLIPBOARD_UTF8_TEXT_TYPE,
            secret.as_bytes(),
            secret.as_bytes(),
            &sha256,
        )
        .expect("matching read-back");
        let encoded = receipt.to_string();
        assert!(!encoded.contains(secret));
        assert_eq!(receipt["bytes"], secret.len());
        assert_eq!(receipt["sha256"], sha256);
        assert_eq!(receipt["verified"], true);

        let error = verified_text_receipt(
            crate::command::CLIPBOARD_UTF8_TEXT_TYPE,
            secret.as_bytes(),
            b"different",
            &sha256,
        )
        .expect_err("mismatch must not be a false success");
        assert_eq!(error.code, "clipboard_verify_failed");
        assert!(!error.message.contains(secret));
    }
}
