use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegQueryValueExW, RegSetValueExW,
};

use super::super::{
    ChromiumRegistryTarget, NativeMessagingRegistryError, NativeMessagingRegistryErrorKind,
    NativeMessagingRegistryReceipt,
};

const MAX_REGISTRY_VALUE_BYTES: u32 = 65_536;

struct OwnedKey(HKEY);

impl Drop for OwnedKey {
    fn drop(&mut self) {
        // SAFETY: the handle was initialized only after RegCreateKeyExW succeeded,
        // and this owner closes it exactly once.
        unsafe { RegCloseKey(self.0) };
    }
}

pub(crate) fn register(
    target: &ChromiumRegistryTarget,
    host_key: &str,
    manifest_path: &Path,
) -> Result<NativeMessagingRegistryReceipt, NativeMessagingRegistryError> {
    let key = open_host_key(host_key)?;
    let before = read_default_value(&key)?;
    let after = manifest_path.to_path_buf();
    let replaced = before.as_ref().is_some_and(|value| value != &after);

    if before.as_ref() != Some(&after) {
        write_default_value(&key, manifest_path)?;
    }
    let verified = read_default_value(&key).map_err(|error| {
        NativeMessagingRegistryError::new(
            NativeMessagingRegistryErrorKind::RegistryVerify,
            error.os_code,
            "native-messaging registry value could not be read back",
        )
    })?;
    if verified.as_ref() != Some(&after) {
        return Err(NativeMessagingRegistryError::new(
            NativeMessagingRegistryErrorKind::RegistryVerify,
            None,
            "native-messaging registry value did not match after write",
        ));
    }

    Ok(NativeMessagingRegistryReceipt {
        target: target.clone(),
        before,
        after,
        replaced,
    })
}

fn open_host_key(path: &str) -> Result<OwnedKey, NativeMessagingRegistryError> {
    let mut encoded: Vec<u16> = path.encode_utf16().collect();
    encoded.push(0);
    let mut key: HKEY = null_mut();
    // SAFETY: all pointers are valid for the call, the subkey is NUL-terminated,
    // and output ownership is transferred to OwnedKey only on success.
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            encoded.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            null(),
            &mut key,
            null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(NativeMessagingRegistryError::new(
            NativeMessagingRegistryErrorKind::RegistryOpen,
            Some(status),
            "current-user native-messaging registry key could not be opened",
        ));
    }
    Ok(OwnedKey(key))
}

fn read_default_value(key: &OwnedKey) -> Result<Option<PathBuf>, NativeMessagingRegistryError> {
    let mut value_type = 0;
    let mut byte_len = 0;
    // SAFETY: key is live; null value name denotes the default value; the size
    // probe supplies writable type and length pointers and no data buffer.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            null(),
            null(),
            &mut value_type,
            null_mut(),
            &mut byte_len,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(registry_error(
            NativeMessagingRegistryErrorKind::RegistryRead,
            status,
            "native-messaging registry value size could not be read",
        ));
    }
    if value_type != REG_SZ
        || !(2..=MAX_REGISTRY_VALUE_BYTES).contains(&byte_len)
        || byte_len % 2 != 0
    {
        return Err(NativeMessagingRegistryError::new(
            NativeMessagingRegistryErrorKind::RegistryValueInvalid,
            None,
            "existing native-messaging registry value is not a bounded REG_SZ",
        ));
    }

    let mut units = vec![0u16; byte_len as usize / 2];
    let mut fetched_len = byte_len;
    // SAFETY: the byte buffer has exactly the probed capacity and all other
    // pointers remain valid for the duration of the call.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            null(),
            null(),
            &mut value_type,
            units.as_mut_ptr().cast(),
            &mut fetched_len,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(registry_error(
            NativeMessagingRegistryErrorKind::RegistryRead,
            status,
            "native-messaging registry value could not be read",
        ));
    }
    if value_type != REG_SZ || fetched_len < 2 || fetched_len > byte_len || fetched_len % 2 != 0 {
        return Err(NativeMessagingRegistryError::new(
            NativeMessagingRegistryErrorKind::RegistryValueInvalid,
            None,
            "native-messaging registry value changed shape while reading",
        ));
    }
    units.truncate(fetched_len as usize / 2);
    if units.last() != Some(&0) || units[..units.len() - 1].contains(&0) {
        return Err(NativeMessagingRegistryError::new(
            NativeMessagingRegistryErrorKind::RegistryValueInvalid,
            None,
            "native-messaging registry value is not one NUL-terminated path",
        ));
    }
    units.pop();
    Ok(Some(PathBuf::from(OsString::from_wide(&units))))
}

fn write_default_value(
    key: &OwnedKey,
    manifest_path: &Path,
) -> Result<(), NativeMessagingRegistryError> {
    let mut encoded: Vec<u16> = manifest_path.as_os_str().encode_wide().collect();
    encoded.push(0);
    let byte_len = encoded
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|length| u32::try_from(length).ok())
        .ok_or_else(|| {
            NativeMessagingRegistryError::new(
                NativeMessagingRegistryErrorKind::InvalidManifestPath,
                None,
                "manifest path exceeds the bounded native path limit",
            )
        })?;
    // SAFETY: key is live and encoded is a NUL-terminated UTF-16 buffer whose
    // exact checked byte length is supplied to RegSetValueExW.
    let status =
        unsafe { RegSetValueExW(key.0, null(), 0, REG_SZ, encoded.as_ptr().cast(), byte_len) };
    if status != ERROR_SUCCESS {
        return Err(registry_error(
            NativeMessagingRegistryErrorKind::RegistryWrite,
            status,
            "current-user native-messaging registry value could not be written",
        ));
    }
    Ok(())
}

fn registry_error(
    kind: NativeMessagingRegistryErrorKind,
    status: u32,
    message: &'static str,
) -> NativeMessagingRegistryError {
    NativeMessagingRegistryError::new(kind, Some(status), message)
}
