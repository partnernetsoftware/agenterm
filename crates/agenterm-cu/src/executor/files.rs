//! Filesystem observations composed from product-neutral platform facades.

use std::{fs::OpenOptions, path::Path};

use sha2::{Digest as _, Sha256};

use super::*;

pub(super) fn file_watch_payload(
    path: &str,
    duration_ms: u64,
    max_events: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    const MAX_DURATION_MS: u64 = 86_400_000;
    const DEFAULT_MAX_EVENTS: usize = 256;
    const MAX_EVENTS: usize = 4_096;
    let max_events = max_events.unwrap_or(DEFAULT_MAX_EVENTS);
    if path.is_empty()
        || !(1..=MAX_DURATION_MS).contains(&duration_ms)
        || !(1..=MAX_EVENTS).contains(&max_events)
    {
        return Err(CuError::new(
            "invalid_input",
            "file-watch requires one non-empty directory PATH, duration-ms in 1..=86400000 and max-events in 1..=4096",
        ));
    }
    let result = agenterm_platform::filesystem_watch::watch_directory(
        Path::new(path),
        duration_ms,
        max_events,
    )
    .map_err(map_file_watch_error)?;
    Ok(serde_json::json!({
        "path": result.path,
        "provider": result.provider,
        "mode": result.mode,
        "duration_ms": result.duration_ms,
        "max_events": result.max_events,
        "events": result.events.into_iter().map(|event| serde_json::json!({
            "t_ms": event.t_ms,
            "kind": event.kind,
            "name": event.name,
            "mask": event.mask,
        })).collect::<Vec<_>>(),
        "emitted": result.emitted,
        "completed": result.completed,
        "truncated": result.truncated,
        "verified": true,
    }))
}

fn map_file_watch_error(
    error: agenterm_platform::filesystem_watch::FilesystemWatchError,
) -> CuError {
    use agenterm_platform::filesystem_watch::FilesystemWatchErrorKind;
    let code = match error.kind {
        FilesystemWatchErrorKind::Unsupported => "file_watch_unsupported",
        FilesystemWatchErrorKind::InvalidInput => "invalid_input",
        FilesystemWatchErrorKind::NotDirectory => "file_watch_not_directory",
        FilesystemWatchErrorKind::Native => "file_watch_failed",
    };
    CuError::new(code, error.message)
}

pub(super) fn file_inspect_payload(path: &str) -> Result<serde_json::Value, CuError> {
    let path = Path::new(path);
    let mut details = agenterm_platform::filesystem_entry::inspect_path(path)
        .map_err(|error| CuError::new("file_inspect_failed", error.to_string()))?;

    let identity = if details.facts.is_link_like() {
        details
            .identity
            .map(|token| {
                serde_json::json!({
                    "available": true,
                    "scope": "final-entry-metadata",
                    "token": token,
                })
            })
            .unwrap_or_else(|| {
                serde_json::json!({
                    "available": false,
                    "reason": "stable link-object identity is unavailable on this platform",
                })
            })
    } else {
        let before = agenterm_platform::file_identity::path_identity(path)
            .map_err(|error| CuError::new("file_identity_failed", error.to_string()))?;
        // Bracket the metadata snapshot with two identities. This prevents a
        // path replacement from publishing metadata for one object under the
        // identity of another.
        details = agenterm_platform::filesystem_entry::inspect_path(path)
            .map_err(|error| CuError::new("file_inspect_failed", error.to_string()))?;
        let after = agenterm_platform::file_identity::path_identity(path)
            .map_err(|error| CuError::new("file_identity_failed", error.to_string()))?;
        if details.facts.is_link_like() || !before.same_object(after) {
            return Err(CuError::new(
                "file_identity_changed",
                "filesystem object identity changed during inspection",
            ));
        }
        serde_json::json!({
            "available": true,
            "scope": "opened-object",
            "filesystem_id": before.filesystem_id.to_string(),
            "object_id": before.object_id.to_string(),
            "hard_link_count": after.hard_link_count.to_string(),
        })
    };
    let kind = if details.facts.is_link_like() {
        "link-like"
    } else if details.facts.is_directory() {
        "directory"
    } else if details.facts.is_file() {
        "file"
    } else {
        "other"
    };

    Ok(serde_json::json!({
        "path": path.to_string_lossy().to_string(),
        "kind": kind,
        "identity": identity,
        "size_bytes": details.length.to_string(),
        "readonly": details.readonly,
        "created_unix_ns": details.created_unix_ns.map(|value| value.to_string()),
        "modified_unix_ns": details.modified_unix_ns.map(|value| value.to_string()),
        "accessed_unix_ns": details.accessed_unix_ns.map(|value| value.to_string()),
        "unix_mode": details.unix_mode.map(|value| format!("{value:o}")),
        "unix_uid": details.unix_uid.map(|value| value.to_string()),
        "unix_gid": details.unix_gid.map(|value| value.to_string()),
        "windows_attributes": details.windows_attributes.map(|value| format!("0x{value:08x}")),
        "followed_final_link": false,
    }))
}

