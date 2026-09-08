//! Filesystem observations composed from product-neutral platform facades.

use std::{fs::OpenOptions, path::Path};

use super::*;

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
