use std::{io, mem::size_of, ptr};

use windows_sys::Win32::{
    Devices::{
        Communication::{
            COMMTIMEOUTS, DCB, EVENPARITY, GetCommState, GetCommTimeouts, NOPARITY, ODDPARITY,
            ONESTOPBIT, SetCommState, SetCommTimeouts, TWOSTOPBITS,
        },
        DeviceAndDriverInstallation::{
            DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA,
            SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA, SetupDiDestroyDeviceInfoList,
            SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW,
            SetupDiGetDeviceInterfaceDetailW,
        },
    },
    Foundation::{
        CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS, HANDLE, INVALID_HANDLE_VALUE,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, OPEN_EXISTING,
        ReadFile, WriteFile,
    },
    System::Ioctl::GUID_DEVINTERFACE_COMPORT,
};

use crate::{
    contract::device_io::*,
    device_inventory::NativeDeviceRecord,
    device_io::{error, write_error},
};

pub(crate) struct NativeResolvedDevice {
    path: Vec<u16>,
    instance_id: Vec<u8>,
}
pub(crate) struct NativeOpenedDevice {
    handle: OwnedHandle,
    original_dcb: DCB,
    original_timeouts: COMMTIMEOUTS,
    active_timeouts: COMMTIMEOUTS,
    restored: bool,
}
impl Drop for NativeOpenedDevice {
    fn drop(&mut self) {
        if !self.restored {
            let _ = restore_raw(self.handle.0, &self.original_dcb, &self.original_timeouts);
        }
    }
}

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if self.0 != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(self.0) };
        }
    }
}
struct DeviceInfoSet(HDEVINFO);
impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        if self.0 != INVALID_HANDLE_VALUE as isize {
            unsafe { SetupDiDestroyDeviceInfoList(self.0) };
        }
    }
}

pub(crate) fn resolve_record(
    record: &NativeDeviceRecord,
) -> Result<NativeResolvedDevice, DeviceIoError> {
    let mut hits = enumerate_com_interfaces()?.into_iter().filter(|candidate| {
        candidate
            .instance_id
            .eq_ignore_ascii_case(&record.identity_material)
    });
    let hit = hits.next().ok_or_else(|| {
        error(
            DeviceIoErrorKind::NotClaimable,
            "device-not-claimable",
            "inventory device has no present COM device interface",
        )
    })?;
    if hits.next().is_some() {
        return Err(error(
            DeviceIoErrorKind::Ambiguous,
            "device-ambiguous",
            "inventory identity matched more than one COM device interface",
        ));
    }
    Ok(hit)
}

pub(crate) fn matches_record(resolved: &NativeResolvedDevice, record: &NativeDeviceRecord) -> bool {
    resolved
        .instance_id
        .eq_ignore_ascii_case(&record.identity_material)
}

