use std::{
    collections::BTreeMap,
    io::Read,
    os::unix::ffi::OsStringExt,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

const MOUNTINFO_BYTES_MAX: u64 = 4 * 1024 * 1024;

use crate::{
    contract::storage::{StorageError, StorageErrorKind},
    storage::{MountedVolume, MountedVolumeInventory},
};

pub(crate) fn mounted_volumes(max: usize) -> Result<MountedVolumeInventory, StorageError> {
    let snapshot = read_mountinfo()?;
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct MountCandidate {
    device: Vec<u8>,
    mount_path: PathBuf,
    local_safe: bool,
}

pub(crate) fn path_mount(path: &Path) -> Result<crate::storage::NativePathMount, StorageError> {
    let snapshot = read_mountinfo()?;
    let metadata = std::fs::metadata(path).map_err(|source| {
        StorageError::new(
            StorageErrorKind::Path,
            format!("{}: {source}", path.display()),
        )
    })?;
    let device = format!(
        "{}:{}",
        libc::major(metadata.dev()),
        libc::minor(metadata.dev())
    );
    let candidates = snapshot
        .split(|byte| *byte == b'\n')
        .filter_map(parse_mount_candidate)
        .collect::<Vec<_>>();
    let selected = select_mount(&candidates, device.as_bytes(), path);
    Ok(match selected {
        Some(candidate) => crate::storage::NativePathMount {
            mount_path: Some(candidate.mount_path.clone()),
            mount_path_reason: None,
            mount_proof: "mountinfo-st_dev-longest-prefix",
            in_inventory: candidate.local_safe,
        },
        None => crate::storage::NativePathMount {
            mount_path: None,
            mount_path_reason: Some("mountinfo-no-matching-entry"),
            mount_proof: "mountinfo-st_dev-longest-prefix",
            in_inventory: false,
        },
    })
}

fn read_mountinfo() -> Result<Vec<u8>, StorageError> {
    let file = std::fs::File::open("/proc/self/mountinfo").map_err(|source| {
        StorageError::new(
            StorageErrorKind::Query,
            format!("open mount inventory: {source}"),
        )
    })?;
    let mut snapshot = Vec::new();
    file.take(MOUNTINFO_BYTES_MAX + 1)
        .read_to_end(&mut snapshot)
        .map_err(|source| {
            StorageError::new(
                StorageErrorKind::Query,
                format!("read mount inventory: {source}"),
            )
        })?;
    if snapshot.len() as u64 > MOUNTINFO_BYTES_MAX {
        return Err(StorageError::new(
            StorageErrorKind::Query,
            "mount inventory exceeds 4194304 bytes",
        ));
    }
    Ok(snapshot)
}

fn parse_mount_candidate(line: &[u8]) -> Option<MountCandidate> {
    let fields = line
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    let device = fields.get(2)?;
    let encoded_path = fields.get(4)?;
    let separator = fields.iter().position(|field| *field == b"-")?;
    let fs_type = fields.get(separator + 1)?;
    let mount_path = PathBuf::from(std::ffi::OsString::from_vec(
        decode_mount_field(encoded_path).ok()?,
    ));
    Some(MountCandidate {
        device: device.to_vec(),
        mount_path,
        local_safe: capacity_query_is_local_safe(&String::from_utf8_lossy(fs_type)),
    })
}

fn select_mount<'a>(
    candidates: &'a [MountCandidate],
    device: &[u8],
    path: &Path,
) -> Option<&'a MountCandidate> {
    let mut selected = None;
    for candidate in candidates {
        if candidate.device != device || !path.starts_with(&candidate.mount_path) {
            continue;
        }
        if selected.is_none_or(|current: &MountCandidate| {
            candidate.mount_path.components().count() >= current.mount_path.components().count()
        }) {
            selected = Some(candidate);
        }
    }
    selected
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

    #[test]
    fn selects_same_device_by_component_prefix_and_prefers_the_last_deepest_mount() {
        let candidates = [
            MountCandidate {
                device: b"1:2".to_vec(),
                mount_path: PathBuf::from("/"),
                local_safe: true,
            },
            MountCandidate {
                device: b"1:2".to_vec(),
                mount_path: PathBuf::from("/mnt/a"),
                local_safe: true,
            },
            MountCandidate {
                device: b"1:2".to_vec(),
                mount_path: PathBuf::from("/mnt/a"),
                local_safe: false,
            },
            MountCandidate {
                device: b"9:9".to_vec(),
                mount_path: PathBuf::from("/mnt/a/deeper"),
                local_safe: true,
            },
        ];
        assert_eq!(
            select_mount(&candidates, b"1:2", Path::new("/mnt/a/file")),
            Some(&candidates[2])
        );
        assert_eq!(
            select_mount(&candidates, b"1:2", Path::new("/mnt/ab/file")),
            Some(&candidates[0])
        );
        assert_eq!(select_mount(&candidates, b"3:4", Path::new("/mnt/a")), None);
    }

    #[test]
    fn parses_mountinfo_device_path_escape_and_filesystem_class() {
        let candidate =
            parse_mount_candidate(b"10 9 1:2 / /media/a\\040b rw - ext4 /dev/x rw").unwrap();
        assert_eq!(candidate.device, b"1:2");
        assert_eq!(candidate.mount_path, PathBuf::from("/media/a b"));
        assert!(candidate.local_safe);
    }
}
