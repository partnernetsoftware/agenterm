//! CoreAudio default-output state. These synchronous HAL calls are worker-safe;
//! the adapter owns no persistent handle or callback and every CF value returned
//! by the property API is released on the same call path.

use std::{ffi::c_void, ptr};

use crate::audio::{
    AUDIO_DEVICE_FIELD_MAX_BYTES, AudioError, AudioErrorKind, AudioOutputSettings, NativeAudioState,
};

type AudioObjectId = u32;
type CfIndex = isize;
type CfStringRef = *const c_void;
type CfTypeRef = *const c_void;
type OsStatus = i32;

const SYSTEM_OBJECT: AudioObjectId = 1;
const MASTER_ELEMENT: u32 = 0;
const UTF8: u32 = 0x0800_0100;

const fn fourcc(bytes: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*bytes)
}

const DEFAULT_OUTPUT_DEVICE: u32 = fourcc(b"dOut");
const DEVICE_UID: u32 = fourcc(b"uid ");
const OBJECT_NAME: u32 = fourcc(b"lnam");
const OBJECT_MANUFACTURER: u32 = fourcc(b"lmak");
const VIRTUAL_MASTER_VOLUME: u32 = fourcc(b"vmvc");
const DEVICE_MUTE: u32 = fourcc(b"mute");
const SCOPE_GLOBAL: u32 = fourcc(b"glob");
const SCOPE_OUTPUT: u32 = fourcc(b"outp");

#[repr(C)]
#[derive(Clone, Copy)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

impl PropertyAddress {
    const fn global(selector: u32) -> Self {
        Self {
            selector,
            scope: SCOPE_GLOBAL,
            element: MASTER_ELEMENT,
        }
    }

    const fn output(selector: u32) -> Self {
        Self {
            selector,
            scope: SCOPE_OUTPUT,
            element: MASTER_ELEMENT,
        }
    }
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyData(
        object: AudioObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> OsStatus;
    fn AudioObjectSetPropertyData(
        object: AudioObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: u32,
        data: *const c_void,
    ) -> OsStatus;
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioHardwareServiceGetPropertyData(
        object: AudioObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> OsStatus;
    fn AudioHardwareServiceSetPropertyData(
        object: AudioObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: u32,
        data: *const c_void,
    ) -> OsStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: CfTypeRef);
    fn CFStringGetLength(value: CfStringRef) -> CfIndex;
    fn CFStringGetMaximumSizeForEncoding(length: CfIndex, encoding: u32) -> CfIndex;
    fn CFStringGetCString(
        value: CfStringRef,
        buffer: *mut i8,
        buffer_size: CfIndex,
        encoding: u32,
    ) -> bool;
}

struct OwnedCfString(CfStringRef);

impl Drop for OwnedCfString {
    fn drop(&mut self) {
        // SAFETY: CoreAudio returned this retained CFString through its object
        // property copy contract. It is non-null and released exactly once.
        unsafe { CFRelease(self.0) };
    }
}

pub(crate) fn query() -> Result<NativeAudioState, AudioError> {
    let device_id = get_audio_value::<u32>(
        SYSTEM_OBJECT,
        PropertyAddress::global(DEFAULT_OUTPUT_DEVICE),
        "default output device",
    )?;
    if device_id == 0 {
        return Err(invalid(
            "CoreAudio returned a zero default-output device id",
        ));
    }
    let uid = get_string(device_id, DEVICE_UID, "device UID")?;
    let name = get_string(device_id, OBJECT_NAME, "device name")?;
    let manufacturer = get_string(device_id, OBJECT_MANUFACTURER, "device manufacturer")?;
    let volume_scalar = get_service_value::<f32>(
        device_id,
        PropertyAddress::output(VIRTUAL_MASTER_VOLUME),
        "virtual master volume",
    )?;
    let muted = get_audio_value::<u32>(
        device_id,
        PropertyAddress::output(DEVICE_MUTE),
        "output mute",
    )?;
    if muted > 1 {
        return Err(invalid("CoreAudio returned a non-boolean mute value"));
    }
    Ok(NativeAudioState {
        device_id,
        uid,
        name,
        manufacturer,
        volume_scalar,
        muted: muted != 0,
    })
}

pub(crate) fn set(
    expected_device: &NativeAudioState,
    settings: AudioOutputSettings,
) -> Result<(), AudioError> {
    let current = get_audio_value::<u32>(
        SYSTEM_OBJECT,
        PropertyAddress::global(DEFAULT_OUTPUT_DEVICE),
        "default output device before mutation",
    )?;
    if current != expected_device.device_id {
        return Err(AudioError::new(
            AudioErrorKind::DeviceChanged,
            "the default output device changed before the CoreAudio write",
        ));
    }
    let current_uid = get_string(current, DEVICE_UID, "device UID before mutation")?;
    if current_uid != expected_device.uid {
        return Err(AudioError::new(
            AudioErrorKind::DeviceChanged,
            "the default output device identity changed before the CoreAudio write",
        ));
    }
    let scalar = f32::from(settings.volume_percent) / 100.0;
    set_service_value(
        current,
        PropertyAddress::output(VIRTUAL_MASTER_VOLUME),
        &scalar,
        "virtual master volume",
    )?;
    let muted = u32::from(settings.muted);
    set_audio_value(
        current,
        PropertyAddress::output(DEVICE_MUTE),
        &muted,
        "output mute",
    )
}

fn get_string(
    object: AudioObjectId,
    selector: u32,
    field: &'static str,
) -> Result<String, AudioError> {
    let value = get_audio_value::<CfStringRef>(object, PropertyAddress::global(selector), field)?;
    if value.is_null() {
        return Err(invalid(format!("CoreAudio returned a null {field}")));
    }
    let value = OwnedCfString(value);
    let length = unsafe { CFStringGetLength(value.0) };
    if length <= 0 || length > AUDIO_DEVICE_FIELD_MAX_BYTES as CfIndex {
        return Err(invalid(format!(
            "CoreAudio returned an invalid {field} length"
        )));
    }
    let maximum = unsafe { CFStringGetMaximumSizeForEncoding(length, UTF8) };
    let capacity = maximum
        .checked_add(1)
        .filter(|capacity| {
            *capacity > 0 && *capacity <= (AUDIO_DEVICE_FIELD_MAX_BYTES * 4 + 1) as CfIndex
        })
        .ok_or_else(|| invalid(format!("CoreAudio {field} exceeds the conversion bound")))?;
    let mut bytes = vec![0_u8; capacity as usize];
    if !unsafe { CFStringGetCString(value.0, bytes.as_mut_ptr().cast(), capacity, UTF8) } {
        return Err(invalid(format!("CoreAudio {field} is not valid UTF-8")));
    }
    let end = bytes.iter().position(|byte| *byte == 0).ok_or_else(|| {
        invalid(format!(
            "CoreAudio {field} conversion omitted its terminator"
        ))
    })?;
    if end == 0 || end > AUDIO_DEVICE_FIELD_MAX_BYTES {
        return Err(invalid(format!(
            "CoreAudio {field} exceeds the public bound"
        )));
    }
    String::from_utf8(bytes[..end].to_vec())
        .map_err(|_| invalid(format!("CoreAudio {field} is not valid UTF-8")))
}

fn get_audio_value<T: Copy + Default>(
    object: AudioObjectId,
    address: PropertyAddress,
    field: &'static str,
) -> Result<T, AudioError> {
    let mut value = T::default();
    let mut size = u32::try_from(std::mem::size_of::<T>()).expect("native property value fits u32");
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &address,
            0,
            ptr::null(),
            &mut size,
            (&raw mut value).cast(),
        )
    };
    check_get(status, size, std::mem::size_of::<T>(), field)?;
    Ok(value)
}

