//! Product-neutral current-host resource snapshot types.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostLoadAverageAvailability {
    Available,
    /// Windows has no native Unix-style 1/5/15-minute load average.
    UnavailableOnWindows,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostLoadAverage {
    pub one_minute: f64,
    pub five_minutes: f64,
    pub fifteen_minutes: f64,
    pub availability: HostLoadAverageAvailability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostFreeMemorySemantics {
    WindowsAvailablePhysical,
    LinuxMemFree,
    MacosFreePages,
}

impl HostFreeMemorySemantics {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WindowsAvailablePhysical => "windows-available-physical",
            Self::LinuxMemFree => "linux-mem-free",
            Self::MacosFreePages => "macos-free-pages",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostResourceMemory {
    pub total_physical_bytes: u64,
    /// Strictly free native physical memory, not reclaimable/available memory.
    pub free_physical_bytes: u64,
    pub available_physical_bytes: u64,
    pub free_semantics: HostFreeMemorySemantics,
    pub availability_semantics: crate::host_memory::HostMemoryAvailabilitySemantics,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HostResourceSnapshot {
    pub platform: crate::PlatformKind,
    pub architecture: &'static str,
    pub hostname: String,
    pub uptime_milliseconds: u64,
    pub load_average: HostLoadAverage,
    pub logical_processors: usize,
    pub processor_model: String,
    pub memory: HostResourceMemory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HostResourceSnapshotErrorKind {
    HostnameQuery,
    UptimeQuery,
    LoadAverageQuery,
    ProcessorQuery,
    MemoryQuery,
    InvalidNativeValue,
    Overflow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostResourceSnapshotError {
    kind: HostResourceSnapshotErrorKind,
    detail: String,
}

impl HostResourceSnapshotError {
    pub(crate) fn new(kind: HostResourceSnapshotErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub const fn kind(&self) -> HostResourceSnapshotErrorKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for HostResourceSnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "host resource snapshot {:?}: {}",
            self.kind, self.detail
        )
    }
}

impl std::error::Error for HostResourceSnapshotError {}

pub(crate) fn checked_memory(
    total_physical_bytes: u64,
    free_physical_bytes: u64,
    available_physical_bytes: u64,
    free_semantics: HostFreeMemorySemantics,
    availability_semantics: crate::host_memory::HostMemoryAvailabilitySemantics,
) -> Result<HostResourceMemory, HostResourceSnapshotError> {
    if total_physical_bytes == 0
        || free_physical_bytes > total_physical_bytes
        || available_physical_bytes > total_physical_bytes
    {
        return Err(HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "native free/available physical memory is incoherent",
        ));
    }
    Ok(HostResourceMemory {
        total_physical_bytes,
        free_physical_bytes,
        available_physical_bytes,
        free_semantics,
        availability_semantics,
    })
}

pub(crate) fn checked_load_average(
    values: [f64; 3],
    availability: HostLoadAverageAvailability,
) -> Result<HostLoadAverage, HostResourceSnapshotError> {
    if values
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
        || (availability == HostLoadAverageAvailability::UnavailableOnWindows && values != [0.0; 3])
    {
        return Err(HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "native load average is invalid",
        ));
    }
    Ok(HostLoadAverage {
        one_minute: values[0],
        five_minutes: values[1],
        fifteen_minutes: values[2],
        availability,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_rejects_free_available_or_total_inversions() {
        let semantics = crate::host_memory::HostMemoryAvailabilitySemantics::LinuxMemAvailable;
        for values in [(0, 0, 0), (10, 11, 10), (10, 5, 11)] {
            assert_eq!(
                checked_memory(
                    values.0,
                    values.1,
                    values.2,
                    HostFreeMemorySemantics::LinuxMemFree,
                    semantics
                )
                .unwrap_err()
                .kind(),
                HostResourceSnapshotErrorKind::InvalidNativeValue
            );
        }
    }

    #[test]
    fn separately_sampled_free_and_available_may_cross() {
        let value = checked_memory(
            10,
            8,
            7,
            HostFreeMemorySemantics::LinuxMemFree,
            crate::host_memory::HostMemoryAvailabilitySemantics::LinuxMemAvailable,
        )
        .unwrap();
        assert_eq!(
            (value.free_physical_bytes, value.available_physical_bytes),
            (8, 7)
        );
    }

    #[test]
    fn unavailable_load_is_explicit_and_zero_only() {
        assert_eq!(
            checked_load_average([0.0; 3], HostLoadAverageAvailability::UnavailableOnWindows)
                .unwrap()
                .availability,
            HostLoadAverageAvailability::UnavailableOnWindows
        );
        assert!(
            checked_load_average(
                [1.0, 0.0, 0.0],
                HostLoadAverageAvailability::UnavailableOnWindows
            )
            .is_err()
        );
        assert!(
            checked_load_average([f64::NAN, 0.0, 0.0], HostLoadAverageAvailability::Available)
                .is_err()
        );
    }
}
