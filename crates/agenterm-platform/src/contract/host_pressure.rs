//! Product-neutral native host-pressure facts.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinuxPsiWindow {
    pub average_10_seconds: f64,
    pub average_60_seconds: f64,
    pub average_300_seconds: f64,
    pub total_stall_microseconds: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostPressureUnavailableReason {
    NotPublishedByPlatform,
}

impl HostPressureUnavailableReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotPublishedByPlatform => "not-published-by-platform",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HostPressureSignal {
    Unavailable {
        reason: HostPressureUnavailableReason,
    },
    LinuxPsi {
        some: LinuxPsiWindow,
        full: Option<LinuxPsiWindow>,
    },
    MacosVmPressure {
        raw_level: i32,
    },
    WindowsMemoryResourceNotification {
        low_memory: bool,
        high_memory: bool,
    },
}

impl HostPressureSignal {
    pub const fn semantics(self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "unavailable",
            Self::LinuxPsi { .. } => "linux-psi-stall-window",
            Self::MacosVmPressure { .. } => "macos-vm-memory-pressure-raw",
            Self::WindowsMemoryResourceNotification { .. } => {
                "windows-memory-resource-notification"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostPressureSnapshot {
    pub memory: HostPressureSignal,
    pub cpu: HostPressureSignal,
    pub io: HostPressureSignal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HostPressureErrorKind {
    ProviderUnavailable,
    NativeQuery,
    MalformedNativeData,
    InvalidNativeValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostPressureError {
    kind: HostPressureErrorKind,
    detail: String,
}

impl HostPressureError {
    pub(crate) fn new(kind: HostPressureErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub const fn kind(&self) -> HostPressureErrorKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for HostPressureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "host pressure {:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for HostPressureError {}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn checked_linux_psi_window(
    averages: [f64; 3],
    total_stall_microseconds: u64,
) -> Result<LinuxPsiWindow, HostPressureError> {
    if averages
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0 || *value > 100.0)
    {
        return Err(HostPressureError::new(
            HostPressureErrorKind::InvalidNativeValue,
            "Linux PSI average is outside 0..=100",
        ));
    }
    Ok(LinuxPsiWindow {
        average_10_seconds: averages[0],
        average_60_seconds: averages[1],
        average_300_seconds: averages[2],
        total_stall_microseconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_psi_rejects_non_finite_negative_and_over_percentage() {
        for averages in [[f64::NAN, 0.0, 0.0], [-0.01, 0.0, 0.0], [100.01, 0.0, 0.0]] {
            assert_eq!(
                checked_linux_psi_window(averages, 0).unwrap_err().kind(),
                HostPressureErrorKind::InvalidNativeValue
            );
        }
    }
}