pub(super) fn file_attributes_payload(
    path: &str,
    include_values: bool,
) -> Result<serde_json::Value, CuError> {
    let path = Path::new(path);
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| CuError::new("file_attributes_open_failed", error.to_string()))?;
    let binding = agenterm_platform::file_attributes::bind_regular_file(path, &file)
        .map_err(map_file_attribute_error)?;
    let observations = agenterm_platform::file_attributes::inspect_xattrs(
        &file,
        binding,
        agenterm_platform::file_attributes::XattrInspectLimits {
            include_values,
            ..Default::default()
        },
    )
    .map_err(map_file_attribute_error)?;
    let identity = binding.identity();
    Ok(serde_json::json!({
        "path": path.to_string_lossy(),
        "identity": {
            "filesystem_id": identity.filesystem_id.to_string(),
            "object_id": identity.object_id.to_string(),
            "hard_link_count": identity.hard_link_count.to_string(),
        },
        "include_values": include_values,
        "attributes": observations.into_iter().map(|item| serde_json::json!({
            "name": item.name,
            "namespace": item.namespace,
            "value_bytes": item.value_bytes.to_string(),
            "value_sha256": item.value_sha256,
            "value_hex": item.value.map(|value| hex_encode(&value)),
        })).collect::<Vec<_>>(),
    }))
}

pub(super) fn file_mode_payload(
    path: &str,
    requested_mode: u32,
    apply: bool,
) -> Result<serde_json::Value, CuError> {
    let path = Path::new(path);
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| CuError::new("file_mode_open_failed", error.to_string()))?;
    let binding = agenterm_platform::file_attributes::bind_regular_file(path, &file)
        .map_err(map_file_mode_error)?;
    let plan = agenterm_platform::file_attributes::plan_mode(&file, binding, requested_mode)
        .map_err(map_file_mode_error)?;
    let identity = binding.identity();
    let mut after_mode = plan.before_mode;
    let mut performed = false;
    let mut verified = apply && plan.before_mode == plan.requested_mode;
    if verified {
        agenterm_platform::file_attributes::verify_mode_plan(&file, &plan)
            .map_err(map_file_mode_error)?;
    }
    if apply && plan.before_mode != plan.requested_mode {
        let result = agenterm_platform::file_attributes::apply_mode(&file, &plan)
            .map_err(map_file_mode_error)?;
        after_mode = result.after_mode;
        performed = true;
        verified = result.after_mode == plan.requested_mode;
    }
    Ok(serde_json::json!({
        "path": path.to_string_lossy(),
        "identity": {
            "filesystem_id": identity.filesystem_id.to_string(),
            "object_id": identity.object_id.to_string(),
            "hard_link_count": identity.hard_link_count.to_string(),
        },
        "before_mode": format_mode(plan.before_mode),
        "requested_mode": format_mode(plan.requested_mode),
        "after_mode": format_mode(after_mode),
        "previous_mode": if apply { Some(format_mode(plan.before_mode)) } else { None },
        "action": {
            "apply_requested": apply,
            "performed": performed,
            "verified": verified,
        },
    }))
}

pub(super) fn file_xattr_set_payload(
    path: &str,
    name: &str,
    value_hex: &str,
    apply: bool,
) -> Result<serde_json::Value, CuError> {
    let value = decode_hex(value_hex)?;
    file_xattr_payload(path, XattrRequest::Set { name, value }, apply)
}

pub(super) fn file_xattr_remove_payload(
    path: &str,
    name: &str,
    apply: bool,
) -> Result<serde_json::Value, CuError> {
    file_xattr_payload(path, XattrRequest::Remove { name }, apply)
}

pub(super) fn file_quarantine_clear_payload(
    path: &str,
    apply: bool,
) -> Result<serde_json::Value, CuError> {
    file_xattr_payload(path, XattrRequest::ClearQuarantine, apply)
}

