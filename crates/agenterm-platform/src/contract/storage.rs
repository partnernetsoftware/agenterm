//! Product-neutral capacity facts for the volume containing a host path.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VolumeSpace {
    pub total_bytes: std::num::NonZeroU64,
    /// Bytes available to the current user, including quota effects.
    pub available_bytes: u64,
    pub allocation_unit: std::num::NonZeroU64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MountedVolumeSpace {
    pub total_bytes: std::num::NonZeroU64,
    /// Free bytes reported by the filesystem, including space reserved from
    /// ordinary callers.
    pub free_bytes: u64,
    /// Bytes available to the current user, including quota effects.
    pub available_bytes: u64,
    pub allocation_unit: std::num::NonZeroU64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VolumePathKind {
    Directory,
    File,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VolumeDriveKind {
    Fixed,
    RamDisk,
    Removable,
    Remote,
    CdRom,
    Unknown,
    NoRoot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathVolume {
    pub canonical_path: std::path::PathBuf,
    pub path_kind: VolumePathKind,
    pub symlink_followed: bool,
    pub mount_path: Option<std::path::PathBuf>,
    pub mount_path_reason: Option<&'static str>,
    pub mount_proof: &'static str,
    pub space: MountedVolumeSpace,
    pub drive_kind: Option<VolumeDriveKind>,
    pub in_inventory: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum StorageErrorKind {
    Path,
    PathNotFound,
    PathDenied,
    Query,
    ZeroCapacity,
    InvalidValue,
    Overflow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageError {
    kind: StorageErrorKind,
    detail: String,
}

impl StorageError {
    pub(crate) fn new(kind: StorageErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub const fn kind(&self) -> StorageErrorKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "storage {:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for StorageError {}

pub(crate) fn checked_space(
    total_bytes: u64,
    available_bytes: u64,
    allocation_unit: u64,
) -> Result<VolumeSpace, StorageError> {
    let total_bytes = std::num::NonZeroU64::new(total_bytes).ok_or_else(|| {
        StorageError::new(StorageErrorKind::ZeroCapacity, "volume capacity is zero")
    })?;
    let allocation_unit = std::num::NonZeroU64::new(allocation_unit).ok_or_else(|| {
        StorageError::new(StorageErrorKind::InvalidValue, "allocation unit is zero")
    })?;
    if available_bytes > total_bytes.get() {
        return Err(StorageError::new(
            StorageErrorKind::InvalidValue,
            "available bytes exceed total volume capacity",
        ));
    }
    if allocation_unit.get() > total_bytes.get() {
        return Err(StorageError::new(
            StorageErrorKind::InvalidValue,
            "allocation unit exceeds total volume capacity",
        ));
    }
    Ok(VolumeSpace {
        total_bytes,
        available_bytes,
        allocation_unit,
    })
}

pub(crate) fn checked_mounted_space(
    total_bytes: u64,
    free_bytes: u64,
    available_bytes: u64,
    allocation_unit: u64,
) -> Result<MountedVolumeSpace, StorageError> {
    let basic = checked_space(total_bytes, available_bytes, allocation_unit)?;
    if available_bytes > free_bytes || free_bytes > basic.total_bytes.get() {
        return Err(StorageError::new(
            StorageErrorKind::InvalidValue,
            "volume capacity must satisfy available <= free <= total",
        ));
    }
    Ok(MountedVolumeSpace {
        total_bytes: basic.total_bytes,
        free_bytes,
        available_bytes,
        allocation_unit: basic.allocation_unit,
    })
}

pub(crate) fn checked_product(
    count: impl Into<u64>,
    unit: u64,
    name: &str,
) -> Result<u64, StorageError> {
    count.into().checked_mul(unit).ok_or_else(|| {
        StorageError::new(
            StorageErrorKind::Overflow,
            format!("{name} multiplied by allocation unit overflowed u64"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_space_rejects_incoherent_values() {
        assert_eq!(
            checked_space(0, 0, 4096)
                .expect_err("reject zero-capacity volume")
                .kind(),
            StorageErrorKind::ZeroCapacity
        );
        for result in [
            checked_space(1024, 1025, 512),
            checked_space(1024, 0, 0),
            checked_space(1024, 0, 2048),
        ] {
            assert_eq!(
                result.expect_err("reject invalid volume facts").kind(),
                StorageErrorKind::InvalidValue
            );
        }
    }

    #[test]
    fn mounted_space_rejects_incoherent_free_capacity() {
        for result in [
            checked_mounted_space(1024, 511, 512, 512),
            checked_mounted_space(1024, 1025, 512, 512),
        ] {
            assert_eq!(
                result.expect_err("reject incoherent mounted volume").kind(),
                StorageErrorKind::InvalidValue
            );
        }
    }

    #[test]
    fn block_product_rejects_overflow() {
        let error = checked_product(u64::MAX, 4096, "blocks").expect_err("reject overflow");
        assert_eq!(error.kind(), StorageErrorKind::Overflow);
    }
}
