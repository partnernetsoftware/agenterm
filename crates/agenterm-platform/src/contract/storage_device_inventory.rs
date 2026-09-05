//! Product-neutral physical and block storage inventory.

pub const STORAGE_DEVICE_MAX_ROWS: usize = 5_000;
pub const STORAGE_DEVICE_SCAN_CEILING: usize = 10_000;
pub const STORAGE_DEVICE_PROVIDER_OUTPUT_CEILING: usize = 2 * 1024 * 1024;
pub const STORAGE_DEVICE_FIELD_CEILING: usize = 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageDevice {
    /// Provider-local identity. This is not a cross-reboot hardware identity.
    pub id: String,
    pub node: Option<String>,
    pub name: String,
    pub kind: Option<String>,
    /// Exact native capacity. `None` means the provider did not publish it.
    pub size_bytes: Option<u64>,
    pub media_type: Option<String>,
    pub bus: Option<String>,
    pub health: Option<String>,
    pub health_semantics: Option<&'static str>,
    pub operational: Vec<String>,
    pub internal: Option<bool>,
    pub removable: Option<bool>,
    pub ejectable: Option<bool>,
    pub solid_state: Option<bool>,
    pub read_only: Option<bool>,
    pub virtual_device: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageDeviceInventory {
    pub devices: Vec<StorageDevice>,
    /// Native rows actually examined, including rows rejected by a future
    /// best-effort provider. It never exceeds the scan ceiling.
    pub visited: usize,
    pub read_errors: usize,
    pub truncated_scan: bool,
    /// Either native scanning or caller-requested projection omitted rows.
    pub truncated: bool,
    /// True only when the provider completed its bounded native snapshot
    /// without omitting or rejecting a row.
    pub complete: bool,
    pub provider: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum StorageDeviceErrorKind {
    InvalidLimit,
    ProviderUnavailable,
    PermissionDenied,
    ProviderFailed,
    Timeout,
    OutputLimit,
    MalformedSnapshot,
    ResourceLimit,
    CleanupFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageDeviceError {
    kind: StorageDeviceErrorKind,
    detail: String,
}

impl StorageDeviceError {
    pub(crate) fn new(kind: StorageDeviceErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> StorageDeviceErrorKind {
        self.kind
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for StorageDeviceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "storage-device {:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for StorageDeviceError {}