enum XattrRequest<'a> {
    Set { name: &'a str, value: Vec<u8> },
    Remove { name: &'a str },
    ClearQuarantine,
}

fn file_xattr_payload(
    path: &str,
    request: XattrRequest<'_>,
    apply: bool,
) -> Result<serde_json::Value, CuError> {
    const MAX_EXISTING_VALUE_BYTES: usize = 4 * 1024 * 1024;
    let path = Path::new(path);
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| CuError::new("file_xattr_open_failed", error.to_string()))?;
    let binding = agenterm_platform::file_attributes::bind_regular_file(path, &file)
        .map_err(map_file_xattr_error)?;
    let operation = match &request {
        XattrRequest::Set { .. } => "set",
        XattrRequest::Remove { .. } => "remove",
        XattrRequest::ClearQuarantine => "quarantine-clear",
    };
    let plan = match request {
        XattrRequest::Set { name, value } => agenterm_platform::file_attributes::plan_xattr_set(
            &file,
            binding,
            name,
            value,
            MAX_EXISTING_VALUE_BYTES,
        ),
        XattrRequest::Remove { name } => agenterm_platform::file_attributes::plan_xattr_remove(
            &file,
            binding,
            name,
            MAX_EXISTING_VALUE_BYTES,
        ),
        XattrRequest::ClearQuarantine => agenterm_platform::file_attributes::plan_clear_quarantine(
            &file,
            binding,
            MAX_EXISTING_VALUE_BYTES,
        ),
    }
    .map_err(map_file_xattr_error)?;

    let before = plan
        .expected_before
        .as_ref()
        .map(|state| (state.value.len(), state.sha256.clone()));
    let requested = match &plan.action {
        agenterm_platform::file_attributes::XattrAction::Set(value) => {
            Some((value.len(), sha256_hex(value)))
        }
        agenterm_platform::file_attributes::XattrAction::Remove => None,
    };
    let changed = match (&plan.expected_before, &plan.action) {
        (Some(state), agenterm_platform::file_attributes::XattrAction::Set(value)) => {
            state.value != *value
        }
        (None, agenterm_platform::file_attributes::XattrAction::Remove) => false,
        _ => true,
    };
    if apply && changed {
        agenterm_platform::file_attributes::apply_xattr(&file, &plan)
            .map_err(map_file_xattr_error)?;
    } else if apply {
        agenterm_platform::file_attributes::verify_xattr_plan(&file, &plan)
            .map_err(map_file_xattr_error)?;
    }
    let identity = binding.identity();
    Ok(serde_json::json!({
        "path": path.to_string_lossy(),
        "identity": {
            "filesystem_id": identity.filesystem_id.to_string(),
            "object_id": identity.object_id.to_string(),
            "hard_link_count": identity.hard_link_count.to_string(),
        },
        "name": plan.name,
        "operation": operation,
        "before": xattr_state_summary(before.as_ref()),
        "requested": xattr_state_summary(requested.as_ref()),
        "after": xattr_state_summary(if apply { requested.as_ref() } else { before.as_ref() }),
        "action": {
            "apply_requested": apply,
            "performed": apply && changed,
            "verified": apply,
        },
        "values_redacted": true,
    }))
}

fn map_file_attribute_error(
    error: agenterm_platform::file_attributes::FileAttributeError,
) -> CuError {
    use agenterm_platform::file_attributes::FileAttributeErrorKind;
    let code = match error.kind {
        FileAttributeErrorKind::Unsupported => "file_attributes_unsupported",
        FileAttributeErrorKind::NotRegularFile => "file_attributes_not_regular",
        FileAttributeErrorKind::IdentityChanged => "file_attributes_identity_changed",
        FileAttributeErrorKind::InvalidName => "file_attributes_invalid_name",
        FileAttributeErrorKind::InvalidMode => "file_attributes_invalid_mode",
        FileAttributeErrorKind::BudgetExceeded => "file_attributes_budget_exceeded",
        FileAttributeErrorKind::PreconditionChanged => "file_attributes_precondition_changed",
        FileAttributeErrorKind::ReadbackMismatch => "file_attributes_readback_mismatch",
        FileAttributeErrorKind::Native => "file_attributes_native_failed",
        _ => "file_attributes_failed",
    };
    CuError::new(code, error.to_string())
}