fn get_service_value<T: Copy + Default>(
    object: AudioObjectId,
    address: PropertyAddress,
    field: &'static str,
) -> Result<T, AudioError> {
    let mut value = T::default();
    let mut size = u32::try_from(std::mem::size_of::<T>()).expect("native property value fits u32");
    let status = unsafe {
        AudioHardwareServiceGetPropertyData(
            object,
            &address,
            0,
            ptr::null(),
            &mut size,
            (&raw mut value).cast(),
        )
    };
    check_get(status, size, std::mem::size_of::<T>(), field)?;
    Ok(value)
}

fn set_audio_value<T>(
    object: AudioObjectId,
    address: PropertyAddress,
    value: &T,
    field: &'static str,
) -> Result<(), AudioError> {
    let status = unsafe {
        AudioObjectSetPropertyData(
            object,
            &address,
            0,
            ptr::null(),
            u32::try_from(std::mem::size_of::<T>()).expect("native property value fits u32"),
            std::ptr::from_ref(value).cast(),
        )
    };
    check_set(status, field)
}

fn set_service_value<T>(
    object: AudioObjectId,
    address: PropertyAddress,
    value: &T,
    field: &'static str,
) -> Result<(), AudioError> {
    let status = unsafe {
        AudioHardwareServiceSetPropertyData(
            object,
            &address,
            0,
            ptr::null(),
            u32::try_from(std::mem::size_of::<T>()).expect("native property value fits u32"),
            std::ptr::from_ref(value).cast(),
        )
    };
    check_set(status, field)
}

fn check_get(
    status: OsStatus,
    actual: u32,
    expected: usize,
    field: &str,
) -> Result<(), AudioError> {
    if status != 0 {
        return Err(AudioError::new(
            AudioErrorKind::QueryFailed,
            format!("CoreAudio {field} query failed with OSStatus {status}"),
        ));
    }
    if actual as usize != expected {
        return Err(invalid(format!(
            "CoreAudio {field} returned {actual} bytes, expected {expected}"
        )));
    }
    Ok(())
}

fn check_set(status: OsStatus, field: &str) -> Result<(), AudioError> {
    if status == 0 {
        Ok(())
    } else {
        Err(AudioError::new(
            AudioErrorKind::MutationFailed,
            format!("CoreAudio {field} mutation failed with OSStatus {status}"),
        ))
    }
}

fn invalid(detail: impl Into<String>) -> AudioError {
    AudioError::new(AudioErrorKind::InvalidNativeValue, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourcc_constants_match_the_coreaudio_spelling() {
        assert_eq!(DEFAULT_OUTPUT_DEVICE.to_be_bytes(), *b"dOut");
        assert_eq!(VIRTUAL_MASTER_VOLUME.to_be_bytes(), *b"vmvc");
        assert_eq!(DEVICE_MUTE.to_be_bytes(), *b"mute");
    }

    #[test]
    fn native_default_output_is_bounded_and_coherent() {
        let state = query().expect("query the current CoreAudio default output");
        assert_ne!(state.device_id, 0);
        assert!(!state.uid.is_empty());
        assert!((0.0..=1.0).contains(&state.volume_scalar));
    }
}