pub(crate) fn open_exclusive(
    resolved: &NativeResolvedDevice,
    config: SerialConfiguration,
) -> Result<NativeOpenedDevice, DeviceIoError> {
    validate_config(config)?;
    // SAFETY: path is a provider-produced, bounded, NUL-terminated device-interface path.
    let raw = unsafe {
        CreateFileW(
            resolved.path.as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            0,
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(map_open_error(io::Error::last_os_error()));
    }
    let handle = OwnedHandle(raw);
    let fresh = enumerate_com_interfaces()?;
    if !fresh.iter().any(|candidate| {
        candidate
            .instance_id
            .eq_ignore_ascii_case(&resolved.instance_id)
            && candidate.path == resolved.path
    }) {
        return Err(error(
            DeviceIoErrorKind::IdentityChanged,
            "device-identity-changed",
            "COM device-interface binding changed while opening",
        ));
    }
    let mut original_dcb = DCB {
        DCBlength: size_of::<DCB>() as u32,
        ..DCB::default()
    };
    let mut original_timeouts = COMMTIMEOUTS::default();
    // SAFETY: handle is live and both output structures are writable.
    if unsafe { GetCommState(handle.0, &mut original_dcb) } == 0
        || unsafe { GetCommTimeouts(handle.0, &mut original_timeouts) } == 0
    {
        return Err(error(
            DeviceIoErrorKind::Unsupported,
            "device-serial-unsupported",
            "opened COM interface does not expose serial state and timeouts",
        ));
    }
    let mut requested = original_dcb;
    configure_dcb(&mut requested, config);
    let requested_timeouts = COMMTIMEOUTS {
        ReadIntervalTimeout: u32::MAX,
        ReadTotalTimeoutMultiplier: 0,
        ReadTotalTimeoutConstant: 0,
        WriteTotalTimeoutMultiplier: 0,
        WriteTotalTimeoutConstant: 1_000,
    };
    // SAFETY: handle and initialized structures are valid.
    if unsafe { SetCommState(handle.0, &requested) } == 0
        || unsafe { SetCommTimeouts(handle.0, &requested_timeouts) } == 0
    {
        let _ = restore_raw(handle.0, &original_dcb, &original_timeouts);
        return Err(error(
            DeviceIoErrorKind::SerialApplyFailed,
            "device-serial-apply-failed",
            "COM serial configuration could not be applied",
        ));
    }
    let mut actual = DCB {
        DCBlength: size_of::<DCB>() as u32,
        ..DCB::default()
    };
    let mut actual_timeouts = COMMTIMEOUTS::default();
    // SAFETY: handle is live and output structures are writable.
    if unsafe { GetCommState(handle.0, &mut actual) } == 0
        || unsafe { GetCommTimeouts(handle.0, &mut actual_timeouts) } == 0
        || !dcb_matches(&actual, config)
        || !timeouts_match(&actual_timeouts, &requested_timeouts)
    {
        let _ = restore_raw(handle.0, &original_dcb, &original_timeouts);
        return Err(error(
            DeviceIoErrorKind::SerialReadbackMismatch,
            "device-serial-readback-mismatch",
            "COM serial configuration readback differed from the request",
        ));
    }
    Ok(NativeOpenedDevice {
        handle,
        original_dcb,
        original_timeouts,
        active_timeouts: requested_timeouts,
        restored: false,
    })
}

pub(crate) fn read_once(
    device: &mut NativeOpenedDevice,
    max: usize,
) -> Result<DeviceReadOutcome, DeviceIoError> {
    let mut bytes = vec![0; max];
    let mut read = 0_u32;
    // SAFETY: handle is live and the buffer is writable for max bytes.
    if unsafe {
        ReadFile(
            device.handle.0,
            bytes.as_mut_ptr(),
            max as u32,
            &mut read,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(error(
            DeviceIoErrorKind::ReadFailed,
            "device-read-failed",
            io::Error::last_os_error().to_string(),
        ));
    }
    bytes.truncate(read as usize);
    let state = if bytes.is_empty() {
        DeviceReadState::WouldBlock
    } else {
        DeviceReadState::Data
    };
    Ok(DeviceReadOutcome { bytes, state })
}

pub(crate) fn write_once(
    device: &mut NativeOpenedDevice,
    bytes: &[u8],
    timeout_ms: u32,
) -> Result<DeviceWriteOutcome, DeviceIoError> {
    let requested_timeouts = COMMTIMEOUTS {
        WriteTotalTimeoutMultiplier: 0,
        WriteTotalTimeoutConstant: timeout_ms,
        ..device.active_timeouts
    };
    if !timeouts_match(&device.active_timeouts, &requested_timeouts) {
        // SAFETY: handle and initialized timeout structure are valid.
        if unsafe { SetCommTimeouts(device.handle.0, &requested_timeouts) } == 0 {
            return Err(write_error(
                "device-write-timeout-apply-failed",
                io::Error::last_os_error().to_string(),
                0,
                false,
                true,
            ));
        }
        let mut actual = COMMTIMEOUTS::default();
        // SAFETY: handle is live and actual is writable.
        if unsafe { GetCommTimeouts(device.handle.0, &mut actual) } == 0
            || !timeouts_match(&actual, &requested_timeouts)
        {
            return Err(write_error(
                "device-write-timeout-readback-mismatch",
                "COM write timeout did not match the requested deadline",
                0,
                false,
                true,
            ));
        }
        device.active_timeouts = actual;
    }
    let mut written = 0_u32;
    // SAFETY: handle is live and bytes is readable for its bounded length.
    if unsafe {
        WriteFile(
            device.handle.0,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(write_error(
            "device-write-failed",
            io::Error::last_os_error().to_string(),
            written as usize,
            true,
            false,
        ));
    }
    let written = written as usize;
    Ok(DeviceWriteOutcome {
        requested_bytes: bytes.len(),
        written_bytes: written,
        delivery: if written == bytes.len() {
            DeviceWriteDelivery::Complete
        } else {
            DeviceWriteDelivery::Partial
        },
    })
}

pub(crate) fn close_restore(mut device: NativeOpenedDevice) -> Result<(), DeviceIoError> {
    let result = restore_raw(
        device.handle.0,
        &device.original_dcb,
        &device.original_timeouts,
    );
    device.restored = result.is_ok();
    result
}

fn enumerate_com_interfaces() -> Result<Vec<NativeResolvedDevice>, DeviceIoError> {
    // SAFETY: all pointers are null or point to initialized values below.
    let raw = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVINTERFACE_COMPORT,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if raw == INVALID_HANDLE_VALUE as isize {
        return Err(error(
            DeviceIoErrorKind::OpenFailed,
            "device-provider-failed",
            io::Error::last_os_error().to_string(),
        ));
    }
    let set = DeviceInfoSet(raw);
    let mut rows = Vec::new();
    for index in 0..10_000_u32 {
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..SP_DEVICE_INTERFACE_DATA::default()
        };
        // SAFETY: set and interface are valid for this enumeration call.
        if unsafe {
            SetupDiEnumDeviceInterfaces(
                set.0,
                ptr::null(),
                &GUID_DEVINTERFACE_COMPORT,
                index,
                &mut interface,
            )
        } == 0
        {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(ERROR_NO_MORE_ITEMS as i32) {
                break;
            }
            return Err(error(
                DeviceIoErrorKind::OpenFailed,
                "device-provider-failed",
                e.to_string(),
            ));
        }
        let mut required = 0_u32;
        let mut info = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..SP_DEVINFO_DATA::default()
        };
        // SAFETY: size probe intentionally supplies no detail buffer.
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                set.0,
                &interface,
                ptr::null_mut(),
                0,
                &mut required,
                &mut info,
            )
        };
        let probe = io::Error::last_os_error();
        if probe.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
            || required as usize > 64 * 1024
            || required < (size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32)
        {
            return Err(error(
                DeviceIoErrorKind::ResourceLimit,
                "device-provider-invalid",
                "COM interface detail size was invalid",
            ));
        }
        let words = (required as usize).div_ceil(size_of::<usize>());
        let mut storage = vec![0_usize; words];
        let detail = storage
            .as_mut_ptr()
            .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        // SAFETY: storage meets the required byte count and alignment from Vec's allocator for this C structure.
        unsafe {
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        }
        // SAFETY: all structures and the required-size buffer are valid.
        if unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                set.0,
                &interface,
                detail,
                required,
                &mut required,
                &mut info,
            )
        } == 0
        {
            return Err(error(
                DeviceIoErrorKind::OpenFailed,
                "device-provider-failed",
                io::Error::last_os_error().to_string(),
            ));
        }
        // SAFETY: DevicePath begins within storage and provider guaranteed a terminated path within required bytes.
        let path_start = unsafe { ptr::addr_of!((*detail).DevicePath).cast::<u16>() };
        let storage_bytes = storage.len() * size_of::<usize>();
        let max = (storage_bytes.saturating_sub(size_of::<u32>())) / 2;
        let mut len = 0;
        while len < max && unsafe { *path_start.add(len) } != 0 {
            len += 1;
        }
        if len == max {
            return Err(error(
                DeviceIoErrorKind::ResourceLimit,
                "device-provider-invalid",
                "COM interface path was not terminated",
            ));
        }
        let mut path = unsafe { std::slice::from_raw_parts(path_start, len) }.to_vec();
        path.push(0);
        let mut instance = vec![0_u16; 512];
        let mut needed = 0_u32;
        // SAFETY: instance is writable and info names the interface device.
        if unsafe {
            SetupDiGetDeviceInstanceIdW(
                set.0,
                &info,
                instance.as_mut_ptr(),
                instance.len() as u32,
                &mut needed,
            )
        } == 0
            || needed == 0
            || needed as usize > instance.len()
        {
            return Err(error(
                DeviceIoErrorKind::ResourceLimit,
                "device-provider-invalid",
                "COM instance identity was unavailable or oversized",
            ));
        }
        instance.truncate(needed.saturating_sub(1) as usize);
        let text = String::from_utf16(&instance).map_err(|_| {
            error(
                DeviceIoErrorKind::OpenFailed,
                "device-provider-invalid",
                "COM instance identity was invalid UTF-16",
            )
        })?;
        rows.push(NativeResolvedDevice {
            path,
            instance_id: text.into_bytes(),
        });
    }
    Ok(rows)
}