fn map_file_mode_error(error: agenterm_platform::file_attributes::FileAttributeError) -> CuError {
    use agenterm_platform::file_attributes::FileAttributeErrorKind;
    let code = match error.kind {
        FileAttributeErrorKind::Unsupported => "file_mode_unsupported",
        FileAttributeErrorKind::NotRegularFile => "file_mode_not_regular",
        FileAttributeErrorKind::IdentityChanged => "file_mode_identity_changed",
        FileAttributeErrorKind::InvalidMode => "file_mode_invalid",
        FileAttributeErrorKind::PreconditionChanged => "file_mode_precondition_changed",
        FileAttributeErrorKind::ReadbackMismatch => "file_mode_effect_unknown",
        FileAttributeErrorKind::Native if error.operation == "file-mode-readback" => {
            "file_mode_effect_unknown"
        }
        FileAttributeErrorKind::Native => "file_mode_native_failed",
        FileAttributeErrorKind::InvalidName | FileAttributeErrorKind::BudgetExceeded => {
            "file_mode_failed"
        }
        _ => "file_mode_failed",
    };
    CuError::new(code, error.to_string())
}

fn map_file_xattr_error(error: agenterm_platform::file_attributes::FileAttributeError) -> CuError {
    use agenterm_platform::file_attributes::FileAttributeErrorKind;
    let code = match error.kind {
        FileAttributeErrorKind::Unsupported => "file_xattr_unsupported",
        FileAttributeErrorKind::NotRegularFile => "file_xattr_not_regular",
        FileAttributeErrorKind::IdentityChanged => "file_xattr_identity_changed",
        FileAttributeErrorKind::InvalidName => "file_xattr_name_invalid",
        FileAttributeErrorKind::BudgetExceeded => "file_xattr_value_limit",
        FileAttributeErrorKind::PreconditionChanged => "file_xattr_precondition_changed",
        FileAttributeErrorKind::ReadbackMismatch => "file_xattr_effect_unknown",
        FileAttributeErrorKind::Native if error.operation == "file-xattr-readback" => {
            "file_xattr_effect_unknown"
        }
        FileAttributeErrorKind::Native => "file_xattr_native_failed",
        FileAttributeErrorKind::InvalidMode => "file_xattr_failed",
        _ => "file_xattr_failed",
    };
    CuError::new(code, error.to_string())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn format_mode(mode: u32) -> String {
    format!("{mode:04o}")
}

fn decode_hex(value: &str) -> Result<Vec<u8>, CuError> {
    const MAX_HEX_BYTES: usize = 8 * 1024 * 1024;
    if value.len() > MAX_HEX_BYTES {
        return Err(CuError::new(
            "file_xattr_value_limit",
            "extended-attribute value exceeds the 4 MiB decoded limit",
        ));
    }
    if !value.len().is_multiple_of(2) || value.bytes().any(|byte| !byte.is_ascii_hexdigit()) {
        return Err(CuError::new(
            "file_xattr_value_invalid",
            "extended-attribute value must be an even-length hexadecimal string",
        ));
    }
    let mut decoded = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = hex_nibble(pair[0]);
        let low = hex_nibble(pair[1]);
        decoded.push((high << 4) | low);
    }
    Ok(decoded)
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => unreachable!("decode_hex validates every nibble"),
    }
}

fn sha256_hex(value: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value);
    hex_encode(&hasher.finalize())
}

