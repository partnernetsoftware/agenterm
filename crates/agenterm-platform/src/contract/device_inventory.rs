//! Product-neutral, privacy-preserving peripheral inventory contracts.

use std::{borrow::Cow, fmt};

pub const DEVICE_INVENTORY_MAX_ROWS: usize = 5_000;
pub const DEVICE_INVENTORY_SCAN_CEILING: usize = 10_000;
pub const DEVICE_INVENTORY_FIELD_CEILING: usize = 512;
pub const DEVICE_INVENTORY_PROVIDER_OUTPUT_CEILING: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DeviceSelector {
    Usb,
    Bluetooth,
    Audio,
    Camera,
    Gpu,
    All,
}

impl DeviceSelector {
    #[must_use]
    pub const fn includes(self, kind: DeviceKind) -> bool {
        matches!(self, Self::All)
            || matches!(
                (self, kind),
                (Self::Usb, DeviceKind::Usb)
                    | (Self::Bluetooth, DeviceKind::Bluetooth)
                    | (Self::Audio, DeviceKind::Audio)
                    | (Self::Camera, DeviceKind::Camera)
                    | (Self::Gpu, DeviceKind::Gpu)
            )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DeviceKind {
    Usb,
    Bluetooth,
    Audio,
    Camera,
    Gpu,
}

impl DeviceKind {
    pub const ALL: [Self; 5] = [
        Self::Usb,
        Self::Bluetooth,
        Self::Audio,
        Self::Camera,
        Self::Gpu,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Usb => "usb",
            Self::Bluetooth => "bluetooth",
            Self::Audio => "audio",
            Self::Camera => "camera",
            Self::Gpu => "gpu",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DeviceProviderState {
    Complete,
    Partial,
    Unavailable,
}

/// Short compatibility name used by product serializers.
pub type ProviderState = DeviceProviderState;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DeviceIdentityContinuity {
    /// The native provider exposes an identifier intended to survive an
    /// ordinary disconnect/reconnect of this installation.
    ProviderStable,
    /// The best available native identity is topology-bound and may change if
    /// the device moves to another port or host attachment point.
    Topology,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceRecord {
    /// Installation-scoped pseudonym. It is not a serial number, MAC address,
    /// system path, or portable hardware identity.
    pub id: String,
    pub identity_continuity: DeviceIdentityContinuity,
    pub kind: DeviceKind,
    pub name: Option<String>,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub transport: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceProviderStatus {
    pub kind: DeviceKind,
    pub state: DeviceProviderState,
    pub provider: &'static str,
    pub visited: usize,
    pub read_errors: usize,
    pub truncated: bool,
    /// Stable typed reason for partial/unavailable state. Never native text.
    pub code: Option<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceInventory {
    pub devices: Vec<DeviceRecord>,
    pub providers: Vec<DeviceProviderStatus>,
    pub truncated: bool,
    /// True only when every requested provider completed and caller projection
    /// omitted no record.
    pub complete: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum DeviceInventoryErrorKind {
    InvalidLimit,
    IdentityMissing,
    IdentityInvalid,
    PermissionDenied,
    ProviderFailed,
    Timeout,
    OutputLimit,
    MalformedSnapshot,
    ResourceLimit,
    CleanupFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceInventoryError {
    kind: DeviceInventoryErrorKind,
    code: Cow<'static, str>,
    detail: String,
}

impl DeviceInventoryError {
    #[cfg(feature = "device-inventory")]
    pub(crate) fn new(
        kind: DeviceInventoryErrorKind,
        code: impl Into<Cow<'static, str>>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            code: code.into(),
            detail: detail.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> DeviceInventoryErrorKind {
        self.kind
    }

    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for DeviceInventoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "device inventory failed ({}): {}",
            self.code, self.detail
        )
    }
}

impl std::error::Error for DeviceInventoryError {}