fn validate_config(c: SerialConfiguration) -> Result<(), DeviceIoError> {
    if c.baud_rate == 0 {
        return Err(error(
            DeviceIoErrorKind::Unsupported,
            "device-serial-unsupported",
            "baud rate must be nonzero",
        ));
    }
    Ok(())
}
fn configure_dcb(d: &mut DCB, c: SerialConfiguration) {
    d.DCBlength = size_of::<DCB>() as u32;
    d.BaudRate = c.baud_rate;
    d.ByteSize = match c.data_bits {
        SerialDataBits::Five => 5,
        SerialDataBits::Six => 6,
        SerialDataBits::Seven => 7,
        SerialDataBits::Eight => 8,
    };
    d.Parity = match c.parity {
        SerialParity::None => NOPARITY,
        SerialParity::Even => EVENPARITY,
        SerialParity::Odd => ODDPARITY,
    };
    d.StopBits = if c.stop_bits == SerialStopBits::One {
        ONESTOPBIT
    } else {
        TWOSTOPBITS
    };
    d._bitfield &= !((1 << 1) | (1 << 2) | (1 << 8) | (1 << 9) | (3 << 12));
    d._bitfield |= 1;
    if c.parity != SerialParity::None {
        d._bitfield |= 1 << 1;
    }
    match c.flow_control {
        SerialFlowControl::None => {}
        SerialFlowControl::Software => d._bitfield |= (1 << 8) | (1 << 9),
        SerialFlowControl::Hardware => d._bitfield |= (1 << 2) | (2 << 12),
    }
}
fn dcb_matches(d: &DCB, c: SerialConfiguration) -> bool {
    d.BaudRate == c.baud_rate
        && d.ByteSize
            == match c.data_bits {
                SerialDataBits::Five => 5,
                SerialDataBits::Six => 6,
                SerialDataBits::Seven => 7,
                SerialDataBits::Eight => 8,
            }
        && d.Parity
            == match c.parity {
                SerialParity::None => NOPARITY,
                SerialParity::Even => EVENPARITY,
                SerialParity::Odd => ODDPARITY,
            }
        && d.StopBits
            == if c.stop_bits == SerialStopBits::One {
                ONESTOPBIT
            } else {
                TWOSTOPBITS
            }
        && match c.flow_control {
            SerialFlowControl::None => {
                d._bitfield & ((1 << 2) | (1 << 8) | (1 << 9) | (3 << 12)) == 0
            }
            SerialFlowControl::Software => {
                d._bitfield & ((1 << 8) | (1 << 9)) == ((1 << 8) | (1 << 9))
            }
            SerialFlowControl::Hardware => {
                d._bitfield & (1 << 2) != 0 && d._bitfield & (3 << 12) == 2 << 12
            }
        }
}
fn timeouts_match(a: &COMMTIMEOUTS, b: &COMMTIMEOUTS) -> bool {
    a.ReadIntervalTimeout == b.ReadIntervalTimeout
        && a.ReadTotalTimeoutMultiplier == b.ReadTotalTimeoutMultiplier
        && a.ReadTotalTimeoutConstant == b.ReadTotalTimeoutConstant
        && a.WriteTotalTimeoutMultiplier == b.WriteTotalTimeoutMultiplier
        && a.WriteTotalTimeoutConstant == b.WriteTotalTimeoutConstant
}
fn restore_raw(h: HANDLE, d: &DCB, t: &COMMTIMEOUTS) -> Result<(), DeviceIoError> {
    // SAFETY: live handle and initialized original settings.
    if unsafe { SetCommState(h, d) } == 0 || unsafe { SetCommTimeouts(h, t) } == 0 {
        return Err(error(
            DeviceIoErrorKind::SerialRestoreFailed,
            "device-serial-restore-failed",
            io::Error::last_os_error().to_string(),
        ));
    }
    Ok(())
}
fn map_open_error(e: io::Error) -> DeviceIoError {
    let kind = match e.kind() {
        io::ErrorKind::PermissionDenied => DeviceIoErrorKind::Busy,
        io::ErrorKind::NotFound => DeviceIoErrorKind::NotFound,
        _ => DeviceIoErrorKind::OpenFailed,
    };
    error(
        kind,
        match kind {
            DeviceIoErrorKind::Busy => "device-exclusive-busy",
            DeviceIoErrorKind::NotFound => "device-not-found",
            _ => "device-open-failed",
        },
        e.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dcb_projection_round_trips_closed_serial_fields() {
        let config = SerialConfiguration {
            baud_rate: 115_200,
            data_bits: SerialDataBits::Eight,
            parity: SerialParity::Even,
            stop_bits: SerialStopBits::One,
            flow_control: SerialFlowControl::Hardware,
        };
        let mut dcb = DCB::default();
        configure_dcb(&mut dcb, config);
        assert!(dcb_matches(&dcb, config));
    }
}
