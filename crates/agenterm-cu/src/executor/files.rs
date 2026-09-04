//! Filesystem observations composed from product-neutral platform facades.

use std::path::Path;

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
}
