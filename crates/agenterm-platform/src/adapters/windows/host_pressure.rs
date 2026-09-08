use super::contract::{
    HostPressureError, HostPressureErrorKind, HostPressureSignal, HostPressureSnapshot,
    HostPressureUnavailableReason,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::Memory::{
        CreateMemoryResourceNotification, HighMemoryResourceNotification,
        LowMemoryResourceNotification, MEMORY_RESOURCE_NOTIFICATION_TYPE,
        QueryMemoryResourceNotification,
    },
};

struct MemoryNotification(HANDLE);

impl MemoryNotification {
    fn create(
        notification_type: MEMORY_RESOURCE_NOTIFICATION_TYPE,
        name: &'static str,
    ) -> Result<Self, HostPressureError> {
        let handle = unsafe { CreateMemoryResourceNotification(notification_type) };
        if handle.is_null() {
            return Err(HostPressureError::new(
                HostPressureErrorKind::ProviderUnavailable,
                format!(
                    "CreateMemoryResourceNotification({name}) failed: {}",
                    std::io::Error::last_os_error()
                ),
            ));
        }
        Ok(Self(handle))
    }

    fn query(&self, name: &'static str) -> Result<bool, HostPressureError> {
        let mut state = 0;
        if unsafe { QueryMemoryResourceNotification(self.0, &mut state) } == 0 {
            return Err(HostPressureError::new(
                HostPressureErrorKind::NativeQuery,
                format!(
                    "QueryMemoryResourceNotification({name}) failed: {}",
                    std::io::Error::last_os_error()
                ),
            ));
        }
        Ok(state != 0)
    }
}

impl Drop for MemoryNotification {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(crate) fn snapshot() -> Result<HostPressureSnapshot, HostPressureError> {
    let low = MemoryNotification::create(LowMemoryResourceNotification, "low-memory")?;
    let high = MemoryNotification::create(HighMemoryResourceNotification, "high-memory")?;
    Ok(snapshot_from_memory_states(
        low.query("low-memory")?,
        high.query("high-memory")?,
    ))
}

fn snapshot_from_memory_states(low_memory: bool, high_memory: bool) -> HostPressureSnapshot {
    let unavailable = HostPressureSignal::Unavailable {
        reason: HostPressureUnavailableReason::NotPublishedByPlatform,
    };
    HostPressureSnapshot {
        memory: HostPressureSignal::WindowsMemoryResourceNotification {
            low_memory,
            high_memory,
        },
        cpu: unavailable,
        io: unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_native_memory_states_without_deriving_cpu_or_io_pressure() {
        for (low_memory, high_memory) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let snapshot = snapshot_from_memory_states(low_memory, high_memory);
            assert_eq!(
                snapshot.memory,
                HostPressureSignal::WindowsMemoryResourceNotification {
                    low_memory,
                    high_memory,
                }
            );
            assert_eq!(
                snapshot.cpu,
                HostPressureSignal::Unavailable {
                    reason: HostPressureUnavailableReason::NotPublishedByPlatform,
                }
            );
            assert_eq!(snapshot.io, snapshot.cpu);
        }
    }
}
