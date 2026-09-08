use std::{collections::BTreeMap, os::unix::ffi::OsStringExt, path::PathBuf};

use crate::{
    contract::storage::{StorageError, StorageErrorKind},
    storage::{MountedVolume, MountedVolumeInventory},
};

pub(crate) fn mounted_volumes(max: usize) -> Result<MountedVolumeInventory, StorageError> {
    let snapshot = std::fs::read("/proc/self/mountinfo").map_err(|source| {
        StorageError::new(
            StorageErrorKind::Query,
            format!("read mount inventory: {source}"),
        )
    })?;
    let mut mount_paths = BTreeMap::new();
    let mut parse_errors = 0usize;
    for line in snapshot
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let fields = line
            .split(|byte| byte.is_ascii_whitespace())
            .filter(|field| !field.is_empty())
            .collect::<Vec<_>>();
        let Some(encoded_path) = fields.get(4) else {
            parse_errors += 1;
            continue;
        };
        let Some(separator) = fields.iter().position(|field| *field == b"-") else {
            parse_errors += 1;
            continue;
        };
        let Some(fs_type) = fields.get(separator + 1) else {
            parse_errors += 1;
            continue;
        };
        let Ok(decoded) = decode_mount_field(encoded_path) else {
            parse_errors += 1;
            continue;
        };
        mount_paths.insert(
            PathBuf::from(std::ffi::OsString::from_vec(decoded)),
            String::from_utf8_lossy(fs_type).into_owned(),
        );
    }
    collect_paths(mount_paths, max, parse_errors)
}

fn collect_paths(
    paths: BTreeMap<PathBuf, String>,
    max: usize,
    parse_errors: usize,
) -> Result<MountedVolumeInventory, StorageError> {
    let visited = paths.len().saturating_add(parse_errors);
    let mut volumes = Vec::with_capacity(max.min(visited));
    let mut read_errors = parse_errors;
    let mut skipped_unsafe = 0usize;
    let mut skipped_zero_capacity = 0usize;
    let mut truncated = false;
    for (mount_path, fs_type) in paths {
        if !capacity_query_is_local_safe(&fs_type) {
            skipped_unsafe += 1;
            continue;
        }
        if volumes.len() == max {
            truncated = true;
            continue;
        }
        match crate::storage::mounted_volume_space(&mount_path) {
            Ok(space) => volumes.push(MountedVolume { mount_path, space }),
            Err(error) if error.kind() == StorageErrorKind::ZeroCapacity => {
                skipped_zero_capacity += 1;
            }
            Err(_) => read_errors += 1,
        }
    }
    Ok(MountedVolumeInventory {
        volumes,
        visited,
        read_errors,
        skipped_unsafe,
        skipped_zero_capacity,
        truncated,
        coverage: "local-mounted-filesystems",
        coverage_complete: true,
    })
}

fn capacity_query_is_local_safe(fs_type: &str) -> bool {
    matches!(
        fs_type.to_ascii_lowercase().as_str(),
        "btrfs"
            | "cgroup"
            | "cgroup2"
            | "debugfs"
            | "devpts"
            | "devtmpfs"
            | "efivarfs"
            | "erofs"
            | "exfat"
            | "ext2"
            | "ext3"
            | "ext4"
            | "f2fs"
            | "hugetlbfs"
            | "iso9660"
            | "mqueue"
            | "nilfs2"
            | "ntfs"
            | "ntfs3"
            | "overlay"
            | "proc"
            | "pstore"
            | "ramfs"
            | "reiserfs"
            | "securityfs"
            | "squashfs"
            | "sysfs"
            | "tmpfs"
            | "tracefs"
            | "udf"
            | "vfat"
            | "xfs"
            | "zfs"
    )
}

fn decode_mount_field(encoded: &[u8]) -> Result<Vec<u8>, StorageError> {
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut index = 0usize;
    while index < encoded.len() {
        if encoded[index] == b'\\' {
            let octal = encoded.get(index + 1..index + 4).ok_or_else(|| {
                StorageError::new(StorageErrorKind::Query, "truncated mount-point escape")
            })?;
            if !octal.iter().all(|byte| matches!(byte, b'0'..=b'7')) {
                return Err(StorageError::new(
                    StorageErrorKind::Query,
                    "invalid mount-point escape",
                ));
            }
            let value = u32::from(octal[0] - b'0') * 64
                + u32::from(octal[1] - b'0') * 8
                + u32::from(octal[2] - b'0');
            let value = u8::try_from(value).map_err(|_| {
                StorageError::new(
                    StorageErrorKind::Query,
                    "mount-point escape exceeds one byte",
                )
            })?;
            decoded.push(value);
            index += 4;
        } else {
            decoded.push(encoded[index]);
            index += 1;
        }
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_kernel_mount_escapes_without_exposing_device_fields() {
        assert_eq!(
            decode_mount_field(b"/media/a\\040b").unwrap(),
            b"/media/a b"
        );
        assert!(decode_mount_field(b"/bad\\09x").is_err());
        assert!(decode_mount_field(b"/bad\\777").is_err());
        assert!(!capacity_query_is_local_safe("autofs"));
        assert!(!capacity_query_is_local_safe("fuse.sshfs"));
        assert!(!capacity_query_is_local_safe("unknown-future-fs"));
        assert!(capacity_query_is_local_safe("ext4"));
    }
}