fn xattr_state_summary(state: Option<&(usize, String)>) -> serde_json::Value {
    match state {
        Some((bytes, digest)) => serde_json::json!({
            "present": true,
            "bytes": bytes.to_string(),
            "sha256": digest,
        }),
        None => serde_json::json!({
            "present": false,
            "bytes": "0",
            "sha256": serde_json::Value::Null,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_file_has_lossless_size_and_stable_identity() {
        let root =
            std::env::temp_dir().join(format!("agenterm-cu-file-inspect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let file = root.join("item");
        std::fs::write(&file, b"hello").unwrap();
        let value = file_inspect_payload(file.to_str().unwrap()).unwrap();
        assert_eq!(value["kind"], "file");
        assert_eq!(value["size_bytes"], "5");
        assert_eq!(value["identity"]["available"], true);
        assert_eq!(value["followed_final_link"], false);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn mode_plan_is_read_only_and_apply_reports_verified_previous_mode() {
        use std::os::unix::fs::PermissionsExt as _;

        let root =
            std::env::temp_dir().join(format!("agenterm-cu-file-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let file = root.join("item");
        std::fs::write(&file, b"mode-fixture").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();

        let plan = file_mode_payload(file.to_str().unwrap(), 0o600, false).unwrap();
        assert_eq!(plan["before_mode"], "0640");
        assert_eq!(plan["after_mode"], "0640");
        assert_eq!(plan["previous_mode"], serde_json::Value::Null);
        assert_eq!(plan["action"]["performed"], false);
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777,
            0o640
        );

        let applied = file_mode_payload(file.to_str().unwrap(), 0o600, true).unwrap();
        assert_eq!(applied["before_mode"], "0640");
        assert_eq!(applied["after_mode"], "0600");
        assert_eq!(applied["previous_mode"], "0640");
        assert_eq!(applied["action"]["performed"], true);
        assert_eq!(applied["action"]["verified"], true);
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777,
            0o600
        );

        let unchanged = file_mode_payload(file.to_str().unwrap(), 0o600, true).unwrap();
        assert_eq!(unchanged["before_mode"], "0600");
        assert_eq!(unchanged["after_mode"], "0600");
        assert_eq!(unchanged["action"]["performed"], false);
        assert_eq!(unchanged["action"]["verified"], true);

        file_mode_payload(file.to_str().unwrap(), 0o640, true).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn xattr_plan_apply_and_remove_never_return_raw_values() {
        let root = std::env::temp_dir().join(format!(
            "agenterm-cu-file-xattr-mutation-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let file = root.join("item");
        let invalid = file_xattr_set_payload(
            root.join("missing").to_str().unwrap(),
            "user.invalid",
            "0",
            false,
        )
        .expect_err("invalid hex must fail before opening the path");
        assert_eq!(invalid.code, "file_xattr_value_invalid");
        std::fs::write(&file, b"xattr-fixture").unwrap();
        let name = if cfg!(target_os = "linux") {
            "user.agenterm-cu-mutation"
        } else {
            "org.agenterm.cu.mutation"
        };
        let secret_hex = "7365637265742d76616c7565";

        let plan = file_xattr_set_payload(file.to_str().unwrap(), name, secret_hex, false).unwrap();
        assert_eq!(plan["before"]["present"], false);
        assert_eq!(plan["after"]["present"], false);
        assert_eq!(plan["requested"]["bytes"], "12");
        assert_eq!(plan["action"]["performed"], false);
        let encoded = serde_json::to_string(&plan).unwrap();
        assert!(!encoded.contains(secret_hex));
        assert!(!encoded.contains("secret-value"));

        let applied =
            file_xattr_set_payload(file.to_str().unwrap(), name, secret_hex, true).unwrap();
        assert_eq!(applied["before"]["present"], false);
        assert_eq!(applied["after"]["present"], true);
        assert_eq!(applied["action"]["performed"], true);
        assert_eq!(applied["action"]["verified"], true);
        assert!(
            !serde_json::to_string(&applied)
                .unwrap()
                .contains(secret_hex)
        );

        let unchanged =
            file_xattr_set_payload(file.to_str().unwrap(), name, secret_hex, true).unwrap();
        assert_eq!(unchanged["action"]["performed"], false);
        assert_eq!(unchanged["action"]["verified"], true);

        let remove_plan = file_xattr_remove_payload(file.to_str().unwrap(), name, false).unwrap();
        assert_eq!(remove_plan["before"]["present"], true);
        assert_eq!(remove_plan["after"]["present"], true);
        let removed = file_xattr_remove_payload(file.to_str().unwrap(), name, true).unwrap();
        assert_eq!(removed["before"]["present"], true);
        assert_eq!(removed["after"]["present"], false);
        assert_eq!(removed["action"]["performed"], true);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn final_symlink_is_reported_without_following_its_target() {
        use std::os::unix::fs::symlink;

        let root =
            std::env::temp_dir().join(format!("agenterm-cu-link-inspect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let target = root.join("target");
        let link = root.join("link");
        std::fs::write(&target, vec![0_u8; 4096]).unwrap();
        symlink(&target, &link).unwrap();

        let value = file_inspect_payload(link.to_str().unwrap()).unwrap();
        assert_eq!(value["kind"], "link-like");
        assert_eq!(value["followed_final_link"], false);
        assert_ne!(value["size_bytes"], "4096");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_path_is_a_typed_failure() {
        let missing = std::env::temp_dir().join(format!(
            "agenterm-cu-missing-inspect-{}",
            std::process::id()
        ));
        let error = file_inspect_payload(missing.to_str().unwrap()).unwrap_err();
        assert_eq!(error.code, "file_inspect_failed");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn file_attributes_default_does_not_disclose_values() {
        let root = std::env::temp_dir().join(format!(
            "agenterm-cu-file-attributes-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let file = root.join("item");
        std::fs::write(&file, b"hello").unwrap();
        let value = file_attributes_payload(file.to_str().unwrap(), false).unwrap();
        assert_eq!(value["include_values"], false);
        assert!(value["identity"]["object_id"].is_string());
        assert!(
            value["attributes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["value_hex"].is_null())
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
