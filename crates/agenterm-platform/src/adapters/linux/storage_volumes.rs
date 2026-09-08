use std::{collections::BTreeSet, os::unix::ffi::OsStringExt, path::PathBuf};

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
    let mut mount_paths = BTreeSet::new();
    for line in snapshot
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let encoded = line
            .split(|byte| byte.is_ascii_whitespace())
            .filter(|field| !field.is_empty())
            .nth(4)
            .ok_or_else(|| {
                StorageError::new(
                    StorageErrorKind::Query,
                    "mount inventory row lacks mount point",
                )
            })?;
        mount_paths.insert(PathBuf::from(std::ffi::OsString::from_vec(
            decode_mount_field(encoded)?,
        )));
    }
    collect_paths(mount_paths, max)
}

fn collect_paths(
    paths: BTreeSet<PathBuf>,
    max: usize,
) -> Result<MountedVolumeInventory, StorageError> {
    let visited = paths.len();
    let mut volumes = Vec::with_capacity(max.min(visited));
    let mut read_errors = 0usize;
    for mount_path in paths {
        if volumes.len() == max {
            break;
        }
        match crate::storage::mounted_volume_space(&mount_path) {
            Ok(space) => volumes.push(MountedVolume { mount_path, space }),
            Err(_) => read_errors += 1,
        }
    }
    Ok(MountedVolumeInventory {
        truncated: volumes.len().saturating_add(read_errors) < visited,
        volumes,
        visited,
        read_errors,
    })
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
            decoded.push((octal[0] - b'0') * 64 + (octal[1] - b'0') * 8 + octal[2] - b'0');
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
    }
}
